# Jobs

Running snippets on many hosts: planning, the engine, kept runs, the Jobs view and its tabs, saved jobs, live jobs and run windows. Config backups, script jobs and enrichment have their own files.

- Jobs (#42, `src/job.rs` planning, `src/job/run.rs` engine). A consumer
  of `SessionManager`'s public API (no session internals). `job::plan(ws,
&Profiles, &JobTarget, &JobStep)` connects to nothing → `JobPlan { step,
hosts: [PlannedHost { id, name, address, environment, platform }],
skipped: [SkippedHost { id, name, reason: SkipReason, message }],
production, missing_variables }` (camelCase; each `PlannedHost` keeps its
  `profile_target` (jump hosts included) and lines privately, not
  serialised). `JobTarget::Folder { path }` (hosts, then subfolders, as
  `allHosts`) | `Hosts { ids }` (deduped; one host, or a selection of
  several in the tree's order since #110: the context menu's job entries;
  a saved job still takes one host only, so `unsaveable` refuses more);
  `JobStep::Snippet { name, answers }` (serde `kind`; open for more step
  kinds). Skip order: `Gone` (id no longer in the workspace), `NotSsh`, `NoPlatform { platform }` (none, or no
  usable definition), `NoProfile`, `ProfileNotSetUp`, `JumpChain`,
  `JumpHostNeedsLogin` (a hop with `SshAuth::Prompt`), `Credentials`
  (store error). Variables: `expand_snippet_for_each` over the hosts that
  run, so a variable any of them lacks is asked once and fills only the
  gaps. `job::start(&manager, plan, JobOptions { parallel
(`DEFAULT_PARALLEL` 10, 1–`MAX_PARALLEL`50, else`StartError::Parallel`), command_timeout (60 s of quiet), command_limit (30 min) })` → `Job` (`next()`
  events, `cancel()`, `canceller()`; dropping it does not stop the job;
  closing the app does since #124, through `RunningJobs`, below) or
  `StartError::NothingToRun|MissingVariables|Parallel`. Per host (a Semaphore
  task): `Waiting` → `Connecting` (`manager.open`, never `open_logged`;
  `JOB_TERMINAL_SIZE` 400×100) → `Running` → `Done | Failed { reason } |
Cancelled`. A `HostKey` question is answered `Cancel` and a login
  `Prompt` closes the session; the reason names the key (address, port,
  fingerprint) or the questions. Prompt: on `PlainText::current_line()`
  (new, public), quiet for a settle; since #136 the very prompt line a
  line was typed at, else a changed one by the rules of "The end of a
  command" below (was: the pattern + `[ \t]*\z` and `PROMPT_SETTLE`
  300 ms, whatever the line); after a line is sent only new
  output counts. A remote exit after the last line is `Done`. Output =
  `PlainText` of everything (echo and prompts included), streamed as
  `JobEvent::Output { host, text }` (complete lines; the last piece at
  the end); `JobEvent::State { host, state }`; last `Finished(JobReport
{ hosts: [HostResult { id, name, state, output }] })` in plan order.
  Driving (#43), after the first prompt: the definition's
  `privilege_command` if any (prompt or `privilege_prompt` question
  awaited; the question gets the profile's `enable_secret`, kept in the
  plan as `EnableSecret` (hidden `Debug`, zeroized); no secret → fails
  naming the profile; the question again after it, or the very prompt
  shown before `enable`, → "refused"; with no `privilege_prompt` a
  timeout says so; since #122, no question and back at the very prompt
  it was typed at is privileged mode only if the device said nothing
  between the echo and the prompt (a login privileged already), else
  "\"enable\" did not reach privileged mode: the device answered \"%
  No password set\" and stayed at r1>" (`said_after`); a changed prompt
  is fine whatever was said), then `disable_paging` if any, then the
  lines (a line, or the disable-paging command, that gets the password
  question fails the host: a job types nothing into it). The first wait
  after login ending at the privilege password question fails the host
  (#122: "the device asked \"Password:\" after logging in, before its
  first prompt; a job never answers login questions"), so neither
  `enable` nor the secret is typed into a login question. Blank
  definition values count as unset. Each wait (#122) gives up after
  `command_timeout` of quiet, counted from the last output ("no prompt
  after \"hang\": the device showed nothing more for 1 s"), or, while
  output keeps arriving, after `JobOptions::command_limit`
  (`DEFAULT_COMMAND_LIMIT` 30 min; "no prompt after \"…\": it was still
  showing output after 30 min, the most one command may take"); the
  wait after login says "after logging in" in both. File and script jobs
  use neither. The Semaphore permit is held until the host's
  session is closed (`close` + waiting up to `CLOSE_WAIT` 2 s for
  `Closed`, then `discard`), so at most `parallel` connections are ever
  open (unless a close takes longer than that); cancel interrupts waiting for room too (states `Waiting` →
  `Cancelled`), and output arriving while closing is kept.
  Shell: `job_plan { target, step }` (keeps the plan in `JobPlans` and
  returns it with its `id`, see "Plans on screen" below),
  `job_start { plan, confirmedProduction, parallel?, label }` (runs
  exactly the plan kept under `plan`, kept if the start is refused;
  returns `{ id, run }` since #44; since #77 its
  events are broadcast to every window until `finished`, whoever
  listens), `job_cancel(id)`
  (`RunningJobs`). Interface: `src/lib/jobs.ts`, a job tab since #69
  (`JobTab.svelte`, `[data-view=job][data-step=snippet|variables|preflight|running]`,
  select `job-snippet`, `[data-job-snippet-text]`, inputs
  `job-variable-<i>` with `data-variable`, rows `[data-job-host=<id>]`,
  `[data-job-skipped=<id>][data-skip-reason=<kind>]`,
  `[data-job-production]` checkbox `job-confirm-production` naming the
  hosts in `[data-job-production-names]` (Start disabled until ticked),
  `[data-job-parallel]` number input `job-parallel` (default 10, Start
  disabled outside 1–50), `[data-job=plan|start|cancel|close|dismiss]`,
  `[data-job-progress][data-finished]`, `[data-job-result=<id>]
[data-host-state=<kind>]` with `[data-job-reason]`, `[data-job-output]`),
  opened by `openJob(target, label)` (a job tab, #69) from
  FolderDetails' `[data-folder-run-snippet]`, HostDetails'
  `[data-host-run-snippet]`, the filter bar's `[data-filter-run-snippet]`
  and the tree's context menu ("Run snippet…"). Closing the tab while
  running leaves the job running (#69). Test server:
  `TestSshServer::start_device(hostname)` / `start_device_on` = device
  mode (prompt `device_prompt(h)` = `<h># `, `show hostname`, `echo
