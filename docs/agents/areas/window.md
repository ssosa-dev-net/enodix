# Window

The window around the terminals: the look, the interface's structure, tabs and panes, view tabs, the rail and side views, the details sheet and dialogs.

- Look (#62, redesign spec #61). `src/app.css` holds the design tokens,
  dark base + `:root[data-theme="light"]` (colours only): grounds `--bg`,
  `--surface`, `--surface-raised`, `--dialog`, `--input`, `--selected`,
  `--terminal`, borders `--border`/`--border-strong`/`--control-border`
  (3:1), text `--text`/`--muted`/`--faint`, `--accent`/`--on-accent`,
  `--ok`/`--danger`/`--danger-fill`/`--warning*`, `--state-connecting|
connected|disconnected`, `--env-*` (+ `-bg`, `-text`, `--on-env-production`),
  `--multi-send*`, `--backdrop`, `--shadow*`; `--space-*` (0-5 = 2px … 8 =
  32px), `--radius-sm|md|lg|full`, `--text-2xs…2xl` (11–24px), `--weight-*`,
  `--font-ui` / `--font-mono`. Components use only tokens in their styles
  (the rem is kept at 15px as a layout unit for widths). Text pairs are
  ≥4.5:1 in both themes, and environment, multi-send and state colours
  differ in lightness. Since #79 the smoke checks contrast (WCAG AA:
  4.5:1, 3:1 for large text) in both themes on every screen it
  photographs and on every side view, view tab and app dialog the
  palette opens, and on the unlock/start-over screen and detached
  windows (`CONTRAST_PROBE`, `checkContrast(evaluate, where)`; failures
  are collected and fail the run at the end; terminals and disabled
  controls are not judged). The UI typeface is Inter, bundled by `@fontsource-variable/inter`
  (imported in `main.ts`); icons are `@lucide/svelte` (`import X from
"@lucide/svelte/icons/x"`, class `lucide-icon`); terminals keep the
  user's font. xterm's own colours (ANSI palette, search matches) stay in
  `preferences.svelte.ts` / `TerminalSession`. Smoke: right after launch it
  checks the font is loaded, icons are drawn and no resource came from
  another origin.
