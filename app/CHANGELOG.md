# Herdr changelog

<!-- yuzora: Yuzora's entries sit above Herdr's; the app shows the first one as its version's notes. -->
<!-- yuzora(L7): each Yuzora entry here has its Traditional Chinese twin in CHANGELOG.zh-TW.md: add a line to both. -->
## [0.1.1] - 2026-11-12

### New
- **Settings › Database sets how many rows a result keeps.** A query or a table read stops once it has this many rows. The limit is 100,000 unless you change it, and can be set from 100 to 5,000,000. The next query or table read uses the new value. Changing it used to take an edit of `settings.json`.

### Fixed
- Git: the branch popup, opened from the branch button in the Git sidebar, lists its actions (Update project, Pull…, Fetch, Push…, New branch… and the rest) and your branches and tags. It used to show only its search box. Pull and Fetch have no other button, so they couldn't be reached. A list longer than the popup scrolls inside it.
- Git: in the Git sidebar's repository picker, pressing Enter before any arrow key no longer drops the repository you pinned when it isn't on the list, because it couldn't be read or the list was cut short before it. The picker closes and the pin stays.
- Git diff: when a row of the right-click menu fails, as when the copy doesn't go through, the menu closes and a toast says why. The error used to be drawn in a row that the popup cut off.
- Database: picking another table in the sidebar while a statement of yours is still running in the pane's SQL tab asks first. You can keep it running, or stop it and open the table. It used to be stopped without a question, and on SQL Server that rolls its transaction back.
- Updates: when an update can't be installed, Settings › Updates says why only while that update is the one waiting. The reason used to stay on the page after a newer update was downloaded.
- macOS: the application menu is named Yuzora and says Hide Yuzora and Quit Yuzora, and the command palette says Quit Yuzora. They said Herdr. Typing "herdr" in the palette still finds the entry.
- Counts of one are written in the singular: "1 pane · 1 Space" and "1 agent" in the status bar, "1 change" and "1 commit to push" in Git, "1 row affected" in Database, and "1 file has conflicts."
- A count from 999,500 to 999,999 is no longer written `1000k`. The Git sidebar's line counts write `999k`, and the Database sidebar's row counts write `1M`.

## [0.1.0] - 2026-11-12

### New
- **Files open in a tab of their own.** Clicking a file in Files, a search hit or a file you just made opens it in a new tab after the current one. A file that is already open is shown where it is, also when its tab was restored after a restart. To split the current tab as before, right-click the file and choose Open to the Side. Renaming an image in Files replaces its pane under the new name.
- **Edit and save files in a Space on another machine.** Files in a remote Space are no longer read-only. A save replaces the file in one step and keeps its permissions and owner. If the file changed on that machine after you opened it, you get a conflict instead of an overwrite. Saving needs `sh` on that machine, and herdr-core 0.1.19 or newer on this machine and on that one. A file over 8 MiB can't be saved there, and a file of 8 to 16 MiB opens read-only. A file also opens read-only when a save can't keep its owner or group, or when you can write it only through an ACL or a supplementary group.
- **New file, new folder and rename in a Space on another machine.** The Files sidebar's toolbar, right-click menu and keys create and rename there as they do on this machine. A name that a file, a folder or a link already has is refused with "already exists", and nothing is replaced. A rename that only changes case works on a file system that ignores case. New files and folders get the permissions that the machine's own `touch` and `mkdir` give. A document open at a renamed file, or under a renamed folder, moves to the new path with its unsaved edits, and the next save goes there. Deleting on another machine isn't available yet.
- **Copy from a Git diff.** Click a row to select it, and Shift-click another row to extend the selection. Cmd+C (Ctrl+C on Windows and Linux) copies the selected lines as they are in the file, with tabs and trailing spaces, and without line numbers or `+` and `-`. In a side-by-side diff, the selection belongs to the side you clicked. Right-click a row for Copy, Copy hunk and Copy as patch.
- **Select diff rows by dragging.** Press on a row's text and drag across other rows to select them, on the side where you pressed. Holding the pointer near the top or bottom edge of the list scrolls it. Cmd+C copies the selection.
- **Dialogs, popups and toasts fade in.** Dialogs and popups fade in and move 6 px into place over 120 ms, and toasts over 160 ms. The right dock's selection slides to the mode you pick, the resource bars move to their new length, and settings switches slide. With the system's Reduce motion setting on, everything appears at once. On macOS and Windows, Yuzora reads that setting only when it starts. On Linux it comes from the desktop portal, and a session without one always plays the animations.
- **Up to date with Herdr 0.1.15.** Terminal rows are as tall as in Ghostty, so more lines fit in the same window. On macOS, files and screenshots can be dragged into a terminal. A terminal printing a lot of output no longer freezes the whole window. With many tabs, the tab bar scrolls to the active tab. The Spaces sidebar groups worktrees right away in large repositories.

