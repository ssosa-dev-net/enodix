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
  or Escape.
- **Terminal area**: tabs and split panes. A tab holds a session or a view.
  Each session has one header line (role, name, address, jump host,
  protocol, markers, status) and a menu with its actions; the same menu is
  the tab's context menu. An empty pane offers the palette and quick
  connect.
- **View tabs**: job (pre-flight, running, result), past run, config
  history (one per host), Preferences, CSV import, Nornir import, platform
  definitions, highlight rules, fill-in review and Python setup. They have
  an icon and no connection dot, move, split and close like session tabs,
  and never join multi-send.
- **Real windows**: a job run and a host's config history can open in a
  window of their own ("Open in window") and dock back. Closing the main
  window closes them; closing a run's window leaves its job running.
- **Status line** (foot of the window): the vault's state and the active
  session's size.
- **Palette** (Ctrl+K): hosts, snippets (`>`), and every action that has
  no button of its own any more: each side view, fold the side view, new
  host or folder, both imports, switch workspace, Preferences, platform
  definitions, highlight rules, Python setup.
- **Questions**: those about one terminal (host key, login questions,
  paste confirmation, a snippet's variables, closing a live tab, joining
  multi-send) cover only that terminal's pane; the other panes stay usable.
  Those about the whole app stay window-modal: delete, rename, new host or
  folder, switching workspace (and keeping or closing its sessions), the
  workspace switcher, the vault's forms, the unlock and start-over screen.
- **First window**: with no workspace open, the active pane shows the
  welcome view (recent workspaces, open a folder, create a workspace,
  quick connect); side views that need a workspace say so.

## Shortcuts

| Keys                                  | What                                |
| ------------------------------------- | ----------------------------------- |
| Ctrl+K                                | Palette                             |
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
| Host and folder properties, inherited settings   | Details sheet (sections device, connection, tags, custom fields, description)      |
| Web links                                        | Details sheet's ways in                                                            |
| Open log folder                                  | Details sheet; tree context menu                                                   |
| Workspace problems                               | Foot of the Hosts view; rail marker while folded                                   |
| Workspaces (open, create, switch, forget)        | Welcome view; workspace name in the Hosts view; Ctrl+Shift+O; palette              |
| Role icons, environment colours                  | Tree rows, tabs, session header and edge                                           |
| Quick connect (SSH, serial)                      | Empty pane; welcome view                                                           |
| Tabs, split panes, drag tabs                     | Terminal area (unchanged)                                                          |
| Duplicate, disconnect, reconnect, send break     | Session menu (header and tab context menu); Enter reconnects                       |
| Session markers (production, unencrypted, ...)   | Session header line and tab                                                        |
| Host key and login questions                     | Over their terminal                                                                |
| Legacy crypto failure advice                     | Over the closed terminal                                                           |
| Session logs                                     | Unchanged; "Not logging" marker in the header                                      |
| Multi-send (join, leave, end, join a pane)       | Session menu, tab bar's join control, Ctrl+Shift+M; questions over the pane        |
| Paste safety                                     | Paste confirmation over its pane                                                   |
| Snippets (send, new, edit, delete, filter)       | Snippets view (the form opens in the view); palette `>`                            |
| Snippet variables                                | Asked over the target terminal's pane                                              |
| Keyword highlighting rules                       | Highlight rules tab (from Preferences or the palette)                              |
| Credential profiles, named secrets               | Credentials view                                                                   |
| Vault (turn on/off, change password)             | Preferences tab; Credentials view links to it; status line shows its state         |
| Unlock and start over                            | Unlock screen at start                                                             |
| Preferences (theme, font, paste, updates, ...)   | Preferences tab (gear on the rail, Ctrl+, , palette)                               |
| Update check and install                         | Notice in the corner; Preferences tab                                              |
| Platform definitions                             | Platforms tab (Hosts view header, details sheet, palette)                          |
| Run snippet, run script on folder/filter/host    | Tree context menu, details sheet, filter actions; job tab                          |
| Back up configs                                  | Tree context menu, details sheet, filter actions, Config backups view; job tab     |
| Fill in from devices and its review              | Tree context menu, details sheet; job tab, then the review tab                     |
| Past runs (search, compare, save, delete, again) | Jobs view, run tab (can open in its own window)                                    |
| Saved jobs                                       | Jobs view; "Save as job" in the job and run tabs                                   |
| Config history and diff                          | Config backups view, details sheet, context menu; history tab (or its own window) |
| Back up on login                                 | Setting in the details sheet; marker in the session header                         |
| Python setup                                     | Preferences tab, then the Python setup tab; palette                                |
| Terminal search, copy, paste, zoom               | Terminal (unchanged)                                                               |

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
