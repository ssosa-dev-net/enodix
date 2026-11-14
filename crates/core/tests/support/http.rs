//! A tiny HTTP server inside the test process, standing in for the server
//! that publishes the release manifest (`latest.json`). It answers every
//! request with the current reply (changeable with `reply`), counts the
//! connections it accepted and keeps the request lines, so that a test can
//! tell whether (and what) the app asked. No real network endpoint is ever
//! contacted.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How the server answers.
#[derive(Clone)]
pub enum Reply {
    /// A status code and a body (sent as `application/json`).
    Status(u16, Vec<u8>),
    /// Accept the connection and never answer.
    Silence,
}

pub struct TestHttpServer {
    port: u16,
    reply: Arc<Mutex<Reply>>,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestHttpServer {
    /// A server answering `200 OK` with `body`.
    pub async fn serving(body: &str) -> Self {
        Self::start(Reply::Status(200, body.as_bytes().to_vec())).await
    }

    pub async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let reply = Arc::new(Mutex::new(reply));
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn({
            let reply = reply.clone();
            let connections = connections.clone();
            let requests = requests.clone();
            async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    connections.fetch_add(1, Ordering::SeqCst);
                    let reply = reply.lock().unwrap().clone();
                    let requests = requests.clone();
                    tokio::spawn(async move {
                        let mut received = Vec::new();
                        let mut buffer = [0u8; 4096];
                        while !received.windows(4).any(|w| w == b"\r\n\r\n") {
                            match stream.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => received.extend_from_slice(&buffer[..n]),
                            }
                        }
                        let text = String::from_utf8_lossy(&received);
                        let line = text.lines().next().unwrap_or_default().to_string();
                        requests.lock().unwrap().push(line);
                        match reply {
                            Reply::Status(code, body) => {
                                let head = format!(
                                    "HTTP/1.1 {code} Test\r\nContent-Type: application/json\r\n\
                                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                                    body.len()
                                );
                                let _ = stream.write_all(head.as_bytes()).await;
                                let _ = stream.write_all(&body).await;
                                let _ = stream.shutdown().await;
                            }
                            Reply::Silence => {
                                // Hold the connection open until the test ends.
                                std::future::pending::<()>().await;
                            }
                        }
                    });
                }
            }
        });
        Self {
            port,
            reply,
            connections,
            requests,
            task,
        }
    }

    /// Answers the next requests with `reply`.
    pub fn reply(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }

    /// The URL of `path` on this server, e.g. `/latest.json`.
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// How many connections were accepted so far.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// The request lines received, such as `GET /latest.json HTTP/1.1`.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