- `src/` — the interface. It only renders; `src/lib/session.ts` and
  `src/lib/workspace.ts` wrap the IPC. `src/lib/inventory.svelte.ts` holds
  the open workspace, selection, expanded folders and the current `Dialog`,
  and runs the edits (each shell command returns the reloaded workspace).
  Host saves are queued and applied to the latest host, so quick edits
  don't overwrite each other. Tree drag and drop is pointer-event based
  (rows carry `data-item`/`data-path`/`data-drop`), not HTML5 drag events,
  which Tauri's own file-drop handler intercepts on Windows. The details sheet
  (`HostDetails`/`FolderDetails`) saves each field on `change`. Both use
  `SettingsPanel` (every effective setting with its origin under
  `[data-origin=<key>]`; controls named `setting-<key>`, "" = inherit,
  a "Clear …" button when set here) and `TagEditor` (own tags plus
  inherited ones, dashed). `effectiveSettings(() => item)` in
  `inventory.svelte.ts` fetches the resolved settings from the core
  (`settings_resolve`) and refetches after every edit. Sessions to inventory hosts are opened
  by host id (`{ kind: "host", id, ... }`); the shell resolves the id against
  the open workspace in the core, so later inheritance applies there.
  Connecting (`prompt` in `App.svelte`) first asks `host_login`: a stored
  profile connects at once with `credentials: null` (the shell calls
  `Profiles::profile_target`); `notSetUp` shows `CredentialsPrompt` with a
  `ProfileForm` for that name; otherwise the username/password prompt.
  `profiles.svelte.ts` holds the profiles view (refetched whenever
  `inventory.workspace` changes) and `profiles.vault` (the vault's status,
  refetched with it; `VaultSection` refreshes after each change), shown by
  the Credentials side view (#66). The shell's store is the keychain (prefix = Tauri
  `identifier`), or memory with `ENODIX_CREDENTIAL_STORE=memory` (the
  smoke test sets it, so it never touches the keychain).
  Host-key questions arrive on `session_open`'s `onHostKey` channel;
  `TerminalSession` shows `HostKeyCheck.svelte` (`[data-host-key=first-use|
changed]`, buttons `[data-decision=…]`; Cancel has focus on a changed key)
  and sends `session_decide_host_key`. The shell's store is
  `KnownHosts::in_data_folder(data_folder)`.
  Agent forwarding questions arrive the same way on `onAgentForwarding`:
  `AgentForwardingCheck.svelte` over the pane (`[data-agent-question]`,
  details in `sessions.md`), answered with
  `session_decide_agent_forwarding`.
  Login questions arrive on `session_open`'s `onPrompt` channel;
  `TerminalSession` shows them over the terminal (`LoginQuestions.svelte`)
  and sends `session_answer`. A closed state carries
  `offersPasswordLogin`; for a profile login the terminal then offers
  "Log in with a password", which opens `CredentialsPrompt` with
  `fallback` (reason + the profile's username).
  Ctrl+K (`App.svelte`, a capture-phase window listener, so it works while
  xterm has focus; it takes Ctrl+K from the terminal) opens
  `CommandPalette.svelte`, which asks `workspace_search` per keystroke;
  Enter connects, Shift+Enter reveals in the tree (`revealHost`). Inputs
  that must take focus from the terminal need an explicit `.focus()`;
  `autofocus` alone did not. The tree's saved-filter view is
  `inventory.activeFilter`/`filterView` (`trackFilterView` refetches after
  edits); the selected item always stays visible. The save-filter dialog
  makes one-condition filters; more need `_filters.yaml`.
- Tabs and split panes (#18, `src/layout.rs`). `Layout` = a tree of
  `Node::Pane { id, tabs, active }` / `Node::Split { direction: Right |
Down, children }` (no single-child splits, no same-direction nesting:
  `tidy`), plus `active_pane` and `revision` (+1 per change; refused
  edits change nothing). A pane with `active: None` shows the connect
  view. `open_tab` (end of the active pane), `open_tab_beside`,
  `activate`, `next/previous_tab` (active pane, wrapping), `close_tab`
  (right neighbour shown, else left; an emptied pane goes unless last),
  `dismiss_tab` (pane stays, connect view), `move_tab_by`, `move_tab(tab,
pane, index)` (index among the other tabs), `split` (moves the shown tab
  into the new pane only if the pane has >1 tab), `close_pane` (tabs go
  to the neighbour in reading order). Tests: `tests/layout.rs`.
  One layout per window (#83, `src/layout/windows.rs`): `Windows` holds a
  `Layout` per `WindowId` (`WindowId::MAIN` always, never closed:
  `LayoutError::MainWindow`) and the app's multi-send group
  (`terminal.md`). `open_session_window` adds a window whose layout is
  `Layout::for_session_window()` (`open_view` → `LayoutError::NoViewTabs`,
  `shows_views()` false), `close_window`, `reset(window)` (afresh, same
  kind; its tabs leave the group), `layout(window)`, `layout_mut(window)`
  → `LayoutMut` (derefs to `Layout`; when dropped the group forgets the
  tabs the edit closed, raising every window's revision if one was a
  member), `view(window)` → `WindowView { layout (flattened),
  group: that window's member tabs }`, serialised exactly as `Layout` was
  before #83. Tab and pane ids are per layout. Tests: `tests/windows.rs`.
  Shell: `Tabs(Mutex<Windows>)`, `layout_reset` (the interface calls it
  on load), `layout_edit { edit: LayoutEdit }` → `{ layout, created }`,
  both on the calling window: `calling_window(&WebviewWindow)` maps its label
  (only `main` for now; any other is refused) to a `WindowId`, so the
  window is never an argument. Not
  persisted (spec wants the window layout in the data folder some day;
  sessions do not survive a restart anyway). Interface: `src/lib/layout.ts`,
  `PaneView.svelte` (recursive; tab bars with `[data-tab]`,
  `[data-tab-state=connecting|connected|disconnected]`, `[data-tab-close]`,
  `[data-new-tab]`, `[data-split=right|down]`, `[data-close-pane]`; tab
  drag is pointer-event based: drop on a tab bar = that position, on a
  pane = its end). Terminals are NOT inside the panes: `App.svelte`
  renders every session once as an absolutely positioned `.slot[data-tab]`
  over its pane's measured body rect (`PaneRects` in `paneRects.svelte.ts`: its `body(pane)` attachment +
  ResizeObserver), so moving a tab keeps its terminal; the connect view
  is a `.slot.connect` over the active pane. `.slot[data-active]` = the
  active pane's shown session (smoke's `SHOWN_SESSION`). Shortcuts
  (capture-phase window listener): Ctrl+Tab / Ctrl+PageDown next,
  Ctrl+Shift+Tab / Ctrl+PageUp previous, Ctrl+Shift+PageUp/PageDown move
  the tab, Ctrl+Shift+W close. Closing a connecting/connected tab asks
  (`ConfirmClose.svelte`, `[data-confirm=cancel|close]`). Tabs keep focus
  off themselves (mousedown preventDefault) and bump `focusRequest` so
  the terminal keeps the keys.
- View tabs (#68, redesign spec #61; `layout.rs`). A tab holds a
  session or a view: `TabKind::Session | View(View { view, subject })`
  (`view` = a name the interface knows, `subject` = what it is about, a
  host or run id, or `None`). `open_view(View)` shows the view's tab if
  one is open (that pane becomes active), else opens one at the end of
  the active pane; `view_tab`, `tab_kind`, `is_view`; refused in a session
  window's layout. View tabs move,
  split, close, next/previous like session tabs; `Windows::join_group` and
  `attach_session` refuse them (`LayoutError::NotASession`), `join_pane`
  skips them, so `input_targets` never reaches one. Serialised: `kinds`
  (tab id → `{kind: session}` | `{kind: view, view, subject}`). Shell:
  `LayoutEdit::OpenView { view, subject }`. A subject renamed (#99, a
  host moved): `Layout::retarget_view(&from, subject)` keeps the tab, its
  place and what is shown, about the new subject (closes it instead if
  that view already has a tab); `LayoutEdit::RetargetView { view, from,
to }`, sent by App for `configs` tabs on `onHostsMoved`. Interface: `layout.ts`
  (`ViewName`, `ViewRef`, `TabKind`, `viewOf`), `views.ts` (`VIEWS`:
  title + icon per view; **`openView(name, subject?)`** from anywhere;
  adding a view = its name in `ViewName`, an entry in `VIEWS`, a branch
  in App's view slot). `App.svelte` renders each view tab once as
  `.slot.view[data-tab][data-view-slot=<view>]` over its pane's body,
  like terminals (`[data-view-active]` when its pane is active; `.slot
[data-active]`, the smoke's `SHOWN_SESSION`, stays sessions only).
  `PaneView` tabs: `TabInfo.view`; a view tab has `[data-tab-view=<view>]`,
  the view's icon (`.view-icon`), no `.state` dot nor `data-tab-state`;
  its context menu holds only "Close tab"; `[data-join-pane]` ignores
  views; closing one never asks. The first view is Preferences (the
  dialog and `preferences.dialog` are gone; Credentials' `[data-open-
vault]` and JobTab's `[data-job-python-preferences]` call
  `openView`). Smoke: global `openPreferences(evaluate)` /
  `closePreferences(evaluate)` (the tab's close button);
  `terminalComforts` checks the tab's look, Ctrl+Shift+PageUp/PageDown
  and Ctrl+PageUp/PageDown on it, splits it into its own pane and checks
  theme and font on the terminal beside it, closes it with Ctrl+Shift+W.
  Job tabs (#121): each pre-flight's plan is kept by the shell under its
  own id (the root's `data-plan`), so several job tabs (and a config
  history window's Back up now, which opens one in the main window) can
  sit at pre-flight at once, each Start running its own; a job tab that
  closes before starting lets the shell forget its plan
  (`forgetClosedDrafts`). Details in `jobs.md`, "Plans on screen".
  Host keys at pre-flight (#129): a planned row whose host has
  `hostKeys` shows its message under the details
  (`[data-job-host-key=device|jumpHost]`, `.host-key-warning`,
  `--warning`), and the summary ends with `[data-job-untrusted=<n>]` "1
  host has a host key not trusted yet and will be refused." (`jobs.md`,
  "Host keys at pre-flight").
- Session header and status line (#67, redesign spec #61). The
  session's `header[data-session-header]` is one line of fixed height
  (30 px, no wrapping; the smoke's `checkHeaderLine` checks it with
  markers shown): role, title, `[data-session-address]` (the host's
  address when the title lacks it), `[data-session-jump-host=<id>]`
  (resolved by `App.connect` with `settings_resolve`, SSH host sessions
  only), `[data-session-protocol=ssh|telnet|serial]`, the markers
  (`[data-environment-badge]`, `[data-unencrypted]`,
  `[data-legacy-crypto]`, `[data-multi-send-bar]`, `[data-login-backup]`,
  `[data-log-failed-marker]` "Not logging", `[data-forwards-marker=<n
  listening>]` "Forwarding n" from `ForwardsMarker.svelte` (#21), whose
  click opens `[data-forwards-panel]`: every session's forwards
  (`forwards.svelte.ts`, refreshed on the `forwards-changed` event),
  `[data-forward-row=<name>][data-forward-state]`, `[data-forward-stop]`),
  the status (`role=status`)
  and `[data-session-menu]` (the one button). Production keeps its 3px
  edge and the header's tint; the tab keeps its own markers. The
  multi-send bar is gone (the marker plus a line under the header).
  Actions: `TerminalSession.actions()` → `SessionAction[]` (module
  type of `SessionMenu.svelte`: `id`, `label`, `shortcut`, `title`,
  `disabled`, `attributes`, `run`), shown by `SessionMenu`
  (`[data-session-actions]`, entries `[data-session-action=details|
duplicate|disconnect|reconnect|new-connection|multi-send|end-group|
send-break]`, Escape/outside click closes) from the header's button and
  from the tab's context menu (`PaneHandlers.menu`, App's `tabMenu`,
  plus `close-tab`). Shortcuts unchanged (Ctrl+Shift+M/W, Enter
  reconnects). A closed session's `[data-session-ended]` bar (or
  `[data-session-failed]` when it never opened) has a "New connection"
  button `[data-session-new-connection]`; the bar is left out while a
  card covers the terminal (`closedCard`: a failed negotiation, the
  password fallback), whose backdrop lay over its button (#94).
  `StatusLine.svelte` (App's
  `.window` grid row under `.layout`): `[data-status-line]` with
  `[data-status-vault=off|locked|unlocked]` (`profiles.vault`) and, for
  the active pane's session, `[data-status-session]` and
  `[data-status-size][data-cols][data-rows]` (`TerminalSession.onsize`).
  Smoke: `sessionAction(evaluate, id, slot?)` replaces clicking header
  buttons; the first session part checks the header, both menus' entries
  (Duplicate from the tab's menu) and the status line against `size`;
  `vault` checks the status line unlocked.
- Pane-scoped dialogs (#74, redesign spec #61). Questions about one
  terminal cover only its pane's body; the other panes stay usable
  (typing included). Host key, login questions and `PasteConfirm` sit
  inside the session's `.slot` (the group paste's in the pasting tab's
  slot, members listed). `ConfirmClose` and `ConfirmJoin` are drawn by
  App inside `.pane-question[data-pane-dialog=<pane>]`, placed over
  `rects[pane]` like a slot (the closing tab's pane; for a join the
  tab's pane, or the pane of `joinPane`). Their Escape is handled on
  the dialog itself (not the window, so Escape typed in another
  terminal reaches its device); none of them is `aria-modal`.
  `ConfirmClose` takes `environment` (`[data-environment]`; Cancel
  focused on production, else Close). Shortcuts stay off while a close
  or join question is open, not during a group paste question (hiding
  its tab cancels it; since #123 so does the group changing, the paste
  then called off and said, `terminal.md`). Focus landing in an inactive pane's slot
  activates that tab without a `focusRequest` (which made the old
  active terminal take the keys back from a question there).
  Window-modal and unchanged: `Dialog` (delete,
  rename…), `ConfirmSwitch`, `ConfirmOpenFolder` (#131), the workspaces dialog, vault forms, the
  unlock / start-over screen. Since #79 a snippet's variables are asked
  over its pane too, and the snippet form is in the Snippets view.
  Smoke: `tabsAndPanes` (paste question inside its slot, not over the
  other pane, which takes `size` meanwhile, Escape cancels; the close
  question over its pane, Escape keeps the tab), `multiSend` (join
  questions' `data-pane-dialog`, Escape cancels; group paste inside the
  pasting tab's slot).
- Who gives a terminal the keys later (#141, `src/lib/keys.ts`). Found:
  in a split session window, x focused and typed in, then y focused,
  and on a loaded computer both lines ran in x. Cause: the slot's
  `focusin` sent `activate` for x; y's `focusin` saw y still `focused`
  in the layout not yet answered and sent nothing; x's answer then made x
  active and `TerminalSession`'s focus effect gave x the keys back.
  Shown on purpose by holding the layout answers (below): without the
  fix the keys ended with x, as in the ticket's failing run; a tab
  clicked, then a click on the terminal that had the keys before the
  answer, likewise ended with the clicked tab's terminal taking them. A
  user clicking pane x then pane y within one layout round trip and
  typing was exposed the same way (the click focuses xterm's textarea
  exactly as the test's `focus()` does); a click on a tab then on
  another pane's terminal too. Rule: a focus scheduled for later is given
  only if the keyboard has not moved since it was asked for. `keys.ts`
  counts moves (a `focusin` on another element than the last one, any
  `pointerdown`; capture listeners on the window), `askForKeys(mark?)`
  asks (with a `keyMark()` taken before an await: only if nothing moved
  since), `giveKeys(el)` focuses only while asked and not counted as a
  move, `givingKeys()` says a focus is its. Places that give a terminal
  the keys unasked at that instant, all through `giveKeys` now:
  `TerminalSession`'s effect (pane made active by a layout answer, tab
  shown, `focusRequest` raised after a tab click, a close or join
  question, the palette, the details sheet, the switcher, a menu, a
  session docked into main, a tab arrived in a session window, a
  snippet sent, a group paste asked, a workspace switched) and the
  session's `open` resolving (`if (focused) giveKeys`). Each window's
  `requestKeys(mark?)` replaces `focusRequest += 1`; `edit(change,
  prepare, ask = true)` asks when sent, `false` for edits that follow
  the keys (`focusin` activations, `followKeys`) or are no user's at
  that instant (`retargetView`, leave/end group, view tabs closed by a
  workspace switch, tabs closed by it). Awaiting callers take a mark
  first: `prompt` (a host's connect), `connect(…, mark)`,
  `switchWorkspace` (after its question), `sendSnippet(ToGroup)` in both
  windows, `connectHost`, `onTabArrived`, `closeByDocking`. A slot's
  `focusin` sends `activate` also while edits are pending
  (`pendingEdits`), unless the focus is `giveKeys`'s (two quick Ctrl+Tab
  would otherwise end on the first). `followKeys` (an effect on every
  layout): with no edit pending and no ask standing, a focused slot whose
  tab is not the active pane's shown one is activated (a click on the
  terminal that had the keys sends no `focusin`). Immediate focuses at a
  click or key on that terminal stay direct `xterm.focus()` (host key,
  login answers and Cancel, paste answer, search close, menu close,
  notices dismissed). Not changed: dialogs and side views focusing their
  own fields (`HostKeyCheck`'s button after a tick, the menu's first
  entry) are not terminals. Test hook: `editLayout` (`layout.ts`) awaits
  `globalThis.__smokeHoldLayoutAnswers` when set and counts
  `__smokeLayoutInFlight` once set; nothing in the app sets them. Smoke
  (`severalInAWindow`, after the split typing step): with the answers
  held, focus x, type, focus y, release: y active with the keys, each
  line in its pane; then focus x, hold, click tab z, click x's terminal,
  release: x active with the keys, the line after in x. Both fail
  without the fix (keys with x; with z). `SMOKE_ONLY=severalInAWindow`
  runs that part alone after three quick connections (windows
  `session-1`/`-2`; grants the clipboard as `editInventory` does) and
  `SMOKE_SPLIT_TYPING_REPEAT=<n>` repeats the split typing; a normal run
  is unchanged. The part's "Dock all back left … connecting" failure under
  load was not this: the tabs say connecting until their terminals have
  attached, and the step read them at once; it now waits for connected,
  as Dock back does.
- First window and empty states (#75, redesign spec #61). With no
  workspace open, the active empty pane's `.slot.connect` shows
  `Welcome.svelte` (`[data-welcome]`, h1 = the app name) instead of the
  connect view: `[data-welcome-workspaces]` lists `workspaces_list`
  (rows `[data-welcome-workspace=<path>]`, a gone folder disabled with
  `[data-unavailable]` "folder not found", each with
  `[data-welcome-forget]`; `[data-welcome-none]` when empty), forms
  `[data-welcome-form=open|create]` (input `welcome-folder` + Browse…),
  `[data-welcome-error]`, and `QuickConnect` `embedded` (h2 "Quick
  connect"). Opening goes through App's `switchWorkspace`; since #131
  opening an existing folder (a row or the open form, here and in the
  workspaces dialog) goes through App's `openFolder` (`onopen`), which
  first asks `ConfirmOpenFolder` (window-modal, Cancel focused) about a
  folder of other things (`workspace.md`, "What kind of folder"). With a
  workspace open the empty pane is the connect view
  (`main[data-connect-view]`, `QuickConnect` with `onpalette`): a
  `[data-empty-pane=palette]` button opens the palette, then SSH/serial
  quick connect as before. Side views without a workspace say so in a
  `[data-needs-workspace]` hint (Hosts, Snippets, Jobs, Config backups,
  Credentials). A workspace with no host at all shows `[data-hosts-empty]`
  above the tree with `[data-hosts-empty-add=host|csv|nornir]` (the add
  control's entries). `UnlockVault` has the new look (a
  `[data-unlock-mark]` lock, a danger mark on start over; markers and
  behaviour unchanged). The switcher stays at the Hosts view's
  `[data-open-workspaces]` and Ctrl+Shift+O. Smoke: the first launch has
  `state.yaml` with `workspaces` (the smoke workspace and a gone one) and
  no `last_workspace`, so `welcome` checks the list, the side views'
  hints, a refused create, forgets the gone one and opens the workspace
  from its row; then the empty pane's palette button; `workspaces` checks
  the empty workspace's offer (New host form, Cancel); `vault` checks the
  unlock mark. Fix found on the way: a terminal whose pane becomes
  active no longer takes the focus from a `[role=dialog]` holding it
  (the close question lost Escape to a late `activate`).
- The first five minutes (#116): the first-run path walked in the built
  app with an empty data folder, as a newcomer (pictures and findings
  outside the repo; the findings are the closing comment of #116).
  Interface changes, words only: the Hosts view's `[data-needs-workspace]`
  names "“No workspace” above"; the connect view's `[data-connect-hint]`
  says a double-click in the Hosts view connects; the switcher's add form
  is "Open an existing folder" / Open (welcome's words); a session's
  ended bar shows the core's advice (`StateMessage.advice`, see
  `sessions.md`) as `[data-session-advice]` and then "Press Enter to
  reconnect." (it said "try again with the same settings" until #130,
  when a reconnect began to use the host's settings and profile of now;
  a refused reconnect shows its reason and advice in the same bar);
  `[data-session-failed]` and the SSH agent's card
  (`[data-password-fallback]`) show it too; `CredentialsPrompt` (a typed
  login, no profile, no agent fallback) has `[data-credentials-hint]`;
  `SettingsPanel` notes for `credential_profile` and `jump_host`
  (`[data-setting-note=…]`); `CredentialsView` `[data-profiles-use]`
  under the list and a fuller empty state; the CSV and Nornir reports'
  `[data-report-next]` when hosts were created. Smoke: `welcome` (the
  refusal's next step, a missing folder in plain words, the Hosts hint),
  main (the connect hint; the auth failure's advice and "Press Enter to
  reconnect.";
  the refused port's advice), `workspaces` (New host: a port in the
  address, an empty name; the switcher's words), `csvImport` (the
  report's next step, none when nothing was created; a missing file; a
  file without column names), `credentialProfiles` (`[data-profiles-use]`,
  the setting's note), `agentAndKeyboardInteractive` (the agent card's
  advice), `legacyCrypto` (the typed login's hint).
- Redesign closed (#79, spec #61). `docs/interface.md` is the user's map
  of the window (rail, side views, details sheet, view tabs, real
  windows, which questions are pane-scoped and which window-modal),
  the shortcuts, and where each 0.2.0 feature now lives; keep it in
  step when moving a feature. No long-lived view is a dialog: the
  snippet form moved into the Snippets view and a snippet's variables
  are asked over its pane (below). Carry-over: `tests/preferences.rs`
  loads a file with every key 0.2.0 wrote; the smoke checks that
  opening the workspace changes none of its files (only
  `_workspace.yaml` may be added). Smoke: `sessionAction` reopens the
  menu until the action is offered (an entry missing just after a state
  change failed now and then; `within` 0 tries once); the DevTools
  client (`connectTarget`) fails a call whose page went away (a window
  closed by that very call never answers: it hung a run) and any call
  unanswered for 60 s; the jobs part takes the new terminal's tab, not
  an older one already showing the device's prompt; `tabsAndPanes`'
  tab drag hovers first and moves in steps; and the panel-width save is
  awaited in the file instead of a fixed sleep.
  The remaining `sleep`s wait for something not to happen (input that
  must not arrive) or for a process to let go of its files.
- Side view width. `Preferences.left_panel_width` (pixels,
  `MIN_PANEL_WIDTH` 200 to `MAX_PANEL_WIDTH` 800, default 288) and
  `left_panel_hidden` are the side view's (see the rail below); the
  right panel's `right_panel_width` / `right_panel_hidden` are retired
  (#64): an older file loads, those keys are ignored and no longer
  written, and the interface's own are ignored too. `App.svelte` sets
  the grid columns (rail, side view, terminals); `PanelResizer.svelte`
  sits on the side view's edge (`[data-panel-resizer=left]`: drag, saved
  on release; double-click = default; arrow keys; `[data-hide-panel]`
  button). Ctrl+Shift+B folds the side view (not Ctrl+B: tmux). The
  terminal area keeps at least 320 px. `changePreferences` keeps unsaved
  changes in `pending` and reapplies them over every answer from the
  core, so an older answer cannot undo a newer change. Smoke:
  `sidePanels` (widens the window to 1400 px).
- Details sheet (#64). No right column: `DetailsSheet.svelte`
  (`section.side[data-details-sheet=host|folder]`, so smoke selectors
  `.side …` still reach the details) lies over the lower part of the
  Hosts side view, the tree keeping the top 38% (`.hosts-area.under-sheet`)
  (its problems line is hidden meanwhile: the rail marks them) so that selecting there changes what it shows (it follows
  `inventory.selected`); it never covers a terminal. `inventory.details`
  = open; `openDetails(item)` (inventory) selects, reveals and opens;
  `showDetails` in `App` also brings back the Hosts view. It closes by
  `[data-details-close]` ("Back to the tree"), Escape outside a field,
  Ctrl+Shift+D (which opens it on the selection too), and by itself when
  nothing is selected or another side view is shown or the side view
  folds. Opened from a row's `[data-row-details=<path>]` control (beside
  the row button, shown on hover), the tree's context menu
  (`[data-menu=details]`), Ctrl+Shift+D and a session's menu entry
  `[data-session-details]` (`TerminalSession.ondetails`, only for a host
  of the open workspace). `HostDetails`: lead (name, file, environment
  `[data-details-environment]`, address, jump host, `[data-ways-in]`
  with `data-connect` and web links, then the actions as one row of icon buttons (`.actions.more`: each keeps its name in a `.visually-hidden` span, which the smoke's `click(label)` finds, and shows it as its tooltip)), sections
  `[data-details-section=device|connection|tags|custom|description]`;
  `FolderDetails` the same without custom fields. `SettingsPanel` takes
  `part: "device" | "connection"` (connection = its rows, the serial
  line and web links). `ways.ts` (`otherWays`, `ownWayBlocked`,
  `jumpHostOf`) is shared by the sheet and the context menu. The
  context menu's entries carry `data-menu=<id>` (`connect`,
  `connect-<protocol>` (disabled with a title when blocked, once the
  host's settings are resolved), `details`, `run-snippet`, `run-script`,
  `back-up`, `enrich`, `config-history`, `log-folder`, `rename`, `move`,
  `delete`; folders `details`, `new-folder`, `new-host`, `import-csv`,
  `import-nornir`, …). Smoke: `showDetails(evaluate)` opens the sheet on
  the selection (by the row control) and is called before every `.side`
  probe; `sidePanels` checks no right column, the row control, follows
  selection, Back, Ctrl+Shift+D, the context menu's entries, closing
  with the side view; `telnet` checks the session header's Details.
- Several hosts selected (#110, interface only: the core already took
  `JobTarget::Hosts { ids }`). `inventory.several` = the host ids of a
  selection of two or more (else `[]`); while it holds hosts
  `inventory.selected` is null, so the details sheet closes by the rule
  above (nothing selected) and Ctrl+Shift+D does nothing: the sheet never
  shows several (chosen over a count with job buttons, which the Hosts
  view's foot and the menu already give). Change the selection only
  through `select(item)` (one item or null; ends several; App's `prompt`
  and every former `inventory.selected =` use it), `clickHost(id,
  { toggle, range })` (TreeFolder's host rows: Ctrl/Cmd toggles, Shift
  takes the range from the last host clicked without Shift, over
  `hostsInTreeOrder(tree, shown)`: folders first then hosts, open folders,
  `hostShown`) and `clearSelection()`. Two left → several, one → that host
  alone, none → nothing. `selectedHostIds`, `isHostSelected`,
  `inTreeOrder(ids)` (the order a job gets). `adopt` (every edit and live
  reload) drops gone hosts (`keepSelectedIn`); `moved` rebases them;
  `setWorkspace` clears. Selected hosts stay shown under a filter, like the
  one selected item. Folder clicks with Ctrl/Shift are plain clicks. In
  `HostsView`: a drag clears it; Escape in the tree (no menu open) clears
  it; F2 and Delete do nothing meanwhile; the context menu on a row of the selection keeps it and shows
  `severalEntries` (`run-snippet`, `run-script`, `send-file`, `back-up`,
  `enrich`, on `{ kind: "hosts", ids }` in tree order, labelled by
  `jobTargetLabel` "N hosts"), the menu marked `[data-menu-hosts=n]`; any
  other row is selected alone first. Under the tree (so rows do not move
  as it appears) `[data-selection=n]` "N hosts selected" with
  `[data-selection-clear]`. No arrow keys in the tree, so no Shift+Arrow.
  Smoke: `mouseAt`'s `modifiers` (`CTRL` 2, `SHIFT` 8); `hostSelection`
  (after `savedJobs`, on the Jobs folder) checks Ctrl+click add and take
  out, the count, the sheet closing, the menu's five entries, a snippet
  job on two (pre-flight exactly those, one skipped, not saveable, runs),
  the clear button, a Shift range, Escape, a right-click outside, Ctrl on
  a folder, a drag, a Shift range under the filter field, selected hosts
  deleted on disk; `fileJob` ends with Send file… on two Staging hosts
  picked with Ctrl, pre-flight in tree order. Not verified: Cmd on macOS
  (no macOS build), the keyboard (Tab to a row then Escape aside).
- Activity rail and Hosts view (#63, redesign spec #61).
  `Preferences.side_view: SideView` (`hosts|snippets|jobs|backups|
credentials`, default hosts; file key `side_view`, camelCase `sideView`;
  an older file or an unknown value gives hosts); its width and folding
  are `left_panel_width` / `left_panel_hidden`. `ActivityRail.svelte` (grid
  column of `RAIL` 48 px in `App.svelte`): `[data-rail=<view>]` buttons
  (`aria-label`, tooltip with the shortcut, `aria-pressed` and
  `data-active` when open; clicking the open one folds, any icon while
  folded opens its view: `selectView`), `[data-rail=hosts][data-problems=n]`
  while problems exist and the Hosts view is not shown, `[data-rail=jobs]
[data-jobs-running=n]` (+ pulsing mark), credentials marked when
  profiles are missing, `[data-open-preferences]` at its foot. The view
  icons keep focus off themselves (`keepKeys`: mousedown preventDefault,
  #94), so a terminal keeps the keys while a side view is picked with the
  mouse.
  `SIDE_VIEWS` holds the shortcuts: Ctrl+Shift+H/S/J/G/L (Hosts,
  Snippets, Jobs, Config backups, Credentials; the open one folds), Ctrl+,
  preferences. The side view is `aside[data-side-view=<view>]`.
  `HostsView.svelte` (was `WorkspacePanel`): header with the workspace
  name `[data-open-workspaces]` (opens the switcher), icon buttons search
  (`aria-label="Search hosts"`, palette), `[data-workspace-settings]`
  (selects the root folder), `[data-open-platforms]`, `[data-add-menu]`
  → `[data-add-entries]` `[data-add=host|folder|csv|nornir]` (into
  `targetFolder()`); the filter field `input[name=host-filter]`
  (`inventory.query`; `trackFilterView`, now run by `App`, sends the
  active saved filter's conditions plus the words as a `text` condition
  to `filter_apply`, so the core does the matching and both combine);
  chips `[data-filter-chip=<name>][aria-pressed]` (click applies, again
  clears; a pressed chip keeps its on-accent text under the mouse, #94),
  `[data-save-filter]` (prefills a `text` condition with the
  typed words, cleared once saved), and with a filter applied
  `[data-filter-actions]` icon buttons (`data-filter-run-snippet|
run-script|back-up|delete`; the smoke's `filters` clicks Run snippet and
  Back up configs since #127: each opens its job tab titled for "the hosts
  of Lab routers"); the tree as before; `[data-problems=n]`
  one line at the foot toggling `[data-problems-list]`. While the
  workspace folder cannot be reached or came back as another workspace
  (#132, `WorkspaceView.unavailable`, see `workspace.md`), a warning
  notice at the top, above the move report:
  `[data-workspace-unavailable=unreachable|otherWorkspace]` with the
  core's message and explanation (and a Workspaces… button
  `[data-workspace-unavailable-open]` for another workspace); it goes when
  a reload loads the folder again. The rail does not mark it. Tree host rows
  carry `data-environment` and `[data-host-environment=production]`
  ("PROD" label) / a staging dot / a lab ring. `SnippetsView` (#65) and
  `SavedJobsPanel` fill their side views, as does `CredentialsView`
  (#66, below), `JobsSideView` (#69) and `BackupsView` (#70, see config
  backups below). Smoke
  helpers `showSideView(evaluate, view)`, `openRun` (#69; switches back to
  Hosts after), `openProfiles` / `newProfile` (#66); `sidePanels` checks the rail, folding,
  shortcuts and leaves Snippets selected for the restart, which checks
  view and width.
- Own window frame (#80). Every window is built with `decorations(false)`
  and `shadow(true)` (`src-tauri/src/lib.rs`: main, and `open_detached` for `window_detach_run` and
  `window_detach_configs`); the shadow gives Windows 11's rounded corners,
  and tao keeps resizing from the edges and Aero snap for an undecorated
  window. Capabilities add `core:window:allow-minimize`,
  `allow-toggle-maximize`, `allow-start-dragging` (double click uses
  `internal_toggle_maximize`, in `core:default`). `WindowControls.svelte`
  draws `[data-window-controls]` with `[data-window-control=minimize|
maximize|close]` (`aria-label` = tooltip; Maximize becomes Restore with
  `data-maximized`, refreshed on `onResized`); Close is the window's own
  `close()`, so it behaves as the system's close did (main closing ends
  the app via `close_detached`). Drag regions are Tauri's
  `data-tauri-drag-region="deep"` (its script skips buttons, fields,
  `role=tab` and focusable elements, so tab dragging is untouched): the
  tab bars of panes at the window's top (`PaneView` props `top`/`right`,
  worked out down the split tree; `[data-window-bar]`), the side views'
  first `<header>`, the detached windows' `header.frame` (title in
  `[data-window-title]`, same text as `document.title`), and a bare strip
  on `UnlockVault` (which covers everything). Only the top-right pane's
  bar (`.bar.framed`, no right padding) holds the buttons; the tabs
  shrink and scroll before them, so they never cover a tab. Known limits:
  Windows 11's snap layouts flyout on hovering Maximize needs a native
  `HTMAXBUTTON` hit test, which a webview button cannot give, so it is not
  offered. Checked by hand on Windows 11 (#80): the window keeps
  `WS_THICKFRAME | WS_MAXIMIZEBOX | WS_CAPTION` (tao hides the caption
  without dropping the style), Win+Left snapped it to half the screen and
  Win+Up then to a quarter; Win+Z (its flyout) was not checked by a
  script, only expected from that style. In a narrow top-right pane the
  pane's tools (`.tools`: join, split, close pane) give way first; the
  shown tab keeps `min-width: 3.5rem` and the window's buttons never shrink.
  Once clipped, the tools fold (#100; clipped, they could not be clicked
  at all, #94): `.tools.folded` takes no room (and is still measured), and
  a `[data-pane-tools-more]` "…" button (aria-haspopup, does not shrink)
  opens a `SessionMenu` with `data-session-action=new-tab|join-pane|
split-right|split-down|close-pane` (new tab and multi-send only where
  the pane `connects`, multi-send while a tab can join, close pane with
  several panes). The new-tab button folds with them: with the "…" alone,
  a 230 px pane pushed the window's Close out of the window. `fitTools`
  (a ResizeObserver on the bar, its tabs, tools and spacer) folds when the
  tools' `scrollWidth` exceeds their width, and unfolds once the spacer
  plus the "…" can hold them and the new-tab button. Closing the menu
  calls `PaneHandlers.refocus` (the terminal takes the keys again). At
  800 px the tools fold in the top-right pane with two panes side by side
  and with two tabs in one pane. Smoke: `tabsAndPanes` and `terminalComforts`
  check the folded entries, `severalInAWindow` a 560 px session window
  (folded) and 1200 px (back in the bar). The tools keep focus off themselves (`keepKeys` in `PaneView`, #94): a
  mouse click on Split right had taken the keys from the terminal.
  A narrow window (#120). Found: once the top-right pane's bar was
  narrower than its parts (about 175 px with an empty pane, 227 with a
  tab), the window's buttons ran out of it, cut off by `.centre`'s
  `overflow: hidden` at the window's edge: a main window under ~560 px
  with the side view open (288 px), under ~685 px with two panes side by
  side. Reproduced in the built app (a throwaway DevTools script on a
  desktop of its own) by overriding the fix's CSS at run time: with one
  tab and the side view at 288 px, at 560 px the buttons ended at 563 px,
  at 200 px at 563 too; with the fix, at 560 px they ended at 560 (the side
  view 261.5 px, the top-right bar 251 px), at 620 and 700 unchanged. Fix, CSS only: the top-right
  pane (`.pane.corner`, `top && right`) keeps `min-width:
  var(--corner-pane-min)` (app.css: the bar's padding, one tab's 3.5rem,
  `--space-6` of empty bar to drag by, the "…" button, the gap and three
  `--window-control-width` buttons, four bar gaps; `--pane-tool-size` and
  `--window-control-width` are the sizes `PaneView` and `WindowControls`
  use); every `.pane` and `.split` has `contain: inline-size` (so tab
  names never make one wider) except a `.split.corner`, which takes
  `min-width: min-content` (its corner child plus the 2 px gaps); other
  panes give way to nothing and `overflow: clip` what does not fit (no
  drawing over the neighbour; clip, not hidden, so a tab's
  `scrollIntoView` never scrolls the pane). The framed bar's `.spacer`
  keeps `min-width: var(--space-6)`, and `fitTools` unfolds only when the
  tools fit beside that. In the main window the grid is `48px minmax(0,
  <width>px) minmax(min-content, 1fr)`: the side view gives way (at 480
  px it shows about 180 px), its resizer's edge follows the width shown
  (`bind:offsetWidth`). Session windows (420 px least, no side view) and
  the run and config history windows (`header.frame`, title `min-width:
  0`) needed nothing more. Smoke: `narrowFrame(evaluate, where, least)`
  sets the window to its minimum (480 main, 420 others) with DevTools'
  `Browser.setWindowBounds`, which does not keep the minimum (asked for
  200 px, the page was 200 px wide), then checks the three buttons whole in the window at its top right, the
  top element at their centre and four corners, nothing else of the bar
  ending past their left edge, an empty part (spacer or title) of 16 px or
  more that is a drag region, Minimize reachable, Maximize and Restore
  clicked (back to the least width), then the old width. Called by
  `tabsAndPanes` (three panes, side view open then folded, Ctrl+Shift+B),
  `sessionWindows` (one pane), `severalInAWindow` (two panes side by side),
  `detachedRunWindow` and `closingTheMainWindow` (config history). Not
  clicked: Minimize (as #80), and Close only where the steps already close
  the window.
  A window-modal dialog's backdrop
  covers the bar, so the window cannot be moved while one is open. Linux:
  not checked on a desktop (item 9 of `docs/linux.md`'s desktop list).
  Smoke: `windowFrame` right after launch (`is_decorated` false via
  `windowCommand`, `checkOwnFrame`: three named buttons at the top-right
  corner; maximize and restore by the button and by a CDP double click on
  the bar's empty part), `tabsAndPanes` (after split right and down: the
  buttons in the top-right pane's bar only, over no tab, the lower pane's
  bar no drag region), `vault` (the unlock screen's strip and buttons),
  `detachedRunWindow` (frame, title, closed by its Close button),
  `closingTheMainWindow` (config window's frame; the main window's Close
  button ends the app).
- Session windows (#84). A session moves live to a window of its own
  and back. Core: `Windows::move_session(from, tab, to)` (a new tab at
  the end of `to`'s active pane, shown, with the tab's session; the old
  tab closes; a member stays in the group since #87; a view tab, the same
  window or an unknown one is `NotASession` / `SameWindow` /
  `NoSuchWindow`, changing nothing), `session_tabs(window)` (session
  tabs with a known session, in reading order), `keep_snapshot(window,
  tab, snapshot)` (what the giving window showed, for the new page),
  `take_session(window)` (since #85 `take_sessions`, every session tab) → `Taken { tab, session, snapshot }` (the first
  session tab, its snapshot used up: a reloaded page gets `None`; `None`
  overall until the tab has moved there) and `hand_to_main(window)` (a
  window gone without docking back: each session tab moves to main, with
  the snapshot its page never took, and the window closes; main is
  refused). Snapshots go with their window. Tests: `tests/windows.rs`.
  The group spans windows (#87: `terminal.md`, "Multi-send across
  windows"). Shell
  (`src-tauri/src/session_windows.rs`): a session window is labelled
  `session-<WindowId>` (`calling_window` maps it; `session-0` is refused),
  built near the calling window (+40, +40 logical) at the inner size the
  interface asks (the slot's size plus the tab bar and the status line,
  so the terminal keeps its columns and rows), shrunk and moved to fit the
  monitor's work area, focused, `decorations(false)` + `shadow(true)`.
  Placed by physical position (#90): the arithmetic is the core's
  `layout::place_session_window(from, scale, work_area, size, frame)`
  (it also keeps `SESSION_WINDOW_MIN_SIZE`, 420x320 logical)
  (`src/layout/placement.rs`, tests `tests/window_placement.rs`), which
  gives an outer position in pixels and a logical inner size. `build`
  makes the window hidden (no position given), moves it there, sets the
  logical size again (moving to a monitor of another scale changes its
  scale), measures its `Frame` (inner minus outer position: the invisible
  border of a window with a shadow, 7 px at 100%, 11 at 150%), places it
  again so that its inner area fits the work area, then shows and focuses
  it.
  Its capability is `capabilities/session.json` (`session-*`). Commands:
  `session_record`, `session_release` (only the window showing it),
  `window_open_session { tab, id, snapshot, width, height }` (main
  only; builds the window first, then moves the tab and keeps its
  snapshot under one lock; returns main's layout; a failed build closes
  the core window and the interface takes the session back),
  `session_window_take` (the page's first call: `take_session`, polled
  for up to 10 s; without a snapshot, whoever held the session is let go
  of), `session_attach` (the window must show it in a
  tab; returns key, offset, transport and the relay's last state),
  `session_dock { tab, id, snapshot }` (moves the tab to main, closes the
  core window, emits `session-arrived` to main, destroys the window),
  `session_window_close` (its tab closed: forgets, discards and closes).
  `WindowEvent::Destroyed` of a session window still in `Windows` (it did
  not dock back): `hand_to_main`, each session let go of, and
  `session-arrived` (snapshot null, or the one its page never took);
  with main gone too, the sessions are discarded. Layout edits from a
  session window: `layout_reset` is refused. A workspace
  switch's `windows_close_detached` leaves session windows open (their
  sessions stay open, as tabs do; main's Destroyed still closes them
  all). Interface:
  `SessionWindow.svelte` (mounted by `main.ts` when `isSessionWindow()`):
  `.window[data-session-window]` (`[data-session-window-error]` when it
  got no session or a dock failed),
  `PaneView` with `tools={false}` (no new tab, split, join or close pane; since #85 `connects={false}`, below, which keeps split and close pane;
  the bar is the window's, with `WindowControls`), the session's
  `.slot[data-tab][data-active]` over the pane's body, `StatusLine`
  (no vault), `ConfirmClose` over the slot when its tab is closed live.
  Its tab and header come from the record; multi-send since #87. `onCloseRequested` always prevents; since #88
  it asks (`ConfirmCloseWindow`, below). A refused Ctrl+Shift+N says nothing (the menu entry gives the
  reason). Shortcuts there: Ctrl+Shift+N dock, Ctrl+Shift+W close,
  zoom. The main window: `openInWindow(tab)` in `App.svelte` (the menu
  entry `[data-session-action=open-in-window]` from `TerminalSession.
  actions()`, the tab's context menu, Ctrl+Shift+N), `onSessionArrived`
  gives the arriving tab an `OpenSession` from its record (`source:
  adopt`) and adopts the layout sent with it. `windows.ts`:
  `openSessionWindow`, `takeSession`, `dockSession`,
  `closeSessionWindow`, `onSessionArrived`. Smoke `sessionWindows`
  (after `multiSend`): a member moved from its menu (`session-1`: the
  window's parts and none of rail, side views, new tab, split; title;
  same header, environment and colours; size reported if the screen did
  not allow it; contrast and own frame; since #87 the group checks of
  `terminal.md`), Dock back from its menu
  (scrollback), Ctrl+Shift+N during `lines 9000` (`session-2`, unbroken),
  `mouse on` + `program` then Ctrl+Shift+N docks back with the alternate
  screen, bracketed paste, application cursor keys, mouse tracking and
  SGR encoding, a paste question disabling the move (title) and the
  shortcut, a closed session moved (`session-3`) and reconnected with
  Enter, `Page.reload` there (the lost line, typing works), its Close
  button docking back with scrollback. Not covered: a session window
  destroyed without docking back. While its page lives, every close goes
  through its handler (which docks), and a crashed page is taken over
  (#89, below) before its window is destroyed. Only the app's exit
  destroys one otherwise, so the `Destroyed` path itself was not driven
  by the smoke nor by hand; it shares `hand_over` with #89's path. Two monitors checked (the sweep
  of #88, below); monitors with different scaling checked for #90 (below). Not checked by
  hand: telnet and serial sessions moved in the app (the core tests and
  the same app path cover them). `sessionWindowSize` measures a pane's
  own tab bar (`[data-tab-bar]`), not `[data-window-bar]`: that is also
  on a side view's header, first in the main window, which is 3px taller
  with no workspace open, and the new window's terminal then gained a row
  now and then (found by the #88 check, fixed).
- Several sessions in a session window (#85). Core (`Windows`):
  `move_session` from a session window left without a tab closes it
  (the main window never closes); `take_sessions(window)` (every session
  tab, reading order, snapshots used up; empty until one moved there)
  replaces `take_session`; `take_tab(window, tab)` (a tab moved into an
  open window, its snapshot used up; `None` if not shown there);
  `session_windows()` (each session window with the session of its
  active tab, `None` while unknown); `hand_to_main` closes the window
  only if a tab is left. Tests: `tests/windows.rs` (moving into an open
  window, from one session window into another, the last tab out closes,
  take every session, take one tab once, windows named by their active
  tab, hand over two). Shell (`session_windows.rs`): `window_open_session
  { tab, id, snapshot, width, height, into }` from any window: `into`
  null builds a new window (near the calling one); else the tab moves
  into that open session window, its snapshot kept, the window told by
  `session-tab-arrived { tab }` (emitted to its label; the page listens
  with `getCurrentWebviewWindow().listen`, since a plain `listen` hears
  events emitted to any window) and focused. Returns the caller's layout,
  or null once the caller closed (its window destroyed). `session_window_take`
  → `{ sessions: [{session, tab, snapshot, record}], layout }`;
  `session_window_take_tab { tab }` → `{ session, layout }`;
  `session_dock` (one tab) and `session_dock_all { docking: [{tab, id,
  snapshot}] }` (in order) move to main, emit `session-arrived` per
  session, and return the window's layout or null (closed);
  `session_windows_list` → `[{window, name}]`, and `session-windows`
  (the same list) is emitted to every window after every open, move,
  dock, close, hand-over and session-window layout edit (`announce`).
  `capabilities/session.json` adds `core:window:allow-set-title`.
  Interface: `PaneView`'s `tools` became `connects` (new tab, Connect
  here, join pane; split and close pane are always offered); without it
  an empty pane says `[data-empty-pane=drop]` "Drag a tab into this
  pane". `PaneRects` (`paneRects.svelte.ts`, shared with `App`) places
  the slots; `sessionWindowSize(rect)` sizes a new window. `TerminalSession`'s
  `move` became `moves: SessionMove[]` (`SessionMenu.svelte`: `id`,
  `label`, `title`, `shortcut`, `attributes`, `refusal()` besides the
  session's own `moveRefusal`). `sessionWindows.svelte.ts`:
  `sessionWindows.list`, `trackSessionWindows()`, `windowMoves(open,
  shortcut)` → `[data-session-action=open-in-window]` "Open in new
  window" and one `open-in-window-<n>` per other open session window
  ("Open in the window of <name>", `[data-into-window=n]`).
  `SessionWindow.svelte` now holds a layout like App's: sessions by tab
  (`source` adopt or duplicate, and the record), states, sizes, the
  `ConfirmClose` over the closing tab's pane (`[data-pane-dialog]`), tab
  context menu; shortcuts Ctrl+Tab / Ctrl+PageUp/PageDown, Ctrl+Shift+
  PageUp/PageDown, Ctrl+Shift+W, Ctrl+Shift+N (dock the active tab),
  zoom; Ctrl+scroll never zooms the page. Each session's moves: `dock`
  "Dock back", `dock-all` "Dock all back" (with more than one tab;
  refused unless every session may move), then `windowMoves`. Closing a
  tab: the last one goes through `session_window_close`, others through
  `closeTab`. Closing the window asks since #88 (below).
  The title (`document.title` and `setTitle`) is the active tab's name.
  Smoke `severalInAWindow` (end of `sessionWindows`, windows `session-4`
  and `-5`): Open in window into the open window from the main window and
  from `session-5` (which closes), the title, Ctrl+Tab and
  Ctrl+Shift+PageUp/PageDown there leaving the main window's tab alone,
  Duplicate beside the original there (closed again), split right with
  typing in each pane, search, copy, paste and highlighting there, zoom
  and theme from the main window, Dock all back (three sessions,
  connected, scrollback). Not covered: the title in the taskbar (only
  `document.title` is read; `setTitle` is called with it).
- Safety follows the session into its window (#86). Core:
  `SessionRelays::reported(session)` → `Reported { state, algorithms,
  log_failure, backup }` (replaces `state`; `Relay::reported` replaces
  `shown_state`): what was delivered to any consumer of the session's
  current relay, so a window taking it later (moved, or taken over from a
  window that died) shows the markers reported once as events. A reconnect
  starts a new relay: it reports afresh. Tests: `tests/session_relays.rs`
  (kept across a release and a dead window; a reconnect reports afresh),
  `tests/login_backup.rs` (the backup outcome kept). `Windows::use_tab(
  window, tab)` records the session of the terminal used last (a view tab
  or unknown window refused; a tab whose session is not known yet changes
  nothing); `Windows::last_used()` → `Used { window, tab, session }`, where
  that session is shown now (it follows moves; `None` once no tab shows
  it). Tests: `tests/windows.rs`. `SessionRecord::host_in(workspace)`: its
  host while that is the workspace it came from and has it (snippet values;
  `tests/session_record.rs`). Shell (`session_windows.rs`):
  `session_attach` adds `algorithms`, `logFailure`, `backup`;
  `terminal_used { tab }` (any window; tells main `snippet-target` when it
  changes), `snippet_target` → `{window, tab, session, name}` or null, and
  `announce` also emits `snippet-target` to main; `snippet_send_to {
  target, tab, id, name }` (main only; refused unless that tab shows that
  session and its last state is connected) emits `snippet-send { tab, name
  }` to that window and focuses it; `session_snippet_expand { id, name,
  answers }` (lib.rs) fills a snippet from the record's `host_in` the open
  workspace; `main_window_request { tab, request }` (session windows;
  `request` `{kind: details}` or `{kind: passwordLogin, reason}`; refused
  unless the record's `host_in` the open workspace, so the asking tab
  stays) emits `main-window-request` (the request, the session's record
  and that host) to main and focuses it. `Windows::session_of(window,
  tab)`: the session a tab shows. Interface: `Session.taken` (`Reported`, replaces
  `takenState`) is applied by `TerminalSession` unless newer events came
  first; every slot's `focusin` calls `terminalUsed(tab)` (`windows.ts`);
  `App` keeps `usedElsewhere` and, while it is a session window's, the
  Snippets view and palette name it (`[data-snippet-target]`) and send
  with `sendSnippetFromView` → `sendSnippetTo`; `onMainWindowRequest`
  opens the details sheet or the credentials prompt (`fallback` from
  `hostLogin`, when the host has a profile) for a host of the open
  workspace, else says it is not there. `SessionWindow`: `sendSnippet`
  (activates the tab, `expandSnippetForSession`, `SnippetVariables` over
  the slot, then the terminal's `sendText`, so `pasteText`), `ondetails`
  and `onpasswordlogin` for a session with a host (the latter closes the
  tab once the main window was asked), `windowQuestion` while variables
  are asked. Paste safety there is the record's (the host as it was when
  the session opened), so a host removed from the workspace keeps its
  safeguard. Smoke `safetyInSessionWindows` (after `pasteSafety`; prod-r1
  moved): production marks in the window, a two-line paste confirmed over
  its pane with the 400 ms delay, the Snippets view naming prod-r1 there,
  a multi-line snippet's production question there with the window
  asked forward (said, not failed, when Windows refuses the focus), a variables question there, the name following the focus back,
  a `drop` + Enter reconnect with known_hosts emptied asking first-use
  there (Trust once), Details opening the sheet in main, Dock back. Not
  covered by the smoke: the unencrypted, legacy-crypto, logging-failed and
  backup markers after a move and after a window died (the core tests
  cover what is kept; the interface applies `taken` as it applies the
  state), the login question and the password login of a reconnect in a
  session window (same paths as the host key), the negotiation-failure
  card there.
- Closing rules and the palette in session windows (#88). Core:
  `Windows::session_window_sessions()` (every session window's session
  tabs, window by window, as `(Member, SessionId)`; the main window's are
  left out, its page knows them) and `Windows::close_sessions(&[SessionId])`
  (closes the session windows' tabs showing them, a member leaving the
  group; a session window left without a tab closes and is returned; the
  main window's tabs are its page's). `SessionRelays::live(session)`: a
  relay whose last reported state is not closed (nothing reported yet is
  opening). Tests: `tests/windows.rs` (listed window by window, closed
  with the emptied window, a member dropped), `tests/session_relays.rs`
  (live while opening and connected, not once closed or unknown).
  `layout::live_in_session_windows(windows, relays, records, workspace?)`
  (how many of the session windows' sessions are live, only those whose
  record `of_workspace` that root when given) and
  `layout::close_workspace_sessions(windows, manager, relays, records,
  workspace)` → `WorkspaceClosed { closed, changed }` (each session of
  that workspace in a session window: its tab closed, relay forgotten,
  session discarded, record dropped; `src/layout/closing.rs`);
  `SessionRecord::of_workspace(root)`. Tests: `tests/session_relays.rs`
  (counted all and by workspace, the main window's left out, a closed one
  not counted; closed in every session window, the emptied one closed,
  the other changed, a quick connection kept). Shell
  (`session_windows.rs`): `session_windows_live { workspace? }` → the
  count; `session_windows_close_workspace { workspace }` (main only: the
  core's `close_workspace_sessions`, `layout-changed` to the changed
  windows (`group::LAYOUT_CHANGED_EVENT`, now `pub(super)`), the closed
  ones destroyed, `announce`); `main_window_close` (main
  only: destroys it, so `close_detached` on its `Destroyed` closes every
  window and the app ends); `main_window_connect { host }` (a session
  window, a host of the open workspace: emits `main-window-request`
  `{kind: connect, host, record: null}` and brings main forward;
  `main_window_request` refuses `connect`). Interface: the main window's
  `onCloseRequested` (App) adds `liveInSessionWindows()` (`windows.ts`, 0
  if the shell cannot tell) to its own
  `liveSessions`: none closes at once (`closeMainWindow`), else
  `ConfirmQuit.svelte` (`[data-confirm-quit]`, `[data-quit=cancel|close]`,
  Cancel focused, names how many are in session windows; window-modal,
  `quitting` stops the shortcuts). `switchWorkspace` adds
  `liveInSessionWindows(current.root)` to the count (`ConfirmSwitch`'s
  `elsewhere`, worded by `inSessionWindows` as `ConfirmQuit` is) and on Close calls
  `closeWorkspaceSessionsElsewhere(current.root)` after its own tabs.
  `onMainWindowRequest` handles `connect` with `prompt(host)`.
  `SessionWindow`: `onCloseRequested` with no live tab closes at once
  (`closeSessionWindow`, its closed sessions with it); else
  `ConfirmCloseWindow.svelte` over the whole window
  (`[data-confirm-close-window]`, `[data-close-window=cancel|close|dock]`,
  the live sessions named, Dock back focused, Cancel on production).
  Dock back (`closeByDocking`) first drops the window's own questions
  (snippet variables, group paste, join), then `dockEvery(true)`; a
  session still refusing (connecting, a login, host-key or paste
  question, a paste being sent) stays, and the error note names each
  with its reason. Close them is `closeSessionWindow`. Ctrl+K opens
  `CommandPalette` there (`openPalette` reads `currentWorkspace()`,
  `workspace.ts`, into `inventory`, which a session window is not told on
  a switch; `workspace-changed` keeps it fresh meanwhile, so a host file
  just written is found and connected; `reveals={false}`: Shift+Enter
  connects; no actions).
  `connectHost` decides as App's `prompt` does, through
  `hostConnection(host, login)` (`src/lib/hostConnect.ts`, shared with
  App, with `addressBesides`): telnet, serial and a profile open a tab here (`openTab` in the active pane, source `target`, a record
  from the inventory, read back from the app with `sessionRecord` on its
  first state, `fetchRecordOnce`), any other asks the main window
  (`connectFromMain`). A snippet (`>`) goes to the active tab while it
  is connected (`sendSnippet`). Smoke: `sessionWindows` (session-3's Close
  asks, Dock back), `closingSessionWindows` (after
  `safetyInSessionWindows`: lab-r9 moved, Ctrl+K there connects the
  telnet host win-telnet into that window, `>window check` runs in
  lab-r9 there, Close asks naming both, Cancel keeps them, Dock back
  returns both connected, win-telnet moved again and Close them ends it,
  `session_windows_live` zero), `workspaces` (ws-r2 in a session window:
  the keep question says "(1 in session windows)", kept connected there,
  then closed with its window by the close switch),
  `quittingWithLiveSessions` (end of the first run: win-telnet from the
  main palette, moved out; main's Close asks naming the session window,
  Cancel keeps every window, Close all ends the app), `closingTheMainWindow`
  (no live session: closes at once, as before). Not covered by the smoke:
  a session that refuses to dock (named in the note), the production
  focus on Cancel, a host whose credentials must be typed from a session
  window's palette (opens in the main window). Known limits: that host's
  session opens in the main window, as the password login after an agent
  failure does (#86); the quit question counts what the relays last
  reported, so a session whose state changes between the count and the
  answer is not asked about again.
- Closing with jobs running (#124). Closing the main window cancels every
  running job and stops a Python setup (the shell's `RunEvent::Exit`
  hook, `jobs.md` "Ending with the app"), so App's `onCloseRequested`
  counts them too: `runningCount() + runningCount(true)` (the shell's live
  jobs of every workspace, not finished); with sessions, transfers and
  jobs all 0 it closes at once, else `ConfirmQuit` with `jobs`
  (`[data-quit-jobs=n]`): "1 job is still running" / "N jobs are still
  running" joins the other clauses with "; ", and the sentence after it
  says what closing does: "Closing ends the session(s), cancels the
  transfer(s) and the job(s) and closes every window." (wording for
  sessions and transfers unchanged; jobs only: "1 job is still running.
  Closing cancels the job and closes every window."). Cancel keeps the
  focus, as before. A Python setup alone does not ask (its view tab
  shows it; closing that tab already cancels it). The run tab shows a
  cut run's `[data-run-cancel-reason]`. Smoke: `quittingWithLiveSessions`
  and `jobCutByClosing` (`jobs.md`). Not asked: closing a session
  window (its jobs are the app's, not its own) or switching workspace
  (jobs go on across a switch).
- The sweep of #81 (#88): each user story, where it is covered.
  1-3, 7-9, 19, 22, 36: `sessionWindows` (smoke, #84). 4-5: core
  `tests/session_handover.rs` (SSH and telnet floods, every byte once and
  in order) and the smoke's move during `lines 9000`. 6: built (+40, +40
  from the calling window, fitted to its monitor's work area); checked on
  two monitors, see "Checked on 2026-11-15" below; monitors with
  different scaling checked for #90, same place. 10-11,
  13-18, 37, 39: `severalInAWindow` (#85). 12: `document.title` only, the
  taskbar not checked. 20: `closingSessionWindows` (#88; limit: a host
  needing typed credentials opens in the main window). 21, 23, 25, 28-29:
  `safetyInSessionWindows` (#86). 24: core tests (`tests/session_relays.rs`,
  `tests/login_backup.rs`), not the smoke. 26: host key by the smoke, the
  login question not. 27: core (`tests/session_handover.rs`, logging in
  one file), not the smoke. 30-35: `sessionWindows` (#87; a group snippet
  across windows not by the smoke). 38, 40, 41: #88 smoke above. 42: a
  destroyed window hands its sessions to main (`hand_to_main`, core
  tests); a page that crashes while its window stays open: #89 below
  (smoke `crashedSessionWindow`, the Task Manager and the slow-window
  checks by hand). Every story is covered or a recorded limit. 43: core
  (a flood handed over mid-stream, flow control pausing a consumer that
  does not acknowledge) and the smoke's move during `lines 9000`. 44:
  twenty sessions over three windows with one flooding checked, see
  "Checked on 2026-11-15" below. 45: telnet moved and docked in the
  app (`closingSessionWindows`), serial by core tests only. 46: the
  design (sessions in `SessionManager` and `SessionRelays`; windows hold
  only layouts and relays' consumers).
- Checked on 2026-11-15 (#88's two checks "by hand"), on the built app
  driven over DevTools as the smoke drives it, windows placed with
  Win32 `MoveWindow`; a throwaway script, not in the repo. Machine:
  Windows 11, a 3440x1440 monitor (primary) and a 1920x1080 one to its
  right, both at 100% scaling. The test SSH server, quick connections,
  no workspace open.
  - Twenty sessions over three windows, one flooding (story 44): 20 SSH
    sessions, 7 in the main window and 6 in a session window on the first
    monitor, 7 in a session window on the second. `flood` in one, shown,
    then behind another tab of its window, for about a minute, then moved
    to the other session window mid-flood and stopped with Ctrl+C. A
    typed key was drawn in 3 to 5 ms (median of 15, worst 7 ms) in every
    window, the same idle, during the flood, beside it in its own window
    and after the move; no key lost. Every window kept 240 frames a
    second (longest gap 8 ms). Preferences opened in the main window in
    about 60 ms during the flood. The app's own process stayed at 39 to
    42 MB (flow control holds: nothing piles up in the shell); the
    flooding page's heap rose to between 85 and 160 MB and fell back to
    about 40 MB. After the move the flood went on in the new window; no
    torn line in the 10,056 kept; the window it left kept the focus in
    its terminal. All 20 sessions answered a command at the end.
  - Two monitors (story 6): with the main window on the second monitor,
    Open in window opened the session window on that monitor, 40 px
    right and down, with the terminal's columns and rows unchanged (after
    the `sessionWindowSize` fix above). With the main window in that
    monitor's bottom-right corner, the new window was fitted inside its
    work area. Maximize filled that monitor's work area. A session window
    carried to the first monitor, across the boundary and back kept its
    terminal size and typing.
  - Monitors with different scaling (#90), checked later the same day
    on the same machine with the second monitor at 150% (the primary at
    100%), by a throwaway script (the built app over DevTools, the main
    window moved and sized with Win32 `SetWindowPos`, the session
    window's rect, DPI and monitor read with `GetWindowRect`,
    `GetClientRect`, DWM's frame bounds and `MonitorFromWindow`; docked
    back with Close, Dock back, between tries). The cause was
    reproduced first with the old `build` (a logical position, the main
    window's physical position divided by its scale, which tao 0.37.2
    resolves by trying each monitor's scale in turn): from a main window
    at x 3600 on the 150% monitor, the session window opened on the
    primary at x 2440, and likewise from x 4200 and 5300. With #90: from
    the 150% monitor it opened on it, 60 px (40 logical) right and down;
    from its bottom-right corner its inner area ended at the work area's
    right and bottom edges (only the 1-2 px visible border beyond); from
    the primary, 40 px right and down, and fitted the same in its
    corner. The terminal kept 79x25 every time; the new window was the
    foreground window and its page had the focus. Fitting also changed
    on one scale: before, the invisible left border was counted inside
    the work area, so the visible window ran 7 px (100%) or 13 px (150%)
    past the right edge (that border, plus the 1-2 px visible one). Two
    monitors at the same scale after #90: checked that evening with both
    back at 100%, by #88's script on the final build (#89 and #90 in).
    From the main window on the second monitor the session window opened
    on it, 40 px right and down, the terminal 129x43 before and after;
    from that monitor's bottom-right corner it was fitted (its outer
    rect 2 px lower and 3 px shorter than before #90: the invisible
    border is now left out); maximize and carrying across the boundary
    as before; the twenty-session flood numbers as in #88's check (keys
    drawn in 3 to 5 ms, none lost, 240 frames a second, the app's
    process at 39 to 42 MB, no torn line, 20 of 20 answering). Not
    checked: a monitor left of or above the primary (the core
    tests cover a work area not at 0,0); whether a window flashes before
    it is placed was read from the code (built hidden, shown last), not
    filmed. Also not checked: the taskbar's titles. (A window killed
    from the Task Manager: checked for #89, below.)
- A session window whose page dies (#89). Core:
  `layout::take_over_dead_window(windows, relays, window)`
  (`src/layout/dead.rs`): `Windows::hand_to_main` and `SessionRelays::
  let_go` for each session taken, so the dead consumer's unacknowledged
  output counts as acknowledged and flow control lets the device go on;
  the main window, or one gone, is refused. Tests:
  `tests/session_relays.rs` (a flooding session whose page stopped
  acknowledging, paused, handed to main with no snapshot and flowing
  again there; refusals). Shell (`session_windows.rs`, the platform
  code in `page_watch.rs`): `build` hooks `watch_page` on every new
  session window, through `with_webview`. Windows: WebView2's `ProcessFailed` (`webview2-com`, the version
  Tauri's wry uses), acting only on
  `COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED` (the page's
  own renderer); an unresponsive renderer may recover, and a frame's, the
  GPU's or a utility process's exit leave the page alive, so they are
  ignored. Linux: WebKitGTK's `web-process-terminated` (`webkit2gtk`, the
  version Tauri uses), any reason. Elsewhere nothing is watched. Detection
  is the engine's report, not a heartbeat, so a page that is only slow
  (a minimised window's throttled timers, a flood being drawn) is never
  taken for dead. On it, `page_died` (spawned, not run inside the
  webview's own event): `hand_over` (shared with `destroyed`: the core's
  `take_over_dead_window`, `announce`, `session-arrived` to main per
  session with the snapshot its page never took, or null), then the
  window is destroyed, always: input is not tied to the window showing a
  session, so a window the shell took sessions from must not stay
  showing them. Its `Destroyed` then finds it gone and does nothing. The
  interface is unchanged: `onSessionArrived` with a null snapshot writes
  the "earlier output was lost" line (#84). Smoke `crashedSessionWindow`
  (after `closingSessionWindows`): lab-r9 moved out and `Page.crash`
  there twice. Quiet (`later 0 before-crash` shown there first): the
  window closes, lab-r9 is in the main window, connected, with the lost
  line and without `before-crash`, takes typing, no session window is
  left and `session_windows_live` is zero. Flooding: the flood goes on in
  the main window and Ctrl+C's prompt comes after it. (The lost line is
  checked only the first time: a flood scrolls it out of the
  scrollback.) Linux: the shell is built and passes clippy in WSL
  (`cargo clippy -p enodix`), not run on a desktop (#58). Known limits:
  the main window's own page dying is not watched (its sessions stay
  held by it until the app is closed, as before); a page that hangs
  without exiting and the browser process exiting: #120 below; WebView2 may share one
  renderer between pages of one origin (here each session window got its
  own, seen in the Task Manager check), and a renderer shared with the
  main window would take main down too; macOS is not a target.
- Checked on 2026-11-15 (#89's checks "by hand"), on the built app driven
  over DevTools (a throwaway script, not in the repo), the test SSH
  server, quick connections, no workspace open; Windows 11, 100% scaling.
  - A renderer ended from outside, as the Task Manager's End task does
    (`taskkill /F` on the session window's own `msedgewebview2.exe
    --type=renderer`, found as the one that appeared when the window
    opened), while the session flooded there: the window closed and the
    session was in the main window, connected, within a second; Ctrl+C
    there gave the prompt back (the flood had been flowing).
  - A session window flooding for 120 s, shown: never taken for dead;
    Ctrl+C and typing worked there afterwards.
  - A session window minimised (its own Minimize button) for 10 minutes:
    never taken for dead; still listed by `session_windows_list`;
    typing while minimised reached its device and was drawn there. Its
    page's `document.visibilityState` stayed `visible` while minimised
    (WebView2 here did not hide it), so this did not see throttled
    timers; the detection uses none, so throttling cannot affect it.
- A session window whose page hangs, and the engine ending (#120).
  Reproduced first (built app, a throwaway DevTools script on a desktop
  of its own, 2026-11-21): a session window's page stopped with an
  endless loop (`setTimeout(() => { for (;;) {} })` over DevTools), a key
  and a click sent to it over DevTools: for 48 s WebView2 raised no
  `ProcessFailed` of any kind (logged for each kind), the session stayed
  in that window (`session_windows_list`, `session_windows_live` 1) and
  nothing offered it back: the cause is that nothing in the app could
  tell a hung page, the engine reporting only a dead one and a channel
  into it saying `Ok`. So the shell asks (core
  `layout::PageAnswers`, `src/layout/answers.rs`): every
  `PAGE_ASK_EVERY` (2 s) `ask_pages` (spawned at setup) asks every
  session window (`answer-check`, emitted to it; the page's listener
  calls `session_window_answering`). "Hung" is decided there: the
  oldest unanswered question `PAGE_ANSWER_WAIT` (10 s) old, said once
  (`not_answering`) until an answer (`Answered::Again`). An event, not a
  page timer, so throttled timers (minimised, hidden) do not delay the
  answer; a flood being drawn answers between two pieces. Then
  `page_not_answering` emits `session-window-not-answering { window,
  names }` to main; an answer after it, `session-window-answering`. Main
  (`App.svelte`) shows a notice per window still open
  (`.job-notice.hung[data-not-answering=<window>]`, role alert, above a
  job notice) "The window of <names> is not answering…", buttons
  `[data-not-answering-action=take-back]` "Bring them here" (command
  `session_window_take_back { target }`, main only: `page_died`, i.e.
  `take_over_dead_window`, `session-arrived` with no snapshot, the window
  destroyed) and `=wait` (hides it until said again). Nothing is taken
  without the user: a page may recover. Core tests
  `tests/page_answers.rs` (answering never said; busy under the wait not
  said; silent said once at the wait, again after answering; gone
  windows forgotten); the take-over itself is #89's
  (`tests/session_relays.rs`, a consumer that stopped acknowledging).
  Smoke `hungSessionWindow` (after `crashedSessionWindow`): lab-r9 moved
  out; a page busy 2 s is not said (14 s watched); a page stopped 27 s is
  said with lab-r9's name, and once it runs the notice goes and it takes
  typing; a page stopped for good, Bring them here: the window closes,
  lab-r9 in main connected with the lost line, `session_windows_live`
  0, typing works. No call is evaluated in a hung page there (it would
  wait 60 s). The engine (WebView2's browser process) ending: reproduced
  by killing it (`taskkill /F`): only `BROWSER_PROCESS_EXITED` (kind 0)
  was raised, the app stayed running with no page for 24 s and more, no
  window could close (a close waits for its page's handler), so the app
  could only be killed; the session log had every completed line. Now
  `page_watch` reports it (`Page::EngineGone`), watched on the main
  window too (`watch_main`, set up with it) and every session window,
  and `engine_gone` ends the app (`app.exit(0)`, so `end_with_the_app`
  runs; the same kill then ended the app within 0.3 s, exit code 0, the
  log as before). Logs: unchanged, every completed line is written as it comes;
  the unfinished last line is not, as on any app exit. Not handled: the
  main window's own renderer exiting alone (as #89); a hung main window
  (nothing to hand its sessions to); Linux has no hung-page question
  path difference (the questions are the shell's own, so they work
  there too, not run on a desktop) but no engine-gone report.
- The Files tab (#103; core and shell in `files.md`). View `files`,
  subject the host id (`VIEWS.files`: "Files <name>", `FolderOpen`), one
  per host (`openView`, so asking again shows it), main window only (the
  core refuses views in session windows; `files_open` refuses other
  windows). Its tab gets the host's environment in `handlers.tab` (colour
  and PROD mark as a session tab). Opened by `openFiles(id)`
  (`inventory.svelte.ts`) from the details sheet's `[data-open-files]` and
  the context menu's `[data-menu=files]` (both only once the host's
  resolved protocol is ssh), and `TerminalSession`'s `onfiles` action
  `[data-session-action=files]` (only for an SSH transport; App passes it
  for a host of the open workspace; `SessionWindow` asks main with `{kind:
  files}`). A workspace switch closes Files tabs; a host renamed or moved
  retargets its tab (`retargetView`, as for configs) and its connection
  stays. `FilesTab.svelte` (`[data-view=files][data-host][data-files-state=
  connecting|credentials|connected|transfer-only|failed|lost]`): on mount
  `hostLogin(host, "ssh")`; a profile opens at once, otherwise
  `LoginQuestions` asks username and password (`[data-files-login]`), the
  login kept for Reconnect once it got in. `HostKeyCheck` and
  `LoginQuestions` lie over the tab (`.files` is `position: relative`),
  ready once the id is back; events of an older browser are dropped (a
  generation count). Markers: `[data-files-reason]`, `[data-files-lost]`,
  `[data-files-failed]`, `[data-files-action=up|
  refresh|new-folder|reconnect]`, input `files-path` in the bar's form,
  `[data-files-folder]`, rows `[data-files-entry=<name>][data-kind]` with
  `[data-files-open]` (folders and links), `[data-files-entry-action=
  rename|delete]`, `[data-files-name-form=rename|new-folder]` (input
  `files-name`), `[data-files-confirm]` with `[data-files-confirm-action=
  cancel|confirm]` (production only, Cancel focused), `[data-files-error]`,
  `[data-files-notice]`, `[data-files-production]`. Destroying the
  component calls `files_close_host` for each host id it opened with
  (#106: its browsers close once a cancelled upload removed its partial
  file; `files_close` is for a Reconnect's old browser).
  Transfers (#106; core and shell in `files.md`): the bar's
  `[data-files-action=upload]` (the dialog plugin's `open({ multiple })`)
  and `[data-files-action=download]` (enabled with files ticked:
  `[data-files-select=<name>]` checkboxes, rows `[data-selected]`; a
  folder dialog with `defaultPath` the last download folder, kept in the
  tab). Drops: `getCurrentWebview().onDragDropEvent` in each tab; a drop
  counts only where `elementFromPoint` (physical position over
  `devicePixelRatio`) is inside that tab with no question over it
  (`[data-files-dragging]` while over it); connected it uploads, in the
  SCP form it fills the send's file. Questions over the tab:
  `[data-files-transfer-confirm=upload|download][data-files-replace=n]`
  (`[data-environment=production]` and `[data-files-transfer-production]`
  for an upload or send on production, Cancel focused; else the confirm
  button), names `[data-files-replace-name]`, buttons
  `[data-files-transfer-action=cancel|confirm]` ("Replace" / "Upload" /
  "Send"). The list `[data-files-transfers]`, rows `[data-transfer=<id>]
  [data-transfer-name][data-direction=upload|download][data-transfer-state=
  waiting|connecting|running|done|failed|cancelled]` with
  `[data-transfer-progress]`, `[data-transfer-speed]` (smoothed, every
  400 ms at most; a 1 s ticker lets a stalled one fall), `[data-transfer-
  action=cancel]` ("Remove" while waiting: the row goes once the core
  has it cancelled), `[data-transfer-outcome]`,
  `[data-transfer-hash=md5|sha256]` (a `code` and a `CopyButton`),
  `[data-transfer-reason]`, `[data-transfer-hint]`,
  `[data-transfer-partial]`. A transfer that ends lists the folder again
  (path box and error kept). States arriving before their `Added` are
  kept and applied. The SCP form (`[data-files-transfer-only]`,
  `[data-files-hint]`, forms `[data-files-send]` (inputs
  `files-send-local`, `files-send-remote`, filled with the platform's
  folder and the file's name until typed over; `[data-files-action=
  choose-send|send]`) and `[data-files-fetch]` (`files-fetch-remote`,
  `files-fetch-local`; `[data-files-action=choose-fetch|fetch]`),
  `[data-files-replaced]`), and `[data-files-status]` saying no
  connection is held. In the SCP form each transfer's own login asks
  over the tab (#109): its `hostKey` / `prompt` events come after
  `transferOnly` and are drawn by the same `HostKeyCheck` /
  `LoginQuestions` (they lie over the tab in any phase), answered with
  `files_answer` / `files_decide_host_key` / `files_cancel` on the same
  browser id; the row shows Connecting… meanwhile; `questionClosed` (the
  transfer cancelled or its login given up unanswered) takes the dialog
  off. The smoke step (`fileTransfers`, a typed `engineer-otp` login on
  the `--scp-only` device: the code cancelled for one send, answered for
  the next) passes; it waits for the row to read Connecting, which can lag
  the question by the row's 200 ms. Not run in the app: a key question per
  transfer (core test only). `fileTransfers.svelte.ts` keeps each host's count
  of running and waiting transfers (`setActiveTransfers`, from an effect
  that clears its host on cleanup); App asks before closing such a tab
  (`closingFiles`, `ConfirmClose` with `transfers`:
  `files`: `[data-confirm-transfers=n]`, Cancel focused, "Close tab"; it
  stays if they all end meanwhile, saying so, since closing clears the
  kept outcomes), `ConfirmQuit` takes `transfers`
  (`[data-quit-transfers]`, the main window asks when sessions or
  transfers are live; jobs too since #124, "Closing with jobs running")
  and `ConfirmSwitch` takes `transfers` (Files tabs
  close with the workspace: "Leave <name>?" with Cancel and Switch when
  no session is live; Cancel focused).
  Smoke `filesTab` (after `savedJobs`; the server runs with `--sftp`, the
  smoke's `sftp` folder): its own profile, platform (`file_folder: /flash`)
  and hosts in `Files/` plus `files-typed.yaml`; known_hosts emptied (put
  back after): the typed login cancelled with the mouse, typed by
  keyboard, the unknown key cancelled with the mouse and trusted with
  Enter; the listing's order and cells; the details and context menu
  giving one tab in `/flash` with no question; path box, folder click, Up,
  Refresh, a missing path; rename, onto an existing name, new folder,
  delete; the production tab's mark and the three questions (Cancel
  focused, cancel and confirm); a session's menu and tab menu; `cut sftp`
  in that terminal giving both tabs' reason, Reconnect; `show sftp`
  counting 2, 1, 0 as tabs close, the terminal connected; from a session
  window; telnet, serial and quick connection not offered. Not covered: a
  device without SFTP in the app (core test only), a changed host key and
  a keyboard-interactive question in the tab (same components and core
  path as the first-use key), Linux.
- WebView2 kept off the network (#115, #135). Every window (`main`,
  `open_detached`, `session_windows.rs`) is built with
  `.additional_browser_args(WEBVIEW_ARGUMENTS)` (`src-tauri/src/lib.rs`:
  wry's own defaults, which any arguments replace, plus
  `--disable-background-networking` (Edge's configuration from
  `config.edge.skype.com`), `--no-proxy-server` (the `wpad` lookups),
  `--disable-component-update` (`edge.microsoft.com` a minute in);
  every window must have the same set, as WebView2 refuses another set
  for the same data folder) and
  `.general_autofill_enabled(false)` (Edge's autofill).
  `src/lib/writingSuggestions.ts` (`noWritingSuggestions()`, called first
  in `main.ts`) marks every `input`, `textarea` and `[contenteditable]`
  `writingsuggestions="false"`, those added later too (a
  MutationObserver: xterm.js makes its own textarea): Edge's text
  predictions send what is typed. (#115 thought these two stopped Office
  connections while typing; #135 found those were the sign-in below,
  which typing does not cause, so what autofill and text predictions
  would connect to on their own was not measured. They stay off.)
  **The Office connections (#135).** WebView2 157.0.4341.60's browser
  process (no `--type`) now and then opens 2 or 3 parallel TLS
  connections to 40.99.x / 52.97.x / 52.98.x port 443, which are the
  addresses of `substrate.office.com` / `outlook.office.com`
  (`shed.outlook.acdc.tm.svc.cloud.microsoft`, no reverse name); not in
  its net log. A reproduction (`enodix.exe` with a temporary
  `ENODIX_DATA_DIR` and WebView2 data folder per start, DevTools on
  9411, the sampler of `watch-connections.ps1`, 30 to 45 s per start)
  gave, in the first hour, 7 of 8 starts typing into the quick-connect
  host field and 7 of 8 starts idle: typing is not the trigger; the time
  varied from 3 to 36 s after start. The browser process had
  `oneauth.dll`, `microsoftaccountwamextension.dll`,
  `windows.security.authentication.web.core.dll` and `wininet.dll`
  loaded in the runs with the connection (`oneauth.dll` in 5 of 5
  checked), and no `oneauth.dll` in 39 runs without. `msedge.dll` holds
  `https://substrate.office.com/profileb2/v2.0/me/Profile` and
  `.../me/image/$value` (`edge_substrate_account_info_helper.cc`), and
  within a second of each of the 5 connections watched for it OneAuth
  rewrote
  `%LOCALAPPDATA%\Microsoft\OneAuth\blobs\<account>_substrate_profile_metadata`
  and `_avatar_metadata` (only times were read, not contents), and after
  none of 52 starts without one: it is OneAuth refreshing the Windows
  Microsoft account's profile and picture. That cache is shared per
  Windows user (Office, Edge, Teams use it), which makes the rate drift:
  14 of 16 control starts in the first hour, 4 of 34 after; refreshes
  came at 16:48, 17:29, 17:30, 17:48, 19:24. So only back-to-back
  comparisons count (switch first, control right after, while stale).
  Counts per variant (`--disable-features` replacing wry's three with
  them plus these; starts with a hit / starts; batches run at different
  times, so not comparable with each other): `msOneAuthWAM` 1/6;
  OneAuth's four (`msOneAuthWAM`, `msForceOneAuthWAM`, `msOneAuthIDHRD`,
  `msLoadOneAuthInBackground`) 0/8 and 0/4 interleaved (controls 0/4);
  implicit sign-in's five (`msImplicitSignin`,
  `msImplicitSignInNetworkRetry`, `msM365LinksImplicitSignin`,
  `msSeamlessWebToBrowserSignIn`, `msEnableWebToBrowserSignIn`) 0/8, then
  1/1 in a stale window (a build with them, 17:29; the control after it
  1/1 too): not enough alone; both groups (the candidate) 0/8, 0/5 with
  19 more account features, 0/13 interleaved with controls 0/13, and in
  the two stale back-to-back triples (17:43-17:49 and 19:23-19:29, the
  second while the owner was using the computer) 0/4 with the controls
  2/4 (each hit right after a candidate start); Edge's OS-account-info
  features (`msEdgeOSAccountInfoSubstrate`,
  `msEdgeOSAccountInfoManagerCache`, `msPrimaryOSAccountInfoCache` and
  six more) 2/8; InPrivate (`.incognito(true)`, a temporary build) 1/8;
  a private `LOCALAPPDATA` (OneAuth ignores it) 0/4; a copy of the
  runtime with `oneauth.dll` deleted (`WEBVIEW2_BROWSER_EXECUTABLE_FOLDER`,
  to test the mechanism, not to ship) 0/4 in the same triples. With the
  candidate, OneAuth was never loaded (and, oddly,
  `windows.security.authentication.web.core.dll` was, in 9 of 9
  checked, where control starts without a connection had it in none:
  the candidate may move Edge to another sign-in path, whose traffic
  would go through Windows' broker process, which the sampler does not
  watch). Not tried: WebView2 policies (none covers sign-in), a local
  Windows account, the candidate in the smoke. The app does not carry
  the candidate yet; next is more stale back-to-back triples (each
  stale window gives about one; they came 20 to 95 minutes apart), then
  ship it, run the smoke, and drop `knownOfficeConnection`. Upstream:
  WebView2Feedback 5760 (157.0.4341.42, at every start; `msOneAuthWAM`
  stopped it on 155, not on 157). The environment
  variable `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` (the smoke's DevTools
  port) adds to these; a second `--disable-features` there replaces the
  app's (seen on the browser's command line). Not verified: Linux's
  WebKitGTK (the arguments are Windows-only), a WebView2 later than
  157.0.4341.60, runs longer than the smoke.