### Fixed
- Files: switching to a Space no longer shows "Loading files…" first. A folder you have seen shows its last tree at once, with its open folders and selection, and is read again in the background. A folder you haven't seen lists its files before Git answers. With a simulated remote machine that adds 100 ms to every call, switching back to its Space showed the tree in under 130 ms, against 927 to 968 ms before (debug build, screen sampled about every 60 ms).
- Files: the right-click Rename…, New file… and New folder… open their name box at once instead of showing "This view could not be re-rendered". When a new file, a new folder, a rename or a move to the trash fails, the notice starts with the cause, for example "notes.txt already exists".
- Git: the Git mode follows a switch of project in the next frame. It used to check once a second and show an empty "Reading the repository…" in between. A repository you have seen shows its last state at once, and `cd` inside the same repository no longer resets the list. Switching between two local projects you had seen took 82 to 101 ms over 7 switches, down from 153 to 1047 ms over 14 (debug build, screen sampled about every 60 ms). The status bar's branch also follows at once instead of every 2 seconds.
- Git: every button acts on the repository it was drawn for. Each repository keeps its own commit message, author and amend, so a message typed in one repository is no longer committed in another. A Discard you were asked about is cancelled when you leave. Yes reads Git again first and deletes nothing if the change is no longer the one you were asked about. For mod authors, `messages()` in `yuzora:git` no longer returns `authors`.
- Git: with several projects open, Graph and the diff, merge and rebase panes stay in the Space you are in. They used to jump back to the project that opened one first. Each Space keeps its own. After updating, a diff request left over from beta.4 no longer shows another project's file.
- Git: the side-by-side diff gives each side half the width. Drag the divider to set the split anywhere from 15% to 85%, and double-click it to go back to half. One split is shared by every diff and kept after a restart. A long line stays on its own side, both sides scroll sideways together, and a scrollbar appears at the bottom when a line is too wide.
- Git: a side-by-side diff that is read again, after a save, a stage, a commit or a fetch, keeps its sideways scroll position. If Yuzora was once too busy to measure a long line, the sideways scroll range no longer stays too wide.
- Git: a tracked file whose text in the index can't be read, as when a required filter fails (git-lfs that isn't installed), no longer opens as a new file. The diff pane shows the error instead. Discarding all of such a file used to delete it.
- Database: table edits save. Rows can be edited as soon as they are read, without waiting for the row count, and a save no longer waits behind the count. Cmd+S (Ctrl+S on Windows and Linux) saves, also while the cell editor is open. A save that waits on a connection or on a row lock held elsewhere can be stopped with Stop, and the message says whether anything may have been saved. The toolbar's Stop stops only its own statement, never a save in progress. The grid says why a table can't be edited, a failed save's full server message can be expanded, and Discard and Write back stay visible in a narrow pane.
- Database: on SQL Server, a trigger that returns rows no longer makes a save roll back, and the first save after the server closed an idle connection reconnects and goes through. A table with a `geography`, `geometry` or `hierarchyid` column shows an error instead of loading forever. On PostgreSQL, Stop goes to the server the connection is on, not to the address the host name points to when you press it. Text an input method is still composing is no longer saved when you click Write back or leave the cell.
- Database: on SQL Server, a `datetime` or `smalldatetime` cell saves a year-first date, such as `2024-01-06 10:00` or `20240106`, as typed under any login language. A date that Yuzora can't read for certain is refused before anything is sent. PostgreSQL tables are read and saved with `datestyle = ISO` and `extra_float_digits = 3`, so a `timestamptz` read with a zone abbreviation such as `CST` is no longer written back hours off, and a row whose key is a floating-point number is found. The SQL tab keeps the server's own settings, as `psql` does.
- Database: clearing a cell of a type that has no empty value (numbers, booleans, dates and times, `uuid`, `inet`, `json`) saves NULL, and the cell shows NULL before you save. SQL Server used to save 0 or 1900-01-01, and PostgreSQL mostly refused. Text cells still save an empty string.
- Database: a table is read and saved on a connection of its own. `SET` statements, temporary tables and `USE` in the SQL tab no longer change how a table is read or saved. A table on a SQL Server that opens transactions by itself (`IMPLICIT_TRANSACTIONS`) now saves.
- Database: picking another table in the sidebar while a table is open no longer shows "This view could not be re-rendered" or stays on "Loading…". The pane switches to the table you picked, and asks first if it has unsaved edits.
- Database: in a table pane's SQL tab, Run and Cmd+Enter no longer do nothing while the table is still being read, for example while it waits on a transaction the SQL tab left open. Run stops that read and runs the SQL, so a COMMIT takes one press. A statement of your own that is still running isn't stopped, and the SQL tab says to press Stop first. Unsaved edits, including a save in progress, are asked about first.
- Database: pressing Stop on a table save before Yuzora has sent COMMIT now always saves nothing, also when the app was too busy to notice the Stop in time. It used to save and then say the database may have saved the changes. On SQL Server, browsing a database's objects or opening a table no longer fails with "deadlocked … Rerun the transaction" (Msg 1205) while another session creates or drops tables. Yuzora reads again instead.
- Database: opening a table shows its rows without waiting for the table's description, and reads that description once per open instead of twice. Over a link with 50 ms round trips and the connections already open, the first rows of a 500-row table showed in 116 ms on PostgreSQL and 67 ms on SQL Server, against 341 ms and 354 ms before (debug build, medians). On a database with no latency you won't notice a difference. Stop pressed before the table's key is read ends the load at once and leaves the rows read-only. A table whose columns changed while it was being opened also opens read-only.
- SSH: when you change the address, port, user or jump host of a host you saved in Yuzora, it becomes a new host and Yuzora ends the connection to the old one, with its tunnels. Its saved password goes with it. An SFTP pane, a transfer or a file opened for editing that belongs to the old one says there is no such host; open them again from the new one. SFTP used to keep working on the old server while the list named the new one.
- Windows: Yuzora can update itself when you start it from its Start menu shortcut. Updates used to fail whenever it was started that way. If an update can't be installed, Settings › Updates says why and what to do, and Restart shows a notice that opens that page. If you installed 0.1.0 over a beta, restart your PC once. The beta's herdr-core still holds Yuzora's install folder and stops the first automatic update until it ends. Restarting ends it, and your terminals with it. If you would rather not restart, run `hc server stop` in a terminal outside Yuzora instead. It ends every terminal and the agents running in them, the same as a restart would.
- Updates: the update notice says `Yuzora <version> is ready` instead of `Herdr <version> is ready`. A beta that updates to 0.1.0 still shows the old wording.

## [0.1.0-beta.4] - 2026-11-10

