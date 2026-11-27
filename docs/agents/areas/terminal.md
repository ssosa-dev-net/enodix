# Terminal

What happens inside a terminal: preferences, rendering, highlighting, snippets, paste safety, input origins and multi-send.

- A closed session in its terminal (#15, #116, #130; `TerminalSession`).
  The scrollback stays; Enter (typed while closed, nothing else reaches
  the shell) or the menu's Reconnect writes `--- reconnecting ---` and
  calls `Session.reconnect(size)`: a host's session connects with the
  host's settings and credential profile of now (the rule is the core's,
  `sessions.md`). The core's note, when the login is not the one the
  session had (a profile it has now, or why the login is asked), is
  written dim on the next line (`noteReconnect`); a refused reconnect
  (host gone, no address, another protocol now) puts its reason and
  advice in the ended bar, which always ends "Press Enter to reconnect."
  (the words "try again with the same settings" of #116 are gone). The
  size is sent again once the command answers if the terminal changed
  meanwhile (09fbfc2). The same component runs in split panes and in
  session windows, so all behave alike.

- When a terminal takes the keys (#141, `src/lib/keys.ts`; the window's
  side and the failure found are in `window.md`, "Who gives a terminal the
  keys later"). `TerminalSession`'s focus effect (its pane made active, a
  window's `focusRequest`, a card or the search bar gone) and the session
  opening focus through `giveKeys`: only if the window asked for the keys
  and nothing moved the keyboard since (a click, a focus on another
  element). So keys typed in another pane while a layout answer or a
  login was on its way stay in that pane. A focus at the instant of a
  click or key on the terminal itself stays a direct `xterm.focus()`:
  answers to the host key, login (and its Cancel, which now focuses
  directly instead of through the effect), paste and search questions,
  the menu closing, notices dismissed. The `[role=dialog]` check is kept.

- The SSH login banner in the terminal (#134; the core's side is in
  `sessions.md`). It arrives as output before `Connected`, so the
  terminal writes it like any output (it is logged and acknowledged the
  same way); a jump host's has a dim line naming it. `TerminalSession`
  sets `bannerShown` when output comes while the state is `connecting`
  (reset on reconnect, written under `--- reconnecting ---`). With it,
  `LoginQuestions` (prop `banner`) sits at the bottom of the pane with no
  backdrop and lets the wheel through to the terminal (the overlay has
  `pointer-events: none`, the card `auto`), and says "The login banner
  is in the terminal above." (`[data-login-banner-above]`); the SSH
  agent's password-fallback card does the same (`.overlay.below-banner`).
  The host-key card stays centred: a device's key is asked before its
  banner can come (only a jump host's banner can be behind it). The Files
  tab's login card has no banner (file connections drop it). Known
  limits: the output and the questions come on different Tauri channels,
  so a large banner may be written just after the card appears (the card
  then moves down); a banner taller than the pane is read by scrolling
  the terminal behind the card.

- Hidden terminals and WebGL (`TerminalSession`, `src/lib/renderer.ts`).
  While hidden, output is held (acknowledged at once up to 1 MiB, then
  only once written, so flow control pauses the device), written when
  shown, and the terminal fits its pane only after that output is parsed.
  `webglRenderer(terminal)`: WebGL addon only while shown, at most 8 at
  once (others DOM renderer, queued for a freed context); a hidden
  terminal's context is released with `WEBGL_lose_context` (the addon's
  `dispose` leaves it to GC, which piled contexts past Chromium's 16); a
  lost context retries after 1 s, max 3 losses per showing. The WebGL
  renderer draws no DOM text, so `.xterm-rows` is gone: the smoke reads
  the screen from the buffer through `.terminal`'s `xterm` property
  (`screenOf`), and the WebGL canvas is `.xterm-screen
canvas:not(.xterm-link-layer)` (the link layer is a 2D canvas). Smoke
  `click(label)` only clicks visible buttons. The addon's canvases carry
  inline z-indexes (the link layer's is 2), so `.terminal` has
  `isolation: isolate`: without it they lay over the questions drawn over
  the terminal (host key, login, the failure cards), which have none, and
  took their clicks. The smoke's `decideHostKey` clicks with the mouse
  through `mouseClick`, and `reachable(evaluate, selector)` fails when
  `elementFromPoint` at a control's centre is something else (also the
  login question and the negotiation failure card). The scripted shell has
  `later <s> <text>` (output after a delay) for the hidden-tab check.
- Key bindings (#24). Core: `Preferences.shortcuts` (action id → chord,
  only moved ones; ids and defaults in `SHORTCUT_ACTIONS`, in priority
  order) and `snippet_keys` (chord → snippet name); `normalize_chord` is the
  one written form (`Ctrl+Alt+Shift+Key`, Ctrl or Alt required except F1–F24;
  keys: letters, digits, some punctuation, F-keys, Tab, PageUp/Down, Home,
  End, Insert, Delete, Space). `normalized` drops unknown actions, bad
  chords, a second action on a taken chord, and snippet keys on a chord an
  app action uses; `effective_shortcuts()`. Interface: `keyBindings.ts`
  (`chordsOf(event)`: the printed key, `event.key`, first, then the position,
  `event.code`; `bindingOf` takes the first chord bound to an action or a
  snippet and stops at a reserved printed chord, so the printed letter wins
  and copy/paste stay where the user sees them; `chordOf` is the first, what
  Preferences records; `typedLetter` is the letter match shared by the
  terminal's copy, paste and search keys and the app's fixed keys: printed
  Latin letter, else position), `App.svelte` `shortcut()` dispatches the nine
  movable actions by `bindingOf`, then snippet keys (only with a connected
  active session, via `sendSnippet`; `event.repeat` sends nothing and is
  swallowed), then the fixed keys (tab cycling, side views). Reserved keys:
  core `RESERVED_KEYS`/`reserved_key` (copy, paste, search, tab keys, zoom,
  side views); `normalized` drops a shortcut or snippet key on one on read and
  write; the shell serves the list as `reserved_keys`, the interface keeps it
  in `reservedKeys` and the Preferences view refuses to record one and says
  why (`[data-key-refused]`). Checked by a throwaway script over key events
  for QWERTY, AZERTY, Dvorak and Cyrillic (no interface test runner).
  Preferences view `[data-key-bindings]` (`preference-key-<action>`,
  `preference-new-snippet-key`). Limits: tooltips/palette hints and
  `SessionWindow.svelte` still use the default keys; zoom and the terminal's
  copy/paste keys are not movable. Smoke: `screensOf080` (Quick open moved
  to Ctrl+Alt+J opens the palette there and no longer on Ctrl+K,
  Ctrl+Shift+F refused with its reason in `[data-key-refused]`, a snippet
  key sending a one-line snippet once; all reset after).
- Colour schemes (#24). `Preferences.color_scheme` (`ColorScheme`:
  default, solarized-dark, solarized-light, dracula, nord, gruvbox-dark;
  kebab-case in the file) and `custom_colors` (name → `#rrggbb` over the
  scheme, names in `COLOR_NAMES`: background, foreground, cursor,
  selection and the 16 ANSI names as highlight rules spell them;
  `normalized` drops unknown names and bad colours, `#rgb` expands). The
  core only keeps the choice; the palettes are `src/lib/colorSchemes.ts`
  (`SCHEME_THEMES`) and `terminalTheme(dark)` layers scheme → own colours,
  `default` being the old `DARK_TERMINAL`/`LIGHT_TERMINAL`. `highlightColor`
  resolves palette names through `terminalTheme`, so rules follow the scheme;
  the highlighter's cache key includes `paletteKey()` so rows redraw when
  the scheme or an own colour changes. Preferences view: `preference-color-
  scheme`, `[data-own-colors]` with `preference-color-<name>` inputs.
  Smoke: `screensOf080` (Dracula sets the terminal's xterm theme
  background to `#282a36`, the contrast check passes, then back to
  default). Own colours are not driven by the smoke.
- Terminal comforts (#19, `src/preferences.rs`). `Preferences { theme:
System|Light|Dark, font_family, font_size (8–36, default 14),
scrollback (0–100,000, default 10,000), copy_on_select,
right_click_paste }` in `<data folder>/preferences.yaml` (snake_case
  keys; camelCase over IPC). `load` is forgiving per key (bad value →
  that default; out of range → clamped); `save` normalises; `zoom(FontZoom::
In|Out|Reset)`. A separate file from `state.yaml` because
  `workspace_open` rewrites `AppState` whole. Shell: `UserPreferences`
  (Mutex), `preferences_get` / `preferences_save` / `preferences_zoom`
  (each returns what was saved; the window frame gets `set_theme`, and
  the saved theme at build). Interface: `preferences.svelte.ts` (requests
  queued in order; `isDark()` = preference or `prefers-color-scheme`,
  live), `App.svelte` sets `<html data-theme=light|dark>` (`app.css`
  light tokens override the dark base), `PreferencesView.svelte`, a view
  tab since #68 (`[data-view=preferences]`, opened by the
  `[data-open-preferences]` gear at the foot of the activity rail or
  Ctrl+,; fields
  `preference-theme|font-family|font-size|scrollback|copy-on-select|
right-click-paste`, saved on `change`). Zoom: Ctrl+= / Ctrl+- / Ctrl+0
  (`zoomShortcut`, works with dialogs open) and Ctrl+scroll over a
  terminal; all terminals share the size. `TerminalSession` keys
  (`attachCustomKeyEventHandler`): Ctrl+Shift+C / Ctrl+Insert copy,
  Ctrl+C copies only with a selection (else ^C goes out), Ctrl+V /
  Ctrl+Shift+V / Shift+Insert paste (so ^V no longer reaches devices,
  as in Windows Terminal), Ctrl+Shift+F opens the search bar
  (`[data-terminal-search]`, `[data-search-results]` "n of m",
  `[data-search=previous|next|close]`, Enter / Shift+Enter, Escape from
  any of the bar's controls (the form's keydown; the arrows keep the keys
  in the field, #94);
  `@xterm/addon-search`, needs `allowProposedApi`). **Every paste goes
  through `pasteText`** (keys, right click, the browser's own paste
  event) and every snippet (#22): hook paste safety (multi-line
  confirmation) there. The
  clipboard is `navigator.clipboard` (WebView2 allows reading it; no
  plugin). Copy-on-select copies on the left button's release (a
  window `mouseup` after a press in the terminal); copying 150 ms later from
  `onSelectionChange` failed in the smoke (cause not established). Smoke: `terminalComforts` (emulates
  `prefers-color-scheme`, uses the real clipboard through PowerShell and
  restores it), then restarts the app and checks the dialog's values.
- Keyword highlighting (#21, `src/highlight.rs`, `src/workspace/highlights.rs`).
  Decoration only: the core matches **screen text**, never session bytes
  (output pipeline, flow control and logs are untouched). `Highlighter::
spans(line)` → `Span { start, end (chars, i.e. code points), color, rule }`,
  non-overlapping, earlier rule wins; `Highlighter::default()` = the 6
  `BUILT_IN_RULES` (`state-down|warning|up`, `ipv4`, `ipv6`, `interface`;
  the IP ones match loose candidates then validate with `std::net` +
  neighbour checks, only while the pattern is the built-in one). Colours:
  `#rrggbb` or `COLOR_NAMES` (`red`, `bright-cyan`, ...: the terminal
  theme's palette, resolved by `highlightColor` in the interface).
  `_highlights.yaml` at the root: name → `pattern`, `color`, `enabled`;
  a built-in name changes that rule (any subset of keys). Order: workspace
  rules (file order), then built-ins. `Workspace::highlight_rules()` →
  `ListedRule { rule, source: BuiltIn|Changed|Workspace, error }` (a bad
  regex stays listed with `error`, is a `Problem`, highlights nothing);
  structurally bad entries are `Problem`s and left out.
  `save_highlight_rule(previous, rule)` (built-ins: only differences are
  written, none → entry removed; an off rule may keep a broken pattern),
  `delete_highlight_rule` (resets a changed built-in). Global switch:
  `Preferences.highlighting` (per user). Shell: `highlight_lines(lines)`
  (open workspace's highlighter, else built-ins), `highlight_built_in_rules`,
  `highlight_rule_save/delete`; `WorkspaceView.highlight_rules`. Interface:
  `highlight.svelte.ts` `highlightTerminal(terminal)` sends the visible
  rows (wrapped rows joined, at most `WRAP_MARGIN` rows off screen) once
  per frame after `onRender`, caches answers by text, draws xterm
  decorations (`foregroundColor`, one marker per row), redraws a row only
  when its text/rules/theme change, clears on buffer switch; smoke reads
  `.terminal`'s `highlights.shown()`. `HighlightRules.svelte`, a view
  tab since #72 (view `highlights`, one tab, no subject, also without a
  workspace: `[data-view=highlights]`, opened by `[data-open-highlights]`
  in the Preferences view or the palette's
  `[data-palette-command=highlights]`; Escape leaves the form only;
  rows `[data-highlight-rule]`/`[data-highlight-source]`,
  `[data-rule-error]`, `[data-rule=new|edit|delete|reset|save|close]`;
  Close closes the tab). Smoke `highlighting` splits it below the
  session and checks rules there (`sessionSlot`, not `SHOWN_SESSION`,
  as the rules' pane may be active). Known
  limits: per-rule switches live in the shared workspace file (no per-user
  per-rule switch); highlights colour over device-set colours; built-in
  `error` does not match `errors` (counter lines would all go red).
- Snippets and templating (#22, `src/template.rs`, `src/workspace/snippets.rs`).
  `Template::parse(text)`: variables `{{name}}` (trimmed, matched ignoring
  case; a `{{` that makes no valid name stays literal; no escape syntax);
  `variables()`, `fill(&Variables)` → text or `MissingVariables { names }`
  (values inserted as-is, never re-expanded). `Variables` = name → value,
  ignoring case; `fill_gaps(other)`. `ResolvedHost::variables()`: `name`,
  `address`, built-in fields by file key, `role` (id), `tags` (effective,
  `", "`-joined), custom fields as `custom.<key>` and bare `<key>` (unless a
  built-in has that name); blank fields are left out (so asked for).
  `WebLink::fill(&Variables)` uses the same templating (see #24 below).
  `_snippets.yaml` at the root: name → text, or → `{ group, text }`;
  `Workspace::snippets/snippet/find_snippets(query)/save_snippet(previous,
&Snippet)/delete_snippet`; never writes over a badly defined entry of the
  same name. `Snippet::expand(&Variables)` → lines (split at CRLF/LF/CR, a
  final break ends the last line). `Workspace::expand_snippet(name,
Option<&HostId>, answers)`: host values first, answers fill the gaps (an
  empty answer counts), else `ExpandError::Missing`. Shell: `snippet_find`,
  `snippet_save`, `snippet_delete`, `snippet_expand` (→ `{kind: lines}` or
  `{kind: missing, names}`); `WorkspaceView.snippets`. Interface:
  `SnippetsView.svelte`, the Snippets side view (#65; `[data-snippets]`:
  header with `[data-snippet-new]`; no snippets → `[data-snippets-empty]`
  one sentence + `[data-snippet-first]`; else filter
  `input[name=snippet-filter]` (`snippets.query`, kept across views;
  matched by the core's `snippet_find`, as the palette, shown in file
  order by group), `[data-snippet-target=<session name or "">]`,
  `[data-snippets-none]`, groups `[data-snippet-group=<group or "">]`,
  rows `[data-snippet=name]` send (with `[data-snippet-preview]`, the
  first line), `[data-snippet-edit|delete]`; `[data-mode=list|edit]`:
  new/edit show `SnippetForm.svelte` in the view in place of the list,
  as the Credentials view's forms (#79; `form[data-snippet-form]
[data-snippet-editing=<previous name or "">]`, fields
  `snippet-name|group|text`, `[data-snippet-save|cancel]`,
  `[data-snippet-error]`; Escape or Cancel back to the list; the form is
  the view's own state, so leaving the view drops it)),
  `SnippetVariables.svelte` (pane-scoped since #79: drawn inside the
  target session's `.slot`, as the paste confirmation, not `aria-modal`,
  Escape handled on itself; `[data-snippet-variables=<snippet>]`, inputs
  `variable-<i>` with `data-variable`, `[data-snippet-variables-send|
cancel]`; hiding or closing its tab drops it, and shortcuts stay on
  while it is open); palette: a query starting with `>` searches snippets
  (`[data-snippet-hit]`), Enter sends. `sendSnippet` in `App.svelte` sends
  to the active pane's connected session via `TerminalSession`'s exported
  `sendText` (`bind:this` into `terminals`), which calls **`pasteText`**:
  snippets and clipboard pastes share it, and it sends line by line (one
  `terminal.paste` per line, line break included); #23 hooked paste
  safety there (see below). Known limit: with bracketed paste on (bash 5.1+), each line arrives as
  pasted text, so the shell inserts rather than runs it (as for any
  paste). Svelte gotcha: a `{@const}` destructured from state re-derives,
  so read it before clearing that state in a handler.
- Environment marking and paste safety (#23, `src/paste.rs`).
  `PasteSafety { environment, line_delay_ms }` (camelCase over IPC;
  `Default` = quick connect) = `ResolvedHost::paste_safety()`;
  `Workspace::host_paste_safety()` gives every host's (shell:
  `WorkspaceView.host_safety`, interface `hostSafety(id)`). Inherited
  setting `paste_line_delay` (ms, 0–`MAX_PASTE_LINE_DELAY_MS` 10,000,
  default 0). `plan_paste(text, &safety, confirm_multi_line)` →
  `PastePlan { lines, confirm, line_delay_ms }`: lines split after
  CRLF/LF/lone CR (breaks kept); `confirm` = more than one line and
  (preference on, or production). One line with its break never asks.
  Preference `Preferences.confirm_multi_line_paste` (default true,
  `preference-confirm-multi-line-paste`). Shell `paste_plan { text, safety }`
  (reads the preference). Interface: `pasteText` in `TerminalSession`
  queues pastes, plans, shows `PasteConfirm.svelte`
  (`[data-paste-confirm=production|staging|lab|none]`, `[data-paste-lines=n]`,
  `[data-paste-text]`, `[data-paste=cancel|send]`; Cancel focused on
  production), then sends line by line with the delay inside the loop,
  stopping if the session closes. Colours: `--env-*` tokens in `app.css`;
  tabs `[data-environment]` (+ `[data-tab-production]` "PROD"), the
  session `.session[data-environment]` border (3px production, 1px
  others) and `[data-environment-badge]`. Since #84 the app keeps each
  session's record (`session::SessionRecord`, `SessionRecords` in the
  shell): name and title (from the interface, `session_open { name,
  title }`), and from the workspace at open time the host id, its
  workspace root, the address (when the title lacks it), the jump host
  (SSH only, not itself), the paste safety and the role
  (`SessionRecord::for_host`; quick connections `SessionRecord::new`). A
  duplicate copies it, `session_discard` forgets it; `session_open` and
  `session_duplicate` return it, `session_record { id }` reads it. A
  session window and a session arriving in the main window show the
  record (the main window then prefers the live host's safety and role,
  as before, while the host is in the open workspace). Known limit: the
  interface still sends the safety it shows to `paste_plan`, though the
  shell now knows each session's host. Smoke: `pasteSafety`. Since #86
  snippets from the main window go to the terminal used last, whichever
  window it is in, and a session window fills and asks for them itself
  (`window.md`).
- Snippet steps (#23, `src/steps.rs`, `src/session/output_watch.rs`,
  `src-tauri/src/steps.rs`, `src/lib/steps.ts`, `StepsConfirm.svelte`).
  Decided for the ticket: snippets gain steps (no interpreter, no Python
  API); steps never run on the multi-send group (refused, one session at a
  time); the user's input during a run stops it and that input is not sent.
  Format: a `_snippets.yaml` entry with `steps` (a list: a line = send
  then wait for the prompt; or `{send, wait, timeout}`, `wait` = `prompt`
  | `none` (or `false`/null) | a pattern; a step needs `send` or `wait`),
  optional `answer` (map question pattern → line, null = empty), `stop`
  (a pattern or a list), `timeout` (seconds, 1–86,400, default 30); `text`
  and `steps` are exclusive, `answer|stop|timeout` only with `steps`.
  Patterns: literal text, `/…/` a regex; a literal `prompt`/`none` is
  written back as `/prompt/`. `Snippet.steps: Option<Steps>` (text empty
  then; `Snippet::with_steps`); `Steps::problem()` (checked on load and in
  `clean`), `variables()` (sends then answers), `fill(&Variables,
  Option<prompt>) -> StepsPlan` (`FillError::Missing|Invalid`; a filled
  line is cut at its first line break). `Workspace::plan_snippet_steps(name,
  host, answers)` fills from the host and uses `host_platform(id).prompt`,
  else `DEFAULT_PROMPT` (`\S*[>#$%]`); `expand_snippet(_for_each)` on a
  stepped snippet is `ExpandError::HasSteps` (so the group path and jobs,
  `PlanError::SnippetHasSteps`, refuse it); `NoSteps`, `BadSteps`.
  `StepsPlan::{len, lines, prompt, preview, digest}` (preview: answers,
  stops, then numbered steps, the core's words; digest: SHA-256 over the
  prompt, steps, waits, timeouts, answers and stops). A job refuses a
  stepped snippet before expanding it, whatever its hosts. Running: `StepRuns` (one run per
  session; `start(manager, session, plan, &Permission, report)` needs a
  Tokio runtime, takes an `OutputWatch` before the first line). The gates
  are in the core: `Permission {safety, confirm_multi_line, previewed,
  confirmed, in_group}`, checked in order: `in_group` → `InGroup`,
  `previewed != plan.digest()` → `Changed` (the snippet, answers or host
  changed since the preview), unconfirmed where `confirms_steps` asks →
  `NotConfirmed`; then `AlreadyRunning|NotConnected`, `stop`, `running`,
  `user_input(targets, origin)` (every origin but `Reply` stops a run on a
  target and drops that target). Events `RunEvent::Step{index,count,doing}
  |Answered{index,question}` (never the reply) `|Ended{end,message}`;
  `RunEnd::Finished|Stopped{step,line,pattern,shown}|TimedOut{step,line,
  waiting_for}|CalledOff{step,line,by: User|Input|Paste}|SessionEnded`, serde
  `kind` camelCase. Engine: output as `PlainText`, looked in from the last
  send (the echo of the step's own line skipped), stops checked first, then
  the step's own pattern, then answers (the earliest match; consumed so it
  never fires twice; the step keeps waiting); the prompt = the current line
  matching `(?:prompt)[ \t]*\z`, only after output came since the last
  send, quiet for `PROMPT_SETTLE` (300 ms). A wait's timeout counts from
  its start, not from the last output. Line ending sent: `\r`. `wait: none`
  keeps the host's paste line delay. Output watch: `SessionManager::
  watch_output(id)` (connected only) → `OutputWatch::next()`, fed in
  `Link::forward_counted` beside the log, no part in flow control, keeps at
  most `WATCH_LIMIT` (1 MiB, oldest dropped), ends on `Closed` (a reconnect
  is a new connection); `SessionManager::is_connected`. Safety:
  `paste::confirms_steps(safety, pref)` = pref or production. Shell:
  `StepRuns` managed; `snippet_expand` returns `{kind: steps}` for a
  stepped snippet with nothing missing; `steps_preview {id, name,
  answers}` (`ready {preview, count, confirm, environment, digest}` |
  `missing`), `steps_start {id, name, answers, previewed, confirmed,
  onEvent: Channel}` (plans again, builds the `Permission`, `in_group` from
  `member_showing`; the core refuses), `steps_stop {id}`; `session_input` filters its targets through
  `user_input`. Interface: both windows' `sendSnippet` call
  `TerminalSession.runSteps(name, answers)` on `kind: steps`
  (variables asked first as for any snippet); it shows `StepsConfirm`
  (`[data-steps-confirm=env|none][data-steps-count]`,
  `[data-steps-preview]`, `[data-steps=cancel|run]`, Cancel focused on
  production), then the bar `[data-steps-run=<snippet>][data-steps-step]`
  with `[data-steps-answered]`, `[data-steps-stop]`, and when ended
  `[data-steps-ended=<end kind>]` + `[data-steps-dismiss]`. Moving the
  session is refused while it runs (its Channel is this window's). Snippets
  view: `[data-snippet-steps]` mark, preview = first line sent; the editor
  shows the steps read-only (`[data-snippet-steps-text]`) and saves name and
  group with the steps kept. Tests: `tests/snippet_steps.rs` (file format,
  round trip, problems, filling and platform prompt, never as lines,
  confirmation rule; runs over SSH device-mode, telnet login as steps,
  serial with the test as the device: prompt waits, answers, stops, the
  echo, own pattern, timeout, user stop, input stop, session end). Not
  covered: the smoke test (no step for it yet), the shell commands and the
  interface (svelte-check only). Known limits: no paging handling (a
  `--More--` waits to the timeout), no editing of steps in the app, no
  loops or conditions.
- A paste stops a run whole (`StepRuns::user_input_of(targets, origin,
  paste)`, shell `session_input { .., paste }`). The interface sends each
  line of a paste as its own `session_input`, all with one id
  (`newPasteId()` in `pasteEach`, passed by `TerminalSession`'s input
  callback for `pasted`/`group-paste` lines). A pasted line that stops a run
  ends it with `CalledOff::Paste` ("none of that paste was sent") and the
  core remembers the id per session (`Runs::refused`, the last one): the
  paste's other lines on that session are refused, typed input and a paste
  with another id go through, replies are never touched. Without an id
  (`user_input`) only the stopping call is dropped. Tests: `snippet_steps`
  (`a_paste_that_stops_the_run_is_refused_whole`, `a_group_paste_line_...`).
  Looked at, not changed: after a `wait: none` step the unread output is
  not discarded (the watch is read in the next wait, after the next line's
  `Screen::sent`, so it counts for that wait); only the last step's output
  is never looked in, by design.
- Input origins (#34, `src/lib/terminalInput.ts`; for multi-send, #33).
  `terminalInput(terminal, onInput)` replaces `onData`/`onBinary` in
  `TerminalSession`: each `Input { data, binary, origin }` has origin
  `typed | pasted | reply | mouse`; only typed is shared (since #123; a
  paste reaches the group only as a group paste, "Multi-send safety").
  Every origin still goes to the terminal's own session. Mechanism:
  xterm.js's internal `_core.coreService.onUserInput` fires synchronously
  just before `onData` only for user input (`wasUserInput`: keys, IME
  composition incl. its `setTimeout` paths, pastes, mouse); everything
  else is a reply (DSR/CPR, DA, DECRQM, CSI t reports, OSC colour
  answers, DECRQSS, focus reports). Mouse reports are user input to
  xterm, so they are recognised by `window.event instanceof MouseEvent`
  (wheel arrows in full-screen programs and alt-click cursor moves count
  as `mouse` too); `onBinary` is always mouse. `input.paste(text)` (used
  by `pasteText`, so every paste and snippet) marks its data `pasted`.
  If the internal hook is missing (xterm upgrade), all but our pastes are
  `reply` (never shared) and a warning is logged. Smoke: `inputOrigins`
  calls `.terminal`'s `input.record()` (off by default: typed input holds
  passwords), then reads `input.recent()` (last 64); the scripted shell has
  `query` (sends `ESC[6n ESC[c`) and `mouse on|off` (1000 + SGR 1006).
- Multi-send (#35, `layout/windows.rs`, `SessionManager::write_to_each`).
  One group for the app (#83), in `Windows`, whatever window each member
  is in: `group()` (`Member { window, tab }`, join order),
  `window_group(window)` (that window's member tabs, serialised as the
  window's `group`), `in_group`, `join_group(window, tab)` /
  `leave_group` (no-ops change nothing), `join_pane(window, pane)`,
  `end_group`; every group change raises every window's revision.
  Closing or dismissing a tab (or closing or resetting its window) drops
  it, moving keeps it, new/duplicated tabs start outside, a tab can join
  before its session is known; not persisted. `Windows` also maps tabs
  to sessions (`attach_session(window, tab, session)`, not serialised, no
  revision bump): the shell attaches in `session_open` /
  `session_duplicate` (both take `tab`, the window is the caller).
  `input_targets(session, origin)` (#123; `InputOrigin::Typed|Pasted|
GroupPaste|Reply|Mouse`, kebab-case over IPC): typed = every member's
  session (any window) if its tab is a member, else just itself; a group
  paste's line = itself while a member shows it, else nobody (it left,
  closed or the group ended mid-paste); pasted, reply, mouse = itself,
  whatever the group. **Only typed input fans out**: a paste reaches the
  group only as a group paste (below), so a paste begun outside the group
  stays its tab's if the tab joins while it is asked about or its lines
  still go. Since #87 a member that moves keeps
  its place in the group (`Windows::move_session`); see "Multi-send
  across windows" below.
  `write_to_each(&[id], bytes)` → `Fanout { written, skipped }`: skips
  unknown, closed and still-connecting sessions (a `connected` flag the
  `Link` sets), never stops on one. Shell: `session_input { id, data,
  origin }` (routed through `input_targets`), `session_write` stays
  for this session only (input while not connected);
  `LayoutEdit::JoinGroup|LeaveGroup|EndGroup`.
  Interface: `TerminalSession`'s `terminalInput` callback sends every
  input through `Session.input(bytes, origin)` while connected (a line of
  a group paste as `group-paste`), else through `Session.write` (so a
  connecting member's typing reaches nobody; resize and break are per
  session anyway); props
  `tab`, `groupSize` (null: not a member), `ontogglegroup`, `onendgroup`. Markers:
  `.session[data-multi-send=receiving|not-receiving]`, the header
  marker `[data-multi-send-bar][data-group-size]` ("Multi-send n") with
  `[data-not-receiving]`, the session menu's `[data-end-group]` and
  `[data-multi-send-toggle=join|leave]` entries (#67),
  tabs `[data-multi-send=…]` + `[data-tab-multi-send]` ("MULTI"),
  `--multi-send*` tokens. Ctrl+Shift+M toggles the active tab.
  Tests: `tests/windows.rs` (group, across windows; #123's routing by
  origin, group paste checks and join checks), `tests/multi_send.rs` (SSH,
  telnet, serial members; #123: a paste begun before its tab joined, replies
  and mouse reports staying local, a member closing or leaving mid-paste,
  a serial break in a member). Smoke: `multiSend` (after `tabsAndPanes`, on
  three of its sessions).
- Multi-send safety (#36, `paste.rs`, `Windows::join_pane`,
  `Workspace::expand_snippet_for_each`). Joining: `PasteSafety::
confirms_joining()` (production, whatever the paste preference);
  `confirm_joining(&[(id, safety)])` → the ids to confirm. Shell
  `group_join_check { joining: [{tab, safety}] }` → tab ids;
  `LayoutEdit::JoinPane { pane }` (`join_pane` adds the pane's
  non-member tabs in order, returns them; none → no revision bump).
  Interface: `join(tabs, edit)` in `App.svelte` (header toggle,
  Ctrl+Shift+M and the tab bar's `[data-join-pane]` all go through it)
  shows `ConfirmJoin.svelte` (`[data-confirm-join][data-join-count]`,
  `[data-join-production=name]`, `[data-join=cancel|join]`, Cancel
  focused); a pane that gained a tab while asked is asked again. Since
  #123 the shell enforces it: `JoinGroup { tab, confirmed }` / `JoinPane
  { pane, confirmed }` (`confirmed`: the question was answered) go
  through `Windows::join_group_confirmed` / `join_pane_confirmed (window,
  tab|pane, confirmed, safety)`, `safety` being the shell's record of
  each session (`SessionRecords::paste_safety`, as `group_members`
  gives a member's); unconfirmed, a production session is refused
  (`LayoutError::JoinNotConfirmed(tabs)`: "a production session joins
  the multi-send group only once its question is answered, so nothing
  joined", the whole pane join too), and so is a tab whose session is not
  known yet (`JoinBeforeSession`); a member already in is not asked
  about again. The interface sends `confirmed: true` only after the
  user's yes, and says a refusal ("Cannot add to the multi-send group:
  …": App's edit error, a session window's error line). Every way of
  joining goes through these two (Ctrl+Shift+M, the session and tab
  menus, `[data-join-pane]`, a session window's toggle); a tab moved to
  another window is not a join (it keeps its place, its join was
  checked). Known limit: a member whose host becomes production after it
  joined stays in. Pastes: `plan_group_paste(&[(text, safety)], confirm_pref)`
  → `GroupPastePlan { members: [PastePlan], confirm (any member's own
plan asks), environment (strictest: production > staging > lab) }`;
  shell `paste_plan_group { members: [{text, safety}] }`. A member's
  `pasteText` hands the text to `ongrouppaste` (since #87 the window's
  `GroupPaste.paste`, queued): members = connected group tabs, the
  pasting one first; one
  `PasteConfirm` with `members` (`[data-paste-members=n]`,
  `[data-paste-member=name]`, a member filled differently shows its own
  text in `[data-paste-member-text]`) over the pasting tab's slot (that
  tab closed or hidden cancels it); then
  `TerminalSession.pasteLines(lines, delay)` per member, all at once,
  each stopping when its own session ends or its tab leaves the group
  (#123). Those lines go as `group-paste` input (`pastingGroup`), not
  through the shell's fan-out, so nothing is sent twice. Typed keys still
  fan out in the shell. `pasteText(text, alone)` decides group or not
  once, when the paste starts; `sendText` (a snippet begun outside the
  group) passes `alone`. Snippets:
  `expand_snippet_for_each(name, &[Option<HostId>], answers)` → each
  member's lines, or `Missing` with every name some member lacks (once,
  snippet order); `None` = no live host (only the answers); a host gone
  from the workspace is `NoSuchHost`. Shell `snippet_expand_group`
  (`{kind: lines, members}` / `missing`); `sendSnippet` takes the group
  path when the tab is a member (`snippetTarget` then names the group)
  and goes to `GroupPaste.pasteEach` directly, so group snippets skip `pasteText`
  (the group paste path is their one way in). Since #123 that choice is
  made once, when the snippet is first sent, and kept through its
  variables question (`asking.group`, which the question's target names):
  begun outside the group it goes to its tab alone (`sendText`) even if
  the tab joined meanwhile; begun in it, it is the group's, and called off
  ("…is no longer in the multi-send group…") if the tab left.
  Tests: `tests/multi_send_safety.rs`, `tests/snippets.rs` (group),
  `tests/layout.rs` (join pane), `tests/windows.rs` (#123 join checks).
  Smoke `multiSend` adds a production
  host `ms-prod.yaml` (removed after): the shell refusing its unconfirmed
  tab and pane joins (`layout_edit` invoked from the page), pane and tab
  join questions, a member's Ctrl+C copy and Ctrl+Shift+F search staying
  local, a paste in the tab outside the group joining (Ctrl+Shift+M)
  while asked reaching it alone, one group paste question (production, 3
  members), the same question closed and the paste called off when a
  member leaves meanwhile (said in the Hosts view's `.edit-error`, nobody
  gets it), per-member snippet. Not covered by the smoke: a member
  closing mid-paste (core only: `multi_send.rs`).
- Multi-send across windows (#87). Core (`Windows`): `move_session` puts
  the moved tab in the member's place (join order kept; `hand_to_main`
  likewise); `group_revision()` rises on every change to the members
  (join, leave, a member's tab or window closing, reset, end), not on a
  move; `WindowView.group_size` (`groupSize`: members in every window);
  `paste_members(window, tab)` gives `[(Member, SessionId)]`, the one
  pasted in first, then join order, known sessions only, empty for a
  non-member; `member_showing(session)`: the member showing it now.
  One rule for every window since #123: a group paste goes to the members
  as they are when it is sent. `group_paste_members(window, tab)` →
  `GroupPasteMembers { revision, members }` (the group's revision with
  them), `GroupPasteRefused::NotAMember` for a tab not in the group;
  `group_paste_targets(window, tab, revision, &sessions)` → the member
  showing each session now (moved ones too), refused whole if the tab is
  no longer a member (`NotAMember`: "the tab pasted in is no longer in the
  multi-send group (it left, or the group was ended), so nothing was
  pasted") or the revision moved (`Changed`: "the multi-send group
  changed before the paste was sent, so nothing was pasted; paste again
  to send it to the group as it is now"). `WindowView.group_revision`
  (`groupRevision`).
  `SessionRecord::safety_in(workspace)`: the host's paste safety as the
  workspace sets it now, while `host_in`, else as it opened;
  `SessionRecords::paste_safety(session, Option<&Workspace>)` (none open:
  as it opened; no record: none). Tests: `tests/windows.rs` (kept in its
  place, handed to main, the group revision, paste members, member
  showing), `tests/session_record.rs`. Shell
  (`src-tauri/src/group.rs`): `tell_windows` (from `layout_edit`,
  `layout_reset` and `session_windows::announce`): when the group
  revision differs from the one last told (`Tabs.told`), every window is
  sent its view as `layout-changed` (to its label) and main the
  `snippet-target` again (which now has `member`). `group_members { tab }`
  (calling window) gives `{revision, members: [{window, tab, session,
  name, safety (safety_in), host (host_in), connected (the relay's
  reported state)}]}`, refused for a non-member (#123);
  `group_paste_send { tab, revision, members: [{session, lines,
  lineDelayMs}] }` (#123: the whole paste at once, checked by
  `group_paste_targets`, refused whole) emits `group-paste-lines { tab,
  session, lines, lineDelayMs }` to the window whose member shows each
  session now, the pasting window included; `group_paste_check { tab,
  revision }` refuses as it would (for the question closing);
  `session_safety { id }` (for a session window's join question). The
  shell no longer refuses a session window's group edits. Interface:
  `src/lib/groupPaste.ts`: `GroupPaste` (one per window, queued; its
  `window` is `thisSessionWindow()` or main) with hooks `terminals`,
  `ready`, `ask`, `withdraw` (#123: closes the open question as Cancel
  does), `failed`; `receiving(from)` (`{revision, members}`: connected
  members; this window's need a terminal), `paste(from, text)`,
  `pasteEach(from, receiving, texts by session)`, `groupChanged(revision)`
  (each window calls it from an `$effect` on its layout's
  `groupRevision`); `expandForMembers(name, members, answers)` (each
  member's `host`, so `snippet_expand_group`). Planning, the question and
  the lines are as in #36, but the members come from the shell; every
  member, in this window or another, gets its lines through
  `group_paste_send`; the receiving window's `GroupPaste` (it listens to
  `group-paste-lines`) calls the `pasteLines` of its terminal showing
  that session (in that tab, or another tab of the window). Since #123
  (one rule for both windows): the group changing while the question is
  open (a join, a leave, a member closing, the end, in any window)
  withdraws the question, and the paste is called off with the shell's
  reason (`group_paste_check`) through `failed` ("Cannot paste to the
  multi-send group: the multi-send group changed before the paste was
  sent, so nothing was pasted; paste again…"); a change the window had
  not seen yet is refused by `group_paste_send`, said the same way; a
  paste in a tab no longer a member (the group ended elsewhere) is called
  off and said ("…the tab pasted in is no longer in the multi-send
  group…"). A member gone from the receiving window as the lines arrive
  is said through `failed`; the others get theirs.
  `onLayoutChanged` adopts (revision-checked). `App` and `SessionWindow`
  both use it; `SessionWindow` adds `toggleGroup` (`sessionSafety`, then
  `joinConfirmations`, `ConfirmJoin` over the pane, `[data-pane-dialog]`),
  Ctrl+Shift+M, the end-group entry, the tab's member marker, the group
  paste question over the slot, group snippets, and `windowQuestionOf`.
  The main window's Snippets view names the group when the terminal used
  last is a member in a session window. Smoke: `sessionWindows` (the
  moved member stays, Multi-send 3 in both windows, typing both ways, a
  two-line paste in the session window asked once there naming 3 members
  and arriving once in each; from the main window a cancelled group
  paste reaching nobody and a confirmed one reaching the session
  window's member once; leave, join and end there shown in both
  windows); `safetyInSessionWindows` (prod-r1's join question there:
  Cancel, then Add to group). Not covered by the smoke: a group snippet
  across windows, a member ending mid-paste in another window, a member
  moving while the question is open (its lines then go through the shell
  to where it is). Known limits: a member's lines (since #123 this
  window's members' too) are not awaited by the pasting window's queue (each terminal queues
  its own pastes, so they stay in order per member); a member that moves
  to another window between `group_paste_send` and its window receiving
  the lines misses them (said in that window); members of the pasting
  window are judged connected by its own states, others by their relay's
  reported state; a session window's own pastes and markers still use the record's
  safety as it opened (#86), while the group question uses `safety_in`.
- Moving a terminal to another window (#84, `TerminalSession`,
  `src/lib/handover.ts`). `handOver()` (null while `moveRefusal()` gives a
  reason: not opened yet, connecting, a host-key, login or paste question
  open, the window's own question about it (`windowQuestion` prop:
  snippet variables, a group paste), or a paste still being sent:
  `pastes`, counted from queueing to the last line in `pasteText` and
  `pasteLines`) reveals a hidden terminal (its held output into xterm),
  calls `Session.release()` (the shell's `session_release`, then waits
  until the channels have received up to the returned offset), lets
  xterm finish parsing, checks `moveRefusal()` again (a paste begun
  meanwhile stays: it reattaches and returns null), and serialises with `@xterm/addon-serialize`
  plus `unserializedModes`. From then on the terminal's unmount does not
  discard the session; `takeBack()` reattaches after a failed move. The
  receiving terminal (`source: { kind: "adopt", id, snapshot }`) writes
  the snapshot (or, when null, one dim line "--- earlier output was
  lost: the window showing it closed ---") before `session_attach`; its
  first state is the relay's last one unless a newer arrived; it sends
  its size to the device once attached, whatever it is; if it cannot
  attach, its going discards the session (`discardSession`). The menu
  entries are the `moves` prop (#85: `[data-session-action=open-in-window|open-in-window-<n>|dock|dock-all]`,
  disabled with the reason as its title). What moves: the screen, the
  scrollback, colours and attributes, the alternate screen with the
  normal one under it, application cursor keys and keypad, bracketed
  paste, insert, origin, reverse wraparound, focus reporting,
  wraparound, the mouse tracking mode (the addon), and the mouse
  encoding (SGR, SGR pixels) and a hidden cursor (`unserializedModes`,
  from xterm's internals). Lost: the scroll region, character sets, the
  cursor's style and saved cursor positions; the receiving terminal is
  sized afresh (the window is opened at the size that keeps the columns
  and rows where the screen allows), and a different size resizes the
  device. The scripted shell's `program` / `program off` (alternate
  screen, bracketed paste, application cursor keys) is for the smoke.

- Security review (2026-11-22): `linkHandler: { activate: () => {} }` in
  `TerminalSession.svelte`: OSC 8 hyperlinks a device prints open nothing
  (xterm's default asked with a dialog, then the window refused the
  window.open).
