// The smoke test's watch on the app's connections (#115): with the update
// check pointed at the test's own release server, the app's processes may
// connect only to what the test set up on this computer.
//
// `scripts/watch-connections.ps1` samples the TCP and UDP tables of the
// app's processes (netterm.exe and netterm-cli.exe started by this node
// process, and everything they start: WebView2, the Python the app runs,
// the netterm-cli a script runs) and prints each endpoint it has not seen
// before; this module judges each one as it arrives. Allowed:
//
// - a listening socket on a loopback address;
// - a connection whose other end is on a loopback address and belongs to
//   the test (this process or another it started: the test servers, the
//   release server, the DevTools client) or to one of the app's own
//   processes (a script's tunnel), or, if its owner cannot be told (it
//   closed already), is on a port the test named;
// - a UDP socket bound to a loopback address, or the one WebView2's
//   network process points at a fixed address to ask Windows whether IPv6
//   works (it sends nothing, which the network log is checked for).
//
// Anything else is a problem, kept with the step that was running. What it
// cannot see is in the testing notes.

import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createInterface } from "node:readline";

const LOOPBACK = /^(127\.|::1$|::ffff:127\.)/;

/** The WebView2 arguments that make it write its network log to `file`:
 * every request and every name it looks up, which the TCP tables cannot
 * show (`netLogProblems`). */
export function netLogArguments(file) {
  return `--log-net-log="${file}" --net-log-capture-mode=Default`;
}

/** Whether a name or address is this computer's (`*.localhost` is
 * answered by WebView2 itself, never looked up). */
function local(host) {
  const name = host.replace(/^\[|\]$/g, "").toLowerCase();
  return (
    name === "localhost" || name.endsWith(".localhost") || LOOPBACK.test(name)
  );
}

/**
 * What a WebView2 network log (`netLogArguments`) shows of names looked up
 * and requests made for anything but this computer, one line each, with
 * how many times. The log is read line by line, so one cut short by the
 * app being ended still counts.
 */