### New
- **Right-hand dock.** A layout with a side panel (main, and the default yuzora) gets a 40px mode bar next to the panel with Files, Git, Database and SSH. A layout without one (terminals, win98, your own) gets a drawer on the right edge, and a mode you click opens in a window. The open mode stays put when you switch Spaces, and changing the layout no longer makes Yuzora's tools disappear.
- **Herdr's own Spaces and Agents on the left.** The default yuzora layout now uses Herdr's native Spaces and Agents lists (the top left reads "Herdr"), and Files, Git, Database and SSH moved to the right-hand dock. The Space color set in Yuzora shows on the avatar in the left list. If you edited `yuzora.json`, your copy is kept as `yuzora-edited`; an unedited one is replaced automatically.
- **A dock setting for each layout, and one for startup.** Settings › Mods › Layouts lets each layout set its right-hand dock to Auto, Drawer or Off, which takes priority over the layout file's own setting. Settings › General › When Yuzora opens picks whether the dock opens in Files, Git, Database or SSH, or stays collapsed.
- **`Cmd+Alt+B` (macOS) / `Ctrl+Alt+B` toggles the right-hand dock.** An unedited beta.2 or beta.3 `keys.json` is updated to the new defaults automatically (this includes Herdr's terminal search, Cmd/Ctrl+F). An edited one is left alone: add `"cmd-alt-b": "yuzora-modebar.toggle"` (or `ctrl-alt-b`) yourself.
- **Synced with the official herdr-app v0.1.14.** A herdr-core connection error now shows its reason and a retry, terminal search (Cmd/Ctrl+F) and scroll commands, a scrollbar on panes, `pane.swap` for mods, and Add machine now tells you the command to run. The herdr-core requirement is unchanged (0.1.11 or newer, generation 1). One step of v0.1.14 is left out: after an update it downloads Herdr's core installer and runs it, to turn a herdr-core that is still a single executable into the Herdr Core app. Yuzora doesn't run a downloaded script by itself; if you want that, run the official herdr-core installer yourself.

### Fixed
- Git: when an open repository's `.git` goes missing, Git commands (fetch, push, status and the rest) no longer fall through to a repository around it and run there. `GIT_CEILING_DIRECTORIES` stops the search; where the path above contains `:` or `;` and no ceiling can hold, each command first asks Git which repository it found and is refused unless it is this one. Hooks get no extra `GIT_DIR`.
- Git Log: a very long commit message can no longer be cut off by the render time limit on a slow machine (each paragraph is now a single text element).

## [0.1.0-beta.3] - 2026-11-05

### New
- **The whole app interface is English only now, and the language setting is removed.** A language you saved earlier (Chinese, for example) causes no error; the app shows English.
- **Git sidebar repository picker.** A Space with several repositories lets you pick which one to show (follows focus by default, or pin it; each Space remembers its choice). Finding repositories runs no Git, and the chosen one goes through the trust check. Command: `yuzora-sidebar-git.pick-repo`.
- **Resizable Git Log detail.** The detail can be scrolled and its top/bottom split dragged; double-click restores the default. The detail shows the full commit message (up to 2000 lines).
- **Simpler tab bar and sidebar.** The "New pane ▾" menu, the two split buttons and "Next layout" are gone, and the `layout.switch` default shortcut (⌘⇧L / Ctrl+Shift+L) is removed. Bind `yuzora-shell-tabs.new-pane` (a dot, not a colon) yourself to open the new pane menu; the old default binding in `keys.json` is skipped.

### Fixed
- Windows no longer pops up a stream of console windows while Yuzora runs Git, file and remote commands.
- The tab bar lists only the current Space's tabs again, as Herdr does, instead of every Space's tabs.
- Git diff, side by side: each side is as wide as its own longest line (an added file's empty left side no longer takes the width of its longest line), and a click on a side's line numbers picks that side's line where the row lays it out.
- Trusted folders: `trusted.json` is changed one writer at a time (threads and processes alike, under `trusted.json.lock`) and replaced whole, so a folder trusted at the same moment as another is no longer lost, and a command that reads the list meanwhile no longer takes the folder for untrusted.
- Untrusted folders: the Changes panel's diff and Commit and the worktree dialog's Enter ask again whether the folder is trusted right before they run Git (its config may have gained a program since the panel last read it), and the Git the Files panel runs reaches no remote at all, a local one included, so a Git older than 2.44 can't fetch for a partial clone either.
- Trust check: the repository's config is read in the same shell as the submodule scan, so each check is one git call (one `hc exec` on a machine), and an answer without the config's exit code counts as unreadable; the Changes panel's refresh no longer asks a second time for the diff it opens on its own.
- Files: on macOS and Windows, changes inside a folder that the root `.gitignore` or `.git/info/exclude` ignores (a build's `target/`, `node_modules/`) no longer wake the Files sidebar and panels.
- Files, Windows: a write into an ignored folder is no longer reported as a change of the folder itself (it made the panels reload once per build batch); the folder being created, removed or renamed is still reported.
- Status bar: the Git branch look takes one round trip on a remote machine instead of three, and two reads run in parallel on a local one.
- Remote Git reads: a diff's independent reads (index entry, blob, kind, HEAD, modes), the log's refs and the repo's operation state are sent together instead of one after another (unstaged diff 5 → 2 round trips, staged 4 → 2, refs 3 → 1), and the index side of a diff is read by the entry's object id so it matches the entry it was checked against.
- Windows: the installer's prompt to remove an older all-users Yuzora now notes that connections and settings are not brought over automatically yet.
- Windows: SFTP "Edit" no longer closes Notepad after 30 seconds, and Settings can move templates to the Recycle Bin and show files in Explorer.
- Windows: the editor, Markdown and image panes now reload when the file changes on disk, and the SFTP local panel shows `C:\…` paths.
- Running programs on this computer (Git, search, scripts) no longer waits for polling: a `git --version` call dropped from about 52 ms to 29 ms on Windows and from about 53 ms to 8 ms on macOS.
- Git diff pane: it loads again only when a change may touch its file, the one it was renamed from, a folder above or a path inside either, or a `.gitattributes`, instead of for any change in the repo; when the changed paths can't be told (the index or refs git wrote, a rescan) it loads again as before.
- Trust check, on this computer (macOS and Linux): a plain repository's answer is kept while its `HEAD`, `index`, `config`, `config.worktree`, `objects/`, `refs/`, `remotes/` and `branches/` stay as they were and `.git` and `commondir` lead to the same dirs; a gitlink in the index, a `modules/` folder, an `include` in its config, a split or sparse index (git is asked whether it reads a shared index), a file system whose kernel doesn't keep change times itself (kept only on APFS, HFS+, ext2/3/4, XFS, Btrfs, tmpfs, ZFS and F2FS; not on FAT, exFAT or network file systems), a file changed in the 3 s before the scan, or one that changed during it means scanning each time. Opening a diff in an untrusted folder took 74 ms and now takes about 38 ms; a Files reload's check, 47 ms and 8 processes, now 0.
- Git sidebar: the +/− counts and buttons are no longer covered by long file names, and 6-digit line counts read `150k`.
- The `herdr.d.ts` in the app home showed mod command shortcuts with a colon, which made the whole `keys.json` invalid when copied; it now shows a dot.

## [0.1.0-beta.2] - 2026-11-04

### New
- Yuzora is now built on herdr-app: the default Yuzora layout (a sidebar with 5 modes on the left, a mode bar and a status bar) and the yuzora-dark theme.
- The Yuzora theme now uses the brand's blue and cyan, with a new light version, yuzora-light; by default it follows the system's light or dark mode.
- No usage data is sent anymore; feedback opens a GitHub issue.
- Workspace sidebar: Spaces / Agents tabs, a scope chip for each machine (an LED shows the connection state), a "Needs attention" section, and a tree of machine → Space → worktree → agent; an agent's state is shown by both color and shape.
- A new Space starts by picking a folder (on a remote machine, you type the path and it is checked on that machine). The Space is named after the folder and its terminals start there; if a Space already exists for that folder, Yuzora switches to it.
- The mode bar shows a badge count for each mode; each tab in the tab bar carries a Space badge and a state, and "+ New pane" mixes tool panes, terminals and agents as splits in one tab.
- Pane headers and frames: the border brightens on focus, and the border and header turn amber when an agent needs a reply; the status bar shows the number of panes, Spaces and agents, the number that need a reply, the current branch with ↑↓, and the HERDR version.
- The Welcome, What's new and Feedback screens now carry Yuzora's brand; the interface font is now the system font.
- When sharing herdr-core with the official Herdr: the version-mismatch screen uses Yuzora's name and explains that the core is shared; if the core restarts as a version Yuzora can't use (for example, Herdr moved to a new generation), Yuzora doesn't connect and keeps the reason on screen (click it to open the update page), and reconnects automatically once that's fixed.
- SSH / SFTP mode: lists ~/.ssh/config, Yuzora's own hosts and herdr machines, with connection state and latency; on first connection the host key's fingerprint is confirmed, and a changed key triggers a warning.
- Side-by-side SFTP transfers: browse this computer and the host side by side and transfer with → / ← or drag and drop; the queue shows progress and speed, and a transfer can be cancelled, retried and resumed where it stopped, keeping permissions and times; a remote file can be opened in a local text editor and is uploaded each time it's saved.
- Host quick actions: open SFTP, a new terminal, a HERDR session, and known-hosts management. Databases can't connect through an SSH tunnel yet.
- The bottom-right corner of the window shows the CPU and memory Yuzora uses on this computer (the app itself, the terminals and agents in herdr-core, and the tools Yuzora starts); click it to expand each category; ⚠ shows when sampling runs into a problem.
- The app now updates itself from Yuzora's GitHub Releases: it checks every hour, only installs an update whose manifest signature and SHA-256 both check out, and applies the update on the next restart. A beta build only updates to a stable release (0.1.0 and later), never to another beta; download betas by hand.
- Installers: `Yuzora_<version>_aarch64.dmg` on macOS (Apple Silicon only, ad-hoc signed; the first time you open it, click "Open Anyway" in Privacy & Security), `Yuzora_<version>_x64-setup.exe` on Windows, which needs no admin rights, and a tar.gz with `install` on Linux. The command-line program is called `yuzora`.
- Settings now open from Yuzora's "General" page, with a new "Yuzora" group listing each feature's settings page; the Privacy page explains that Yuzora collects and sends no data.
- General settings: language (follow the system, English, 繁體中文), theme, font and size, zoom, and terminal clipboard writes.
- Agent templates: manage each machine's herdr-core templates in Settings (edit in a form, check before saving, move to the trash), and "Start from template" in a chosen Space and tab.
- Mods management: lists shipped, edited, cloned and personal mods and layouts with their errors; you can clone, show in folder, check, reset and switch layouts, and install from a folder after a trust warning.
- Databases: the sidebar's "Databases" mode connects to PostgreSQL, SQL Server and SQLite (passwords are kept in the system keychain), with a database picker and a schema tree; the table pane has Data (a virtual-scrolling grid, sorting, copying, and editing cells that are written back in a single transaction), Structure (columns, indexes, foreign keys, DDL) and SQL tabs; the SQL pane runs the selection or the whole script, with several result sets, cancelling and a history.
- Files sidebar: follows the current pane's repo, with Git marks and file icons colored by type; create, rename, move to the trash, copy the path, and reveal in Finder; fuzzy file-name search and content search (with ripgrep when it's there, a built-in search otherwise).
- Code editor: tree-sitter highlighting for 14 languages, conflict checks on save, and find and replace; a file changed on disk reloads by itself, and you're told when an agent is editing the same file.
- Markdown preview (kept in sync with the editor as you type, with a switch to the source) and an image / SVG viewer. A repo's SVG is drawn in a separate process with time and memory limits; one past them shows a placeholder, with the reason in the image viewer (a Markdown preview shows only the placeholder). Files in a remote Space can be browsed and opened read-only.
- Git (modeled on IntelliJ IDEA): the sidebar's Git mode stages, commits (amend, sign-off, skipping hooks, message history and templates) and pushes; the diff pane stages or discards a single hunk or the selected lines; the commit log with its branch graph, filters and context menu; the branch popup (smart checkout, merge, rebase, compare); three-way merge for conflicts; interactive rebase; Annotate; stash, tag, remote, worktree and patch management; a Git settings page.
- Git asks before it trusts a folder: until you trust it, Yuzora runs no Git there that would start a program the repo's own settings name (filters, SSH commands, remote helpers), including the settings of the Git folder kept in, or named from, `.git/modules` for a submodule that isn't checked out, and a push, of a branch or of tags, leaves submodules alone until then; a partial clone asks first too, since it fetches what it lacks as it reads (the Files panel's folder watch on Linux waits for it as well). Reads Yuzora runs on its own stay offline until then. The background fetch, and a fetch, pull or push you start yourself, use only Git's built-in transports, never a remote helper, until then, and in a folder whose config runs programs they wait for trust too. The background fetch runs no hooks; Git you start yourself, a commit or a push for example, still runs ordinary hooks.
- Untrusted folders: until you trust a folder in the Git mode, Yuzora runs no Git there that could start a program the repository's own config names (a filter, an fsmonitor hook). The Changes and Files panels and the worktree dialog say the folder isn't trusted yet, the Spaces list reads the branch and ↑↓ from refs only, and on Linux watching such a folder no longer asks Git for its ignored folders.
- Synced herdr-app upstream to f6fe216 (after v0.1.12): optional terminal font ligatures (`font_ligatures` in `settings.json`, off by default), the shortcuts help closes when pressed again, and fixes for the starting size of pane programs, the focus frame, control-key aliases and more.

## [0.1.15] - 2026-11-10

### New
- **Terminal rows are as tall as in Ghostty.** Herdr used to add about 15% extra space to every line, so the same font looked looser than in other terminals. Rows are now sized from the font itself, at every zoom and font size, and more lines fit on screen. (#66)
- **A layout can choose the mod that shows notifications:** `"toast": "<mod>"` in the layout. The shipped toast is unchanged. (#364, thanks @ngotchac)
- Mods can scroll a normal scroll area to one of its children: `ScrollHandle.new()`, `.track_scroll(handle)` and `handle.scroll_to_item(index)`.
- Mods: `knownAgents()` now rejects when the check itself fails, instead of reporting every agent as not installed.

### Fixed
- **On macOS, files and screenshots can be dragged into a terminal.** Dropping a file from Finder works right after typing, and the floating screenshot thumbnail (Cmd+Shift+4) attaches to Claude Code and other programs. (#654, #709, thanks @EliaTogni and @rqres)
- **On macOS, the + menu no longer shows every agent as "not installed" right after login or a restart,** and `herdr-app check` works then too. When Herdr can't check, it says so instead of calling agents missing.
- **On macOS, reopen older terminals once.** Terminals opened before herdr-core 0.1.20 can still end when Herdr quits, and this update quits Herdr once. Once herdr-core is 0.1.20 or newer (Settings > Updates shows it), close and reopen those terminals, or restart your Mac, and they are safe from then on.
- **On Linux, Herdr opens on a fresh Ubuntu 26.04 or Fedora 45.** It no longer needs the X11 keyboard library on Wayland. (#839)
- **A terminal printing a lot of output no longer freezes the whole window.** (#748)
- **An agent you come back to leaves "done"** in the agents list.
- With many tabs, the tab bar scrolls to the active tab. (#802)
- Coming back to a terminal that another device (your phone) resized takes its size back when you click it or switch to its tab or space, not only when you type.
- The Spaces sidebar groups worktrees right away in large repositories (seconds before).
- The Open worktree dialog takes the keyboard after a slow first load. (#798, thanks @ilstar)
- Creating a worktree on this machine while a terminal on another machine is focused no longer fails and leaves the space out of the sidebar.
- Playing a GIF or other animated image in a terminal uses far less memory, and gives it back when the image leaves the screen. (#560)
- Settings: the font list stays inside the window, and Add machine shows login instructions you can copy.
- Layout terminal blocks stay in place when the connection to their terminal drops. (#761)
- Dragging a block that is squeezed very small no longer crashes Herdr.
- On Windows, clicking into an ssh or tmux terminal no longer sends a stray drag that changes its selection. (#759)
- Ctrl and Alt held during a mouse click or scroll, and Shift during a scroll, reach programs that track the mouse. Shift+click still selects text.
- The picker's earlier rows can be reached with the keyboard.
- The wide CJK space (U+3000) is drawn when the font has a glyph for it. (#777)
- A sub-agent on another machine shows under its parent in the agents list.
- A bottom pane header draws its divider above the header. (#712, thanks @Zihao-Qi)
- An agent's spinner after a title prefix no longer shows in its name. (#626, thanks @namastex888)
- An agent title that is a folder no longer replaces the tab's name. (#740, thanks @chasereyn)
- Clicking the "Esc" hint in a dialog closes it. (#690, thanks @VinhLe1410)
- Mod icons load when the mod folder is a link. (#427, thanks @ngotchac)
- The Changes diff keeps tab indentation. (#366, thanks @sundarshahi)
- From this version on, Herdr keeps space and tab details that a newer Herdr saved on the same machine, and saved files keep their permissions. (#729, #722, thanks @abhijit-s)
- On Windows, updates apply when Herdr was started from its install folder, file links with encoded characters open, and Add machine lists every host from a wildcard `Include`. (#673, #663, #672, thanks @sundarshahi)
- On KDE Plasma, the Herdr icon shows right after install. (#703, thanks @okurmustafa)


## [0.1.14] - 2026-11-08

### New
- Settings > Add machine tells you what to run when a machine needs a password, a new host key or herdr-core installed there, instead of opening a setup terminal. Once you've added it yourself, the list shows it as added.
- Mods can swap two panes by id with `pane.swap`. (#665, thanks @sQVe)
- Kimi Code gets its own icon. (#647, thanks @SuTang-vain)
- Mods can give `exec` a timeout: `exec(cmd, args, { timeout })` in milliseconds, up to 10 minutes (30 seconds by default). Opening a worktree in a large repository uses it.

### Fixed
- **Mods no longer all break at once after a while.** Opening Settings, the palette or a picker, or switching layouts, slowly filled up the mods' memory until every mod showed "This view could not be re-rendered". Mods also open faster now. For mod authors: the top of a mod's file now runs once and is shared by all its copies; keep per-copy state in the class.
- **On macOS, `herdr-app check` no longer leaves a second Herdr icon in the Dock.** Quitting that icon, or switching Herdr off under Background App Activity in Settings, ended every terminal. (#398, thanks @revofusion)
- Cmd+W and Cmd+Shift+W with Settings, the palette or a picker open close that window, not the terminal behind it.
- A pane that opens while the window is still laying out shows all of its output: a long line could lose its end, on screen and when copied.
- Layouts you've edited by hand keep all their pieces when a terminal in them ends or can't be reached.
- Two agents in one tab show their own names in the agents list.
- Codex started from the + menu keeps its own session, so closing one Codex no longer ends the others. This needs Codex 0.156 or newer. (#657, thanks @martintrifunov)
- On Windows, the + menu no longer marks installed agents as not installed.
- When Herdr can't reach a running core, it says why and how to fix it, instead of showing the install screen.
- On Linux with fcitx4, Chinese and other input methods keep working. (#558, thanks @SailorMoonQ)
- On Linux, the input method keeps working after switching windows.
- On KDE, the window no longer opens tiny. (#601, thanks @zdziejowski)
- New terminals start in the right folder when you open several at once, and start in your home folder when the folder they'd inherit is gone. (#511, thanks @JJLiebig)
- Zooming into a pane while moving between agents shows the right pane. (#568, thanks @martintrifunov)
- A kept sub-agent shows once in the agents list, under its parent. (#539, thanks @mertkaradayi)
- Toasts name agents by their proper name. (#530, thanks @thuanlm215)
- Find highlights all of a wide character. (#587, thanks @chanyang721)
- Clicking in a program that takes the mouse clears your selection. (#369, thanks @sundarshahi)
- The Changes list fills its panel and scrolls to every changed file. (#474, thanks @shadownrx)
- The palette scrolls past group titles, and shows ⌘ only on macOS. (#531, thanks @patrick-xin; #478, thanks @JJLiebig)
- Dialogs, the confirm dialog and Settings look right: sized to their content, rounded corners, layout previews and long theme names stay in their cards. (#593, thanks @karlAlnebratt; #570, #437, #621, thanks @martintrifunov; #457, thanks @yankewei)
- The sidebar rail's items line up. (#453, thanks @log4b)
- Pressing rename again closes the rename prompt. (#569, thanks @martintrifunov)
- Idle windows no longer wake every mod 20 times a second. (#548, thanks @abhijit-s)
- `herdr-app check` no longer touches the running app's shared mod state. (#516, thanks @JJLiebig)
- The terminal menu leaves out pane items where there's no pane, and mods see which terminal has the keyboard. (#553, #554, thanks @thuanlm215)

## [0.1.13] - 2026-11-04
<!-- spotlight macos -->

### New
- **On macOS, programs in your terminals can reach your local network and use the microphone.** Herdr's core is now its own app, Herdr Core: macOS asks once for each (the network prompt may say herdr-core), click Allow. Terminals opened before this update need to be reopened once. Node, Copilot or a model server on another machine connect; voice input works. (herdr-core#100)
- **Find in a terminal.** Cmd+F on macOS, Ctrl+Shift+F on Linux and Windows searches the screen and its history. If you've edited your keys.json, add "cmd-f": "terminal.search" (or "ctrl-shift-f") to its terminal section yourself. (#436, thanks @SOUMITRO-SAHA)
- Terminal panes show a scroll thumb you can drag. (#428, thanks @msly)
- Programming ligatures, off by default: set font_ligatures to true in settings. (#244, thanks @ruilopesm)
- Mods and agents can scroll a terminal's history with terminal.scroll, and keys.json can bind a command with arguments. (#455, thanks @sQVe)
- dsh-tui agents have their own icon and start from the new agent menu. (#335, thanks @tsonglew)

### Fixed
- Programs start at the size of their pane, so the first screen of an agent isn't drawn at the wrong width.
- On Windows, Nerd Font icons show again.
- On Windows, links with an = in them open in the browser.
- On Windows, agents and the Files panel work on remote Windows machines.
- On Windows without a GPU, Herdr uses far less CPU. (#424, thanks @kiote)
- On Windows, OneDrive folders and paths with curly quotes open in Files. (#354, thanks @chanyang721)
- Ctrl+[, Ctrl+I, Ctrl+M, Ctrl+_ and Ctrl+- reach programs as they do in other terminals.
- Cmd+C reaches the program when nothing is selected. (#319, thanks @sllrs)
- The agents list no longer breaks with "This view could not be re-rendered" on busy setups. (#485, thanks @mike-north)
- Open worktree no longer breaks in repos with many worktrees. (#486, thanks @martin-ro)
- Switching back to the tab from a home page gives the terminal the keyboard again. (#484, thanks @ngotchac)
- Cmd+1 to 9 follow the order of spaces in the sidebar.
- The pane focus ring shows only where the keyboard really is.
- Shortcut keys work right after start, even with no space open.
- The shortcuts overlay closes with the same key, and Ctrl+? opens it on Linux.
- Wide fallback symbols like 🕹 fit their cell instead of covering the next one. (#468)
- Braille dots are spaced evenly. (#351, thanks @leoscricca)
- On Linux, no light flash when Herdr starts on a dark desktop. (#225, thanks @claytonfaria)
- On Wayland with several seats, the mouse and keyboard work. (#123)
- Tabs you're not looking at resize with the window. (#340, thanks @imakil195)
- Mods see when the window loses and gets focus. (#350, thanks @ngotchac)
- The font picker no longer crashes on some fonts. (#316, thanks @ruilopesm)
- The command palette scrolls smoothly. (#348, thanks @patrick-xin)
- Add machine lists hosts from Include files in ~/.ssh/config. (#328, thanks @ashrodan)
- Selected text in mod text views stays readable.

## [0.1.12] - 2026-11-02
<!-- spotlight macos -->

### New
- **On macOS, your agents now run fully apart from the app.** Close Herdr, quit it, update it: your agents keep running. Agents already running pick this up the next time they start.
- On macOS, the system may ask once if **herdr-core** can access a folder like Documents. That's Herdr; click Allow.
- On macOS, Settings > Keyboard chooses what Option does: Alt, typing characters like á and €, or left or right Option as Alt.
- A machine that was rebuilt or replaced shows "rebuilt · Connect" in the sidebar: one click connects to the new one.
- Seen and pinned agents are shared between Herdr windows and the phone app.
- Mods can take the keyboard, so moving focus can land on them. (#283, thanks @Osc100)
- PiG agents have their own icon. (#299, thanks @balcsida)
- Add machine takes a port, like `can@server:2222`.

### Fixed
- On Linux, color emoji show in color instead of blank (Fedora's emoji font).
- Emoji like ✍️ and ❤️ take two cells and draw in color. (#287, thanks @occultsound)
- On macOS, programs in Herdr terminals can ask for the microphone, camera and to control other apps. (#137, thanks @shivanandasai-altir; #252)
- Agents on another machine find the commands their shell setup puts on PATH. (#262, thanks @tajchert)
- New spaces open on the machine you're looking at.
- Horizontal scrolling reaches programs like vim and less.
- Clicks work in Textual apps.
- btop's braille graphs draw without gaps.
- Nerd Font icons use your terminal font.
- Rounded corners and dashed lines in boxes draw like Ghostty. (#255, thanks @occultsound)
- Herdr uses less CPU with many busy agents.
- A big project no longer makes the folder watcher burn a CPU core. (#263, #257)
- Mod pictures and icons update when their file is saved.
- Moving focus back into a split returns to the pane you came from. (#304, thanks @sundarshahi)
- Unnamed tabs show what their agent is doing in the agents list. (#281, thanks @sundarshahi)
- A new tab with no space open starts a space. (#303, thanks @sundarshahi)
- Files marks the files inside a new folder, and opens folder symlinks. (#309, thanks @Nagell; #301, thanks @azaeldrm)
- Lists in mods follow the zoom. (#296, thanks @Calonca)
- The + menu shows your agents right away. (#251, thanks @shellscape)
- Grok shows its own icon. (#293, thanks @Zynoo71)
- A malformed command can't close the tab you're on. (#297, thanks @chanyang721)
- On Linux, Ctrl+Insert and Shift+Insert copy and paste in new installs; add them to your keys.json to get them on an existing one. (#258, thanks @dr-moreira)
- In full screen on macOS, the sidebar header lines up. (#261, thanks @runkids)
- On Windows, file names in Korean, Chinese and other languages show and open in Files. (#236, thanks @loonslo)
- On Windows, Esc then Ctrl+C reach the program as two keys. (#162, thanks @JJLiebig)
- In Git Bash on Windows, the installer points to the PowerShell one. (#238, thanks @sundarshahi)

## [0.1.11] - 2026-10-31

### Fixed
- The window no longer freezes or spikes the CPU for seconds at a time while mods run.
- Programs get the new colors right away when you change the theme or switch light and dark. (#122, thanks @geoqiao)
- On macOS, Vietnamese input methods like OpenKey, DauKey and Gõ Nhanh type in terminals. (#230, thanks @emizuki)
- Links a program breaks across lines open with Cmd+click. (thanks @pdeveltere)
- Editing the theme you're using repaints the window. (thanks @shawnyeager)
- Mod tooltips show in embedded windows. (thanks @ruilopesm)
- Opening the palette or a popup again closes it instead of stacking another. (#151, thanks @ruttybob)
- The status bar shows your real palette key. (#150, thanks @ruttybob)
- The Go-to popup keeps the selected row in view. (#152, thanks @ruttybob)
- No empty strip under the agents list in a wide sidebar. (thanks @kaofelix)
- A failed update check no longer says "Up to date". (#186, thanks @weichieh0204)

## [0.1.10] - 2026-10-30

### Fixed
- Images that Claude Code, yazi and other programs draw with kitty placeholders show up instead of rows of boxes. (#110)
- Images are the right size and sharp on Retina and other HiDPI screens. (#70, thanks @wpdmitry)
- Hover effects work in Claude Code and other programs that track the mouse.
- A drag that starts in another pane no longer reaches nvim or tmux, and a drag inside one keeps working when you let go outside it.
- Dragging a selection past the top or bottom of a terminal scrolls it.
- Closing a tab or space no longer brings its terminals back as unnamed tabs for a moment. (#108)
- The + menu starts agents with 1 to 9 while it's open, and shows your own shortcuts. (#102, #133, thanks @notHuman9504)
- Add a machine lists every host in your SSH config and shows the install prompt on top. (#95)
- Your own alert sounds play, and sound changes apply without a restart.
- A layout block that shows the focused agent stays in your layout when that agent ends.
- The Files panel keeps your open folders and preview when you switch spaces. (#86, thanks @ruilopesm)
- Dragging a space in the sidebar shows where it will land. (#114, thanks @takahashi-seiryu)
- Long agent titles cut off before the pin and the time. (#127, thanks @rashim2104)
- The + above Spaces lines up with the sidebar button. (#121, thanks @weskor)
- On macOS, a slow mouse wheel scrolls terminals. (#80, thanks @juanibiapina)
- On macOS, Cmd+V pastes copied files and screenshots into agents. (#105, thanks @WesleyBLDC)
- On macOS, Cmd+Shift+[ and ] switch tabs, and other Shift+punctuation shortcuts work.
- On macOS, Option types [ ] { } on ABC-QWERTZ and ABC-AZERTY.
- On macOS, Vietnamese Telex and other input methods compose. (#93)
- On macOS, keys typed with Fn held, like Fn+Esc for a backtick, reach terminals.
- On macOS, agents started from the + menu load your login setup, like Homebrew's PATH.
- On Windows, Ctrl+V pastes text, like Windows Terminal.
- On Windows, the Files panel opens folders with spaces or parentheses in their path. (#130)
- On Windows, the window buttons line up and stay crisp at every display scale.
- On Linux, Chinese typed through fcitx or ibus on X11 comes out right. (#97)
- On Linux, accent keys no longer type a stray character first. (#87, thanks @ruilopesm)
- On Linux, right-clicking a tab no longer opens the window manager's menu too. (#79, thanks @dev-gvs)
- `terminal.open` with a command starts in the folder you give it.
- `herdr-app check` no longer crashes on mods with scrolling lists or layout animations.

## [0.1.9] - 2026-10-30

### Fixed
- On macOS, updates keep Herdr's Apple signature again, so macOS stops forgetting the permissions you gave Herdr.
- Herdr no longer uses CPU in the background when the Changes or Files panel is open in a git repo.
- Mods use less CPU while they wait for work.

## [0.1.8] - 2026-10-30

### New
- **Option works as Alt on macOS.** Claude Code's Option shortcuts work, automatically on US keyboards. Change it with `option_as_alt` in settings.json.
- **Programs can copy to your clipboard,** also from other machines. Pi's or Claude Code's own mouse selection lands on your clipboard.
- **Your own commands in the + menu.** Entries in programs.json show up, shell functions included.

### Fixed
- Accent keys work on macOS and with ibus on Linux, and Caps Lock types capitals on macOS.
- @, [ and friends type on Slovenian, German and other Mac layouts in every program.
- Terminal colors show exactly as programs set them, so powerline prompts look right.
- Double-click selects a word, triple-click a line.
- Agents start from the + menu under fish, and one that fails at launch says why.
- On macOS, the window buttons stay clear of a narrow sidebar and the tabs.
- You can type a commit message in the Changes panel.
- Spaces named with emoji or accents work everywhere.
- The font list in Settings scrolls on its own.
- A downloaded update installs at the next start, even after a crash.

## [0.1.7] - 2026-10-29

### Fixed
- Herdr no longer freezes when a terminal shows an older Nerd Font icon it has no picture for, like some powerlevel10k battery icons. It draws them as text instead.

## [0.1.6] - 2026-10-29

### New
- **Coming from herdr?** The welcome says what carries over and gives you one line for your agent to bring your keybinds and settings across. Open it again with Show the welcome again in the command palette.
- **Updates at a glance.** Settings → Updates shows the app and the core on one row each, with What's new a click away.
- **herdr-core updates with Herdr.** After Herdr updates, its core moves to the latest version too. Your terminals keep running.
- **Screenshots in feedback.** Drop images straight onto the feedback form.

### Fixed
- Paste and copy in a peek go to the terminal you're peeking at, not the agent behind it.
- Scroll back through thousands of lines instead of a few hundred.
- Mod data, like topics and colors, can't get lost if Herdr closes in the middle of saving.
- Commands with an argument they don't know say so instead of acting on the current terminal.
- On Windows, file links at the end of a sentence open the right file.

## [0.1.5] - 2026-10-27
<!-- spotlight -->

### New
- **Drag panes where you want them.** Hover the top of a pane and grab the dots. Drop it on another pane to swap them, or near an edge to put it beside. A highlight shows where it lands.
  ![Dragging a pane beside another](https://media.herdr.dev/0.1.5/pane-drag.webp)
- **Ctrl-click a terminal to peek at it.** When an agent mentions another terminal, like t2, Ctrl-click (Cmd on macOS) its address to see it over your tab. Go to jumps there.
  ![Peeking at a terminal an agent mentioned](https://media.herdr.dev/0.1.5/peek.webp)
- **Agents can see the window.** `herdr-app shot` saves a picture of Herdr as it looks right now, so an agent building a mod can check its work. `herdr-app --skill` and `hc --skill` teach an agent the rest.
- **What changed, after every update.** Release notes like these, in Settings → Updates, including versions you skipped.

### Fixed
- nvim and other full-screen programs redraw right after you resize their pane.
- On Windows, AltGr and accent keys type the right characters, and Shift+Enter adds a new line in Claude Code.
- On Windows, new terminals start in PowerShell 7 when you have it, and Herdr follows their folder.
- On macOS, Herdr keeps the permissions you gave it across updates.
- Short menus open next to the pointer instead of near the top of the window.
- Removing a worktree asks once, then removes it.
- Mods can fetch from the web, know their size, and clean up when they close.

## [0.1.4] - 2026-10-23
<!-- spotlight -->

### New
- **See the agents that need you first.** Pick Priority beside the Agents title: agents waiting on you, then the ones that just finished, then the ones still working. Grouped by space is still there.
- **Pin the ones you're watching.** Hover an agent and click the pin. Pinned agents stay on top in both views until they end.
- **Fold what you don't need.** Click a space or section header to fold it; the header still shows how many agents are inside.

### Fixed
- Temporary agent sandboxes stay out of your spaces and machine lists.
- The agents list keeps its place when you scroll.
- An agent you're in moves to its new section right away.
- A pane closes when its terminal finishes instead of showing “has finished”.
- On macOS, the red close button hides Herdr instead of leaving it without a window.
- Mods reload cleanly when you save them, and tell you when a save didn't load.
