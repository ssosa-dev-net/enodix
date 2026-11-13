//! Runs the session test harness's SSH server standalone, so the app can be
//! tried by hand without a real device:
//!
//! ```text
//! cargo run -p netterm-core --example test_ssh_server -- 2222
//! ```
//!
//! Then quick-connect to 127.0.0.1 on that port with the printed login. The
//! shell behind it is scripted; see `tests/support/mod.rs` for its commands.

#[path = "../tests/support/mod.rs"]
mod support;

#[tokio::main]
async fn main() {
    let port = match std::env::args().nth(1).map(|arg| arg.parse()) {
        None => 2222,
        Some(Ok(port)) => port,
        Some(Err(_)) => {
            eprintln!("usage: test_ssh_server [port]");
            std::process::exit(2);
        }
    };
    let server = support::TestSshServer::start_on(port).await;
    println!(
        "test SSH server listening on 127.0.0.1:{} (username {:?}, password {:?})",
        server.port(),
        support::USERNAME,
        support::PASSWORD
    );
    std::future::pending::<()>().await;
}
