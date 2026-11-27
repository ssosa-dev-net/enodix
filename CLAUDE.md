# enodix

A terminal for network engineers: Tauri 2 shell, Rust core, Svelte 5 + TypeScript
interface, xterm.js. What it does is in `README.md` and `docs/`; what comes next in
`ROADMAP.md`; tickets are GitHub issues.

## Layout

- `crates/core/` — `enodix-core`, the UI-free core. **All behaviour lives here.**
  Its public API is the single test seam. Sessions and transports are in
  `src/session/`, the workspace in `src/workspace.rs` and `src/workspace/`,
  credentials in `src/credentials.rs` and `src/credentials/`, jobs in
  `src/job.rs` and `src/job/`, the tab layouts (one per window) and the
  multi-send group in `src/layout.rs` and `src/layout/`, file connections
  and SFTP in `src/files.rs`, `src/files/` and `src/session/connection.rs`. `tests/`
  holds integration tests through the public API only; `tests/support/` is
  their harness (SSH, telnet, serial and agent servers inside the test
  process).
- `crates/cli/` — `enodix-cli`, the headless binary. It has no window and
  needs no running app; like the shell it is a thin adapter over the core.
  Its command line is a contract that the Python bridge builds on.
- `python/` — the Python bridge: the pure-Python package `enodix`. It only
  runs the headless binary. Its Nornir mapping is the one piece of behaviour
  outside the core.
- `src-tauri/` — the `enodix` binary: a thin adapter (commands and channels) over
  the core. No behaviour here. Session output goes to the interface over a Tauri
  `Channel` as raw bytes, never the event system.
- `src/` — the interface. It only renders; `src/lib/session.ts` and
  `src/lib/workspace.ts` wrap the IPC. Components use only the design tokens
  of `src/app.css` in their styles.
- `scripts/` — `smoke.mjs` (the end-to-end smoke test that drives the built
  app) and the release, wheel and Linux scripts.
- `docs/` — the spec, the user docs (`interface.md` is the user's map of the
  window: keep it in step when moving a feature; `files.md` is the page for
  files on a device, with its known limits) and `docs/agents/`.

## Area notes

The detail of each area is in `docs/agents/areas/`: types and functions,
shell commands, the `data-*` markers the smoke test relies on, test files and
support, known limits, what was and was not verified. These files are **not**
loaded by themselves: read the ones for the area before working in it (a
feature often spans two, e.g. `jobs.md` and `window.md`).

When a ticket adds or changes behaviour, write its notes in the area file,
not here. This file holds only what every session needs.