<text>`, anything else `UNKNOWN_COMMAND` + line; `prompt()`); #43
  added: `lines <n>` paged by `PAGE_LENGTH` 24 with `MORE` (any key but
  `q` goes on), `terminal length <n>` (0 = off), `enable` (asks
  `ENABLE_QUESTION`, `ENABLE_SECRET`, `DENIED_SECRET` → `ACCESS_DENIED`
  and back at `>`, three wrong → `BAD_SECRETS`),
  `disable`, `hang` (no prompt until Ctrl+C), `pause <ms>`;
  `start_unprivileged_device(h)` starts at `unprivileged_prompt(h)` =
  `<h>> `; `connections_open()` / `most_connections_open()` count
  connections from their first session channel until the handler drops.
  #122 added: `drip <n> <ms>` (a line every `ms`, then the prompt),
  `set_enable(EnableMode::Secret|NotSet|Notice(text))` (`NotSet` prints
  `NO_ENABLE_PASSWORD` and stays at `>`, as IOS without an enable
  password; `Notice` prints the text and goes to `#`),
  `set_config_pace(d)` (show-config a line every `d`), `show
  running-config` at `>` prints a `^` line and `INVALID_INPUT`,
  `ask_password_at_login()` (every shell asks `LOGIN_QUESTION` before
  its first prompt until `PASSWORD`; `typed_at_login()` lists what was
  typed there).
  The example server's `--device <port>` (`lab-r1`, privileged, the
  smoke's 22725). Tests: `tests/jobs.rs` (privilege with/without/wrong
  secret on `eos`, paging vs a definition without `disable_paging`,
  timeout, parallel limit on one server with many hosts, cancel with
  done/running/waiting hosts; #122:
  `a_privilege_command_that_stays_at_the_prompt_saying_something_fails_the_host`
  (no password set / a notice and `#` / privileged already),
  `a_password_question_right_after_login_fails_the_host_and_is_never_answered`,
  `output_still_arriving_after_the_command_timeout_is_waited_for`,
  `output_that_never_ends_fails_its_host_at_the_command_limit`).
  Known limit: a device whose privilege command comes back to the same
  prompt saying nothing, without reaching privileged mode, still counts
  as reached (it cannot be told from a login privileged already). Smoke: `jobs` (profile, folder with
  `platform: frr`, pre-flight skips + production confirmation, the
  untrusted key warned of at pre-flight (#129) and failing at run time,
  trusted in a terminal (no warning then), a run from the host panel with 1 host
  at once (0 and 51 refused) whose output starts with `terminal length
0`, then `hang` cancelled keeping its output). On real devices: the
  built-ins' privilege and paging values since #47, and the enable
  password question since #113 (a privilege-1 login on cEOS and on the
  CSR 1000v: asked, answered, and a wrong secret refused; see
  `docs/containerlab.md` and `testing.md`).
