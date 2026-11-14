# Checking netterm on Linux

Everything below the window (the core, the headless binary `netterm-cli`
and the Python bridge) is checked on Linux as well as on Windows. The
window itself (`src-tauri`) is not built there yet.

```
npm run test:linux
```

runs, on Linux:

1. `cargo clippy -p netterm-core -p netterm-cli --all-targets -- -D warnings`
2. `cargo test -p netterm-core -p netterm-cli`
3. `npm run test:python` (`node scripts/test-python.mjs`)

On Linux it runs in the checkout. On Windows it runs in WSL: the working
tree is copied with rsync (following `.gitignore`, so no `target/` and no
keys) into `~/netterm-linux-check` inside the distribution, and the checks
run there. Building from `/mnt/<drive>` would be slow and would write a
Linux `target/` into the Windows checkout. `NETTERM_WSL_DIR` changes the
folder, `NETTERM_WSL_DISTRO` the distribution (default: WSL's default one).

## Setting up WSL once

A WSL 2 distribution with network access (`wsl --install Ubuntu`), then
inside it:

```
sudo apt-get install build-essential pkg-config libudev-dev rsync \
  python3 python3-venv nodejs
curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy
```

`libudev-dev` is for the `serialport` crate. Rust must be 1.90 or newer.
The Python suite installs Nornir from the package index the first time.

## What is skipped on Linux

- `tests/keychain.rs` and the headless binary's keychain test: the
  keychain store is Windows' Credential Manager only; elsewhere every call
  is an error. So turning the vault on (which moves the keychain's
  entries) fails on Linux until a Linux keychain is supported.
- `connection_errors::an_unreachable_network_is_reported`
  (ignored): it relies on Windows' handling of 240.0.0.0/4.
- Pageant, the credential agent pipe's access list, and a vault file that
  cannot be removed (Windows file sharing); the agent's Unix socket mode
  and the binary's terminal prompt have Unix tests of their own.
- The Python package's default install location of the headless binary
  is the Windows installer's; elsewhere it is found through `NETTERM_CLI`
  or `PATH`.

Verified on Ubuntu 26.04 under WSL 2 (Rust 1.100, Python 3.14).