| File                | Read it when working on                                                                                                                                                                                                |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sessions.md`       | The session manager, output batching and flow control, SSH (login flow, agents, host keys, algorithms and legacy crypto, jump hosts, timeouts, reconnect), telnet, serial, session logging                             |
| `workspace.md`      | The workspace format and edits, inherited settings, roles, search, saved filters, live reload, web links, several workspaces and their identity, platforms and platform definitions                                    |
| `import-export.md`  | CSV import, Nornir import, the inventory export                                                                                                                                                                        |
| `credentials.md`    | Credential profiles, the keychain, named secrets, the vault and its upkeep, the credential agent, the Credentials view                                                                                                 |
| `terminal.md`       | What happens in a terminal: preferences (theme, font, zoom, copy and paste keys, search), WebGL and hidden terminals, keyword highlighting, snippets and templating, paste safety, input origins, multi-send           |
| `window.md`         | The window around the terminals: look and tokens, tabs and split panes, view tabs, the activity rail and side views, the details sheet, session header and status line, pane-scoped dialogs, empty states, the palette |
| `files.md`          | File connections (a login with no terminal), SFTP and its errors, SCP and the fall back, the transfer engine, the Files tab, what was verified on the lab (the user's page is `docs/files.md`)                         |
| `jobs.md`           | Jobs: planning, the engine, kept runs, the Jobs view and job and run tabs, saved jobs, live jobs and run windows                                                                                                       |
| `config-backups.md` | The config backup job and history, back up on login, the config history window                                                                                                                                         |
| `python-scripts.md` | The Python environment setup, script jobs, host enrichment                                                                                                                                                             |
| `tunnels.md`        | Tunnels that let scripts reach hosts behind jump hosts (core, cli and Python sides)                                                                                                                                    |
| `cli.md`            | `enodix-cli`: the inventory and credentials commands, where it finds credentials, its tests and packaging                                                                                                              |
| `python-bridge.md`  | The `enodix` Python package, the Nornir inventory plugin, the wheel, `npm run test:python`                                                                                                                             |
| `testing.md`        | Any test: the layout of `tests/`, the session test harness, the containerlab suite, the example server, what the smoke test covers                                                                                     |
| `release-linux.md`  | Packaging, the update check, the Linux checks and the Linux release                                                                                                                                                    |

Rules from those files that must hold whatever the task:

- Consumers of session output must acknowledge it, or reading stops
  (`sessions.md`).
- Always connect through `Profiles::profile_target` / `prompted_target`, never
  a bare `Host` (`workspace.md`).
- Every paste and every snippet with text goes through `pasteText` in
  `TerminalSession`, key-bound ones too. A snippet with steps is never pasted:
  the core runs it (`steps.rs`) through its own gates (refused in the
  multi-send group, the previewed digest must match, always asks on
  production), so never send its lines any other way (`terminal.md`).
- Tests never use the user's real SSH agent, never open the machine's real
  serial ports, and touch the keychain only under a random `enodix-test-*`
  prefix (`testing.md`). The opt-in containerlab suite is the one exception
  for the agent.
- Run the smoke test on an unlocked workstation (its clipboard steps fail
  otherwise).

## Commands

Run from the repo root (Git Bash on Windows).

| What                           | Command                                                   |
| ------------------------------ | --------------------------------------------------------- |
| Install JS dependencies        | `npm install`                                             |
| Everything below, in one go    | `npm run check` (format check, lint, tests)               |
| Tests (core, in-process SSH)   | `npm test` (= `cargo test --workspace`)                   |
| One test                       | `cargo test -p enodix-core --test ssh_session <name>`     |
| Lint (clippy + svelte-check)   | `npm run lint`                                            |
| Typecheck the interface only   | `npm run typecheck`                                       |
| Format / check formatting      | `npm run format` / `npm run format:check`                 |
| Run the app in dev mode        | `npm run app:dev`                                         |
| Build the app (no installer)   | `npm run app:build` → `target/release/enodix.exe`         |
| Update-check tests             | `cargo test -p enodix-core --test update_check`           |
| Headless binary tests          | `cargo test -p enodix-cli`                                |
| End-to-end smoke test          | `npm run app:build && npm run smoke`                      |
| The same, out of sight         | `npm run smoke:hidden` (a desktop of its own, Windows)    |
| The same, in Windows Sandbox   | `npm run smoke:sandbox` (setup: `testing.md`)             |
| Installer check, clean Windows | `npm run test:installer:sandbox -- --old <setup.exe>`     |
| Containerlab device suite      | `npm run test:lab` (needs a lab: `docs/containerlab.md`)  |
| Python bridge tests            | `npm run test:python` (Python 3.10+; network once)        |
| Linux checks (core, cli, Py)   | `npm run test:linux` (in WSL on Windows: `docs/linux.md`) |
| Drive the Linux app (WSLg)     | `npm run test:linux-desktop` (setup: `docs/linux.md`)     |
| Linux release (in WSL)         | `npm run release:linux` (`docs/releasing.md`)             |
| Release build (no upload)      | `npm run release` (env vars: `docs/releasing.md`)         |
| Test SSH server by hand        | `cargo run -p enodix-core --example test_ssh_server`      |

The smoke test starts the test SSH server and the built app, drives the real
interface over WebView2's DevTools port and checks what is on screen. Set
`SMOKE_SCREENSHOTS=<folder>` to keep pictures of the window. On a computer
someone is using, run it with `npm run smoke:hidden`: `npm run smoke` puts
the app's windows on their screen and takes the keyboard, and their own
typing and clicking then fails it at random steps. One smoke run at a time
(its ports are fixed).

`cargo build -p enodix` in debug loads the Vite dev server, so use `app:dev` or
`app:build` to run the app.

## Conventions

- Tests assert on what a user would observe (bytes received, session states), through
  the core's public API. No mocks of internals; extend `tests/support` instead.
- `russh` is pinned to an exact version with the `ring` backend and the legacy
  features (`des`, `dsa`, `rsa`); bumping it is a deliberate change.
- The user-visible app name is defined once: `productName` in
  `src-tauri/tauri.conf.json`. Do not hard-code it anywhere else; the shell's
  `app_name` command serves it to the interface.
- The core must not contain Windows-only assumptions.
- In shell heredocs, backslash escapes and apostrophes get mangled by the agent Bash
  tool; write files with the file tools instead.
- No AI credit anywhere: no `Co-Authored-By` trailer for Claude or
  `noreply@anthropic.com`, no `Claude-Session` trailer, no "Generated with
  Claude Code" line or session link in pull request descriptions or comments.
  `.claude/settings.json` turns these off; this rule covers sessions that do
  not read it. The required `Attribution` check only fails a Claude
  `Co-Authored-By` line in the description or a commit message, or a commit
  authored or committed as Claude. It ignores a "Generated with Claude Code"
  line and never looks for a `Claude-Session` trailer, a session link, the
  title or comments, so a pass is not proof: keep to the rule itself.

## Agent skills

### Issue tracker

Issues and specs live in GitHub Issues, through the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the root, created when first needed. See `docs/agents/domain.md`.