- The end of a command (#136, `src/job/run.rs` `HostRunner::shown`,
  `wait_for`, `Expect`, `changes_prompt`). The end of a command is the
  device's prompt, not text that resembles one. The runner keeps `at`,
  the trimmed prompt line it last came back to (the one the next line is
  typed at), and `whole_lines`: whether the pattern matched the whole
  first prompt line (`prompt_line` = `\A[ \t]*(?:p)[ \t]*\z`; else only
  `prompt_end` = `(?:p)[ \t]*\z`, today's rule, is used for changed
  prompts, so a user pattern written for the line's end, `[>#]`, keeps
  working). Each wait has an `Expect`; what counts, and the quiet it
  needs (from the last output; output restarts it, so nothing is typed
  and no session closed while output arrives):

  | Where the job is | Counts as the prompt | Settle |
  | --- | --- | --- |
  | First prompt after login (`First`) | a line the pattern matches whole | `PROMPT_SETTLE` 300 ms |
  | | the pattern at the end of a longer line | `PROMPT_CHANGE_SETTLE` 1 s |
  | After any line but the first wait | the same line as `at` (alone on its line, blanks aside) | 300 ms |
  | After a line expected to change it (`Change`): the privilege command and the enable secret's answer; a snippet line whose first word (`/` aside, any case) is or abbreviates configure (≥ conf), enable (≥ en), disable, end, exit, quit, return, edit, enter, commit, prompt, hostname, system-view (≥ sys), cli, vtysh, start, shell, bash, tclsh, ssh, telnet, su, sudo, cd; any snippet line typed at a prompt other than the one the snippet started at (inside a configuration mode) | a changed line the pattern matches whole (`whole_lines`), else at its end | 1 s |
  | After other lines (`Same`): `show` commands, the disable-paging command | a changed line, as above | `PROMPT_DOUBT_SETTLE` 3 s |
  | After the show-config command (`Only`) | only the same line; a quiet stop at another prompt-like line fails "…, and its last line \"core-7#\" is not the prompt \"show running-config\" was typed at (r1#); nothing was backed up" | 300 ms |
  | The privilege password question | its pattern at the end of the line, as before | 300 ms |

  A settle longer than what is left of the command timeout ends with it
  (a 1 s timeout takes a changed prompt after 1 s). Why the first prompt
  is safe with 300 ms: banner and message-of-the-day lines end with
  their line break, so they are never the line being shown for long; a
  last line that only ends like a prompt (words, then `core-1#`) needs
  1 s; and a misread one (a banner line that is all prompt-like, then a
  pause) costs time, never output: the first line is typed early, the
  device answers it at its real prompt, which differs from `at` and so
  is waited for as a changed prompt (3 s, or 1 s after `enable`), and
  every later prompt (the show-config command's included) is compared
  with the real one
  (`a_banner_taken_for_the_first_prompt_costs_time_not_output`). Back up
  on login runs the same `drive` (same rule).
  Cost: a prompt that is the same line costs what it did (300 ms);
  1 s once per changed prompt where expected, 3 s where not, 1 s once
  per host for an end-only pattern. Measured (debug build, shared
  machine): `edge_job_scale` 100 hosts 12.7 s at 10 at once and 2.7 s at
  50, before and after; `jobs` 6.7/6.8 s, `config_backup` 5.0/5.1 s (3
  tests more), `login_backup` 5.0/5.1 s, `edge_jobs` 13.2/13.4 s (7 tests
  more), `edge_job_kinds` 3.6/3.6 s. Known limits: text equal to the
  prompt line itself (`r1#` alone in a configuration) followed by a
  pause still ends the command (it cannot be told from the prompt); a
  pause longer than 3 s after a whole prompt-like line in a `show`
  answer (1 s inside a configuration mode) still does; a command the
  list does not know that changes the prompt (`cd` aside, a Linux
  prompt's path) costs 3 s per line; a device whose prompt line carries
  something that changes (a clock) would wait 3 s per line and fail its
  backups. Test server (`support/mod.rs`): `halt <ms> <text>` (text,
  pause, then `HALT_REST` and the prompt), `log <ms> <text>` (prompt,
  then a log line and the prompt again), `rename <name>` (as `prompt`,
  a command not in the list), `set_config_stall(line, pause)` (show-config
  pauses after that configured line, before its break),
  `set_shell_start(pieces)` (pieces after pauses in place of the first
  prompt). Tests: `edge_jobs.rs`
  `a_pause_after_text_like_the_prompt_does_not_end_the_command`,
  `a_table_line_ending_like_the_prompt_and_a_pause_do_not_end_the_command`,
  `a_changed_prompt_counts_only_after_a_longer_settle` (config mode in
  and out, renamed, `rename`: gaps between output events ≥ the settles),
  `the_privileged_prompt_counts_after_the_longer_settle`,
  `a_two_line_prompt_is_followed_and_a_pause_in_an_answer_is_not_taken_for_it`
  (`A:srl1` on `srlinux`),
  `a_login_banner_ending_like_a_prompt_is_not_taken_for_it`,
  `a_banner_taken_for_the_first_prompt_costs_time_not_output`,
  `a_log_message_between_two_prompts_is_kept_and_waited_for` (the last
  two passed before too); `config_backup.rs`
  `a_configuration_line_like_a_prompt_and_a_pause_after_it_are_kept_whole`,
  `a_pause_in_the_middle_of_a_long_configuration_keeps_it_whole`,
  `a_configuration_ending_at_another_prompt_is_not_kept`;
  `login_backup.rs`
  `a_pause_after_a_configuration_line_like_a_prompt_is_backed_up_whole`.
  Not verified on the lab devices yet (`docs/containerlab.md`).
- Job results (#44, `src/job/results.rs`, `src/diff.rs`). `RunStore::
new(root)` (shell: `DataFolders::jobs`, never a workspace; since #128
  the local data folder's `jobs` on Windows, `release-linux.md`):
  `<root>/<workspace id>/<run id>/` with `run.yaml` (`StoredRun { id,
workspace, target, step, started, ended (None = running or interrupted),
hosts: [RunHost { id, name, address, state, file }], skipped: [{ id, name,
reason }] }`, plain snake_case = IPC) and `output/<tree path>/<stem>.txt`
  per host. Run id = local start `YYYY-MM-DD_HH-MM-SS[_n]`.
  `begin(ws_id, &target, &plan)` writes it all before the start →
  `RunRecorder`; `record(&JobEvent)` saves the description (temp file +
  rename) and appends the output that came, at most every `SAVE_EVERY`
  (250 ms; since #119, below: it used to on every event), and on
  `Finished` writes what waits, rewrites the files that missed a piece
  with the report's output and sets `ended`; `flush()` (and drop) writes
  what waits. Two hosts whose output files would be one (`r1.yaml`,
  `r1.yml`) get `r1.txt` and `r1_2.txt` (#119). `list(ws)` → `RunList
{ runs (newest first by `started`, then id), damaged: [{ id, reason }] }`;
  `run`, `output(ws, &OutputRef { run, host })`, `search(ws, run, text)`
  (substring ignoring case → `SearchHit { host, name, count, lines (≤
`MAX_FOUND_LINES` 100) }`, blank = none), `compare(ws, &old, &new)`,
  `export(ws, run, folder)` (copies to `<folder>/<run id>[_n]`),
  `delete` (export and delete of a run being recorded:
  `ResultsError::Running`; clones of a store share its active set; a
  recorder stops counting at `Finished` or when dropped; `running(ws)`
  lists them). Workspace and run ids must be plain names (no `.` start, no
  separators): `BadWorkspaceId` / `NoSuchRun`. Nothing pruned or
  encrypted. `job::plan_again(ws, &Profiles, &StoredRun)` = `plan` on
  its target and step (answers reused). `job::check(&plan, &options)` =
  `start`'s refusals, without starting. `diff::diff_lines(old, new)` →
  `DiffLine::Same { old, new, text } | Removed { old } | Added { new }`
  (Myers after common prefix/suffix; beyond `MAX_DIFF_EDITS` 2000 the
  middle is all removed then added), `diff::lines` (LF/CRLF); meant for
  config history (#45) too. Shell: `JobPlans` keeps `KeptPlan {
workspace id, target, plan }`s (core `job::KeptPlans`, #121);
  `job_start` refuses a workspace without an
  id, checks, `begin`s, then starts; returns `{ id, run }`; its event loop
  records then forwards. `RunningJobs` (id → canceller; the core's since
  #124), `job_runs`
  (`RunList` + `running` ids), `job_run`, `job_run_output { output }`,
  `job_run_search { run, text }`, `job_compare { old, new }`,
  `job_run_export { run, folder }`, `job_run_delete`, `job_run_again(run)` → `{ target, plan }` (kept for
  `job_start`). Interface: `jobs.svelte.ts` (`liveJobs`: jobs started
  here with states/outputs and their workspace root, `launchJob`,
  `runningCount(elsewhere?)` (the header counts the open workspace's;
  its title mentions the others), `liveJob(run)` (open workspace only),
  the job tab shows its live job; a finished one is forgotten when its
  tab goes). The rail's `[data-rail=jobs]` carries `data-jobs-running`
  and a pulsing mark. Since #69 the runs are listed in the Jobs side
  view (rows `[data-run=<id>][data-run-state=running|ended|interrupted]`,
  `[data-runs-empty]`, `[data-run-damaged]`) and each opens in a run tab
  (`RunView.svelte`, `[data-run-details=<id>]`,
  hosts `[data-run-host=<id>][data-host-state]`, `[data-run-output]`
  (live while running here, else from disk), form `[data-run-search]`
  input `run-search` → `[data-search-hit=<host>][data-count]` /
  `[data-search-none]`, selects `compare-old` (a host of the run) /
  `compare-new` (`<run>|<host>`: another host, or that host in another
  run of the same snippet) → `[data-diff][data-diff-changes]` rows `[data-diff-line=same|
added|removed]`, form `[data-run-export]` input `run-export-folder` +
  Browse…, `[data-run-notice]`, `[data-run-action=cancel|again|delete|
confirm-delete|keep|search|compare|save]`, `[data-run-error]`). Run
  again opens a job tab with `planned` (at pre-flight, or its variables).
  An interrupted run's unfinished hosts read "Not finished".
  Tests: `tests/job_results.rs` (a real run recorded; order across a
  restart; interrupted run; search; compare hosts and runs; export,
  delete; a running run refused both; run again with hosts added/gone; damaged and bad names),
  `tests/diff.rs`. Smoke `jobs`: the hang job's tab closed (notice,
  rail), the side view's order, cancel there, output, search, compare with the refused
  run, Save to a temp folder, delete (gone on disk), run again's
  pre-flight. Known limits (`liveJobs` was per window until #77: now
  the shell keeps it); the answers given to a run are
  kept in `run.yaml` in clear; a failed write while recording is only
  logged to stderr.
- Plans on screen (#121, `src/job/kept.rs`). Until #121 the shell kept
  one plan for the whole app and Start ran the last one made, so with two
  job tabs at pre-flight one tab's Start could run the other's plan
  (production hosts included, unasked). Now core `KeptPlans` keeps each
  plan under a `PlanId` (a `u64`, never reused) given when it is made:
  `keep(KeptPlan) -> PlanId` (at most `MAX_KEPT_PLANS` 32, the oldest
  forgotten), `get`, `ready(id, confirmed: &[HostId]) -> Result<&KeptPlan,
PlanRefused>` (`Gone`: "this pre-flight is no longer current (the job
  started already, another workspace was opened, or many jobs were
  planned since): plan the job again"; `ProductionNotConfirmed(names)`:
  "the production host jobs-r1 was not confirmed for this job: confirm it
  in its pre-flight", when a production host of the plan is not in
  `confirmed`), `forget(id)`, `clear()`. Shell: `job_plan`,
  `job_run_again` and `saved_job_plan` return the plan with its `id`
  (`ShownPlan`, the plan flattened); `job_start { plan,
confirmedProduction, parallel?, label }` checks `ready` before and after
  the interpreter check and `forget`s the plan once started;
  `job_plan_forget { plan }`; `Shell::open` (another workspace) clears
  them. So the production question is enforced by the shell for the plan
  started, not only by the Start button. Interface: `ShownPlan`
  (`jobs.ts`), `startJob(plan, confirmedProduction, parallel, label)`,
  `forgetPlan`; `JobDraft.shown` (the plan its pre-flight shows;
  `showPlan(subject, id)` forgets the one shown before when the tab plans
  again, `started(subject, run)`, `forgetClosedDrafts` forgets a closed
  tab's plan); JobTab sends `plan.production` as confirmed only when its
  checkbox is ticked, carries `data-plan=<id>` on its root, and after a
  refused Start offers `[data-job=replan]` "Plan again". Every way in
  (job tab, backup tabs planning as they open, Run again, saved jobs, the
  file job, script and enrich jobs, the hosts view's and filter bar's
  entries, a config history window's Back up now) goes through these.
  Tests: `tests/kept_plans.rs` (two plans each found by its id, one
  forgotten after its start; production confirmed only by its own hosts,
  the refusal naming those left; the bound and `clear`, ids not reused).
  Smoke `twoPreflights` (after `savedJobs`): a production tab (`Jobs`,
  `Two prod`) planned first and a lab tab (`TwoJobs`, `Two lab`) planned
  last; the production tab still asks, the shell refuses its plan
  started with no confirmation or with the lab host's, then it runs
  `echo prod-plan` on jobs-r1 only; the lab tab runs `echo lab-plan` on
  its host only; the started plan cannot start again.
- Host keys at pre-flight (#129, `job.rs` `JobPlan::check_host_keys`,
  `HostKeyWarning`, `UntrustedKey`, private `KeyPlace` in `HostWork.keys`).
  A job refuses a host whose key is not trusted yet (a session's
  `HostKey` question is answered Cancel; a script's tunnel refuses it,
  `tunnels.md`); pre-flight now says so first. `plan` stays as it was
  (no store; 160-odd callers) and records, per planned host, where its
  run checks keys: snippet, config backup and file jobs the session's
  chain from `profile_target` (each jump host's `host`/`port`, then the
  device's); script and enrich jobs, from `tunnelled`, the same for a
  host behind jump hosts (the device by `ssh_target_with`, as
  `tunnel::plan`), nothing for a host the script reaches by itself (its
  library checks, not the app). `plan.check_host_keys(&KnownHosts)` (a
  second step beside `plan`; looks up only, `KnownHosts::trusted`, never
  connects; checking again replaces the warnings) sets
  `PlannedHost.host_keys: Option<HostKeyWarning { keys: [UntrustedKey {
jumpHost?, address, port }], message }>` (camelCase `hostKeys`, skipped
  when none); `untrusted_keys()` counts them. The host stays planned and
  `check`/`start` ignore the warning: the run looks keys up again and
  decides, so a kept plan (#121) whose warning went stale because the key
  was trusted since runs the host. Message: "the host key of the device
  (10.9.0.2 port 2222) is not trusted yet, so the job will refuse this
  host: connecting to it once in a terminal (Trust and remember) trusts
  the key"; a jump host: "of its jump host bastion.yaml (…)"; several:
  "the host keys of its jump host bastion.yaml (…) and of the device
  (…) are not trusted yet, … trusts them"; scripts and enrich say "the
  script's tunnel will refuse this host"; an unreadable store "its host
  key cannot be checked (…), so …" with no keys. A **changed key** (the
  store holds another key for the address) gets no warning: pre-flight
  cannot know what the device presents without connecting; the run
  refuses it with "has changed". Shell: `keep_plan` (every way in:
  `job_plan`, `job_run_again`, `saved_job_plan`) calls it with
  `SessionManager::known_hosts()` before keeping the plan. Interface:
  `PlannedHost.hostKeys` (`jobs.ts`), JobTab rows
  `[data-job-host-key=device|jumpHost]` with the message, summary
  `[data-job-untrusted=<n>]` (`window.md`). The CLI and Python bridge
  show no plan: unchanged. Tests: `tests/preflight_host_keys.rs`
  (`a_host_with_no_known_key_is_planned_with_a_warning`,
  `a_host_behind_a_jump_host_whose_key_is_not_known_names_the_jump_host`,
  `every_kind_of_job_is_warned_as_it_connects`,
  `a_changed_key_shows_no_warning_and_the_run_refuses_it`,
  `the_run_decides_whatever_the_plan_said`). Smoke `jobs`: the first
  pre-flight warns of jobs-r1's device key (port 22725) and counts 1;
  after it is trusted in a terminal the second has no warning. Known
  limits: a host whose device is unreachable but whose key is stored
  looks trusted; a key trusted with "Trust once" is not remembered, so
  the warning (and the run's refusal) stays.
- Jobs view and job tabs (#69, redesign spec #61). No job dialog or
  jobs dialog: two more views (`ViewName` `job`, `run`). A **job tab**
  (`JobTab.svelte`, was JobDialog; root `[data-view=job][data-step=
snippet|script|variables|preflight|running]`, the `data-job*` markers
  of #42–#55 inside unchanged) is opened by every way to start a job
  (`openJob`/`openScript`/`openBackup`/`openEnrich`/`runSavedJob`, Run
  again) through `views.ts` **`openJobTab(draft)`**: its subject is a
  key `new-<n>` naming a `JobDraft { kind: snippet|script|backup|enrich,
target, label, planned, run }` in `jobTabs.svelte.ts` (`jobDrafts`;
  `run` set once started; drafts of closed tabs are dropped by App's
  `forgetClosedTabs` via `forgetClosedDrafts`). Tab names from
  `draftTitle` / `runTitle` (`runTitles`, filled as runs are listed).
  `[data-job=dismiss|close]` close the tab ("Close tab"),
  `[data-job=view]` ("Open run") opens the run's tab and closes the job
  tab, `[data-job=review]` closes it and opens the enrich review (still a
  dialog). A **run tab** (`RunView.svelte`, was JobsView's details; root
  `[data-view=run][data-run-tab=<id>]`, subject = run id) has the run's
  `[data-run-details]` markers as before (output live while its job runs
  here, search, compare, save, delete (then the tab closes), run again,
  save as job, review), or `[data-run-gone]`. `openRunTab(run)` shows the
  job tab that started it if open, else the run tab. Closing a view tab
  whose job still runs (`App.closeViewTab`) leaves it running and shows
  `[data-job-still-running=<run>]` for 10 s (`[data-job-notice=show|
dismiss]`); the rail's `[data-rail=jobs][data-jobs-running=n]` counts
  as before. Switching workspace closes every job and run tab (their
  jobs go on). **Jobs side view** `JobsSideView.svelte`
  (`[data-jobs-view]`): `[data-runs-running=n]` group at the top, then
  `SavedJobsPanel`, then `[data-runs]` past runs newest first; rows
  `[data-run=<id>][data-run-state=running|ended|interrupted]` (`.title`,
  progress) open `openRunTab`; running ones started here have
  `[data-run-cancel=<id>]`; `[data-runs-empty]`, `[data-run-damaged]`.
  No core change (view names are the interface's). Smoke: `JOB_TAB` /
  `RUN_TAB` (the shown `.slot.view:not([hidden])` one), `JOBS_SIDE`,
  `openRun(evaluate, id?)`, `closeTabOf(evaluate, selector)`; `jobs`
  closes the hang job's tab (notice, rail), reopens it from the Jobs
  view, splits its run tab beside a terminal to the device, types there,
  cancels from the Jobs view and sees the tab update; the other job
  parts use the tabs.
- Saved jobs (#53, `src/workspace/jobs.rs`, `job.rs`). `_jobs.yaml` at
  the root: name → exactly one step key (`snippet: <name>` with optional
  `answers:` mapping, or `script: <path>`) and one target key (`folder:
<path>` (`''`/empty = root), `filter: <name>`, `host: <id>`); written
  step, answers, target. `SavedJob { name, step: JobStep, target:
JobTarget }` (re-exported from `workspace`); `Workspace::saved_jobs()`
  (file order), `saved_job(name)` (ignoring case), `save_job(previous,
&job)` (rename/change in place; trims, `\` → `/`, folder slashes
  stripped; refuses a clash, a config backup, a list of ≠1 hosts, blank
  names; never writes over a badly defined entry of the same name),
  `delete_job` (file goes with the last); `refresh` compares them.
  Structurally bad entries (not a mapping, no/two steps, no/several
  targets, `answers` on a script or not a mapping, unknown key, a
  duplicate name) are `Problem`s on `_jobs.yaml` and left out; an entry
  whose snippet/script/folder/filter/host is gone still loads:
  `job::saved_job_problem(ws, &job)` → `Option<PlanError>` says what
  (`NoSuchSnippet|NoSuchScript|NoSuchFolder|NoSuchFilter|NoSuchHost`),
  and `job::plan_saved(ws, profiles, name)` refuses with it (or
  `NoSuchJob`), else → `(target, plan)` (kept answers fill variables; others
  are asked as ever). New `JobTarget::Filter { name }` (`{kind: filter,
name}`): planned as the filter's hosts in tree order _at plan time_
  (`Workspace::filter(name)`, ignoring case); the filter bar's run
  snippet/script/back up now send it, so runs keep the filter name and
  Run again re-applies the filter. Shell: `WorkspaceView.saved_jobs:
[{ name, step, target, problem }]` (problem computed per view),
  `saved_job_save { previous, job }`, `saved_job_delete { name }`,
  `saved_job_plan { name }` → `{ target, plan }` (kept for `job_start`
  like `job_run_again`). Interface: `SavedJobsPanel.svelte` in the Jobs
  side view (`[data-saved-jobs]`, rows `[data-saved-job=<name>]`
  plan → a job tab at pre-flight via `runSavedJob`, `[data-saved-job-
problem=<name>]`, `[data-saved-job-rename|delete]` (the generic
  dialog), refusal in `[data-saved-jobs-error]`); `SaveJobForm.svelte`
  ("Save as job…", `[data-save-job=open|save|cancel]`, input
  `save-job-name`, `[data-save-job-saved=<name>]`, `[data-save-job-
error]`; disabled with a title per `unsaveable`) in JobTab's
  pre-flight (not for backups) and RunView's run actions;
  `jobTargetLabel(target)`. Tests: `tests/saved_jobs.rs` on the `jobs`
  fixture (loading, every bad entry, gone step/target, planning folder/
  filter/host/answers, saving/renaming/refusals/deleting on a copy,
  refresh). Smoke: `savedJobs` (after `jobs`): save from pre-flight,
  run from the panel (production confirmed), save from the run tab,
  rename, delete, external edit with a gone filter (refused) and a bad
  entry (problem). Known limits: a deleted `.py` changes no
  `WorkspaceView` (the watcher's refresh sees no difference), so a saved
  script job's problem marker can lag until the next reload; running
  still refuses it. Saved answers live in the shared file, except secret-
  looking ones since #126 (below). No scheduling; config backups cannot be
  saved.
- Saved answers that look like secrets (#126, decided: choice 2;
  `src/workspace/jobs.rs`). A saved job keeps its snippet answers except
  for variables whose names look like secrets by the imports' rule
  (`import::is_secret_name`: a word of the name is password, secret, key,
  token, community, credential, psk...); those are asked at each run.
  `workspace::secret_answers(&step)` → those names (empty for non-snippet
  steps). `clean` (so `save_job`) drops them before writing (`kept_answers`).
  Loading (`parse`) drops them from `step.answers` and lists their names in
  the new `SavedJob.ignored_answers` (serde default, skipped when empty,
  never written; `SavedJob::new(name, step, target)` leaves it empty); the
  value is never read into the job, so it is in no plan, `Debug`, IPC or
  run. The file is never rewritten for it: not on load, and a rename or
  change in place copies the old entry's secret-looking answers into the
  new one as written (`keep_ignored_answers`), so only the user removes
  them. Shell: `saved_job_secret_answers { step }` → names;
  `ListedSavedJob` carries `ignored_answers`. Interface: `SaveJobForm`
  asks `secretAnswers(step)` when it opens and shows
  `[data-save-job-asked=<names,>]` "Not saved, asked at each run (the
  names look like secrets): {{enable_password}}." (the "values are saved"
  note only when other answers remain); `SavedJobsPanel` shows
  `[data-saved-job-ignored=<names,>]` "The answer to {{enable_password}}
  in _jobs.yaml is no longer used (the name looks like a secret): it is
  asked at each run. Remove it from the file; the app cannot remove it
  from the git history." Tests (`tests/saved_jobs.rs`):
  `which_answers_look_like_secrets_follows_the_imports_rule`,
  `a_secret_looking_answer_is_not_saved_and_is_asked_at_each_run`,
  `a_secret_looking_answer_already_in_the_file_is_ignored_and_left_there`.
  Smoke `savedJobs`: "Saved secret" (`{{vlan}} {{enable_password}}`) saved
  from pre-flight (form names enable_password; the file holds only
  `vlan: '42'`; the saved job asks enable_password again), and the
  external edit's "Old secret" entry shows the notice, not the value, and
  stays in the file. Known limits: kept runs still hold every answer in
  `run.yaml` in clear (outside the workspace, as before); a variable named
  without such a word (`{{enable}}`, `{{pw}}`) is kept; naming a named
  secret of the store for such a variable (choice 3) is left for later.
- A job run in a real window (#77, redesign spec #61; `src/job/live.rs`).
  Live jobs are the shell's, not one window's: core `LiveJobs` (`start(
NewLiveJob { id, workspace (root), run, label, target, plan })`,
  `record(id, &JobEvent)` → the event's number (from 1; `None` for an
  unknown job), `jobs()`, `job(id)`, `forget_finished(keep)`) of
  `LiveJob { …, states, outputs, backups, exits, finished, events }`
  (camelCase; `events` = the latest number; a finished report replaces
  outputs). Shell: `Live` state; `job_start { parallel, label }` records
  the job and emits `job-started` (the `LiveJob`) to every window, then
  `job-event` `{ id, number, event }` per event (no channel any more);
  `jobs_live` → every record, `jobs_forget_finished { keep }`.
  Interface `jobs.svelte.ts`: `startLiveJobs()` (App and the detached
  window; listens, then reads `jobs_live`, events met before applied
  after; an event is applied only if numbered `events + 1`, a gap
  re-reads the record; records adopted in place, as tabs hold them),
  so a webview reload keeps live progress, the rail counts and Cancel
  works. Windows (`src/lib/windows.ts`, shell): `window_detach_run
{ run }` opens (or focuses) the window labelled `run-<run id>`
  (same page; `main.ts` mounts `Detached.svelte` when `detachedRun()`
  finds the label); `window_dock` (from that window) emits `dock-run`
  to the main window (App: `openRunTab`), focuses it and closes the
  window; `windows_close_detached` (App, on a workspace switch, as it
  closes the job/run tabs); the main window's `Destroyed` closes every
  other window (so the app ends). Capabilities: `detached.json` for
  `run-*` (core, dialog open, `core:window:allow-close`), and the
  main window may close itself (the smoke closes windows that way).
  `workspace_current` = the open workspace's view without reopening it
  (the detached window's first look; it then follows
  `workspace-changed`). `preferences-changed` (every save/zoom) carries
  the preferences to every window, and every window frame gets the
  theme. `Detached.svelte` (`main[data-detached=run][data-detached-run=
<id>]`, `[data-detached-no-workspace]`) = `RunView` with `detached`:
  `[data-run-action=dock]` "Dock back" in place of
  `[data-run-action=detach]` "Open in window" (run tabs), and no
  review, run again, save as job or delete there (they open tabs or
  change the workspace: dock back first); search, compare, save to a
  folder and cancel stay. The job tab has `[data-job=detach]` once
  started. Both close their tab quietly after detaching. `--font-mono`
  = `fontStack(fontFamily)` in every window (outputs and configs in
  the terminal font). Config history detaches too since #78 (below).
  Smoke (`jobs`, helpers `pageTargets`, `connectWindow(label)` = the
  page whose `__TAURI_INTERNALS__` label matches, `windowOpen`):
  `detachedRunWindow` detaches the running hang run (tab gone, window
  shows output so far, same theme and font, rail and Jobs view still
  count it), reloads the window (still shown), closes it (job goes
  on), detaches again from the reopened tab and docks back; later the
  window opened beside the run tab cancels the job (tab beside and
  window update). `closingTheMainWindow` (end of `noKeychain`, last)
  opens a window for a gone run (`[data-run-gone]`), closes the main
  window and waits for the app to end. Not checked: placing a window
  on another monitor (an OS matter).
- Ending with the app (#124, `src/job/running.rs`). Core `RunningJobs`
  (Clone, shared; replaces the shell's own `RunningJobs` map):
  `follow(canceller, recorder)` → `FollowedRun` (`id()` = the job id the
  interface cancels by, `run()` = the run id, `record(&event)` = the
  recorder's; dropped → no longer running), `count()`, `cancel(id)`
  (false once ended), `end_all(reason, within)` → how many still run (it
  cancels every job as Cancel does, then waits on a `Condvar` until every
  `FollowedRun` is dropped or `within` passes). The reason is kept with
  the end: `RunRecorder::cancelled_because` (crate-private) →
  `StoredRun.cancel_reason: Option<String>` (`cancel_reason` in
  run.yaml and IPC, skipped when none), set only if the report has a
  cancelled host (a job that finished meanwhile keeps none; a user's
  Cancel keeps none). `APP_CLOSED` = "the app was closed while the job
  ran", `APP_CLOSE_WAIT` = 5 s. Shell: `job_start` follows its job
  (`jobs.follow`, the loop records through it), `job_cancel` =
  `jobs.cancel(id)`; `run()` is now `.build(context)?.run(...)` with
  `RunEvent::Exit` → `end_with_the_app`: `PythonSetup::cancel_running()`,
  `end_all(APP_CLOSED, APP_CLOSE_WAIT)`, then the setup's `wait` for what
  is left of the 5 s (stderr says what had not ended). Exit comes after
  the last window is gone, whichever way (Close all, `app_quit`).
  What each job kind leaves: a script's or enrichment's process tree ends
  (`python-scripts.md`, "Ending with the app"; its tunnels with it);
  snippet and backup jobs close their sessions (`CLOSE_WAIT` each) and a
  file job cancels its transfers, removing a partial SFTP upload, within
  the wait; their runs end Cancelled with the reason. A job still running
  after the wait, or any job when the app crashes or is killed, keeps a
  run without an end: listed `interrupted`, its unfinished hosts "Not
  finished" (run.yaml is always written whole, temp file + rename; output
  files hold what was appended; a config version is written the same way,
  so none is half written). In-process jobs (snippets, backups, files)
  need no help to end with a killed app: their connections die with it
  (a partial upload then stays on the device). Interface: RunView
  `[data-run-cancel-reason]` "Cancelled: <reason>." at the top of the
  run's details; `StoredRun.cancel_reason?` in `jobs.ts`. The closing
  question names running jobs (`window.md`, closing rules). Tests:
  `tests/script_jobs.rs` `closing_the_app_cancels_a_running_script_and_
  its_kept_run_says_so` (Cancel by id keeps no reason, then `end_all`:
  0 left, processes gone, run ended with the reason, listed so);
  `tests/job_results.rs` `closing_the_app_ends_a_snippet_job_and_its_
  run_says_why` (a `hang` on a device: Cancelled with the reason and the
  output so far; a job finished before keeps none) and, as before,
  `a_run_the_app_did_not_finish_is_listed_without_an_end` (the crash
  case). Smoke `quittingWithLiveSessions` starts a `Close check: hang`
  job on `Jobs/jobs-r1.yaml` over `job_plan`/`job_start` (the snippet
  appended for the step, restored after), the question names it, Cancel
  keeps it running, Close all ends the app; `jobCutByClosing` after the
  restart opens that run from the Jobs view: `ended`, the host
  `cancelled`, the reason shown. Not covered: a file job cut on the lab;
  a job started in the moment between the question and the close is
  cancelled without being named in the question.
- The file job (#107, `src/job/file.rs`; prior art: enrichment's step).
  `JobStep::SendFile { file, destination, overwrite }` (serde
  `sendFile`; `file` a local path, `destination` a folder, `""` = each
  host's platform file folder). `plan` reads the file's metadata (never
  its bytes) into `JobPlan.file: Option<PlannedFile { name, size }>`
  (`PlanError::LocalFile { path, reason }` when it is missing or not a
  file) and sets `JobPlan.default_parallel` (`DEFAULT_FILE_PARALLEL` 3;
  every other step `DEFAULT_PARALLEL` 10; the shell's `job_start` uses
  it when no `parallel` is given). Per host (`file_runnable`, connects to
  nothing): `NotSsh`, `NoProfile` / `ProfileNotSetUp`, `JumpChain`,
  `JumpHostNeedsLogin`, then the destination: empty → the definition's
  `file_folder` else `NoDestination { platform }`; typed → filled with
  `Template` from `ResolvedHost::variables()` (the snippets' variables;
  a custom field named like a host field, e.g. `site`, is only
  `{{custom.site}}`), else `DestinationVariables { names }` ("the
  destination uses {{x}}, which it has no value for"). No platform is
  needed with a typed destination. `PlannedHost.destination` = the
  remote path (`files::join_path(folder, name)`). Run: `job::start`
  spawns `file::spawn` (one `Transfers` engine per job, keyed by host
  id). Per host, behind the `Semaphore` (permit held until the file
  connection is closed): `Waiting` → `Connecting`
  (`FileConnection::open_unattended` with `SessionManager::known_hosts()`
  (new, the same store), so an untrusted key or a login question fails
  with the jobs' reasons) → `start_files(definition)` (SFTP kept open as
  `Link::sftp`, SCP as `Link::scp`) → without overwrite, `presence`:
  same size `HostState::AlreadyThere` (new, final), other size "a file
  of that name exists with another size (N bytes on the device, M to
  send); tick overwrite to replace it", SCP `CANNOT_TELL` ("this device
  cannot say whether the file is there; tick overwrite to send it
  anyway") → `Running` while the engine uploads, `JobEvent::Progress {
  host, done, total }` (new; at most every `PROGRESS_EVERY` 200 ms, the
  last held one flushed; not kept with the run) → `Done` (bytes in
  `HostResult.bytes`) or `Failed` with the engine's error and its partial
  file. Cancel: hosts not started go `Cancelled`; while connecting the
  connection is dropped; while sending `Transfers::cancel` and the
  engine's outcome is awaited, so a partial SFTP upload is removed (the
  host's output says what was left). Each host's output is a few lines
  of what happened (protocol, sent bytes and the size check, partial
  file). The local file's MD5 and SHA-256 are computed beside the hosts
  (`files::hash_file`, crate-private, `spawn_blocking`) into
  `JobReport.file: Option<SentFile { name, size, hashes }>` (new field;
  `Hashes` deserialises now). Kept run: `StoredRun.file` (name and size
  at `begin`, hashes at `Finished`), `RunHost.destination` and
  `RunHost.bytes`; `LiveJob` gained `progress` (`[done, total]`),
  `bytes` and `file`. Run again = `plan_again` (the step is kept whole).
  Saved jobs refuse it ("sending a file cannot be saved as a job (it
  names a file on this computer)"; interface `unsaveable`). Interface:
  `openSendFile(target, label)` (draft kind `file`), from
  `[data-folder-send-file]`, `[data-host-send-file]`,
  `[data-filter-send-file]` and the tree menus' `send-file` entry. Job
  tab step `file` (`[data-job-file-form]`, inputs `job-file`,
  `job-destination`, checkbox `job-overwrite`, `[data-job=browse-file]`
  (the system dialog), `[data-job-file-note]`); pre-flight
  `[data-job-file-name]`, `[data-job-file-size=<bytes>]`, each host's
  `[data-job-destination=<path>]`, `job-parallel` at the plan's
  `defaultParallel`; running: states read Sending / Sent / Already
  there, `[data-job-sending=<id>][data-done][data-total]` (a
  `<progress>`) and `[data-job-bytes]`. Run tab: `[data-run-file=<name>]`
  with `[data-run-file-name]`, `[data-run-file-destination]`,
  `[data-run-file-overwrite=yes|no]`, `[data-run-file-md5]`,
  `[data-run-file-sha256]`; hosts `[data-run-destination]`,
  `[data-run-bytes]`; `alreadyThere` counts as done in the run and side
  view counts. Tests: `tests/file_jobs.rs` (three hosts sent byte for
  byte with progress, states and hashes against `md5`/`sha2`; every skip
  reason and its message, nothing connected, a missing local file; a
  typed destination with a host variable per host and a host lacking it;
  already there / another size / overwrite; SCP only without and with
  overwrite; an untrusted key and a one-time code failing beside a sent
  host; 3 at once by default and 1 for a run (SFTP writes held); cancel
  removing the partial file and never connecting waiting hosts; the kept
  run's file, hashes, states, bytes and destinations, run again, saving
  refused). Smoke: `fileJob` (after `fileTransfers`; a `Staging` folder
  of two SFTP hosts and a telnet one on the example server, each SFTP
  host with a platform and file folder of its own (`/stage`,
  `/stage-prod`) since #108: with one folder they were two devices
  sharing a file, and now and then the second met the first's
  half-written file ("exists with another size"); from the
  folder's panel: pre-flight's name, size, destinations, skip, production
  and 3 at once, Save as job disabled; both Sent and the file on disk;
  the run tab's hashes against the file's, destination, bytes, the save
  refusal's title; run again: both already there). Verified on the lab
  through its jump host in the built app (#108, 2026-11-21; the walk is in
  `files.md`): the folder's pre-flight with all four skip reasons, the
  three destinations (`/mnt/flash`, `/var/tmp`, `flash:`), a typed
  `/tmp/{{name}}`, a saved filter's Send file; without overwrite ceos1
  already there, csr1 "cannot say", cjunos1 failing on an untrusted key
  while the others went on; with overwrite all three Sent (two at once,
  csr1 over SCP at 2.7 MB/s), the kept run's hashes equal to each
  device's own checksum; cancel while csr1 was sending. Found there and
  fixed: csr1 failed with "its files cannot be reached: the connection
  was lost" (the IOS-XE hang-up, `files.md`). Not verified: Linux; a file
  changed between pre-flight and the run is #137's (below; "already
  there" still compares the planned size). Since #110 a job's hosts can
  also be a selection of several in the tree (`window.md`, "Several hosts
  selected").
- A file changed during a file job (#137, `src/job/file.rs`). Every host
  gets the file pre-flight read, or fails saying it changed. `PlannedFile`
  keeps the modification time pre-flight read (private, not serialised).
  At the start the job reads the file's hashes once (`planned_hashes`:
  size and time checked before and after hashing, else `Reference::
  Changed`), the reference every host is held to. Before a host's upload
  its size and time are compared with the planned ones: changed → that
  host `Failed` "the file changed since pre-flight (it was 102400 bytes at
  pre-flight and is 153600 bytes now): nothing was sent; plan the job
  again to send the file as it is now" (or "(it was written to since
  pre-flight (its modification time changed))"), its connection closed,
  nothing sent. After an upload the engine's hashes of what was read and
  sent are compared with the reference: another file → `Failed` "the file
  changed while it was sent: the device got another file than the one
  pre-flight read (SHA-256 … sent, … planned); what was sent was left on
  the device"; the reference itself changed → "the file changed during
  the run (…), so what was sent may not be the file pre-flight read; it
  was left on the device". Decided: what was sent is **left** on the
  device (it may have replaced a file there with overwrite; removing it
  would not bring that back, and the user is told), the host is Failed
  with no `bytes`, and the run's size and hashes stay the planned file's
  (true of every host Done). A file that cannot be read again after its
  upload, or a job cancelled while the check waits, keeps the host Done
  with a line saying it was not checked. A file gone is the engine's to
  say, as before. Tests: `edge_job_kinds.rs`
  `a_local_file_changed_during_the_run_is_not_sent_as_the_planned_one`
  (no longer ignored; between two hosts, to another size and to other
  bytes of the same size: the first has the planned bytes, the second
  fails and has nothing, the run's hashes are the planned file's),
  `a_local_file_changed_while_it_is_sent_fails_the_host` (8 MiB, the
  device holding the rest while it is rewritten). Known limits: a change
  that keeps the size and the modification time (a copy that preserves
  times, a file system with coarse times) is not seen before sending; it
  is caught after, by the hashes, only if it came after the job read them
  at its start. The run tab was not looked at with
  these reasons.
- Edge testing (#119, 2026-11-21): hostile use of jobs, in test files of
  their own (`tests/edge_jobs.rs`, `edge_job_scale.rs`, `edge_job_kinds.rs`,
  `edge_job_agent.rs`, `edge_runs.rs`; scripts in `python-scripts.md`), over
  `tests/support/jobs.rs` (`Lab`: a temporary workspace with an id and the
  `lab` profile stored; `host`/`host_at` host files; `follow(job, within,
  each)` follows a job to its report checking what every job owes (one
  report last, each host once and final, one final state, streamed output
  equal to the report's), `Ran { report, events, times, took }` with
  `failure`, `states`, `ended_at`; `closes(server, within)`). What was
  tried and what it showed:
  - Every way of failing in one run (`hosts_failing_in_every_way_at_once_
    each_end_with_their_own_reason`): a wrong password, an unknown and a
    changed key, a one-time-code login, a refused port, a device that never
    speaks, a login never answered, a shell never given (each with a 1 s
    connect timeout), a name that does not resolve, a jump host down (and
    the host behind it), `drop` in the middle of a command, `hang`, a pager
    the definition does not turn off, a wrong enable secret, a flood, beside
    a host that works and one whose line is unknown: 18 hosts planned and
    one skipped (no profile) add up to the 19 files; each fails with its own
    words, all within 15 s with a 1 s timeout and a 2 s limit (the run took
    about 3 s). The three connect timeouts read the same ("the device did
    not answer within 1 s"), whichever stage stalled (the sessions'
    wording, not a job's). No password or secret in the report or events.
  - Cancelling (`cancelling_with_hosts_at_every_stage_...`): one host done,
    one connecting (a silent device, 60 s connect timeout), one in the
    privilege step (a platform whose `privilege_command` is `hang`), one
    flooding, one hanging after taking the room the done one left, one
    waiting for room; cancelled three times in a row once all stages are
    reached: ended within 10 s (well under 2 s here), each host cancelled
    but the done one, the waiting one never connecting, the kept run as the
    report with no cancel reason, planned and started again.
    `a_cancel_as_the_last_host_finishes_changes_nothing`: after the last
    final state, before the report: every host done, no reason kept.
    Planning cannot be cancelled: it connects to nothing and returns at
    once (`a_plan_for_a_target_that_is_gone_is_refused`).
  - Prompts (#118 left them here): `a_prompt_that_changes_is_followed`
    (config mode and a new name, through the device's `prompt <name>`);
    `slow_consoles_are_waited_for` (a byte every 40 ms and every 450 ms,
    the prompt included: `crawl <ms> <text>`; an answer after 2.5 s half
    way through a line: `stall <ms> <before> <after>`). **Finding, fixed
    by #136** (`a_pause_after_text_like_the_prompt_does_not_end_the_
    command`, no longer ignored; "The end of a command" below): output
    that ended a line like the prompt (`total#`) and then paused longer
    than `PROMPT_SETTLE` was taken for the prompt; the job typed the next
    line, took that line's prompt for the end and closed the session: the
    rest of the first answer was lost and the host was Done.
  - The cases #127 found untested: `a_job_uses_the_hosts_connect_timeout`
    (2 s, ended after it), `a_job_uses_the_hosts_legacy_crypto_setting` (an
    old device: done with `legacy_crypto: true`, failing with the advice
    without), `a_job_logs_in_with_an_agent_profile` (`edge_job_agent.rs`,
    alone in its binary: `SSH_AUTH_SOCK` set to a `TestAgent`),
    `a_job_leaves_an_open_terminal_to_the_same_host_untouched` (a
    connection of its own; the terminal saw nothing and still answers).
    Not testable through the core: that job sessions write no session log
    (a job opens with `manager.open`, which has none; a log file is only
    ever given by the shell's `session_open`).
  - Secrets (`the_enable_secret_and_the_password_are_never_shown`): a
    device whose enable question echoes what is typed
    (`echo_enable_secret()`). **Fixed**: the secret was in the host's
    output, its events and its kept run; `HostRunner` now shows
    `HIDDEN_SECRET` (`********`) in its place in the answer to the
    question, and only there (`typed_secret` is cleared once that wait
    ends): text equal to the secret that the device shows later is the
    user's output, and a config backup never has part of its text replaced
    (`text_equal_to_the_enable_secret_shown_later_is_kept_as_it_is`; found
    in review). The plan, as serialised for the interface and as `Debug`,
    has neither the secret nor the password.
  - Output held whole. **Fixed**: a host's output (and a script's) was kept
    whole in memory (`HostRunner.output`, the report, `LiveJob.outputs`,
    the events, the rewrite at the end) until the command limit (30 min),
    or for ever for a script. Now `JobOptions::output_limit`
    (`DEFAULT_OUTPUT_LIMIT` 32 MiB of plain text) per host: past it the
    output is still read but not kept, shown or written, and a last line
    says so (`output_cut(limit)`: "[enodix: the output went on past 32
    MiB, the most a job keeps of it; the rest was not kept]"). A config
    backup whose output was cut keeps nothing ("\"show running-config\"
    showed more than the 4 KiB a job keeps of a host's output; nothing was
    backed up", `a_configuration_larger_than_a_job_keeps_is_not_kept_cut_
    short`). The 32 MiB is a choice (a `show tech-support` of a large
    chassis can be larger); the run tab was not looked at with it.
  - Scale (`edge_job_scale.rs`: one device server, `echo` and `lines 30`
    per host, debug build, a machine shared with other builds): 100 hosts
    (in the suite) and 500 (`#[ignore]`d: `cargo test -p enodix-core
    --test edge_job_scale -- --ignored --nocapture`), at 10, 50 and 50
    again. **Fixed**: recording the run was the bottleneck: `RunRecorder`
    rewrote `run.yaml` on every state (4 per host, each of a file growing
    with the hosts) and opened the output file for every piece of output;
    100 hosts at 50 at once spent 5.5 to 21 s of a 5.6 to 21 s run
    recording (the shell records before it forwards, so the window lagged
    as much). Now the description and the output are written at most every
    `SAVE_EVERY` (250 ms) or when `PENDING_OUTPUT` (1 MiB) waits, a host's
    file stays open while it runs and is let go at its final state, the end
    writes again only files that missed a piece, and `flush()` (also on
    drop) writes what waits. A crash loses at most 250 ms of states and
    output, or what came since the last event (`a_running_runs_
    description_is_written_now_and_then_and_at_the_end`). After: 100 hosts
    at 50 at once in 2.9 to 3.7 s (recording 1.5 to 2.2 s), at 10 at once
    in 13 s (three prompts of 300 ms settle per host); 500 hosts at 50 in
    15 to 16.5 s (recording 9.6 to 11 s, mostly the 500 output files
    written and closed on a disk with a virus scanner), at 10 in 65 s;
    planning 500 hosts 20 to 40 ms; memory peak +21 MiB at 10 at once, +66
    to 75 MiB at 50; handles flat from run to run (252, 252 for 100; 286,
    287 for 500); every host's output opened again in 7 to 10 s for 500
    (each `output` call reads `run.yaml` again), searched in 0.3 s.
  - Two runs at once on the same hosts (`two_runs_at_once_on_the_same_
    hosts_keep_their_own_output`): own run ids, own output, both listed.
    Two backups at once of one device: one version, one `Saved` and one
    `Unchanged` naming it (`edge_job_kinds.rs`).
  - Same file stem (#127): **fixed** for kept runs: `r1.yaml` and `r1.yml`
    shared one output file (`output/r1.txt`, the second written over the
    first); `begin` now gives the later `r1_2.txt` (`_3`...; compared
    ignoring case) (`hosts_with_the_same_file_stem_keep_an_output_file_
    each`). Their config history has a folder each since #127's second
    part (`config-backups.md`, "Hosts whose files share a stem").
  - Backups and files cut short (`edge_job_kinds.rs`): a backup cancelled,
    or cut by `end_all(APP_CLOSED)`, while the configuration is shown keeps
    no version and no file, the latter's run saying why; a full device disk
    (`sftp_quirks().full_after(bytes)`: "the device could not write
    /images/image.bin; the partial file was removed from the device") and a
    refused folder fail only their hosts; a local file gone before the run
    fails every host naming it, with no hashes. The app closing while a
    file is sent: cancelled within the wait, the partial file removed; with
    a device that stopped answering, given up on at the stall limit (three
    connect timeouts: 3 s here; 45 s by default, more than
    `APP_CLOSE_WAIT`, so such a run was left without an end when the app
    closed, and a user's Cancel took as long: **fixed by #137**, the
    remove is waited for at most `CANCEL_REMOVE_WAIT` 2 s, `files.md`),
    the output saying a partial file may remain. **Finding, fixed by
    #137** (above, "A file changed during a file job"): a local file
    changed after pre-flight was sent as it was then (100 KiB planned, the
    second host got 150 KiB, the run's size the planned one and its hashes
    of whatever was read). A run whose folder
    was removed: run again refused with `NoSuchFolder`; a run already going
    goes on with the hosts it planned. Not played: the local file deleted
    while a host sends it (the engine holds it open; hosts after it fail as
    when it is gone before).
  - Kept runs (`edge_runs.rs`): a description cut every 7 bytes, one not
    UTF-8, a folder without one, a leftover `.run.yaml.tmp`: each listed
    whole or damaged with a reason, never twice, never lost; an output path
    outside the run is `Damaged`; output that is not UTF-8 opens and
    searches with replacement characters, a missing output file is empty;
    a run deleted while open (a file of it held open by a reader): every
    call on it says `NoSuchRun`, nothing of it listed (the run tab and
    window were not looked at). **Finding, fixed by #137** (below,
    "Listing kept runs"): `list` read every description whole each time:
    1 000 listed in 1.2 s, 5 000 in 9.6 s (`five_thousand_kept_runs_list`,
    `#[ignore]`d; debug build, a busy machine), which the Jobs view waits
    for each time it lists.
  - Already covered elsewhere: the app closing during a snippet job
    (`job_results.rs`) and a script (`script_jobs.rs`); a saved job whose
    folder, filter, host, snippet or script is gone (`saved_jobs.rs`); the
    flaky cancel tests of `script_jobs.rs` (found and fixed in #124).
- Listing kept runs (#137, `src/job/results.rs` `RunStore::list`). The
  store keeps what it last listed (`listed`, shared by its clones: each
  run's `StoredRun` by its description file, with that file's size and
  modification time); a list looks at every run folder's `run.yaml`
  (one metadata call) and reads and parses again only those whose size or
  time changed, or that are new; runs gone are forgotten, damaged ones are
  never kept and are read each time. The folders are looked at on several
  threads (`available_parallelism`, at least `RUNS_PER_THREAD` 64 each).
  The time is stamped before the file is read, so a change in between is
  read again next time. What was read is used again only if the file had
  already lain unchanged for `STAMP_SETTLES` (3 s) when it was looked at:
  a description written a moment ago can be written again with the same
  size and, a file clock being coarse, the same time (on Linux a host's
  state going from `waiting` to `running` within one tick left the list
  showing `waiting`; found by the Linux checks, test
  `a_fresh_description_rewritten_with_the_same_size_and_time_is_read_again`,
  which fails without the rule on Windows too). So the runs in progress
  are read at every list, and a time ahead of this computer's clock never
  settles. The result is the same `RunList` as before (no
  IPC or interface change). Measured with `five_thousand_kept_runs_list`
  (5 000 runs of 10 hosts, the run folders on a temporary folder, this
  machine shared with other builds), before / after: debug 2.0 to 2.3 s
  first and 1.7 to 1.8 s again / 0.35 to 0.52 s and 0.26 to 0.29 s;
  release 0.83 to 0.85 s and 0.71 to 0.76 s / 0.15 s and 0.085 s (the
  9.6 s of #119 was on a busier machine). Tests (`edge_runs.rs`):
  `listing_many_kept_runs_again_is_quick` (2 000 runs: a second list
  under half of reading every description one by one, measured beside
  it; before: 654 ms against 552 ms, after: 67 ms against 539 ms),
  `a_second_listing_sees_every_change_since_the_first` (a run recorded
  on and ended, one deleted, one damaged then repaired, one rewritten by
  hand to the same size, one added; a fresh store on the folder sees the
  same). Known limits: the first list after the app starts still reads
  every description (in parallel); a hand edit that keeps the file's
  size and modification time is not seen until the app restarts; the
  interface still receives every run whole over IPC each time it lists
  (not measured).
