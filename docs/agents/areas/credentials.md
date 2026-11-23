# Credentials

Credential profiles, named secrets, the vault, the credential agent and the Credentials view.

- `src/credentials.rs` — credential profiles (`CredentialProfile`:
  username, `ProfileAuth::Password | PrivateKey { key_file, passphrase }`,
  optional `enable_secret`, which jobs (#43) and enrichment (#55) send). Stored in a
  `CredentialStore` (trait: `list/get/put/delete` by scope + name,
  names ignoring case): `MemoryStore` for tests, `KeychainStore::new(prefix)`
  (`credentials/keychain.rs`: Windows Credential Manager via `windows-sys`,
  one generic credential `<prefix>:<scope>:<lower-case name>` holding
  the profile as JSON, persisted local-machine; other OSs return errors).
  `KeychainStore::remove_all()` (#111) deletes every generic credential
  named `<prefix>:*` or `<prefix>.secret:*` (by name, readable or not, all
  scopes) → `RemovedEntries { profiles, secrets }`; `<prefix>x:*` is
  another prefix and kept (`tests/keychain.rs`). `count_all()` (#127)
  counts what it would remove, removing nothing. `enodix-cli
  clear-keychain` calls them (`--yes` removes; the uninstaller, data box
  ticked); since #125 the app's start over does too, through the trait
  method `CredentialStore::remove_everything` (default: `contents()` then
  deletes, then `contents()` must be empty; `KeychainStore`: `remove_all`
  then `count_all` must be 0; else "N profiles and M secrets are still
  there after being removed"). Also from #125:
  `CredentialStore::check_fits(&contents)` (default `Ok`; `KeychainStore`
  encodes each entry as written and refuses one over 2,560 bytes,
  `CRED_MAX_CREDENTIAL_BLOB_SIZE`, or a user name over 513 UTF-16 units,
  naming it: `the profile "<name>" (workspace <scope>) is too large for a
  Windows credential (N bytes; at most 2560)…`); entries are encoded from
  borrowed structs into exactly-sized `Zeroizing` buffers and blobs read
  back are `Zeroizing` too (what Windows allocates in `CredRead` /
  `CredEnumerate` is freed by `CredFree`, out of our hands).
  Scope = `Workspace::credential_scope()` = the workspace id (#25, see
  below), else the canonical root path (`path_credential_scope()`, how
  profiles were kept before). Workspace files only hold the name
  (`credential_profile`). `Profiles::new(store,
&workspace)`: `list` (summaries, no secrets), `missing` (names the
  workspace uses with nothing stored), `save(previous, &ProfileDraft)`
  (a `None` secret keeps the stored one, `Some("")` removes passphrase /
  enable secret; renaming does not touch workspace references), `delete`,
  `login(id)` → `HostLogin::Profile { target } | NotSetUp { profile } |
Prompt`, `profile_target(id)`. `SshTarget.auth` is `SshAuth::Password |
PrivateKey` (the key is loaded before connecting; failures are
  `OpenError::PrivateKey`). `Debug` of all of these hides secrets.
  `ProfileAuth::Agent` (no secret) → `SshAuth::Agent(SshAgent::System)`.
- Named secrets (#37, `src/credentials/secrets.rs`). A name (ASCII
  letters, digits, `-_.`, ≤ `MAX_SECRET_NAME` 100, trimmed, matched
  ignoring case) and a non-empty text value per credential scope.
  `CredentialStore` gained `secret_names/get_secret/put_secret/
delete_secret`, kept apart from profiles (same name, both kept):
  `MemoryStore` has its own map; `KeychainStore` writes
  `<prefix>.secret:<scope>:<lower-case name>` (a profile's target has `:`
  right after the prefix, so they never meet) holding `{scope, name,
value}` JSON. `Secrets::new(store, &ws)`: `list` (names only, sorted
  ignoring case), `add` (`SecretError::AlreadyExists`), `replace` (keeps
  the first spelling; `NotFound`), `delete`, `get` → `SecretValue`
  (`expose()`; `Debug` hides it) for later consumers (bridge, NetBox):
  **nothing in the shell or interface calls `get`**. `prepare_workspace`
  moves secrets from the path scope too. Shell: `ProfilesView.secrets`
  (names), `secret_save { name, value, replace }`, `secret_delete`.
  Interface: the Credentials view's Secrets list (`[data-secrets]`, rows
  `[data-secret=<name>]` with `[data-secret-replace]` /
  `[data-secret-delete]`, `[data-secret-new]`), `SecretForm.svelte`
  (`form[data-secret-form]`, inputs `secret-name|secret-value`,
  `[data-secret-action=save|cancel]`, `[data-secret-error]`; the delete
  confirmation's button is `[data-secret-action=delete]`).
  Smoke: in `credentialProfiles`.
- Vault (#38, `src/credentials/vault.rs`). `CredentialStore` gained
  `contents()` → `StoreContents { profiles: [ScopedProfile { scope,
profile }], secrets: [ScopedSecret { scope, name, value }] }` (every
  scope; `.sorted()` to compare; keychain lists `<prefix>:*` and
  `<prefix>.secret:*`). `Vault` = a `CredentialStore` in one JSON file
  `<data folder>/credentials.vault` (`vault_file(dir)`, `VAULT_FILE`):
  plain header (format `enodix-vault`; `netterm-vault`, the name before
  the app was renamed, is still read and replaced at the next save; test
  fixture `tests/fixtures/netterm-format.vault`), version, `kdf` = Argon2id `KdfParams`, default
  64 MiB / 3 / 4, read from the file), `salt`, `check` (sealed empty
  message: fails only for a wrong key → `VaultError::WrongPassword`),
  `nonce`, `body` (XChaCha20-Poly1305, AAD = header), `checksum` (SHA-256;
  a parse/checksum/body failure → `Damaged`, never `WrongPassword`).
  `create(path, pw)` / `create_with(path, pw, kdf, contents)` (refuses an
  existing file or empty password), `unlock(path, pw)`, `check(path)` (#125:
  everything but the key, so `Damaged` before a password is asked). Every
  change takes the lock (below), re-reads the file with the held key (a
  replaced file → `Damaged`), applies, writes a temporary file + rename.
  Lock between copies (#125): `<file>.lock` (`VAULT_LOCK_FILE`), an
  exclusive `std::fs::File::lock` (stable since 1.89; the repo's MSRV is
  1.90; `LockFileEx` on Windows, `flock` on Linux, both per open handle, so
  two handles in one process exclude each other too) held by a private
  `WriteLock` (unlocked on drop) around: `Vault::change` (reread → edit →
  write), `create_with`, `turn_on` (from listing the keychain to emptying
  it and checking it is empty), `change_password` and `turn_off` (from
  re-reading the file to the end), `start_over`, the take-back at unlock,
  and every keychain change of an `ActiveStore` that is off
  (`with_change`: under the lock it looks for the vault file again and
  refuses with "turned on" if it is there; if no lock file can be made it
  goes ahead, as no vault can be written there either; without a keychain
  no lock). Argon2 runs before the lock is taken (`Vault::keyed`), so it
  is held for milliseconds. Under the lock the file is read as it is
  (`read_sealed(path, Some(lock))`, `present`); a reader (`Vault::unlock`,
  `exists`, `check`, the cli) takes no lock, except that a name found
  missing is looked for again after taking the lock shared for a moment
  (`wait_for_writer`: waits out a writer; no lock file → returns), so it
  never concludes "no vault" (which sends a copy to the keychain) while a
  write is under way. The lock file is never removed (removing it under a
  waiter would let a third copy lock a new file); a process that dies lets
  go of the lock with its handles; the file it leaves is no lock. Windows:
  a rename over the vault refused because another program holds the file
  without sharing its deletion (access denied / sharing violation, as an
  antivirus scanner can) is tried again every 10 ms for up to 3 s
  (`rename_over`, `HELD_BY_ANOTHER`) instead of failing the save.
  Key in `Zeroizing`, opened contents wiped on drop
  (`CredentialProfile::wipe`, `SecretValue::wipe`, crate-private); `Debug`
  shows path + kdf only. `ActiveStore::new(keychain, vault_file)` is the
  app's store (the one owner of the unlocked vault, for the later agent
  pipe): `status()` → `VaultStatus::Off | Locked | Unlocked` (Locked iff
  the file exists at start; never a preference), `unlock(pw)` (stays
  until dropped), `turn_on(pw, repeated)` (same password twice, else
  `Invalid`): key derived → lock → no vault file (else `AlreadyExists`:
  of two copies turning it on at once one wins, the other is refused and
  then `Locked`) → keychain `contents()` → vault written (temporary file,
  flushed, renamed) → read back and compared → deleted from the keychain
  → keychain `contents()` again, which must be empty (#125: a delete that
  says done is not believed); any failure removes the vault file and puts
  back what was already deleted (only if a put-back fails too is the vault
  kept, unlocked and in use, with an error). The keychain is never emptied
  before the vault is on the disk; a crash after the rename leaves the
  vault plus copies in the keychain, taken back at the next `unlock`
  (#125): under the lock, every keychain entry the vault lacks is put into
  it (one the vault has is the vault's: the copy is older), the vault
  written, then the copies deleted and the keychain checked empty; a
  failure there is printed to stderr and the unlock stands (tried again
  next time). Plain copies taken while moving are `Wiping` (turn on too,
  #125), and the plain body is encoded into an exactly-sized `Zeroizing`
  buffer (`encoded_exactly` in `credentials.rs`: a growing `Vec` left the
  parts it outgrew in freed memory). Keychain
  `contents()` fails on an entry it cannot decode (nothing left behind
  unnoticed). `unlock` on an unlocked vault still checks the password.
  Locked: every store call
  is a `StoreError` ("the vault is locked"). Shell: `Shell.credentials:
ActiveStore` over the keychain (or the smoke's memory store),
  `vault_status`, `vault_unlock { password }`, `vault_turn_on { password,
repeated }` (errors `VaultFailure { kind: wrongPassword|damaged|invalid|
failed, message }`), `app_quit`; `workspace_restore` refuses while
  locked. Interface: `src/lib/vault.ts`; `App.svelte` asks `vault_status`
  first and, if locked, shows `UnlockVault.svelte` (`[data-unlock-screen]`,
  input `vault-unlock-password`, `[data-unlock=unlock|quit]`,
  `[data-unlock-error=<kind>]`; the capture key handler ignores keys)
  before restoring the workspace. Preferences' `VaultSection.svelte`
  (`[data-vault-status=off|locked|unlocked]`, `[data-vault=turn-on|
confirm|cancel]`, `form[data-vault-form]`, inputs `vault-password|
vault-repeat`, `[data-vault-differ]`, `[data-vault-warning]`,
  `[data-vault-error]`). `[profile.dev.package.argon2|blake2]` are
  optimised (debug Argon2 is very slow). Tests: `tests/vault.rs` (store
  checks on memory/vault/`ActiveStore`, unlock, damage, moves with
  `support/credentials.rs` `FailingStore`), keychain `contents` in
  `tests/keychain.rs`. Smoke: `vault` (last; restarts the app; since #127
  also the "damaged" message: the vault file replaced by text, the master
  password then gives `[data-unlock-error=damaged]` "…cannot be opened
  with any password…" and no workspace, the file put back) and
  `vaultUpkeep` (since #127: Quit on the unlock screen ends the app, which
  is started again to the unlock screen). Two app
  instances writing at once lost about half their changes before #125 (no
  lock); `edge_vault.rs`
  `two_copies_writing_at_once_keep_every_change_they_were_told_was_saved`
  and `two_processes_writing_at_once_…` (real processes) now pass. (A
  system without a keychain uses `ActiveStore::without_keychain`, #57.)
- Edge testing (#117, `tests/edge_vault.rs`, `tests/edge_logins.rs`). What
  was tried and holds: a vault file cut at every length, 600 random byte
  flips, another vault's body under the right header (checksum made right
  or not), a newer format version, an unknown key derivation, a greedy
  memory parameter, a folder in its place: all `Damaged` (or `Io`), never
  `WrongPassword`, never an empty store, and nothing (unlock, put, turn
  on/off, change password, `Vault::create`) writes over the file or the
  keychain. Writes: each has its own temporary file
  `<file>.<pid>-<n>.tmp` (`create_new`, flushed, renamed over; on Unix the
  folder is flushed too), since before two copies shared `<file>.tmp` and a
  copy's save failed with "cannot find the file" when the other renamed it
  first; `ActiveStore::new` removes temporary files of the vault untouched
  for a minute (`LEFTOVER_AGE`, a write cut by a power cut; the old
  `<file>.tmp` too) and leaves younger ones (another copy writing). A
  reader (the cli) reading in a loop while the app writes 150 times never
  sees half a file, on Windows or Linux. Two copies: an `ActiveStore` that
  is off notices a vault file that appeared (another copy turned the vault
  on and emptied the keychain) and is `Locked` from then on (`status()`,
  `unlock`, every store call; the first refused call says it was turned
  on), instead of reading and writing a keychain nobody uses. An unlocked
  copy whose file was turned off elsewhere refuses changes saying so
  ("turned off ... not saved"); one whose master password was changed
  elsewhere says that (not "damaged") and `unlock` with the new password
  now reopens it (before, `unlock` on an unlocked vault only checked the
  password and kept the stale key). `change_password` and `turn_off`
  re-read the file, so they take what another copy wrote. Locked vault and
  a keychain refusing every read (`support/credentials.rs`
  `Failures.reads`, `READ_REFUSED`) met by every feature that needs a
  login: login, `profile_target` (sessions, Files tab), `prompted_target`
  behind a jump host, `host_profile` and secrets (cli), the Credentials
  view's lists, `job::plan` of every step (skipped with
  `SkipReason::Credentials` naming the reason), `LoginBackup::plan`,
  `tunnel::plan`, the agent (answers within 10 s with the reason): all say
  the store's reason at once (no fault found). Not tried: a real
  Credential Manager denying access (it does not deny its own user; the
  failing store stands in), a real power cut, two real app processes (two
  stores in one process share nothing but the files, as two processes
  do; since #125 two real processes writing the vault, the test binary
  run as helpers, and one killed in the middle of its writes, are tested;
  two copies of the app itself are not).
- The reader that saw no vault file (#125).
  `every_write_leaves_a_whole_vault_for_a_reader_at_every_moment` failed
  once in #117 (1486 of 1844 reads `NoVault`). Reproduced on Windows with
  a scratch stress test (not kept): the vault written 2,000 times by one
  handle while four threads `fs::read` it, two `Vault::unlock` it and one
  holds it open 2 ms at a time, four such runs at once, about 30 s each:
  in 2 of 77 runs the name was missing for about 50 ms (seen by every
  reader at the same moment, every write reported done, so inside one
  write's rename). `std::fs::rename` on Windows in Rust 1.100 is
  `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`, falling back to
  `SetFileInformationByHandle(FileRenameInfoEx, REPLACE_IF_EXISTS |
  POSIX_SEMANTICS)` only on access denied. Not the vault's doing: a plain
  rename loop (no vault, 4 readers, one file holder, 3,000 renames a run)
  showed the same gaps on a loaded machine (up to 80 ms) with
  `fs::rename` (5 of 61 runs, then about 11 of 60) and with the POSIX
  rename called directly (7 of 60), and readers were also refused the file
  for a moment ("Access is denied", os error 5) in many runs. The cause
  inside Windows (a file system filter such as the antivirus is likely)
  was not found. The fix is the reader's: a file found missing or refused
  is read again once no writer holds the lock (`wait_for_writer` above).
  After it, 60 runs of the vault stress test: the raw readers still saw
  the name missing in 7 runs, the vault's readers (`Vault::exists` and
  `Vault::unlock`, about 1.2 million calls) never; with the refusal
  handled too, 40 more runs beside the rename loop: raw readers saw the
  name missing in 4 and were refused in 12, the vault's readers (about
  740,000 calls) neither. Found on the way: a program holding the file
  without sharing its deletion made every save fail ("Access is denied"),
  never a gap: now retried for 3 s
  (`a_scanner_holding_the_vault_file_for_a_moment_only_delays_a_save`).
  The scratch tests are not kept (minutes of load each); the test in the
  suite keeps its fuller failure message.
- Credential Manager used by several processes at once (#138). It is
  Windows', not ours: `crates/core/examples/credential_manager_race.rs`
  calls only `CredWriteW`/`CredReadW`/`CredEnumerateW`/`CredDeleteW` (no
  enodix code), from N copies of itself, each under its own
  `enodix-test-race-<hex>` prefix, looping write, read back, overwrite,
  read back, list its own prefix, delete, read, list. On this machine (a
  big real store, other agents' tests running): 1 process, 300 rounds: no
  fault; 16 threads of one process, 3,200 rounds: no fault; 16 processes,
  40 to 200 rounds: faults in most runs, about 1 to 5 per 100 rounds (e.g.
  12 lost writes, 14 old values read after an overwrite, 12 deletes that
  found nothing in 3,200 rounds), fewer with 2 to 8 processes (0 to 23
  faults in 400 to 2,400 rounds; 3 of 7 such runs had none). Kinds: a read right after a write gives `None` or
  the value before (even one from rounds earlier, after it was deleted);
  the listing lacks an entry or shows the old value; a deleted entry comes
  back; a delete finds nothing because the write was lost. With
  `ENODIX_RACE_SETTLE=1` (read again every 5 ms): most wrong reads come
  right within 100 ms (a few up to 500 ms), but some writes never show
  (54 of 960 rounds in one run): lost for good. With
  `ENODIX_RACE_LINGER_MS=200`: a write read back right is gone or old
  200 ms later in about 5 of 100 rounds (53 and 47 of 960). With
  `ENODIX_RACE_LOCK=1` (every call under one file lock, so no two calls
  of these processes overlap): 0 faults in 3 × 640 rounds, but 15 to 30
  times slower (about 36 ms a call here; calls of different processes
  otherwise run in parallel). So: changes of different processes that
  overlap in Credential Manager overwrite each other, for any names (each
  looks like a read-modify-write of the user's whole credential set);
  threads of one process are not affected (their calls seem to be taken
  one at a time). The mechanism inside Windows (lsass) was not looked at.
  Our code had no part in it: the filters (`<prefix>:<scope>:*`,
  `<prefix>:*`, prefix match ignoring case, `*` only at the end) match
  nothing of another prefix and every decoded entry's scope is checked;
  an overwrite is one `CredWriteW`; nothing is kept between calls. The
  fix (`keychain.rs` `os::Turn`): every Credential Manager call of every
  enodix process (app, `enodix-cli`, tests, any prefix) holds the named
  mutex `Local\enodix-credential-manager` (this logon session; abandoned
  by a dead holder = taken; waits up to 30 s, then "the Windows
  credentials are held by another copy of the app for more than 30 s");
  `os::write` and `os::delete` read the entry back in the same turn and
  refuse a change not kept ("Windows did not keep the change, so the
  profile was not saved (another program was changing the Windows
  credentials at the same moment?): try again"; no retry). The read-back
  is only a net: a program that does not take turns (another program, a
  enodix older than #138) can still undo a change a moment after it was
  read back right, which nothing here can see. Since it is not the vault's
  lock (per data folder, writes only), it also covers reads, the cli and
  copies with other data folders. Tests: `tests/keychain.rs`
  `several_processes_using_the_keychain_at_once_each_read_back_what_they_wrote`
  (4 helper processes × 10 rounds of save, read, change, read, secret,
  `contents`, delete, read, `count_all` through `KeychainStore`, role
  `keychain-user`), and `…_under_load` (`#[ignore]`: `cargo test -p
  enodix-core --test keychain under_load -- --ignored --nocapture`;
  `ENODIX_TEST_KEYCHAIN_PROCESSES` / `ENODIX_TEST_KEYCHAIN_STRESS_ROUNDS`,
  default 16 × 100, which takes about 10 minutes taking turns). 8 × 30
  with the turn switched off for the experiment: 24 and 33 inconsistencies
  (plus 1 and 5 refused by the read-back); with it: 0, in 68 s. The bare
  program: `cargo run -p enodix-core --example credential_manager_race --
  16 40` (`count` counts `enodix-test-*` entries and the ones older than
  an hour; `clean` removes what a stopped run left, its own names only).
  Leftovers: the "not enough memory resources" (os error 8) refusals came
  with some hundreds of `enodix-test-*` entries left by test processes
  killed part-way (a test that panics unwinds and its guard removes its
  prefix; a killed one removes nothing). `tests/support/keychain.rs`
  `sweep_leftovers()` (once per test process, from `TestKeychain::new`
  and the cli's two keychain tests, which include the file by `#[path]`)
  removes each `enodix-test-` prefix whose every entry was last written
  (`CREDENTIAL.LastWritten`) more than an hour ago, through `remove_all`
  (so in turn), and only if every name under it starts with the prefix
  exactly (the filter ignores case); a prefix with one young entry is
  kept. It removed 100 such entries on its first run here. Not tried: the
  error 8 limit itself (filling the owner's store on purpose), two copies
  of the real app, a holder that hangs for 30 s.
- Vault upkeep (#39, `vault.rs`). `ActiveStore::change_password(current,
password, repeated)` (unlocked only, else `Locked`/`NoVault`; `current`
  re-opens the file → `WrongPassword`; new salt and key, same `KdfParams`,
  atomic replace, so a failure leaves the old password working).
  `turn_off(password)` (master password asked again): vault contents
  (re-read under the lock) → `keychain.check_fits` (#125: an entry too
  large for a Windows credential is named, "nothing was moved, and the
  vault stays on") → keychain puts → keychain `contents()` must hold every
  entry → vault file removed → `Off` (a crash before the removal: vault
  plus copies, taken back at the next unlock). Before each put it notes the keychain's own entry of
  that name (or that it had none); any failure (put, read-back, file
  removal) puts the keychain back as it was and keeps the vault unlocked
  and in use (`VaultError::Store`, says if the put-back was incomplete).
  `start_over()` (Locked only, so also a damaged file; Unlocked →
  `Invalid`: turn it off instead), under the lock: the keychain's
  `remove_everything` first (#125: copies a cut-short turn on left would
  otherwise come back after a warning that calls them gone; if it fails
  the vault is not deleted and the error says so), then the file → `Off`.
  Shell: `vault_change_password
{ current, password, repeated }`, `vault_turn_off { password }`,
  `vault_start_over` (both → status). Interface: `UnlockVault.svelte`
  `[data-unlock=start-over]` → `[data-start-over]` panel
  (`[data-start-over-warning]`, `[data-start-over=cancel|confirm]`,
  Cancel focused, Escape cancels) → `onstartedover` reopens the workspace;
  `VaultSection.svelte` when unlocked: `[data-vault=change|turn-off]`,
  `form[data-vault-change-form]` (`vault-current|vault-new|
vault-new-repeat`, `[data-vault=change-confirm]`),
  `form[data-vault-off-form]` (`vault-off-password`,
  `[data-vault=turn-off-confirm]`), `[data-vault-done]` after success.
  Tests: `tests/vault.rs` (`support/credentials.rs` `Failures.failing_put`
  fails one put; a Windows-only test holds the file open so it cannot be
  removed). Smoke: `vaultUpkeep` (after `vault`: change + restart, turn
  off + restart, turn on + profile + restart + start over).
- Credential agent (#49, `src/credentials/agent.rs`).
  `AgentEndpoint::for_data_folder(dir)`: Windows
  `\\.\pipe\enodix-agent-<24 hex of the lower-cased absolute folder's
SHA-256>`, else `<dir>/agent.sock` (so the smoke's and tests' data folders each get their own).
  `AgentServer::start(endpoint, Arc<dyn CredentialStore>)` (own thread,
  current-thread tokio runtime; dropping stops it): Windows pipe created
  as first instance (`AgentError::InUse` if it exists), remote clients
  rejected, DACL `D:P(A;;GA;;;<user SID>)` (read back as `FA` in the test);
  Unix socket bound as `agent.sock.<pid>`, chmod 0600, then renamed into
  place (#127: a client never sees it open to others), peer uid checked
  against the socket's owner, a stale socket replaced. Protocol (private): one JSON line each way,
  requests `status` / `profile {scope,name}` / `secret {scope,name}`,
  answers `status` / `profile` / `secret` / `locked` / `failed`; buffers
  holding secrets are zeroized; store calls run in `spawn_blocking`.
  `AgentStore::connect(&endpoint)` → `None` if nobody listens (Windows
  retries a busy pipe 5 s; opens with `SECURITY_IDENTIFICATION` so the
  server cannot impersonate it, then refuses with `AgentError::NotOurs`
  if the pipe's server process (`GetNamedPipeServerProcessId`) runs as
  another user SID: the name is predictable, so it can be squatted;
  Unix (#127): the path must be a socket (`symlink_metadata`: a file or a
  link → `AgentError::NotAgent("… is not a socket")`), owned by `geteuid`
  (else `NotOurs`), with no group/other bits (`NotAgent("… can be opened by
  other users (mode …)")`), and the connected peer's uid (`SO_PEERCRED`,
  `getpeereid` off Linux) this user's (else `NotOurs`): a data folder shared
  with others could hold another user's socket. Missing / nobody listening
  → `None` as before; answers capped at 1 MiB); `status()`;
  implements `CredentialStore` for `get`/`get_secret` only (the rest
  refused; locked → `VaultError::Locked`'s text), so `Profiles`/`Secrets`
  run over it unchanged. New trait method `CredentialStore::vault_status()`
  (default `Off`; `Vault` → `Unlocked`; `ActiveStore` → its status;
  `AgentStore` → the remote one, `Locked` if it cannot ask).
  `Profiles::host_profile(id)` → the resolved SSH profile with secrets
  (`NotFound`, `Invalid("… has no credential profile")`, `NotSetUp`).
  Output types for the binary (format 1, `Debug` hides secrets, wiped on
  drop): `HostCredentials { format, host, profile, username, auth
(ProfileAuth's serde: password | private_key {key_file, passphrase} |
agent), enable_secret }`, `NamedSecret { format, name, value }`. Shell:
  `Shell.credentials` is `Arc<ActiveStore>`; setup starts the agent on the
  data folder (`CredentialAgent` state; a failure is logged and the app
  goes on; a squatted name thus only disables the agent). Trusts every
  process of the same user (as an SSH agent), for every scope. Tests:
  `tests/credential_agent.rs` (memory store, unlocked vault, locked then
  unlocked `ActiveStore`, read-only, two clients, second server refused,
  the DACL / socket mode; refused cases since #127: Windows
  `a_client_that_is_not_the_current_user_cannot_open_the_pipe`, another
  user played by this user's token with its SID deny-only (impersonated
  on a thread: read and read-write refused, while a default-DACL pipe
  stays readable to it, the control), Unix
  `a_socket_others_can_open_or_a_path_that_is_no_socket_is_not_asked`
  (mode 0666, a file, a symlink to another agent's socket). Not playable:
  a real other user, so neither the pipe server SID check nor the Unix
  owner and peer checks are refused in a test).
- Credentials view (#66). `CredentialsView.svelte` replaces the profiles
  dialog (gone, with `profiles.dialog`): `[data-credentials][data-mode=
list|edit|secret|delete|deleteSecret]`, header `[data-profile-new]`;
  every form opens in the view in place of the lists (Escape or Cancel
  goes back): `[data-credentials-form=profile|secret]` titles over
  `ProfileForm` / `SecretForm`, confirmations `[data-credentials-confirm=
profile|secret]` (`[data-profile-action=delete]` /
  `[data-secret-action=delete]`). Lists: the vault's state
  `[data-credentials-vault=off|locked|unlocked]` (from `profiles.vault`)
  with `[data-open-vault]` (Preferences), `[data-profiles-needs-vault]`
  without a keychain, profiles `[data-profiles]` rows
  `[data-profile=<name>]` (no secrets: username, login kind, enable
  secret) with `[data-profile-edit]` (also renames) /
  `[data-profile-delete]`; missing ones first, `[data-profile-missing]`
  (warning colour) with `[data-profile-setup]` (the form with the name
  fixed); none at all → `[data-profiles-empty]` (what a profile is and,
  since #116, where a host names one) + `[data-profile-first]`; under a
  list, `[data-profiles-use]` says a host uses the profile named in the
  Credential profile setting of its details or its folder's (#116; the
  setting has a note too, and a typed login `[data-credentials-hint]`,
  see `window.md`); secrets as in #37 (`[data-secret-new]` on the
  Secrets heading). Leaving the view (another rail icon) drops an open
  form. Smoke: `openProfiles(evaluate)` shows the view with its lists,
  `newProfile(evaluate)` opens the form; parts switch back to Hosts
  (`showSideView(…, "hosts")`, then `showDetails` where they edit the
  host) before using the tree or restarting, since the side view is
  remembered. `credentialProfiles` checks the empty state, rename (the
  old name then missing) and delete; `vault` sets up a missing profile
  and checks the vault state off → unlocked; `noKeychain` as before in
  the view.

- Security review (2026-11-22): new master passwords need 8 characters
  (`MIN_PASSWORD_CHARS`, create, turn on, change; unlocking an older short
  one still works); `derive_key` refuses more than 16 passes or lanes
  (`MAX_ITERATIONS`, `MAX_PARALLELISM`) besides 4 GiB; `WrongPassword`
  says the file may have been altered. On Unix the `private` module makes
  the vault, its temporary files and its lock 0600 and its folder 0700,
  and tightens existing ones at unlock and at `make_folder`. The Linux
  agent trusts `geteuid()`, no longer the socket file's owner read after
  the rename. The uninstaller removes `ENODIX_KEYCHAIN_PREFIX` from its
  environment before `clear-keychain --yes` (`hooks.nsh`; the sandbox
  installer check sets it to the other program's prefix and requires that
  program's entry to stay).
