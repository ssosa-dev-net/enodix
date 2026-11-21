# The interface

How the window is laid out since the redesign (spec #61, tickets #62 to
#79), and where each feature of 0.2.0 now lives. Behaviour did not change:
everything 0.2.0 could do, the app still does, reached differently.

## The window

- **Frame**: the app draws its own (no system title bar). The top of the
  window is the app's own bar: the rail and the side view's heading on the
  left, the tab bars across the rest. Dragging an empty part of it moves
  the window (and snaps it to screen edges), a double click maximizes or
  restores it; a tab, button or field never drags. Minimize, Maximize or
  Restore, and Close sit at the right end of the top-right pane's bar;
  Close does what closing the window always did. The window resizes from
  its edges as before. Run and config history windows have the same bar,
  with their title. Windows 11's snap layouts menu on hovering Maximize is
  not offered (a drawn button cannot have it); Win+Z and Win+arrow keys
  still snap.
- **Activity rail** (left edge): icons for the side views Hosts, Snippets,
  Jobs, Config backups and Credentials, with Preferences (the gear) at the
  foot. Clicking the open view's icon folds the side view away; any icon
  while folded opens its view. The Hosts icon is marked while workspace
  files have problems and the view is not shown; the Jobs icon counts
  running jobs; Credentials is marked while profiles are missing.
- **Side view**: what the rail selected, resizable by its edge
  (double-click resets it). The side view, its width and whether it is
  folded are remembered.
- **Details sheet**: a host's or folder's details, opened when asked for
  (the row's details control, the tree's context menu, Ctrl+Shift+D, or
  Details in a session's menu), over the lower part of the Hosts view. It
  follows the selection and goes back to the tree with "Back to the tree"
  or Escape. It shows one item only: it closes when several hosts are
  selected.
- **Several hosts** (#110): Ctrl+click in the Hosts view's tree adds a
  host to the selection or takes it out; Shift+click selects the hosts
  between the last one clicked and this one, as the tree shows them (open
  folders only, and under a filter only the hosts shown). The foot of the
  Hosts view says how many are selected, with a button to clear them;
  Escape in the tree clears them too. The context menu on one of them
  offers only the jobs (Run snippet…, Run script…, Send file…, Back up
  configs, Fill in from devices), each on exactly those hosts. A plain
  click, a drag, a right-click on another row or another workspace ends
  the selection; a host removed from the workspace leaves it. Folders are
  not part of it: Ctrl or Shift on a folder is a plain click.
- **Workspace folder out of reach** (#132): when the workspace folder
  goes away (a network drive that disconnects, a folder moved or
  removed), a notice at the top of the Hosts view says it cannot be
  reached; the hosts loaded stay shown and nothing can be saved. The app
  looks for the folder every few seconds and loads it again by itself
  once it is back. A folder that comes back as another workspace (its
  `_workspace.yaml` names another one) is not taken for the one that was
  open: the notice says so, and Workspaces… opens it as what it is now.
- **Terminal area**: tabs and split panes. A tab holds a session or a view.
  Each session has one header line (role, name, address, jump host,
  protocol, markers, status) and a menu with its actions; the same menu is
  the tab's context menu. An empty pane offers the palette and quick
  connect. In a narrow pane at the window's top-right, the pane's
  buttons (new connection, multi-send, split right and down, close pane)
  fold into a "…" button that lists them.
- **View tabs**: job (pre-flight, running, result), past run, config
  history (one per host), Preferences, CSV import, Nornir import, platform
  definitions, highlight rules, fill-in review, Python setup and Files
  (one per host). They have an icon and no connection dot, move, split
  and close like session tabs, and never join multi-send. Several job
  tabs can wait at pre-flight at once: each one's Start runs the hosts
  its own pre-flight lists, after its own production question (#121). A
  pre-flight that is no longer current (another workspace was opened, or
  many jobs were planned since) refuses to start and offers Plan again.
- **Send a file to many hosts** (#107): "Send file…" beside Run snippet
  and Run script (a folder's or host's details sheet, the tree's context
  menu, a saved filter's actions) opens a job tab: the file (Browse… or a
  typed path), a destination folder (empty: each host's platform file
  folder; host variables such as `{{site}}` allowed) and Overwrite. Its
  pre-flight shows the file's name and size and where it goes on each
  host; three hosts send at once unless changed. While it runs each host
  shows Waiting, Sending with its progress, Sent, Already there or Failed
  with the reason. Its run tab shows the file's name, size, MD5 and
  SHA-256, the destination and each host's bytes; it cannot be saved as a
  job (the file is on this computer). [files.md](files.md) is the page
  for it and for the Files tab: what each platform needs, and the limits.
- **Files tab** (#103): an SSH host's files over SFTP, titled "Files" and
  the host's name, in the main window only (not in a window of its own).
  Opened from the host's details sheet, the tree's context menu, and a
  session's menu or its tab's menu; from a session window it opens in the
  main window, which comes forward. Not offered for telnet or serial hosts
  or quick connections; asking again shows the open tab. It connects when
  it opens, with the host's stored profile; a login to type, an unknown or
  changed host key and a login question are asked over the tab; Cancel
  leaves the tab saying why it is not connected, with "Connect again". It
  starts in the folder the host's platform puts files in, or else where
  the login lands. The listing (folders first, then by name, hidden files
  included) shows size, time and permissions; the path box takes an
  absolute path or one from the folder shown, a folder's name opens it,
  and there are Up, Refresh and New folder, with Rename and Delete (a file
  or an empty folder) on each row. On a production host each change asks
  first, Cancel focused, and the tab has the production colour and mark.
  A lost connection is said with its reason, with Reconnect. Closing the
  tab closes its connection.
  Transfers (#106): Upload opens a file dialog (several files), or files
  dropped from Explorer on the tab go up; either way into the folder
  shown, and the listing is refreshed after each transfer. Tick files and
  Download puts them in a folder you pick; the next download starts there.
  An upload asks first on a production host (Cancel focused) and when a
  file of that name is already there (one question listing them all); a
  download asks only before replacing a local file. Below the listing,
  the transfers: direction, name, progress and speed, waiting ones in
  order (Remove), running ones with Cancel, finished ones with their
  outcome (a cancelled upload says whether its partial file was removed),
  an upload's MD5 and SHA-256 with a copy button. Transfers go on while
  another tab is shown; closing the tab, the main window or switching
  workspace while one runs or waits asks first.
  A device without SFTP (IOS and IOS-XE; a Junos device whose SFTP server
  is off) is reached over SCP, which the app falls back to by itself. Its
  tab shows, in place of the listing, a form: Send (a
  local file, chosen or dropped, to a path that starts as the platform's
  file folder and gets the file's name) and Fetch (a path into a local
  folder).
  It says that a file of that name is replaced (the device cannot be
  asked), that no connection is held between transfers, and shows a
  device's refusal in its words with the app's hint under it. Each
  transfer logs in again, and what that login asks (a one-time code, a
  host key trusted once) is asked over the form while the transfer shows
  as connecting; Cancel fails that transfer alone.
- **Real windows**: a job run and a host's config history can open in a
  window of their own ("Open in window") and dock back. Closing the main
  window closes them; closing a run's window leaves its job running.
- **Closing the main window** closes every window, session windows
  included, and ends every session (#88). It also cancels every running
  job, as Cancel does (a script's processes and the tunnels it holds end
  with it), and stops a Python setup under way (#124); the run then says
  "Cancelled: the app was closed while the job ran." If a session is
  connecting or connected in any window, a Files transfer runs or waits,
  or a job runs, you are asked once first ("1 job is still running.
  Closing cancels the job and closes every window.", "Close all" or
  Cancel, which has the focus); otherwise it closes at once. A script's
  processes end with the app even when it crashes or is killed.
- **Session windows** (#84, #85): a session can move to a window of its
  own, live, with its scrollback and screen: "Open in new window" in its
  menu or its tab's context menu (or Ctrl+Shift+N), or "Open in the
  window of …" to add it as a tab to a session window already open
  (from the main window or from another session window). A session
  window has tabs and split panes like the main window (split right or
  down, close pane, drag tabs between panes; the tab shortcuts work on
  its own tabs), each session with its header and terminal, and the
  status line; no rail, side views or new-tab button. Its
  title names the session of its active tab. Duplicate there opens the
  new session beside the original, in that window. "Dock back" (its
  menu, Ctrl+Shift+N) returns one session to a tab of the main window;
  "Dock all back" (its menu, with more than one tab) returns them all.
  Closing a session window (its Close button, Alt+F4, the taskbar) with a
  session connecting or connected asks (#88): "Dock back" (the default)
  returns them all to the main window, still connected; "Close them" ends
  them with the window; Cancel keeps it open (and has the focus when one
  of them is a production host). A session that cannot move just then (a
  login, host-key or paste question open on it) stays, and the window
  says why. With no live session, closing it closes it and its closed
  sessions at once. A window whose last tab moves out, docks back or
  closes closes itself. Its palette (Ctrl+K) connects a host into that
  window (Shift+Enter connects too: there is no tree to show it in); a
  host whose credentials must be typed is connected by the main window,
  which comes forward to ask. A snippet found there with `>` goes to that
  window's active terminal. Switching workspace asks about the sessions
  in session windows as about those of the main window, and keeps or
  closes them alike. A move is not offered while the session is connecting,
  while a question is open on it, or while a paste is still being sent.
  The multi-send group spans windows (#87): a member stays in it when it
  moves; every member, in every window, shows the marker and the group's
  size; typing in one reaches the connected members in every window; you
  join, leave and end the group from a session window as from the main
  window (the production question asked there). A group paste or group
  snippet is asked once, over the pane pasted in, naming the members
  wherever they are; each then gets its lines through its own terminal,
  with its own line delay. A paste goes to the group as it is when you
  press Send (#123): if the group changes while you are asked (a tab
  joins or leaves, a member closes, the group ends, in any window), the
  question closes and nothing is pasted, and the window says so; paste
  again. A member that leaves the group, or closes, while a group paste's
  lines are going gets no more of them. A paste (or snippet) you began
  in a tab outside the group stays that tab's, even if the tab joins
  while it is asked about. A production session joins only through its
  question, whichever way you add it. If a session window goes away without
  docking back, or its page crashes while the window stays open (#89),
  the main window takes its sessions, still connected, saying their
  earlier output was lost, and the crashed window closes. A window that
  is only slow (minimised, or drawing a flood) is never taken for crashed.
  Checked on Windows; on Linux it is built but not yet tried on a desktop.
  Not handled: a page that hangs without crashing (its sessions stay
  stuck until the app is closed) and the web engine crashing as a whole,
  which takes the main window's page too. Everything that protects you comes along (#86): the
  production colour and PROD mark, the unencrypted, legacy-crypto,
  logging-failed and backup markers, paste confirmation (over the pane
  pasted in, with the host's line delay), host-key and login questions of
  a reconnect there, and the password login offered after the SSH agent
  failed (the main window then asks for the password). Snippets from the
  main window's Snippets view or palette go to the terminal you used last,
  whichever window it is in: the view names it, and its questions appear
  over that pane, in that window, which comes forward. Details on a moved
  session's header opens the details sheet in the main window.
- **Status line** (foot of the window): the vault's state and the active
  session's size.
- **Palette** (Ctrl+K): hosts, snippets (`>`), and every action that has
  no button of its own any more: each side view, fold the side view, new
  host or folder, both imports, switch workspace, Preferences, platform
  definitions, highlight rules, Python setup.
- **Questions**: those about one terminal (host key, login questions,
  paste confirmation, a snippet's variables, closing a live tab, joining
  multi-send) cover only that terminal's pane; the other panes stay usable.
  So do a Files tab's (its login, host key, names, production, upload
  and download questions, and closing it while transfers run).
  Those about the whole app stay window-modal: delete, rename, new host or
  folder, switching workspace (and keeping or closing its sessions),
  making a folder of other things a workspace, the
  workspace switcher, the vault's forms, the unlock and start-over screen,
  closing the main window, and closing a session window with live
  sessions (over that window).
- **First window**: with no workspace open, the active pane shows the
  welcome view (recent workspaces, open a folder, create a workspace,
  quick connect); side views that need a workspace say so. The
  workspace switcher (Ctrl+Shift+O) says the same: "Open an existing
  folder", never "add". A folder that is not there, a file chosen as a
  folder, and a create over a folder that has files each say what to
  do instead (#116). A folder that holds other things and no host file
  (a Documents folder chosen by mistake) is asked about first, from the
  welcome view, the switcher or a recent workspace whose folder changed:
  "Make this folder a workspace?" with its path, saying no host files are
  there and that opening writes one small file, `_workspace.yaml`, into
  it; Cancel (focused) writes nothing (#131). An empty folder, or one with
  host files, opens at once. With a workspace open, the empty pane says that a
  double-click on a host in the Hosts view connects it.
- **Imports and addresses** (#131): the CSV and Nornir imports refuse an
  address with a space in it, as New host does, and report the row with
  the others left out; a CSV address with its port (`10.0.0.1:2222`) is
  refused the same way, saying where the port goes, while a Nornir
  `hostname` with its port gives the host that port. "Import 0 hosts" is
  off until a new host is found or an existing one ticked.
- **Login banner** (#134): the notice a device sends before the login
  (its `banner login`, usually a legal warning) is the first thing in the
  terminal, as the device sent it, on every connection and reconnect,
  also when the login then fails, and it is in the session log. A jump
  host's comes first, under a grey line naming the jump host. When the
  login asks something (a password, a one-time code), the question's card
  sits at the bottom of the terminal, so the banner stays readable (and
  scrollable) above it. Jobs, config backups, the backup on login and the
  Files tab never show or keep it.
- **A connection that fails** (#116) says what happened and what to
  check next, in the bar under the terminal (or the SSH agent's card):
  the address for a name that does not resolve, the Port setting for a
  refused port, the address, the device and the Connect timeout for no
  answer, the username and password (and the Credentials view) for a
  refused login, the agent for an agent login; a jump host's failure
  says it is the jump host's. Fix it where the advice says, then press
  Enter in the terminal to reconnect.
- **Reconnecting** (Enter in a closed session, or the session menu's
  Reconnect; the same in a split pane and in a session window) connects
  a workspace host as it is now (#130): its address, port, protocol, jump
  host, timeouts and logging, and its credential profile as the
  Credentials view has it. A login typed for the session is used again,
  unless the host has a profile now that it did not have then;
  a host whose profile is gone or not set up asks for the username and
  password. A grey line under `--- reconnecting ---` says when the login
  is not the one the session had. A host deleted (or moved outside the
  app), with no address, or set to another protocol since is not tried:
  the bar says why. A quick connection reconnects as it was made, and so
  does a session kept from a workspace that is not open now (the grey
  line says so).
- **First hosts and logins** (#116): the New host form refuses an
  address with a port after it (`10.0.0.1:2222`) saying to set the
  Port in the host's details. The Credential profile setting says what
  it takes (a profile of the Credentials view, or a new name set up at
  the first connection), and the Jump host setting that a host is named
  by its id; a jump host named by its host name says which id to use.
  A login asked at each connection says how to keep it, the
  Credentials view how a host uses a profile, and the CSV and Nornir
  reports, after creating hosts, how to connect and give them a
  profile.

## Shortcuts

| Keys                                  | What                                |
| ------------------------------------- | ----------------------------------- |
| Ctrl+K                                | Palette (in a session window too)   |
| Ctrl+Shift+H / S / J / G / L          | Hosts / Snippets / Jobs / Config backups / Credentials (again: fold) |
| Ctrl+Shift+B                          | Fold or unfold the side view        |
| Ctrl+Shift+D                          | Open or close the details sheet     |
| Ctrl+,                                | Preferences                         |
| Ctrl+Shift+O                          | Workspace switcher                  |
| Ctrl+Tab / Ctrl+PageDown              | Next tab                            |
| Ctrl+Shift+Tab / Ctrl+PageUp          | Previous tab                        |
| Ctrl+Shift+PageUp / PageDown          | Move the tab                        |
| Ctrl+Shift+W                          | Close the tab                       |
| Ctrl+Shift+M                          | Join or leave multi-send            |
| Ctrl+Shift+N                          | Open the session in a window / Dock back |
| Ctrl+= / Ctrl+- / Ctrl+0, Ctrl+scroll | Terminal font size                  |
| Ctrl+Shift+F                          | Search the terminal's scrollback    |
| Ctrl+Shift+C, Ctrl+Insert             | Copy (Ctrl+C copies a selection)    |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert    | Paste                               |
| Enter in a closed session             | Reconnect                           |

## Where 0.2.0's features live

0.2.0 had a toolbar of text buttons over the tree, the tree with the
snippets and saved jobs under it, a permanent properties column on the
right, a block of buttons over each terminal, and dialogs for everything
else. Each of its features, and where it is now:

| Feature (0.2.0)                                  | Where it is now                                                                    |
| ------------------------------------------------ | ---------------------------------------------------------------------------------- |
| Host tree: folders, drag and drop, roles, reload | Hosts view                                                                         |
| Connect (double-click, Enter)                    | Hosts view; palette (Enter); details sheet's ways in                               |
| Connect by telnet or SSH instead                 | Details sheet's ways in; tree context menu                                         |
| Search hosts                                     | Palette; Hosts view's search icon                                                  |
| Saved filters (apply, save, run on them)         | Hosts view: filter field, chips, save, filter actions                              |
| New folder, new host                             | Hosts view's add control; tree context menu; palette                               |
| CSV import, Nornir import                        | Add control, folder context menu, folder details, palette; open as tabs            |
| Rename, move, delete                             | Tree context menu; drag and drop (window-modal confirmations)                      |
| What a rename or move left behind (#99)          | Notice at the top of the Hosts view                                                |
| Host and folder properties, inherited settings   | Details sheet (sections device, connection, tags, custom fields, description)      |
| Web links                                        | Details sheet's ways in                                                            |
| Open log folder                                  | Details sheet; tree context menu                                                   |
| Workspace problems                               | Foot of the Hosts view; rail marker while folded                                   |
| Workspace folder cannot be reached (new)         | Notice at the top of the Hosts view                                                |
| Workspaces (open, create, switch, forget)        | Welcome view; workspace name in the Hosts view; Ctrl+Shift+O; palette              |
| Role icons, environment colours                  | Tree rows, tabs, session header and edge                                           |
| Quick connect (SSH, serial)                      | Empty pane; welcome view                                                           |
| Tabs, split panes, drag tabs                     | Terminal area (unchanged)                                                          |
| Duplicate, disconnect, reconnect, send break     | Session menu (header and tab context menu); Enter reconnects                       |
| A session in a window of its own (new)           | Session menu, tab context menu, Ctrl+Shift+N; Dock back, Dock all back in its window |
| Session markers (production, unencrypted, ...)   | Session header line and tab                                                        |
| Host key and login questions                     | Over their terminal                                                                |
| Legacy crypto failure advice                     | Over the closed terminal                                                           |
| Session logs                                     | Unchanged; "Not logging" marker in the header                                      |
| Multi-send (join, leave, end, join a pane)       | Session menu, tab bar's join control, Ctrl+Shift+M (any window); questions over the pane |
| Paste safety                                     | Paste confirmation over its pane                                                   |
| Snippets (send, new, edit, delete, filter)       | Snippets view (the form opens in the view); palette `>`                            |
| Snippet variables                                | Asked over the target terminal's pane (in its session window, if it is in one)     |
| Keyword highlighting rules                       | Highlight rules tab (from Preferences or the palette)                              |
| Credential profiles, named secrets               | Credentials view                                                                   |
| Vault (turn on/off, change password)             | Preferences tab; Credentials view links to it; status line shows its state         |
| Unlock and start over                            | Unlock screen at start                                                             |
| Preferences (theme, font, paste, updates, ...)   | Preferences tab (gear on the rail, Ctrl+, , palette)                               |
| Update check and install                         | Notice in the corner; Preferences tab                                              |
| Platform definitions                             | Platforms tab (Hosts view header, details sheet, palette)                          |
| Run snippet, run script on folder/filter/host    | Tree context menu, details sheet, filter actions; job tab                          |
| A job on several selected hosts (new, #110)      | Ctrl+click or Shift+click in the tree, then its context menu; job tab              |
| Send a file to many hosts (new, #107)            | "Send file…" in the tree context menu, details sheet, filter actions; job tab      |
| Back up configs                                  | Tree context menu, details sheet, filter actions, Config backups view; job tab     |
| Fill in from devices and its review              | Tree context menu, details sheet; job tab, then the review tab                     |
| Past runs (search, compare, save, delete, again) | Jobs view, run tab (can open in its own window)                                    |
| Saved jobs                                       | Jobs view; "Save as job" in the job and run tabs                                   |
| Config history and diff                          | Config backups view, details sheet, context menu; history tab (or its own window) |
| A host's files, upload, download (new, #103/106) | Details sheet, tree context menu, session menu, tab context menu; Files tab        |
| Back up on login                                 | Setting in the details sheet; marker in the session header                         |
| Python setup                                     | Preferences tab, then the Python setup tab; palette                                |
| Terminal search, copy, paste, zoom               | Terminal (unchanged)                                                               |

Where the app keeps logins and files, what is encrypted, and every
connection it can make are in [trust.md](trust.md).

Nothing of 0.2.0 is out of reach. Preferences and workspace files carry
over unchanged: an old `preferences.yaml` loads with every choice kept (its
right panel's width and folding are no longer used), and opening a
workspace writes nothing to its files beyond the identity file every
release since 0.1.0 writes.

## Contrast

Text meets WCAG AA (4.5:1, 3:1 for large text) in both themes. The smoke
test checks this on every screen it photographs and on every side view,
view tab and window-modal dialog, in the theme shown and in the other one;
terminals (whose colours are the device's) and disabled controls are not
judged.