export function netLogProblems(file) {
  if (!existsSync(file)) return [`there is no network log at ${file}`];
  const found = new Map();
  const note = (what) => found.set(what, (found.get(what) ?? 0) + 1);
  let events = 0;
  let datagramSent = null;
  for (const line of readFileSync(file, "utf8").split(/\r?\n/)) {
    // One event per line; the first line is the log's constants, which
    // give each kind of event its number.
    if (line.startsWith('{"constants"')) {
      const type = /"UDP_BYTES_SENT":(\d+)/.exec(line);
      if (type) datagramSent = Number(type[1]);
      continue;
    }
    if (!line.startsWith("{")) continue;
    let event;
    try {
      event = JSON.parse(line.replace(/,\s*$/, ""));
    } catch {
      continue;
    }
    events += 1;
    // A UDP socket that is only pointed at an address sends nothing (the
    // IPv6 reachability check does that); one that sends is a lookup or a
    // connection WebView2 made itself.
    if (datagramSent !== null && event.type === datagramSent)
      note("sent a datagram");
    const { host, url } = event.params ?? {};
    if (typeof host === "string") {
      // "name:port", "scheme://name" or "scheme://name:port".
      const name = host
        .replace(/^[a-z][a-z0-9+.-]*:\/\//i, "")
        .replace(/:\d+$/, "");
      if (!local(name)) note(`looked up ${name}`);
    }
    if (typeof url === "string") {
      let parsed = null;
      try {
        parsed = new URL(url);
      } catch {
        // Not a URL to judge.
      }
      if (
        parsed &&
        /^(https?|wss?|ftp):$/.test(parsed.protocol) &&
        !local(parsed.hostname)
      )
        note(`requested ${parsed.origin}`);
    }
  }
  if (events === 0) return [`the network log ${file} holds no event`];
  return [...found].map(([what, count]) => `WebView2 ${what} (${count}×)`);
}

/**
 * Starts the watch; resolves once the first sample is taken. `ports`: the
 * loopback ports the test set up (more can be added with `allow`).
 */
export async function watchConnections({ ports = [], intervalMs = 50 } = {}) {
  const allowed = new Set(ports);
  const problems = [];
  const processes = new Map();
  const started = [];
  const connections = new Map();
  const udp = [];
  const probes = [];
  let step = "the start";
  let reported = 0;
  const watcher = spawn(
    "powershell",
    [
      "-NoProfile",
      "-ExecutionPolicy",
      "Bypass",
      "-File",
      "scripts/watch-connections.ps1",
      "-Root",
      String(process.pid),
      "-IntervalMs",
      String(intervalMs),
    ],
    { stdio: ["ignore", "pipe", "pipe"], windowsHide: true },
  );
  let errors = "";
  watcher.stderr.on("data", (chunk) => (errors += chunk));
  let exited = false;
  const ended = new Promise((resolve) =>
    watcher.once("exit", (code) => {
      exited = true;
      resolve(code);
    }),
  );

  // A watched process and the watched processes it was started by.
  const lineage = (pid) => {
    const names = [];
    for (let at = pid; processes.has(at) && names.length < 8;) {
      const { name, parent } = processes.get(at);
      names.push(`${name} ${at}`);
      at = parent;
    }
    return names.join(" < ");
  };
  // KNOWN FINDING (#135): WebView2's browser process (its
  // msedgewebview2.exe started by netterm.exe) now and then connects to
  // port 443 of Microsoft's Exchange Online / Office addresses
  // (40.96.0.0/13, 52.96.0.0/14), outside its network stack: its sign-in
  // with the Windows account. The app now starts WebView2 with that
  // sign-in off, and no run has made the connection since; until enough
  // runs have shown none, one is still let through and listed at the end
  // of the run. Anything else fails.
  const known = [];
  const browserProcess = (event) =>
    event.name === "msedgewebview2.exe" &&
    processes.get(processes.get(event.pid)?.parent)?.name === "netterm.exe";
  // A process WebView2's browser process started (its network process is
  // one of them).
  const networkProcess = (event) => {
    const parent = processes.get(event.pid)?.parent;
    return (
      event.name === "msedgewebview2.exe" &&
      processes.get(parent)?.name === "msedgewebview2.exe" &&
      processes.get(processes.get(parent)?.parent)?.name === "netterm.exe"
    );
  };
  const knownOfficeConnection = (event) => {
    const [a, b] = event.remoteAddress.split(".").map(Number);
    return (
      browserProcess(event) &&
      event.remotePort === 443 &&
      ((a === 40 && b >= 96 && b <= 103) || (a === 52 && b >= 96 && b <= 99))
    );
  };
  const judge = (event) => {
    if (event.kind === "process") {
      processes.set(event.pid, { name: event.name, parent: event.parent });
      started.push(event.name);
      return null;
    }
    const who = `${event.name} (pid ${lineage(event.pid)})`;
    if (event.kind === "udp") {
      udp.push(event);
      if (LOOPBACK.test(event.localAddress)) return null;
      // WebView2's network process asks Windows whether IPv6 reaches the
      // internet by pointing a UDP socket at a fixed Microsoft address and
      // reading back the local address Windows picked (Chromium's IPv6
      // reachability check, made when a name is resolved, `tauri.localhost`
      // included). Nothing is sent: `netLogProblems` fails on any datagram
      // the network log shows. Where IPv6 works the socket lives long
      // enough to be seen here now and then. Any other UDP socket fails.
      if (networkProcess(event)) {
        probes.push(event.local);
        return null;
      }
      return `${who} has a UDP socket on ${event.local}`;
    }
    if (event.state === "LISTEN") {
      return LOOPBACK.test(event.localAddress)
        ? null
        : `${who} listens on ${event.local}, not only on this computer`;
    }
    const key = `${event.name} ${event.remote}`;
    connections.set(key, (connections.get(key) ?? 0) + 1);
    if (!LOOPBACK.test(event.remoteAddress)) {
      const what = `${who} connected to ${event.remote} (${event.state})`;
      if (knownOfficeConnection(event)) {
        known.push(`${what}, during ${step}`);
        return null;
      }
      return what;
    }
    if (event.remoteWatched || event.remoteTest) return null;
    if (allowed.has(event.remotePort)) return null;
    const owner = event.remotePid
      ? `${event.remoteName} (pid ${event.remotePid})`
      : "nobody the test knows";
    return `${who} connected to ${event.remote} on this computer, owned by ${owner} (${event.state})`;
  };

  const lines = createInterface({ input: watcher.stdout });
  const ready = new Promise((resolve, reject) => {
    ended.then((code) =>
      reject(
        new Error(`the connection watch ended (${code}): ${errors.trim()}`),
      ),
    );
    lines.on("line", (line) => {
      let event;
      try {
        event = JSON.parse(line);
      } catch {
        return;
      }
      if (event.kind === "ready") {
        resolve();
        return;
      }
      const problem = judge(event);
      if (problem) problems.push(`${problem}, during ${step}`);
    });
  });
  await ready;

  return {
    /** A loopback port the test set up after the start. */
    allow(port) {
      allowed.add(port);
    },
    /** Names the step now running, for the problems found during it. */
    during(name) {
      step = name;
    },
    /** Fails with every problem not reported before, or if the watch ended. */
    check() {
      if (exited)
        throw new Error(`the connection watch stopped: ${errors.trim()}`);
      if (problems.length > reported) {
        const fresh = problems.slice(reported);
        reported = problems.length;
        throw new Error(
          [
            "the app connected where the test did not send it:",
            ...fresh.map((problem) => `  ${problem}`),
          ].join(String.fromCharCode(10)),
        );
      }
    },
    /** What was seen: the processes by name, the connections by remote end. */
    summary() {
      const names = {};
      for (const name of started) names[name] = (names[name] ?? 0) + 1;
      const remotes = {};
      for (const [key, count] of connections) remotes[key] = count;
      return {
        processes: names,
        connections: remotes,
        udp: udp.length,
        probes: probes.length,
        known: [...known],
      };
    },
    stop() {
      watcher.kill();
    },
  };
}
