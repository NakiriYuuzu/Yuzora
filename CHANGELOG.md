# Changelog

<samp>English · <a href="CHANGELOG.zh-TW.md">繁體中文</a></samp>

## [0.1.0] - 2026-11-12

The first stable release of the new Yuzora, built on herdr-app. Files can edit, create and rename files on other machines, and opens each file in a tab of its own. Git switches projects faster, and its diff splits in half and can be copied. Table edits in Database save. Dialogs, popups and toasts fade in. From 0.1.0 on, Yuzora on Windows updates itself.

> **Before you upgrade.** On macOS and Linux, beta.2 to beta.4 update to 0.1.0 by themselves. On Windows, a beta can't update itself, and the Tauri app (v0.0.19 and 0.1.0-beta.1) doesn't offer 0.1.0. In both cases, download the installer by hand. The Tauri app's settings and data don't carry over. See the installation notes below.

### Changed

- **Files open in a tab of their own.** Clicking a file in Files, a search hit or a file you just made opens it in a new tab after the current one. A file that is already open is shown where it is, also when its tab was restored after a restart. To split the current tab as before, right-click the file and choose Open to the Side. Renaming an image in Files replaces its pane under the new name.
- **Up to date with Herdr 0.1.15.** Terminal rows are as tall as in Ghostty, so more lines fit in the same window. On macOS, files and screenshots can be dragged into a terminal. A terminal printing a lot of output no longer freezes the whole window. With many tabs, the tab bar scrolls to the active tab. The Spaces sidebar groups worktrees right away in large repositories.

### Added

- **Edit and save files in a Space on another machine.** Files in a remote Space are no longer read-only. A save replaces the file in one step and keeps its permissions and owner. If the file changed on that machine after you opened it, you get a conflict instead of an overwrite. Saving needs `sh` on that machine, and herdr-core 0.1.19 or newer on this machine and on that one. A file over 8 MiB can't be saved there, and a file of 8 to 16 MiB opens read-only. A file also opens read-only when a save can't keep its owner or group, or when you can write it only through an ACL or a supplementary group.
- **New file, new folder and rename in a Space on another machine.** The Files sidebar's toolbar, right-click menu and keys create and rename there as they do on this machine. A name that a file, a folder or a link already has is refused with "already exists", and nothing is replaced. A rename that only changes case works on a file system that ignores case. New files and folders get the permissions that the machine's own `touch` and `mkdir` give. A document open at a renamed file, or under a renamed folder, moves to the new path with its unsaved edits, and the next save goes there. Deleting on another machine isn't available yet.
- **Copy from a Git diff.** Click a row to select it, and Shift-click another row to extend the selection. Cmd+C (Ctrl+C on Windows and Linux) copies the selected lines as they are in the file, with tabs and trailing spaces, and without line numbers or `+` and `-`. In a side-by-side diff, the selection belongs to the side you clicked. Right-click a row for Copy, Copy hunk and Copy as patch.
- **Select diff rows by dragging.** Press on a row's text and drag across other rows to select them, on the side where you pressed. Holding the pointer near the top or bottom edge of the list scrolls it. Cmd+C copies the selection.
- **Dialogs, popups and toasts fade in.** Dialogs and popups fade in and move 6 px into place over 120 ms, and toasts over 160 ms. The right dock's selection slides to the mode you pick, the resource bars move to their new length, and settings switches slide. With the system's Reduce motion setting on, everything appears at once. On macOS and Windows, Yuzora reads that setting only when it starts. On Linux it comes from the desktop portal, and a session without one always plays the animations.

### Improved

- Files: switching to a Space no longer shows "Loading files…" first. A folder you have seen shows its last tree at once, with its open folders and selection, and is read again in the background. A folder you haven't seen lists its files before Git answers. With a simulated remote machine that adds 100 ms to every call, switching back to its Space showed the tree in under 130 ms, against 927 to 968 ms before (debug build, screen sampled about every 60 ms).
- Git: the Git mode follows a switch of project in the next frame. It used to check once a second and show an empty "Reading the repository…" in between. A repository you have seen shows its last state at once, and `cd` inside the same repository no longer resets the list. Switching between two local projects you had seen took 82 to 101 ms over 7 switches, down from 153 to 1047 ms over 14 (debug build, screen sampled about every 60 ms). The status bar's branch also follows at once instead of every 2 seconds.
- Database: opening a table shows its rows without waiting for the table's description, and reads that description once per open instead of twice. Over a link with 50 ms round trips and the connections already open, the first rows of a 500-row table showed in 116 ms on PostgreSQL and 67 ms on SQL Server, against 341 ms and 354 ms before (debug build, medians). On a database with no latency you won't notice a difference. Stop pressed before the table's key is read ends the load at once and leaves the rows read-only. A table whose columns changed while it was being opened also opens read-only.

### Fixed

- Files: the right-click Rename…, New file… and New folder… open their name box at once instead of showing "This view could not be re-rendered". When a new file, a new folder, a rename or a move to the trash fails, the notice starts with the cause, for example "notes.txt already exists".
- Git: every button acts on the repository it was drawn for. Each repository keeps its own commit message, author and amend, so a message typed in one repository is no longer committed in another. A Discard you were asked about is cancelled when you leave. Yes reads Git again first and deletes nothing if the change is no longer the one you were asked about. For mod authors, `messages()` in `yuzora:git` no longer returns `authors`.
- Git: with several projects open, Graph and the diff, merge and rebase panes stay in the Space you are in. They used to jump back to the project that opened one first. Each Space keeps its own. After updating, a diff request left over from beta.4 no longer shows another project's file.
- Git: the side-by-side diff gives each side half the width. Drag the divider to set the split anywhere from 15% to 85%, and double-click it to go back to half. One split is shared by every diff and kept after a restart. A long line stays on its own side, both sides scroll sideways together, and a scrollbar appears at the bottom when a line is too wide.
- Git: a side-by-side diff that is read again, after a save, a stage, a commit or a fetch, keeps its sideways scroll position. If Yuzora was once too busy to measure a long line, the sideways scroll range no longer stays too wide.
- Database: table edits save. Rows can be edited as soon as they are read, without waiting for the row count, and a save no longer waits behind the count. Cmd+S (Ctrl+S on Windows and Linux) saves, also while the cell editor is open. A save that waits on a connection or on a row lock held elsewhere can be stopped with Stop, and the message says whether anything may have been saved. The toolbar's Stop stops only its own statement, never a save in progress. The grid says why a table can't be edited, a failed save's full server message can be expanded, and Discard and Write back stay visible in a narrow pane.
- Database: on SQL Server, a trigger that returns rows no longer makes a save roll back, and the first save after the server closed an idle connection reconnects and goes through. A table with a `geography`, `geometry` or `hierarchyid` column shows an error instead of loading forever. On PostgreSQL, Stop goes to the server the connection is on, not to the address the host name points to when you press it. Text an input method is still composing is no longer saved when you click Write back or leave the cell.
- Database: on SQL Server, a `datetime` or `smalldatetime` cell saves a year-first date, such as `2024-01-06 10:00` or `20240106`, as typed under any login language. A date that Yuzora can't read for certain is refused before anything is sent. PostgreSQL tables are read and saved with `datestyle = ISO` and `extra_float_digits = 3`, so a `timestamptz` read with a zone abbreviation such as `CST` is no longer written back hours off, and a row whose key is a floating-point number is found. The SQL tab keeps the server's own settings, as `psql` does.
- Database: clearing a cell of a type that has no empty value (numbers, booleans, dates and times, `uuid`, `inet`, `json`) saves NULL, and the cell shows NULL before you save. SQL Server used to save 0 or 1900-01-01, and PostgreSQL mostly refused. Text cells still save an empty string.
- Database: a table is read and saved on a connection of its own. `SET` statements, temporary tables and `USE` in the SQL tab no longer change how a table is read or saved. A table on a SQL Server that opens transactions by itself (`IMPLICIT_TRANSACTIONS`) now saves.
- Database: picking another table in the sidebar while a table is open no longer shows "This view could not be re-rendered" or stays on "Loading…". The pane switches to the table you picked, and asks first if it has unsaved edits.
- Database: in a table pane's SQL tab, Run and Cmd+Enter no longer do nothing while the table is still being read, for example while it waits on a transaction the SQL tab left open. Run stops that read and runs the SQL, so a COMMIT takes one press. A statement of your own that is still running isn't stopped, and the SQL tab says to press Stop first. Unsaved edits, including a save in progress, are asked about first.
- Database: pressing Stop on a table save before Yuzora has sent COMMIT now always saves nothing, also when the app was too busy to notice the Stop in time. It used to save and then say the database may have saved the changes. On SQL Server, browsing a database's objects or opening a table no longer fails with "deadlocked … Rerun the transaction" (Msg 1205) while another session creates or drops tables. Yuzora reads again instead.
- Windows: Yuzora can update itself when you start it from its Start menu shortcut. Updates used to fail whenever it was started that way. If an update can't be installed, Settings › Updates says why and what to do, and Restart shows a notice that opens that page. If you are on a beta on Windows, read the installation notes below first.
- Updates: the update notice says `Yuzora <version> is ready` instead of `Herdr <version> is ready`. A beta that updates to 0.1.0 still shows the old wording.

### Known limitations

- Files: switching back to a Space shows its tree from the top. The scroll position isn't kept.
- Files: in a remote Space, the Files sidebar still reads Git status with four calls to the machine, one after another, so the Git marks show up after the tree.
- Git diff: Copy as patch writes every file with mode 100644, so a new executable file or symlink applies as a regular file. For a file with bytes that aren't UTF-8, the patch may not apply even though the notice says it was copied. With `diff.renames=copies`, a copy whose source also changed is written as a rename. When lines are added to a CRLF file whose last line has no line ending, the added line ending is LF.
- The Settings dialog, the welcome screen, What's new and the dock's drawer window appear without fading in, and nothing animates when it closes.
- Database: an open transaction in a pane's SQL tab can hold up that pane's table. On SQL Server, the table waits until you press Stop. On SQLite, the save fails after about 5 seconds. Press Stop, then run COMMIT or ROLLBACK in the SQL tab.
- Database: a pane now uses up to three connections: the SQL tab, the table, and the row count while it runs. On a login with a tight connection limit, the SQL tab or the table can fail with "too many connections".
- Database: a value that the server changes when it saves, by rounding or by a collation, still shows in the grid as you typed it.
- Database: if a connection's password isn't saved, the sidebar says it is missing after a restart instead of asking for it. After you reconnect, a pane that was open needs Run to load.
- Database: connecting through an SSH tunnel isn't supported yet. A connection set up with a tunnel returns an error and never connects directly.
- Database: if a column is renamed, or two columns swap names, after a table was opened and before you save, the save can go to the wrong column. Refresh the table before you edit it when its columns may have changed.
- Database: on SQL Server, a `rowversion` column and a temporal table's visible period columns look editable. The server refuses a save that changes one.
- Database: not checked on Azure SQL, with Always Encrypted columns, with CLR user-defined types or with memory-optimized tables.
- Dependencies: `tokio`, the three `zstd` crates and `rusqlite` were raised to the versions their advisories name. `rusqlite` 0.41.1 bundles SQLite 3.54.1 (RUSTSEC-2026-0459). `rsa` 0.10.0-rc.18 (the Marvin timing advisory, RUSTSEC-2023-0071) and `crypto-bigint` 0.7.5 stay as they are. No fixed release is recorded for them. SSH uses them for RSA keys and for hosts that offer only RSA.
- Files: a save on another machine that runs out of time can leave a folder named `.yuzora-save-<hex>` beside the file, with the new content inside. Files and search don't list it, but `git status` does, and `git add -A` adds it. Yuzora doesn't remove it. Once no save to that folder is running, delete it on that machine with `rm -r`.
- Files: a tab keeps the name its file had when it was opened, also after the file is renamed. A file renamed while its tab was open shows "Can't open this file" in that tab after a restart. Open it again from Files.
- Files: renaming a folder on another machine whose `mv` is GNU coreutils without `--no-copy` (8.32, as on Ubuntu 22.04) needs `perl` there. Without it, a rename that the file system can only do by copying leaves a copy under the new name. Yuzora answers "already exists" or says the folder was copied or replaced, and the old folder may still be there.

### Installation notes

- On macOS and Linux, installs of beta.2 to beta.4 update themselves to 0.1.0. They check 10 seconds after Yuzora starts and then every hour, and apply the update the next time Yuzora restarts.
- **Windows, if you are on a beta (0.1.0-beta.2 to beta.4):** the beta cannot update itself on Windows, so close Yuzora, download `Yuzora-windows-x64-setup.exe` from this release and run it over your install. (With the beta still open, the installer may not be able to replace its files.) Then restart your PC once. The beta started a background program (herdr-core) that still holds Yuzora's install folder, and that stops the first automatic update (0.1.0 to the next) until it is gone. Restarting ends it, and your terminals with it. If you would rather not restart, run `hc server stop` in a terminal outside Yuzora instead: it ends every terminal and the agents running in them, the same as a restart would. From 0.1.0 on, Yuzora updates itself.
- The update list of 0.1.0 leaves out Windows on purpose. A beta on Windows can't install an update, so with a Windows entry it would show an update that never installs. Install 0.1.0 by hand as described above.
- The Tauri app (v0.0.19 and 0.1.0-beta.1) doesn't offer 0.1.0. Download and install 0.1.0 yourself. It is a different app, and the Tauri app's settings and data don't carry over: set up database connections, SSH hosts and settings again. The Tauri app's files stay where they were, so you can go back by installing it again. The new app has no Browser, Companion or AgentZone, and its editor has no LSP. On Windows, the installer offers to remove the old all-users install.
- The macOS app supports only Apple Silicon. Download `Yuzora-macos-aarch64.dmg`. The app has only an ad-hoc signature, with no Apple Developer ID signature or notarization, so Gatekeeper blocks the first open. Go to "System Settings › Privacy & Security" and click "Open Anyway", or run `xattr -dr com.apple.quarantine /Applications/Yuzora.app`. Do this only for a DMG you downloaded from this repository's Releases. After an update, macOS may ask again for microphone, camera or folder access.
- On Windows, `Yuzora-windows-x64-setup.exe` is a per-user installer that needs no admin rights. Authenticode isn't enabled, so SmartScreen may show a warning. Choose More info › Run anyway.
- Linux (experimental) gets `yuzora-0.1.0-linux-x86_64.tar.gz` and `yuzora-0.1.0-linux-aarch64.tar.gz`. Unpack one and run `yuzora/install`.
- This release was checked by hand on a real Mac. On Windows and Linux, it was checked only by automated runs on CI runners (on Windows, installing, updating twice and uninstalling; on Linux, running `install`). Nobody tried it on a real Windows or Linux machine.
- Requires herdr-core 0.1.11 or newer. If it isn't installed, the first screen offers to install it. Saving a file on another machine needs herdr-core 0.1.19 or newer on both machines. With an older one, the save is refused and says to update it.

## [0.1.0-beta.4] - 2026-11-10

The tools move to a right-hand dock, the left side becomes Herdr's own Spaces and Agents, and the official herdr-app is synced to v0.1.14.

### Added

- **Right-hand dock.** A layout with a side panel (main, and the default yuzora) gets a 40px mode bar next to the panel with Files, Git, Database and SSH. A layout without one (terminals, win98, your own) gets a drawer on the right edge, and a mode you click opens in a window. The open mode stays put when you switch Spaces, and changing the layout no longer makes Yuzora's tools disappear.
- **Herdr's own Spaces and Agents on the left.** The default yuzora layout now uses Herdr's native Spaces and Agents lists (the top left reads "Herdr"), and Files, Git, Database and SSH moved to the right-hand dock. The Space color set in Yuzora shows on the avatar in the left list. If you edited `yuzora.json`, your copy is kept as `yuzora-edited`; an unedited one is replaced automatically.
- **A dock setting for each layout, and one for startup.** Settings › Mods › Layouts lets each layout set its right-hand dock to Auto, Drawer or Off, which takes priority over the layout file's own setting. Settings › General › When Yuzora opens picks whether the dock opens in Files, Git, Database or SSH, or stays collapsed.
- **`Cmd+Alt+B` (macOS) / `Ctrl+Alt+B` toggles the right-hand dock.** An unedited beta.2 or beta.3 `keys.json` is updated to the new defaults automatically (this includes Herdr's terminal search, Cmd/Ctrl+F). An edited one is left alone: add `"cmd-alt-b": "yuzora-modebar.toggle"` (or `ctrl-alt-b`) yourself.
- **Synced with the official herdr-app v0.1.14.** A herdr-core connection error now shows its reason and a retry, terminal search (Cmd/Ctrl+F) and scroll commands, a scrollbar on panes, `pane.swap` for mods, and Add machine now tells you the command to run. The herdr-core requirement is unchanged (0.1.11 or newer, generation 1). One step of v0.1.14 is left out: after an update it downloads Herdr's core installer and runs it, to turn a herdr-core that is still a single executable into the Herdr Core app. Yuzora doesn't run a downloaded script by itself; if you want that, run the official herdr-core installer yourself.

### Fixed

- Git: when an open repository's `.git` goes missing, Git commands (fetch, push, status and the rest) no longer fall through to a repository around it and run there. `GIT_CEILING_DIRECTORIES` stops the search; where the path above contains `:` or `;` and no ceiling can hold, each command first asks Git which repository it found and is refused unless it is this one. Hooks get no extra `GIT_DIR`.
- Git Log: a very long commit message can no longer be cut off by the render time limit on a slow machine (each paragraph is now a single text element).

### Known limitations

- In the drawer window, Files' "show hidden files" setting resets every time the window opens.
- The screenshots in the README still show the old left side; they will be retaken.

### Installation notes

- beta.3 does not update itself to beta.4 (a beta build only updates to a stable release). Download beta.4 by hand from the `v0.1.0-beta.4` Pre-release on GitHub and install it over beta.3.
- The macOS app supports only Apple Silicon and has only an ad-hoc signature, with no Apple Developer ID signature or notarization, so Gatekeeper blocks the first open. Go to "System Settings › Privacy & Security" and click "Open Anyway" (or run `xattr -dr com.apple.quarantine /Applications/Yuzora.app`).
- Windows uses a per-user NSIS installer that needs no admin rights, `Yuzora_<version>_x64-setup.exe`. Authenticode isn't enabled yet, so SmartScreen may show a warning.
- Linux (experimental) provides `yuzora-<version>-linux-<arch>.tar.gz` for x86_64 and aarch64. Unpack it and run `yuzora/install`. It has only passed CI and hasn't been tried on a real machine.
- The known limitations of beta.2 still apply.

## [0.1.0-beta.3] - 2026-11-05

Fixes for beta.2, mostly on Windows, and faster Git and Files on large folders and remote machines.

### Changed

- **The whole app interface is now English only, and the language setting is removed.** A language or locale you saved earlier (Chinese, for example) causes no error; the app just shows English. The README, the changelog and the release notes still come in both English and Traditional Chinese. beta.2 was the last version with a Chinese interface: from beta.3 on, all text in the app, including the update notes in What's new, is English only.
- The tab bar no longer has the "New pane ▾" menu or the two split buttons, and the sidebar no longer has "Next layout". The `layout.switch` default shortcut (⌘⇧L / Ctrl+Shift+L) is gone and Next layout is hidden from the command palette. The new pane menu now opens from a shortcut you bind yourself, using `yuzora-shell-tabs.new-pane` (a dot, not a colon), or with `yuzora do yuzora-shell-tabs.new-pane`. After upgrading, the old default binding in `keys.json` is skipped.

### Added

- When a Space holds more than one repository, the top of the Git sidebar lets you pick which one to show. It follows focus by default or can be pinned, and each Space remembers its own choice. Finding repositories only looks at the file system and runs no Git; the chosen repository goes through the usual trust check. The command is `yuzora-sidebar-git.pick-repo`.
- The Git Log detail can be scrolled and its top/bottom split dragged to resize; double-click to restore the default. The detail now shows the full commit message (up to 2000 lines).

### Improved

- Checking whether a folder needs trust takes one Git call instead of two (one round trip to a remote machine instead of two), and the Changes panel checks once per refresh instead of twice.
- The Files sidebar and panels no longer reload on macOS and Windows every time a build writes into an ignored folder such as `target/` or `node_modules/`.
- On Windows, writing into an ignored folder no longer makes the panels reload once per build batch because of the folder itself; creating or removing the folder still shows up.
- The status bar's Git look takes one round trip on a remote machine instead of three.
- On a remote machine, opening a diff takes 2 round trips instead of 5 (unstaged) or 4 (staged), and the Git log reads its refs in 1 instead of 3: reads that don't depend on each other now go out together.
- Running programs on this computer (Git, search, scripts) no longer waits for polling: a `git --version` call dropped from about 52 ms to 29 ms on Windows and from about 53 ms to 8 ms on macOS.
- An open Git diff no longer reloads whenever anything in the repository changes, only when its own file (or the one it was renamed from) may have.
- Git: on this computer (macOS and Linux), in a folder not trusted yet, opening a diff and reloading Files no longer scan the repository again each time; the answer is reused until a file it came from changes. Folders on a remote machine or on Windows, repositories with submodules, an `include` in their config or a split or sparse index, and any on a FAT, exFAT or network drive are still scanned each time.

### Fixed

- Windows no longer pops up a stream of console windows while Yuzora runs Git, file and remote commands.
- The tab bar lists only the current Space's tabs again, as Herdr does, instead of every Space's tabs.
- Git: in a side-by-side diff, each side is now as wide as its own longest line, so a new file's lines show right away instead of sitting past an empty left column.
- Git: trusting two folders at the same moment keeps both trusted, and a Git command that runs while a folder is being trusted no longer reads the list half written.
- Git: if a folder's Git config gains a program after Yuzora last checked it (a Git LFS filter, for example), the Changes panel's diff and Commit and the worktree dialog now check trust again right before they run Git, and the Files panel's Git marks and search never fetch, even with a Git older than 2.44.
- Windows: the installer's prompt to remove an older all-users Yuzora now notes that your old data stays, but connections and settings are not brought over automatically yet.
- Windows: SFTP "Edit" no longer closes Notepad after 30 seconds, and Settings can move templates to the Recycle Bin and show files in Explorer.
- Windows: the editor, Markdown and image panes now reload when the file changes on disk, and the SFTP local panel shows `C:\…` paths.
- Git sidebar: the +/− counts and the buttons are no longer covered by long file names, and 6-digit line counts are shortened to `150k`.
- The `herdr.d.ts` written into the app home showed a mod command shortcut with a colon (`mod:command`); following it made the whole `keys.json` invalid. It now shows a dot.

### Installation notes

- beta.2 does not update itself to beta.3 (a beta build only updates to a stable release). Download beta.3 by hand from the `v0.1.0-beta.3` Pre-release on GitHub and install it over beta.2.
- The macOS app supports only Apple Silicon and has only an ad-hoc signature, with no Apple Developer ID signature or notarization, so Gatekeeper blocks the first open. Go to "System Settings › Privacy & Security" and click "Open Anyway" (or run `xattr -dr com.apple.quarantine /Applications/Yuzora.app`).
- Windows uses a per-user NSIS installer that needs no admin rights, `Yuzora_<version>_x64-setup.exe`. Authenticode isn't enabled yet, so SmartScreen may show a warning.
- Linux (experimental) provides `yuzora-<version>-linux-<arch>.tar.gz` for x86_64 and aarch64. Unpack it and run `yuzora/install`. It has only passed CI and hasn't been tried on a real machine.
- The known limitations of beta.2 still apply.

## [0.1.0-beta.2] - 2026-11-04

Yuzora is rewritten as a native app built on [herdr-app](https://github.com/herdrdev/herdr-app) (Rust, GPUI, libghostty, QuickJS mods). Terminals and agents now run in herdr-core (`hc`) and keep running when you close the window. The interface is redesigned.

### Added and improved

- Workspace sidebar: Spaces / Agents tabs, a scope chip for each machine (an LED shows the connection state), a "Needs attention" section, and a tree of machine → Space → worktree → agent. An agent's state is shown by both color and shape (working, needs a reply, done, idle).
- A new Space starts by picking a folder (on a remote machine, you type the path and it is checked on that machine). The Space is named after the folder. If a Space already exists for that folder, Yuzora switches to it.
- Mixed splits: "+ New pane" puts a tool pane, a terminal or an agent to the right of or below the current pane, and they can be mixed in one tab. Each tab in the tab bar carries a Space badge and a state. When an agent needs a reply, the pane border and header turn amber.
- The status bar shows the number of panes, Spaces and agents, the number that need a reply, the current branch with ↑↓, and the HERDR version. The bottom-right corner shows the CPU and memory that Yuzora uses on this computer (the app, the terminals and agents in herdr-core, and the tools Yuzora starts). Click it to expand the details.
- Git is modeled on IntelliJ IDEA Community and uses the staging area: changes, staging and commit in the sidebar; a diff that stages hunks and lines, which opens in a dialog whose Keep button moves it into a pane; the commit log with its branch graph; branches; push; three-way merge; interactive rebase; annotate; and stash, tag, remote, worktree, submodule and patch. In a folder whose own Git config runs programs, Yuzora runs no Git that could start them (in Git mode, the Files tree or the Spaces list) until you trust the folder. (Until you trust a folder, the reads Yuzora runs there on its own stay offline. The background fetch, and a fetch, pull or push you start yourself, use only Git's built-in transports, never a remote helper, and in a folder whose config runs programs they wait for trust too. The background fetch runs no hooks; Git you start yourself, a commit or a push for example, still runs ordinary hooks.)
- Files and editor: a file tree that follows the current pane's repo, with Git marks and file icons, and that creates, renames, moves to the trash, copies paths and reveals in Finder; file-name and content search; a code editor with tree-sitter highlighting for 14 languages (it checks for conflicts before saving, has find and replace, picks up changes on disk right away and tells you when an agent edits the same file); a Markdown preview that follows the editor as you type; and an image and SVG viewer. A repo's SVG is drawn in a separate process with time and memory limits. If that fails, the image pane shows a placeholder and the reason, and the Markdown preview shows the placeholder.
- Databases: PostgreSQL, SQL Server and SQLite connections (passwords are stored in the system keychain), a database picker and a schema tree. The table pane has Data (a virtual-scrolling grid, sorting, copying, and editing cells that are written back in a single transaction), Structure (columns, indexes, foreign keys, DDL) and SQL tabs. The SQL pane runs queries with several result sets, can cancel them and keeps a query history.
- SSH / SFTP: lists hosts from `~/.ssh/config`, Yuzora's own hosts and herdr machines, with connection state and latency. On first connection, the host key is confirmed by fingerprint, and a changed key triggers a warning. The side-by-side SFTP pane supports drag and drop, cancel, retry and resume, and keeps permissions and times. A remote file can be opened in a local text editor, and each save uploads it automatically.
- Settings: a "General" page (language, theme, font and size, zoom, terminal clipboard writes) and a "Yuzora" group that lists each feature's settings page. The Privacy page states that Yuzora does not collect or send any data.
- Agent templates: manage the herdr-core templates of each machine in Settings (edit in a form, check before saving, move to the trash), and start one in a chosen Space and tab.
- Mods management: lists shipped, edited, cloned and personal mods and layouts with their errors. You can clone, check, switch or reset a layout, and install from a folder after a trust warning is shown. Yuzora's own interface is made of mods, so you can clone and change it.
- Dark and light themes (yuzora-dark, yuzora-light), following the system by default. The interface is available in English and Traditional Chinese.
- Sharing herdr-core with the official Herdr: the version-mismatch screen uses Yuzora's name and explains that the core is shared. If herdr-core restarts as a version Yuzora can't use, Yuzora does not connect to it and keeps the reason on screen. It reconnects automatically once the core is fixed.
- Updates now come from this repo's GitHub Releases: Yuzora checks every hour, only installs an update whose manifest signature and SHA-256 both check out, and applies the update on restart. A beta build only updates to a stable release (0.1.0 and later), never to another beta.
- No usage data is sent anymore. Feedback opens a GitHub issue.
- The command-line program is called `yuzora` and doesn't overwrite the official Herdr's `herdr-app`.

### Removed

- The Browser, Companion (phone remote control) and AgentZone are not in beta.2. The editor has no LSP.

### Known limitations

- beta.1 data isn't moved over automatically: database connections, SSH hosts and settings need to be set up again. beta.1's files are left untouched, so you can still go back to beta.1.
- Git: the mode bar's Git change count and the status bar's Git summary start updating only after you open Git mode once. Repos on remote machines have no file watching, so changes made outside Yuzora show up after about 25 seconds. Trusting a folder only covers the Git that Yuzora runs itself; what runs in a terminal, agents included, isn't affected.
- Databases can't connect through an SSH tunnel yet. Files on remote machines can only be browsed and opened read-only in this version.
- A beta version has no update channel setting: it only updates to a stable release. (A stable version can take pre-releases too through `update.channel` in `~/.config/yuzora/yuzora/settings.json`; the Settings page has no option for it yet.)
- When the app home already has a layout (for example, if you used a development build), new interface elements (such as the resource usage in the bottom-right corner) aren't added automatically. Reset the layout for them to appear.
- When used together with Herdr, closing an adopted Space, tab or pane in either app ends its terminal in herdr-core, so it closes in the other app too. While either app is open, the core restarts within one second after `hc server stop`.
- Windows remote templates, trash and SSH agent, and the behavior of sharing the core with Herdr on Windows and Linux, have only passed CI builds and tests. They haven't been verified on real machines.

### Installation notes

- The macOS app supports only Apple Silicon and has only an ad-hoc signature, with no Apple Developer ID signature or notarization, so Gatekeeper blocks the first open. Go to "System Settings › Privacy & Security" and click "Open Anyway" (or run `xattr -dr com.apple.quarantine /Applications/Yuzora.app`). After each update, macOS may ask again for microphone, camera or folder access.
- Windows uses a per-user NSIS installer that needs no admin rights, `Yuzora_<version>_x64-setup.exe` (installs to `%LOCALAPPDATA%\Programs\Yuzora`), and no longer ships an MSI. After installing, it asks whether to remove the beta.1 all-users install. Authenticode isn't enabled yet, so SmartScreen may show a warning.
- Linux (experimental) provides `yuzora-<version>-linux-<arch>.tar.gz` for x86_64 and aarch64. Unpack it and run `yuzora/install`. It has only passed CI and hasn't been tried on a real machine.
- Requires herdr-core 0.1.11 or newer. If it isn't installed, the first screen offers to install it.
- Coming from 0.0.x or 0.1.0-beta.1: download and install beta.2 yourself. The old app's updater doesn't offer beta.2; updating to the new app from inside the old one comes with 0.1.0 Stable.

## [0.1.0-beta.1] - 2026-10-26

### Added and improved

- Adds an experimental Companion (phone remote control), off by default; turn it on in "Settings → Companion". A paired phone can view HERDR agents, browse Spaces, and view Git status, history and diffs read-only. The phone can send input only after the desktop grants control permission. When an agent is waiting for input or finishes, the phone can receive a Web Push notification (you must add it to the Home Screen first; the notification doesn't contain terminal output).
- Yuzora includes a built-in Tailscale node, so this computer doesn't need the Tailscale app. The phone needs Tailscale installed and signed in to the same account. Before using it, enable HTTPS certificates on the tailnet. The node name and the tailnet name are publicly recorded in the Certificate Transparency log.
- The phone pairs by QR code. After both sides check the confirmation code, a passkey is registered, and every later connection must pass the passkey. The desktop grants control permission one device at a time (15 minutes, 1 hour or 8 hours), and the phone must confirm again with its passkey. You can revoke a single device at any time. The enabled period can be 1 hour, 8 hours or until you quit Yuzora.
- Input and takeover from a phone show a notification on the desktop and are recorded in Logs. Writing input to a HERDR terminal now has a time limit: if the other side stops reading, the write is aborted with a notice instead of waiting forever.
- The bundled HERDR is upgraded to 0.9.5.
- Open files and folders from the system: choose Yuzora in Finder's "Open With", drag a file or folder onto the Dock icon, use `open -a Yuzora`, or launch with command-line arguments on other platforms, and it opens right in Yuzora. A file opens as a tab. A folder switches to the local Space with the same root directory, or creates a new Space if there isn't one. Opening again while Yuzora is already running also goes to the same window, and the workspace trust prompt and the unsaved-changes confirmation appear as usual. On Windows, a second launch is handed to the already-running Yuzora.
- macOS: Yuzora appears in the "Open With" list for text, images and folders. The nine extensions that no other app claims (`.rs`, `.go`, `.jsx`, `.kt`, `.vue`, `.svelte`, `.scss`, `.lock` and `.env`) open with Yuzora by default. The default app for other file types doesn't change.
- The editor can preview PDFs directly: a `.pdf` tab no longer shows a binary notice but a scrollable PDF viewer, for local and SSH workspaces. It supports zoom (buttons, Ctrl / ⌘ + scroll wheel, fit width) and jumping to a page, and keeps the current reading position when you zoom, resize the pane, or the file is rewritten externally.
- Each surface keeps its own state when you switch: HERDR terminals and the Browser aren't reloaded when you switch away and back, and the page stays as it was. The scroll position and zoom of Markdown documents and source, PDFs, files and so on, and the editor's undo history, are kept when you switch tabs, Spaces or workspaces.
- On macOS, ⌘Z / ⇧⌘Z now undo and redo editor content (previously even indentation couldn't be undone). The search box, the replace field and the Markdown document editor keep the system's native undo.
- When HERDR can't reach a Session (for example a Windows server run as an administrator), Yuzora shows "Unable to connect" with the reason HERDR reported. It no longer wrongly reports "Stopped" and no longer suggests starting it with `herdr session attach`. For such a Session, start, stop and deleting the saved layout aren't offered, so a Session that is still running can't be deleted by mistake.
- Adds a customizable ⌘ / Ctrl + Shift + F shortcut that opens the command palette to search workspace file contents. The input placeholder now reads "Search file contents, run commands…". If you've already bound those keys to another function, your existing setting is kept and the new default is automatically left empty.
- Browser: if you enter a URL in the address bar and it fails to load, it shows "Unable to load …" instead of silently going back to the previous page. Entering a local or UNC path explains why instead of trying to open it as `https:///…`. Clicking the address bar still selects all, even if focus was just in the web page.
- Git: after push, pull, fetch and background auto-fetch, Git Log updates the loaded range in place. `origin/*` labels no longer go stale, and the selected commit, the details and the number of loaded pages aren't reset.
- Row numbers in database query results are continuous across pages and widen automatically with the number of digits.
- Agent Attention notifications now show the agent or tab name instead of the pane ID. When you create a Space from a folder, Yuzora remembers the Space you left and restores its original tab when you go back.
- The delete-file confirmation now shows the relative path and says the deletion can't be undone.

### Performance

- Switching back to a HERDR terminal or the Browser no longer recreates the view. A terminal's mouse hover state redraws the whole view only when something is actually hovered. Heavy output from a terminal in a background tab is no longer buffered and processed in the background; the latest screen is shown directly when you switch back.
- CPU use of process resource sampling (status bar and resource usage) is much lower: a single idle sample drops from about 7.5 ms to about 0.06 ms.
- Probing login shell environment variables, SQLite paged queries and similar paths also do less unnecessary work.

### Fixed

- Editor and files:
  - When you close a split group that holds unsaved content that exists only in that group, Yuzora asks to save or discard it first instead of losing it.
  - Local saves now write to a temporary file and then replace the original, so a save that fails midway doesn't leave a truncated file.
  - For POSIX files whose names contain a backslash, delete, rename and copy no longer act on the wrong file. When rename, delete, paste and drag and drop finish asynchronously, they no longer overwrite newer content or act on a replaced workspace.
  - Images reload after being rewritten externally, and you can also press ↻ in a background tab. "Zoom to fit" no longer enlarges a large image beyond the window.
  - "Open with external program" now goes through workspace authorization instead of always failing, and shows the reason when it fails. For safety, programs and scripts aren't opened externally; you can still reveal them in the file manager.
  - The external-change conflict dialog and the merge tool no longer overwrite newer content with stale read results. On macOS, keyboard copy and paste after right-clicking in the file tree now act on the selected item.
  - Collapsed folders in the file tree are listed again after files change. After you edit `.git/info/exclude`, file-name search applies the new exclusion rules immediately.
- Git:
  - Unstaging a staged rename no longer leaves a half-applied index state, and staging no longer reports false errors.
  - Force push's lease now targets the remote commit you see, not the tracking branch after an auto-fetch, so it won't force-push over a commit the branch has never integrated.
  - The merge tool refuses to handle conflicts in non-UTF-8 files, UTF-16 files with a BOM, or symbolic links, instead of rewriting them as UTF-8 or writing through the link. The position where a chunk is applied in CRLF files is also fixed.
  - When the upstream branch has been deleted, Yuzora no longer rewrites the tracking setting automatically or pushes to the wrong remote branch. When a branch name is the same in the heads and remotes namespaces, the Tracked label and remote selection are no longer mixed up. When a Git root path has trailing spaces, Git no longer acts on another repo with a similar name.
  - Queued writes to the same repo no longer take all the execution slots and slow down other reads and other repos. When Yuzora quits, no Git operation starts after cleanup.
  - The status bar no longer shows `main` in a non-repo folder or while loading. The merge tool's title bar avoids the macOS traffic-light buttons.
  - Stash apply now locks the exact commit, so it won't apply a different entry if another Git tool changes the stash list at the same time.
- Databases:
  - "Run current statement" no longer runs the next statement by mistake when the cursor is in a trailing comment. Semicolons inside PostgreSQL dollar-quoted strings are no longer split incorrectly.
  - Transaction state (open transactions) is tracked per connection and is cleared correctly after a SQLite or MSSQL disconnect. When MSSQL returns several result sets, it now reports an error instead of mixing them into the first result set.
  - Multi-row table edits are atomic and leave no partial update when they fail. SQLite generated columns are no longer treated as editable. The column cache refreshes after the table list reloads.
  - Result sorting is correct for mixed types, infinite values and timestamps with milliseconds. Query history is no longer truncated and then replayed as if it were complete SQL, and MSSQL batch replay no longer merges later statements into the previous batch.
  - Races in canceling queries, switching result pages and disconnecting are fixed. When the connection drops, a cell edit shows its result instead of disappearing. In the cell editor, ⌘ / Ctrl + A selects only the cell's text.
- SSH / SFTP:
  - When a key needs a password, Yuzora asks for the passphrase. Opening SFTP and disconnecting no longer block each other, and SFTP recovers automatically after its underlying connection stops.
  - The overwrite confirmations for multi-file uploads no longer cancel each other. Downloads and renames aren't affected by later navigation once the destination or parent folder is chosen. The local panel refreshes after a download finishes. Double-clicking a folder goes in one level only and no longer continues into the next subfolder. The local and remote "Up one level" buttons now have different names.
  - When an SSH connection finishes, it no longer overrides a host you chose later.
- HERDR and terminals:
  - Quitting Yuzora or releasing a terminal connection no longer waits forever because another process holds the pipe (it finishes within about 1.5 seconds). No zombie processes are left after the native client closes.
  - A terminal link opens only if the matching file exists locally, so a path printed by a remote pane is no longer treated as a local file. Hovering over a UNC path no longer probes the network host.
  - An event-triggered snapshot update is no longer dropped by an update already in progress, and Attention for a pane that has ended doesn't come back. Compatibility is renegotiated after a Session reconnects or the server version changes. HERDR can be retried after a failed start.
  - A new Space uses the Session selected when you created it, no longer changed by an automatic switch midway. The agent switcher, recent-use order and cycling after a restore no longer pick a removed or wrong agent. Quickly clicking a pane to switch back to the original focus is no longer ignored.
  - Input in the native Session view isn't swallowed while the pane and plugin aren't ready. Chinese IME punctuation is no longer lost in native dialogs. The order of terminal paste and copy, and the paste target on reconnect, are fixed.
  - Terminals re-measure and refit after fonts load or when the screen scale (DPR) changes. An agent completion notification no longer reuses the earlier read state, and a deferred notification follows the latest notification preferences.
- Search and settings:
  - File content search results and positions are fixed for non-ASCII characters that differ in case, files with more than 500 matches, "Next" over an empty range, and preview highlighting. A search that is rejected no longer stays in the loading state.
  - The shortcut conflict check on macOS now tells Command from Control. Browser tab shortcuts match the current surface. In a narrow window, the two left-sidebar shortcuts are mutually exclusive so they don't squeeze the screen at the same time.
  - After the command palette closes, focus returns to where it was. The "Settings" search box no longer pops up autofill suggestions. Log settings are written atomically, so a failed write no longer resets them to defaults after a restart, and the Logs source menu includes sources that appear later.
- Startup and security:
  - At startup, Yuzora no longer hangs and fails to start when a background process launched by the shell profile keeps holding the output pipe.
  - In an untrusted workspace, file watching no longer runs the Git commands configured by the repo. Granting or revoking trust takes effect immediately.
  - Web pages opened in the Browser from outside can no longer read the contents of files in your authorized workspaces.
  - On macOS, "Reveal in file manager" for packages such as `.app` now selects them in the parent folder instead of launching them. In "Settings → Security", "Sync external changes" and "Confirm destructive git operations" had no effect when toggled; they are now marked as always on.
  - After an update install fails, pressing "Retry" with unsaved documents now shows a notice instead of doing nothing.

### Known limitations

- The macOS Beta has no Developer ID publisher identity, notarization or Gatekeeper trust, so you may see a warning or be blocked the first time you open it. Download only from Yuzora's official GitHub Pre-release.
- Companion is experimental: it hasn't been verified on real iPhone, Android and Windows / Linux desktops.
- Enabling Companion is equivalent to granting remote shell access to this computer's account. Enable it only on a tailnet you trust and only for as long as you need it.
- Companion works only while Yuzora is running and it is enabled. After the node name changes, paired phones must pair again.
- For setup and troubleshooting, see `docs/operations.md`.
- Opening files from the system: `.ts` and `.mts` don't appear in macOS's "Open With" list because the system treats them as a video format (MPEG-2 Transport Stream). The Windows Explorer context menu and the Finder folder "Services" item aren't available yet. Dragging onto the Dock hasn't been verified on a real machine, and file-opening integration on Windows and Linux has only passed compilation.
- PDF preview doesn't yet support text selection, search, links and forms. Standard fonts that aren't embedded fall back to system fonts. When page sizes vary widely, a restored scroll position may be slightly off until the pages finish measuring.
- Browser load-failure notice: the first navigation of a new Browser session has no previous page to compare, so failures aren't detected. Windows (WebView2) doesn't report load state, so no failure notice is shown. Linux shows WebKitGTK's own error page, so the notice rarely appears.
- Force push needs an explicit push target and the branch's reflog, and refuses when they are missing instead of falling back to an unprotected force push. Atomic save replaces the file with a new inode, so hard links are broken, and extended attributes other than mode and owner aren't carried over.
- MSSQL still splits batches at every `;`, so `DECLARE @x …; SELECT @x;` loses the variable. A `BEGIN … COMMIT` within one batch keeps its transaction flag set until a later `COMMIT` fails.
- Live HERDR and Companion behavior on Windows and Linux, and the process-pipe and holder-related fixes outside macOS, have only passed CI builds and unit tests and haven't been verified on real machines. If you run the HERDR server with admin rights yourself, Yuzora can only show "Unable to connect" and doesn't handle it automatically.
- After the editor's "Undo to saved content", the unsaved dot remains. The Markdown document editor's undo history isn't kept across tabs.

## [0.0.19] - 2026-10-10

### Added and improved

- "HERDR Tools" is redesigned as "HERDR Actions". It opens with "What do you want to do?" and lists task cards for opening an isolated branch (Worktree), starting an Agent, sending a message to an Agent, moving a Pane, managing Sessions, and opening the full Session view. Each card opens a single-purpose form. Actions that are temporarily unavailable state the reason directly (for example, the Session is not connected yet, the HERDR version does not support it, or there is no Pane or Agent). The top always shows "Acts on: host › Session". When you start an Agent, the name is generated automatically, and the arguments and submit key are tucked under "Advanced". All actions can also be run from the ⌘K command palette (type "HERDR") and follow the Session selected in the sidebar. The Pane context menu offers "Start Agent" or "Send message to Agent" depending on whether the Pane already has an Agent. The notification toggle moves to "Settings → HERDR".
- Dragging files from Finder or File Explorer onto the sidebar file tree copies them into the folder under the cursor (dropped on a file, they are copied into that file's folder; dropped on empty space, into the workspace root). The target folder is highlighted while you drag. Files with the same name are renamed automatically. Local and WSL workspaces are supported; WSL needs the matching version of the Yuzora helper. You can also press ⌘/Ctrl+V on empty space in the file tree to paste files copied in Finder into the root folder. SSH workspaces do not support import yet; dropping onto the file tree still opens the file in the editor.
- When an Agent such as Claude Code runs in fullscreen mode in a HERDR terminal, mouse movement is also forwarded to the program, so hover effects display correctly (requires HERDR 0.9.3 or later; remote needs the matching version of the Yuzora helper).

### Performance

- Git: Directories ignored by `.gitignore` (for example `target/` and `dist/`) no longer trigger a Git status refresh when a build writes heavily to them, and Git status also updates faster. The file watcher queue is larger, so a build no longer overflows the queue and forces a rescan of the whole workspace.
- HERDR: The idle Session list now uses a 12-second cache, and background `herdr` subprocesses drop from about 15 per minute to about 5. Terminal output frame throughput is about 2 times what it was. Session snapshot processing time is reduced by about one third. When one Agent's status changes, other terminal tabs no longer redraw. Capability checks for different Sessions no longer wait on each other.
- Switching editor groups no longer redraws the whole sidebar file tree (CPU is about three quarters lower with 1,000 files).
- While typing, shortcut matching checks dialog state only after a key combination matches. Typing CPU is about 90% lower when the screen has many elements.
- When a Browser preview contains multiple iframes, unchanged state is no longer sent repeatedly (CPU about 89% lower with 64 iframes).
- File watching and status bar resource sampling use almost no CPU when idle. On macOS, reading the login shell environment variables at startup drops from about 54 ms to about 11 ms.
- CPU use is also lower for the terminal output queue when switching tabs, full-text search, large database query results, Git status parsing, and remote helper connections. After opening many file types that have no syntax highlighting support, cache entries no longer accumulate.

### Fixed

- Claude Code's mascot Clawd and other block characters (U+2580–U+259F) no longer look broken or show gaps in HERDR terminals: Yuzora now draws these characters directly at the cell size instead of relying on fonts.
- If a new event arrives while a HERDR snapshot update is in progress, one more update runs right after it finishes. You no longer wait for the next event or for the periodic sync 12 seconds later.
- Text in some dialogs (for example "Add Herdr Session") is no longer blurry: dialogs now use layout centering, so content with a fractional height no longer lands on half a pixel.

### Known limitations

- In Claude Code's default (non-fullscreen) mode, mouse reporting is not enabled, so Clawd can't be clicked in any terminal. Run `/tui fullscreen` in Claude Code.
- HERDR Sessions started or stopped outside Yuzora show up in the sidebar after up to about 16 seconds. Changes made inside Yuzora update immediately.
- The macOS app supports Apple Silicon only. It is not signed with an Apple Developer ID or notarized, so a Gatekeeper prompt can still appear on first launch.

## [0.0.18] - 2026-10-08

### Added and improved

- Adding a remote HERDR Session now uses HERDR's native machines: "Add Session" gets a new Machines tab where you can add a saved SSH host from Yuzora (the official setup flow runs in an embedded terminal), view status, rename, disable, and remove it. The live view of a remote Session opens in the official client, and the sidebar Agents list also shows Agent status on each host. The previous way of adding a remote HERDR through a Yuzora SSH connection is marked as a legacy path and can be converted to a HERDR machine in one click. Requires HERDR 0.9.2 or later.
- Settings → HERDR: Errors are now readable explanations, with diagnostic details you can copy. After switching the local source, you can restart Yuzora right away. When a custom path stops working, you can switch back to the bundled version in one click.
- Clicks, drags, and double-clicks in a HERDR terminal are forwarded to programs that have mouse reporting on (for example Claude Code, `vim` with `mouse=a`, lazygit). A regular shell still uses local selection. Hold Shift to force local selection, and ⌘/Ctrl+click to open links. Requires HERDR client 0.9.2 or later; remote (SSH/WSL) needs the matching version of the Yuzora helper.
- The sidebar Agents view is redesigned: below each Agent's title, labels show its folder and Git branch in order (the folder uses the custom project name, a linked worktree shows the main repo, and a non-Git folder shows only the folder). This replaces the branch-only small text used since v0.0.17. The Agent switcher subtitle matches.
- Smart checkout now also handles a checkout blocked by untracked files (the target branch tracks the same path): only the blocking files are stashed, and your version is written back after the switch and appears as modified in the Git panel. The version committed on the target branch can still be restored. Other untracked files and build artifacts are not affected.
- Windows ships only the MSI installer: the download page and README now point to `Yuzora-windows-x64.msi`. After updating from the NSIS (`.exe`) version to this version, Yuzora removes the uninstall entry and duplicate shortcuts that NSIS left behind on first launch. "Add or remove programs" then shows only one Yuzora, and settings and data are kept.
- Settings → Appearance adds "Background": follow the theme color, use a custom gradient (similar to Arc: drag up to 3 color points on the palette and adjust the intensity, with 8 built-in gradients and up to 12 custom palettes; you can also extract the main colors from an image to generate a gradient), or use an image directly as the background (image intensity is adjustable; the image is downsized and stored locally, and the original file is not modified). The default still follows the theme color, so the existing look is unchanged.
- New "Glass window": when on, the blurred desktop shows through the window background, the sidebar and middle area become translucent, and you can adjust the background opacity. Editor text and terminal content stay opaque. macOS uses the system under-window material; Windows needs Windows 11 22H2 or later (Acrylic). Older Windows versions and Linux do not show this option.
- File, Browser, and HERDR terminal tabs can be dragged to reorder, and file and terminal tabs can also be dragged to another editor group. Columns in database query results can be dragged to reorder.
- Dragging files from Finder or File Explorer into a HERDR terminal pastes the escaped path, which makes it easy to hand a screenshot or file to an agent such as Claude Code. WSL Sessions convert it to a `/mnt/…` path; remote Sessions do not support local files and show a notice. Dropping anywhere other than the terminal still opens the file in the editor.
- The file tree supports drag and drop: drag to a terminal to paste the path, drag to an editor group to open the file, and drag into a folder to move it (with a confirmation first; SFTP workspaces do not support moving).
- After splitting a HERDR terminal, you can drag a pane's title bar to swap positions (requires HERDR support for `pane.swap`).

### Fixed

- Windows: The "Installed" source now picks only `herdr.exe` and no longer picks a script with the same name by mistake. Quotes or leading and trailing whitespace in a pasted custom path are removed automatically, and the path must be an `.exe`.
- The WSL toggle is now stored and enforced by the Yuzora backend. When it is off, WSL is no longer discovered or connected, and open WSL connections are aborted immediately.
- When Windows installs an update, only the HERDR bundled with Yuzora is stopped. A HERDR you installed yourself is only listed in the confirmation window, and its Sessions are not terminated.
- When Yuzora is launched from inside a HERDR pane (for example by running `tauri dev` or `open` in a pane), Yuzora's HERDR connection no longer connects by mistake to the server of the outer pane. It uses the Session selected in settings.
- When WSL is turned off, a WSL host that is still connecting no longer finishes connecting and gets added after the toggle.
- When renaming, enabling/disabling, or removing fails in the Machines tab, you can expand "Diagnostics" to see the reason HERDR reported, in addition to the error explanation.
- On Windows, the first SSH/SFTP connection that accepts a host key no longer aborts with "Unable to sync the known_hosts temp file". The host key is saved correctly.
- On Windows, when antivirus or an indexer briefly locks the temp file just written while saving a host key, Yuzora waits and retries. It no longer occasionally aborts the connection before authentication.
- Case-insensitive file content search no longer misses non-ASCII characters that are case equivalents (for example `ς` when searching for `Σ`). The match column is also computed on the original text, so the highlight in the result preview matches the text that actually matched.
- When a PostgreSQL connection does not specify a database and the `postgres` database does not exist, you have no permission to connect, or it does not accept connections, Yuzora falls back to a database with the same name as the user to list the available databases. If neither can be connected, the error message lists each attempt and suggests specifying a database.
- While the external change conflict window is open, if the file becomes binary, too large, or unreadable, it now shows the matching notice screen. It no longer offers "Use disk version" or "Resolve and save" with the old content read earlier. When the file becomes comparable text again, it returns to the comparison view automatically.
- When applying, popping, or deleting a stash in the Stash dialog, if another Git tool has changed the stash index after the list loaded, the operation aborts and the list reloads. It no longer acts on a different stash.
- In light themes, terminal text printed in white (for example PowerShell parameters, paths, and numbers) is no longer almost invisible. The highlight for selected text in PowerShell also stays readable.
- Kitty graphics: when an image is placed without `C=1`, the cursor now moves to the right of the image as the spec requires, so later text no longer overlaps the image. The official HERDR client always sends `C=1`, so its display is not affected.
- Dragging editor tabs and database result columns could never be dropped on macOS and Windows: Tauri's file drop layer intercepted the web page's HTML5 drag and drop. It now uses pointer events everywhere.
- If pasted terminal content contained a split bracketed paste end marker, removing it once used to reassemble a new marker, which ended the paste early and sent the following text as keystrokes. Now the marker is removed repeatedly until it no longer appears.

### Known limitations

- When you open an Agent on a machine from the sidebar, the official client can't select that host automatically. You need to switch manually in the official sidebar. Agents on a machine do not show a branch.
- In a program with mouse reporting on (for example lazygit, `vim` with `mouse=a`), clicking or dragging in a HERDR terminal also shows a local selection highlight: HERDR does not tell Yuzora whether a program has mouse reporting on, so local selection can't be kept only for a regular shell. The highlight does not affect the actions the program receives. To copy, hold Shift while selecting.
- Windows does not support "Reconnect" for machines (an official HERDR limitation). Use ssh-agent to manage keys.
- Smart checkout still can't handle a directory/file conflict (for example, the path of an untracked file is a folder on the target branch). It puts the file back in place and shows the original error.
- File names that contain control characters, and on Windows file names that contain `$`, a backtick, `"`, `%`, `!`, or smart quotes, can't be escaped safely, so they are not pasted into the terminal.

### Installation notes

- The macOS app supports Apple Silicon only. It is not signed with an Apple Developer ID or notarized, so a Gatekeeper prompt can still appear on first launch.
- Windows Authenticode is not enabled yet, so a SmartScreen prompt can still appear on install or first launch.
- The fixed download link `Yuzora-windows-x64-setup.exe` stops being updated from this version (assets of older versions are unchanged). Use `Yuzora-windows-x64.msi` instead.

## [0.0.17] - 2026-10-07

### Added and improved

- Git conflict resolution: In the conflict dialog you can use "Accept Yours/Accept Theirs" on a file or open the three-pane merge tool, apply left and right changes block by block, apply all non-conflicting blocks in one click, and auto-resolve simple conflicts. For merge, rebase, cherry-pick, and revert, you can continue, abort, or skip from the banner. A file that still has conflict markers is not marked as resolved.
- Common Git operations: The branch context menu can merge into the current branch, rebase, rename, and delete (asking about force delete if the branch is unmerged). A commit can be reverted, the last commit can be undone, or you can reset to a given commit (Soft/Mixed/Hard). New Stash dialog, Pull (rebase), Push (with tags), and force Push (with lease).
- Smart checkout: When checking out a branch, remote branch, tag, or commit would overwrite local changes, you can choose "Smart checkout". It stashes your changes first and restores them automatically after switching. If restoring conflicts, the conflict dialog opens and your changes stay in the stash. Pull, merge, and rebase automatically stash and restore local changes.
- Multi-folder Git: When you open a folder that is not itself a Git repo, Yuzora lists the repos in its subfolders recursively (skipping common directories such as node_modules), and you can switch between them in the Git panel.
- The file tree supports cut, copy, paste, and duplicate (context menu or ⌘C/⌘X/⌘V/⌘D, Ctrl on Windows/Linux) and works with Finder and File Explorer: copied files can be pasted into Finder or File Explorer, and files and folders copied from Finder or File Explorer can be pasted into the workspace. Duplicate names are named "… copy" automatically.
- When deleting files or folders takes more than a moment, progress and count appear in the lower-right corner, and a local delete can be canceled. Large folders (for example node_modules) no longer stop halfway through deletion.
- Dragging a selection in the terminal past the top or bottom edge of the view scrolls automatically, so you can select and copy off-screen content.
- When you scroll the terminal with the mouse wheel, the right scrollbar follows the current position more promptly.
- The bundled HERDR is upgraded from 0.9.1 to 0.9.3 (private protocol 22 unchanged), and mixed-version compatibility with 0.9.0 and 0.9.1 servers is kept. The Kitty and escape-key behavior changes since 0.9.2 still need hands-on verification on a real machine.
- Markdown document mode keeps the cursor, selection, and scroll position after you switch files. Switching to an already connected Session no longer reinitializes it, which reduces stutter.
- Settings → Editor adds "Markdown default view", where you choose whether Markdown files open as a document preview or as source by default. Files you have switched manually keep their own choice.
- Markdown code blocks get a copy button: in document mode, split preview, and read-only preview, hover over a block to copy it as plain text.

### Fixed

- With macOS Chinese input methods (Zhuyin, Pinyin), typing punctuation in the terminal no longer needs two key presses to appear, and no longer enters a duplicate space.
- Git push sets the upstream automatically when the branch has none. Pull merges by default instead of failing outright when branches have diverged (a configured strategy is still respected). Git operations running at the same time no longer fail as "busy", and reads are no longer blocked by write operations. Error messages keep the key lines from git.
- "Checkout" of a commit in the Git Log now uses detached HEAD and no longer always fails. After renaming or deleting a branch, the Log updates its branch labels immediately.
- Remote Git operations on Windows SSH hosts no longer all fail after detection (they used to report a repository mismatch). File name search on Windows SSH hosts also no longer always returns incomplete results.
- When you open a folder that is not a Git repository over WSL or SSH remote, the Git panel no longer shows the raw `untrustedWorkspace` error and lists child repositories as usual. A repository that is not trusted yet shows a "Trust workspace…" button, and detection reruns automatically after you trust it.
- Fullscreen TUIs (for example Claude Code, Codex) can be scrolled with the mouse wheel in the native terminal.
- Switching with Alt+1–9 or the sidebar to an Agent in a split pane now selects that Agent's pane and accepts input right away. It no longer always selects the first pane of the tab.
- Overly long code lines in the Markdown preview now wrap, so the whole document no longer scrolls horizontally.
- `Infinity`/`-Infinity`/`NaN` cells in PostgreSQL float columns can be edited and saved.
- When too many remote terminals are open at once, they now queue and retry automatically. After retries run out, you can reconnect manually, and the terminal no longer fails permanently.
- The delete confirmation button now reads "Delete" instead of "Continue".
- Installing an update on Windows no longer fails because HERDR holds files open: the confirmation window lists the HERDR version, path, and number of running processes currently in use, and warns that all Session progress will be lost. After you confirm, that HERDR process is terminated before installing. The confirmation window now uses a compact size that fits its content.

### Known limitations

- Smart checkout only handles tracked files that would be overwritten. When untracked files block a checkout, the raw git error still appears. Merge and Pull (merge mode) on git 2.24–2.26 do not support automatic stashing.
- Remote workspaces can't import files from Finder or File Explorer. Pure SFTP workspaces do not support copy and move. SSH/SFTP delete shows no progress and can't be canceled, and a single delete still has item and time limits.
- Multi-folder Git watches only the currently selected repo.
- In the read-only Markdown preview, a single code block over about 32,000 characters is shown in segments and has no copy button.

### Installation notes

- The macOS app supports Apple Silicon only. It is not signed with an Apple Developer ID or notarized, so a Gatekeeper prompt can still appear on first launch.
- Windows Authenticode is not enabled yet, so a SmartScreen prompt can still appear on install or first launch.

## [0.0.16] - 2026-09-22

### Added and improved

- New HERDR tools: Worktree create/open/remove, moving panes across tabs and Spaces, Agent start/prompt/wait/rename/keys, and Integration and Session lifecycle management. You can also view Agent status explanations and the Plugin log.
- New Plugin install, enable/disable, action, and pane management. "Open Herdr window" brings up the full official Session view, which handles native popups, history search, keyboard Copy mode, and Kitty images. Closing the view keeps running tasks.
- Agents show a toast when they finish or need input, with optional system notifications and sounds. Duplicate notifications for historical status, read items, and foreground panes are suppressed.
- New Windows x86_64 SSH host helper, named-pipe routing, native paths, and ConPTY payload. Host artifacts grow to five targets. Real Windows SSH and full Session visual acceptance still need to be done in the matching environments.
- The database workbench is reorganized: after connecting, you choose a PostgreSQL/MSSQL database, and data browsing is separated from a SQL workspace with adjustable height. The SQLite, PostgreSQL, and MSSQL engines are kept.
- Table preview supports editing scalar columns by double-click or keyboard. Saving matches on the full primary key and the original column values, and keeps 64-bit integer/decimal precision. Results without a primary key, arbitrary SQL queries, binary values, and existing JSON values stay read-only.
- The table context menu opens a structure editor, where you can preview and apply table renames, column renames, and adding columns that allow NULL.
- Rows are virtualized, and independent result cursors are released in parallel. Repeated Agent list scans are reduced, and stale state is cleared when a connection completes or is removed.
- A single click in the file tree opens a file in a preview tab (name in italics), and the next single click replaces it. Double-clicking the file or tab, editing the content, or pinning turns it into a regular tab.
- The action buttons on sidebar Space and branch rows now appear inside the row only on mouse hover. Clicking a Space opens the last used branch, or the first branch if there is no history.
- New shortcuts: `⌘⇧E` (`Ctrl+Shift+E` on Windows/Linux) switches the Spaces/Agents view. `Alt+1`…`9` jumps to the matching Agent. macOS `⌥Tab`/`⌥⇧Tab` (``Alt+` ``/``Alt+Shift+` `` on Windows/Linux) switches Agents in most-recently-used order, and releasing Alt confirms. You can change them in "Settings → Keyboard".
- The "Add Herdr Session" dialog is redesigned: choose a source on the left, and Sessions are listed by host on the right. You can select with arrow keys and load with Enter or double-click. The Session currently in use and stopped Sessions are marked.
- The Agent Inspector is removed. Clicking an Agent opens its terminal directly.
- Herdr operations reuse the Session list and connection check for a short time, which reduces the extra processes started for each operation. The interval for waiting for a response on a local Windows connection is also shorter.

### Fixed

- SSH password authentication supports the server's keyboard-interactive password prompt, keeping OTP/password-change protection and the authentication timeout.
- Fixes being unable to unstage in a new Git repo that has no first commit yet. Working files and changes made after staging are kept.
- Busy and action messages in the Git branch menu use a neutral color, and only real errors appear in red.
- Git Graph subjects and refs adapt to the available space. Author and date stay visible in narrow windows and when scrolling horizontally across many branches.
- Large document search moves to a cancelable background Worker. Closing the search, changing the document, or unmounting the editor clears the Worker, timers, and document cache.
- The search button fully follows the current theme, so a light gradient no longer covers the button text in dark mode.
- After a Space is closed successfully, its terminal pages are unmounted at the same time, which releases streams and layout retries. If closing is canceled or fails, the pages are kept.
- Spaces/Agents keep their source order and no longer reorder when status changes.
- Fixes Herdr terminal scrolling on macOS and Windows that lagged behind and kept scrolling after you stopped. It now scrolls the same way as WSL.
- App shortcuts such as `Ctrl+K` on Windows/Linux also work while the terminal has focus.
- When another window takes control of a terminal, it now shows a notice and a "Reconnect" button instead of staying on "Stream closed".

### Installation notes

- The macOS app supports Apple Silicon only. It is not signed with an Apple Developer ID or notarized, so a Gatekeeper prompt can still appear on first launch.
- Windows Authenticode is not enabled yet, so a SmartScreen prompt can still appear on install or first launch.

## [0.0.15] - 2026-09-18

### Added

- You can customize the shortcuts for tabs 1–9 and for the previous and next tab. They work with pinned tabs, split panes, editors, terminals and the built-in Browser.
- HTML/HTM files can be previewed with "Preview here" from the right-click menu of the file tree, tabs and editor. It works in local, WSL and SSH workspaces. The preview shows the saved content and updates after you save.
- The Browser can select page elements and copy their source, selector, HTML, text, size and key styles, so you can give them to an AI to make changes.

### Improvements and fixes

- The bundled HERDR is upgraded to 0.9.1 and stays compatible with 0.9.0. The upgrade does not stop existing Sessions.
- Reduces content misalignment, flicker and brief shrinking in the terminal when you change the font size, split a pane or resize it.
- Fixes extra-wide character spacing, or whole lines squeezed to the left, after you change the terminal font size on Windows.
- Input focus returns to the current Terminal after you switch back to the app, and after you finish naming or cancel a new terminal. Focus in other input fields and dialogs is left alone.
- After the trust dialog for a new workspace closes, input focus returns to the original Terminal.
- Fixes a Browser tab covering the close confirmation dialog for other unsaved files.
- A successful Git operation and a failed status refresh afterward are now reported separately. When busy, only reads are retried, so pull or branch operations are not run twice.
- Fixes files on a WSL Windows drive showing a failure even though they were saved. The prompts for conflicts and for content left unsaved after an update are kept.

### Usage and installation notes

- HTML preview reads only saved files. You still manage the project's dev server yourself. Element copy outputs text only. It supports same-origin iframes and open shadow DOM. For cross-origin iframes, only the frame itself is selected. Passwords and live form values are not captured.
- The macOS app supports Apple Silicon only and is not signed with an Apple Developer ID or notarized. Gatekeeper may still show a prompt the first time you open it.
- Windows Authenticode is not enabled yet. SmartScreen may still show a prompt when you install or first open the app.

## [0.0.14] - 2026-09-15

### Fixed

- After you upgrade Yuzora, the host helper for WSL/SSH is synced automatically and each Session's capability info is refreshed. This fixes Spaces drag and move up/move down being disabled by an old helper. Existing HERDR work and source settings are kept.
- Improves HERDR terminal scrolling and fast repeated wheel input in WSL, so Agent input is not affected. Pending scroll actions are dropped when you switch or close a connection.
- The scrollbar is enabled only when the HERDR session supports full scroll info.
- When pane scroll metadata is temporarily unavailable, scrolling falls back to the compatible terminal scroll, so wheel input no longer breaks on runtime 0.8.2 and above.
- When sending input to a HERDR terminal fails, Yuzora reconnects to the same Session automatically, so the terminal no longer stays stuck in a paused-input state after the connection fails.
- Copying from a HERDR terminal trims extra whitespace before and after paragraphs and keeps the layout of Markdown, code, lists, tables and graphics. It uses LF or CRLF depending on the installed system; WSL on Windows also uses CRLF. Large selections are laid out in the background, and consecutive copies keep the last result.
- Drag-to-reorder of WSL HERDR Spaces now works even when the WebView cannot get pointer capture, and falls back to row position to decide the drop target.

### Added

- In HERDR Sessions that support ordering, you can drag a Space or use move up/move down in the right-click menu to change the order. Whole worktree groups move together, depending on what the runtime supports.

### Improved

- The Spaces/Agents switch is now tab-style and remembers your last choice.
- High-frequency scroll requests are merged and the scroll range is synced in the background, so fast scrolling does not fill the host request queue.

### Installation notes

- The macOS app supports Apple Silicon only and is not signed with an Apple Developer ID or notarized. Gatekeeper may still show a prompt the first time you open it.
- Windows Authenticode is not enabled yet. SmartScreen may still show a prompt when you install or first open the app.

## [0.0.13] - 2026-09-12

### Fixed

- Map WSL `/mnt/<drive>/...` paths to native Windows drive paths when revealing files and folders in Explorer.


This changelog only lists changes you can notice directly; it leaves out commits, file names and implementation details.

## [0.0.12] - 2026-09-12

### Fixed

- The update check reads the signed release info for the selected Stable/Preview channel, and stays compatible with older versions that do not yet have the new native update command.
- Opening a WSL folder now uses an Explorer path fallback, so it can be opened in Windows File Explorer.

## [0.0.11] - 2026-09-11

### Fixed

- The macOS app bundle is now fully ad-hoc signed. This keeps Gatekeeper from showing a downloaded app as damaged because of an incomplete resource archive.
- The Windows update confirmation window now tells you to close HERDR first, so the installer does not fail because `herdr.exe` is locked.

## [0.0.10] - 2026-09-11

### Added

- Work tools are combined into a "Files | GIT" card. It supports creating files, creating folders, copying the full path and opening in Finder/Explorer.
- The editor gets shortcut settings and GitHub/Yuzora/One syntax color themes, and adds recognition for common languages and file extensions.

### Improved

- The Git Graph extends dynamically with the branch topology, and commit content scrolls horizontally. Large Markdown and code are rendered for continuous reading.
- Scrollbars in HERDR Session, Git diff, Minimap and the workspace now use thin scrollbars you can drag directly. Restoring of tabs, Spaces, window focus and the last selection is improved.
- The GIT navigation expands the commit history and branch graph directly. Large documents, Git status and Space/tab switching avoid unnecessary redraws.

### Fixed

- Fixes losing input focus after switching back to the app, a click on a tab Host badge not switching, stale state after Git Fetch making a branch read-only, and the Preview update channel not being able to continue updating.
- Fixes opening Git files from the right-click menu, copying the full path, Finder/Explorer reveal, and the scope of HERDR/Git context menus.

### Changes and known limitations

- Syntax Highlight is verified for 29 languages and 57 file extensions. Some semantic roles in Java, C#, Ruby, Swift, Kotlin, Dart, Shell, PowerShell, SQL and YAML are still limited by the upstream grammar classification.
- Svelte TypeScript, native CSS and templates are supported. The `style lang="scss|sass|less"` preprocessors are still partially supported.
- The macOS installer stays Apple Silicon only and is not signed with an Apple Developer ID or notarized. Windows/WSL restore after restart and the Explorer flow still need separate verification on those platforms.

## [0.0.9] - 2026-09-10

### Added

- A new Space/Agent work view brings together terminals, Agent status and Attention across multiple hosts. The Files/Git sidebar can be collapsed independently.
- Native HERDR on Windows, and WSL that you can optionally enable. Terminals, files and Git for local, WSL and SSH workspaces run on the host you select for each.
- Each host can use a Yuzora-managed, installed or custom HERDR, with entry points for compatibility checks, updates and diagnostics.
- Remote workspaces support file editing, safe saving, Git/worktree, Browser and SQLite. Plain SFTP supports direct editing and manual upload/download.
- Markdown document editing with source switching, file tab pinning with restore after restart, settings search, theme adjustments, resource usage and a Log on/off switch that is saved.
- Appearance settings add a Bot animation switch. It immediately stops the blinking, expressions, swaying and mouse-interaction movement of the Space companion. It is off by default on low-spec devices, and your manual choice is saved.
- The terminal supports pasting multiple lines as a block, copying selected text automatically, and Option/Alt+V to save a clipboard image on the host where the terminal runs and paste its path. Auto-copy can be turned off in settings.
- The website adds an interactive web demo where you can try a sample terminal, file editing, Git diffs, databases and appearance settings. The Chinese and English sites and the demo are deployed to GitHub Pages together.

### Improved

- The bundled HERDR is upgraded to 0.9.0 and can reconnect to a compatible existing server. Closing Yuzora keeps the running HERDR, Agents and WSL.
- Updates the brand icon and theme colors, and adjusts the Space character animation, sidebar, settings page, folder picker and unsaved-changes confirmation window.
- The Git diff window has clear added/deleted markers, side-by-side reading and consistent window actions. The HERDR Session picker shows host and status.
- Scrollbars in the editor and Git diff follow the light and dark theme. The entries for the commit history and branch graph are now clear buttons.
- The width of the database connection sidebar can be adjusted by dragging or with the keyboard, and long connection names no longer push out the edit and remove buttons.
- Faster content loading for Git status, branch lists, workspace diffs and diffs between historical versions.
- Smoother switching between multiple HERDR terminal tabs, keeping the background terminal screens and connections.
- Lowers the cost of duplicate redraws, Git scans and background tool threads with multiple terminals, and releases workspaces, subscriptions and connections that are no longer used.

### Fixed

- Fixes WSL path conversion, applying stale results after switching hosts, a folder picker that kept popping up after being canceled, and Files failing to load for non-Git folders.
- Fixes the workspace trust confirmation being invalidated by background queries. WSL files can be shown in Windows File Explorer under the distribution they belong to.
- Fixes the Windows terminal start directory showing the PowerShell provider prefix, and an empty Session being unable to create its first terminal.
- Fixes edge cases in fast typing and multi-line paste, applying stale image results after switching terminals, and an incomplete screen when a terminal that was hidden for a long time is shown again.
- Fixes a locking problem when the Browser opens on Windows, remote file reconnection and resource release, SQLite path selection and database page spacing.

### Changes and known limitations

- Starting with v0.0.9, the macOS app supports Apple Silicon (M series) only. Intel/universal installers are no longer provided. Remote Intel macOS Hosts remain supported.
- Terminals are now all provided by HERDR. You start Agents manually in a terminal. The standalone local/SSH terminals, shell profiles and the add-Agent form are removed.
- The LSP language server feature is removed. Syntax editing, search and Markdown/SVG/image viewing remain. The Browser keeps URL navigation, history and remote port forwarding. You start dev servers yourself in a terminal.
- Existing WSL/SSH hosts keep their original tool paths. After upgrading, check and update the selected source in HERDR settings. Incompatible existing servers are not stopped automatically, and running processes are not moved.
- Pinning of HERDR/Browser tabs lasts only for the current app session. Clipboard image recognition depends on the Agent in the terminal supporting image paths.
- The web demo uses sample data and resets files and query results when you refresh. It does not connect to local or remote hosts.
- Windows Authenticode is not enabled yet. SmartScreen may still show a prompt when you install or first open the app.
- The macOS installer is not signed with an Apple Developer ID or notarized. Gatekeeper may warn about or block it the first time you open it. Download it from the official GitHub Release, confirm the source, then follow the "Open Anyway" flow in macOS "Privacy & Security". Stable auto-update still verifies the Tauri updater signature.

## [0.0.9-beta.3] - 2026-09-09

### Added

- Windows supports native HERDR. You can enable WSL in settings, then pick a Linux distribution for a folder. Each workspace's terminals, files and Git run on the selected host.
- HERDR settings can manage the bundled, installed or custom version on each host, check compatibility, update tools and copy diagnostics. Errors offer a direct entry point to repair the host.
- The shared add-folder entry supports local folders, full SSH workspaces and plain SFTP. Recent folders and documents are labeled with their host.
- The new work view shows Agents and Attention for multiple hosts in the Space/Agent sidebar. The Files/Git tool sidebar can be collapsed independently. Remote workspaces provide editing, safe saving, Git/worktree, Browser and SQLite.
- SFTP supports direct editing and manual upload/download, and confirms the remote version before saving.
- Markdown offers document editing and source switching. Content that is not suited to document-mode editing keeps safe reading and source operations.
- Work tabs can be pinned. The pinned state of file tabs is restored after restart.
- Settings add search, and theme and gradient adjustments. Resource usage shows App, WebView and managed-tool categories.
- You can turn off Yuzora Log recording, and the setting is saved. Existing records can still be viewed and exported.

### Improved

- The official HERDR is upgraded to 0.9.0/protocol 22 and can reconnect to a running 0.9.0 server. Host tools are installed in a per-user version directory, and the existing runtime is kept.
- When a connection drops, unsaved content is kept, and on reconnect the file state is checked again. Closing Yuzora keeps HERDR, Agents and WSL.
- The sidebar and Session menu show only Sessions that are still running. Spaces/Agents created externally can open a terminal directly, without first being bound to a file folder.
- Git files can open a large side-by-side diff view directly, and status color and text tell apart modified, untracked, staged and deleted files.
- Adjusts the sidebar, top bar and folder picker in narrow windows. The terminal uses JetBrains Mono by default.

### Changes

- Terminals are now all provided by HERDR; the standalone local/SSH terminals and shell profiles are removed. You start Agents manually in a terminal, and the add-Agent form is removed. Existing Agent status and Inspector are kept.
- LSP smart editing, language server downloads and related settings are removed. Syntax editing, search and Markdown/SVG/image viewing remain.
- Preview is folded into Browser, keeping URL navigation, history, refresh and remote port forwarding. You now start dev servers yourself in a HERDR terminal. The static Preview server and Dev Server management are no longer provided.

### Fixed

- Fixes recent remote folders failing to open when not connected, and receiving old directory results after switching hosts.
- Fixes Linux file reads wrongly triggering an external-change warning, and dropped characters when typing fast in a remote terminal.
- Fixes background Session updates repeatedly reopening the folder picker, or switching folders on their own, after you cancel folder selection.
- Fixes WSL conversion of Windows paths you type manually, and applying stale browse results after switching host/distribution. WSL paths on other distributions are explicitly rejected.
- The close button on a terminal tab now closes the corresponding HERDR tab. If the action fails, the tab stays and an error is shown, instead of just hiding the view.
- Fixes "Snapshot loaded / No Space yet" still showing when HERDR is incompatible or reading the snapshot fails. It now shows the client/server version, protocol, optional path and recovery instructions, and hosts and Sessions use readable names.
- When updating host tools, the original managed HERDR choice is kept, and the snapshot is read again after the event subscription is established, so changes made during the connection are not missed.

### Known limitations

- This candidate build has not completed full platform and Agent acceptance, so it must not be labeled a complete replacement. The candidate installer must be verified before the official release.
- Beta is manual download only. It has no OTA and does not replace Stable Latest. Pinning of HERDR/Browser tabs lasts only for the current app session.
- Old Windows workspaces must be rebound to WSL2. Old session data is kept, and running processes are not moved across environments. Native Agent restore depends on official integration support.
- An incompatible existing HERDR server is not stopped or restarted automatically. Save your work before handling the version migration.
- Configured WSL/SSH hosts keep their original binary path. After upgrading, select the host from "Add folder", confirm it uses the HERDR bundled with Yuzora, and click "Update host tools" to apply the new client. This does not stop an existing server automatically.
- macOS Beta has no Developer ID publisher identity, notarization or Gatekeeper trust, so it may be warned about or blocked the first time you open it. Download it only from the official Yuzora GitHub Pre-release.
- Windows Authenticode is not enabled yet. SmartScreen may still show a prompt the first time you open it.

## [0.0.9-beta.2] - 2026-08-28

### Improved

- The macOS Beta now ships as an unsigned installer that does not need an Apple Developer Program certificate. Stable builds still require Developer ID signing and notarization.

### Known limitations

- The macOS Beta has no Developer ID publisher identity, notarization, or Gatekeeper trust, so it may be flagged or blocked on first launch. Download it only from the official Yuzora GitHub Pre-release.
- Windows Authenticode is not enabled yet, so SmartScreen may still show a prompt on first launch.

## [0.0.9-beta.1] - 2026-08-24

### Added

- The ADE can launch an Agent directly in the selected Space from the Agent catalog announced by HERDR. You can explicitly choose a verified bypass-permissions flag. If creation fails, only the tab created by that action is cleaned up.
- The command palette searches HERDR Agents and jumps to them in the priority order blocked, done, working, unknown, idle. It can also switch Spaces quickly.
- Terminal copy and paste support both keyboard shortcuts and system Clipboard events. When the Tauri clipboard service is temporarily unavailable, Terminal falls back to the WebView clipboard. A HERDR terminal that has not yet taken control does not send pasted content to the server.

### Improved

- On Windows, Yuzora connects only to Windows-native HERDR. It shows every Agent identity and status reported by the HERDR snapshot and events as-is. It uses Windows `PATHEXT` to check `.exe`, `.cmd`, `.bat`, and `.com` Agent launchers, but HERDR still validates the actual launch.
- When a new pane is still initializing, New Agent retries briefly, but only while the terminal, tab, and Space identities stay consistent and the foreground process is still a shell. If another process has taken over the pane, it stops safely and reclaims the tab created by that action.
- When HERDR has no Space, or the connection fails, stops, or is unavailable, the ADE provides clear entry points to create a Space and open a local folder. A local Terminal with no workspace opens a folder picker instead of silently doing nothing.
- Creating the first Space requires only `workspace.create`, and it is reported as done only after a later snapshot confirms it. Terminal connection, split resize, and taking over control are safely disabled according to HERDR capabilities.
- Opening, sizing, visibility, and closing of the native Preview child webview are serialized against the latest workspace, overlay, and focus state. A preview no longer covers an app dialog or returns to a stale position.

### Fixed

- Fixed theme colors not applying and saving immediately, the English settings page still mixing in Traditional Chinese, and Logs with many results lacking accessible pagination and an invalid-date message.
- Fixed the Agent Inspector having no discoverable entry point and not returning focus after being closed with the keyboard, and the Git user and date filter menus not cancelling properly with Escape.
- Fixed Preview showing no error when it rejects a dangerous URL scheme, and external child webviews that could remain after switching Space or closing a tab.
- Fixed swapped accessible names for the SSH authentication methods, the first-connection host key prompt hanging for a long time when it could not be accepted or rejected, and SFTP delete lacking a remote path safety check.

### Known limitations

- This is a Beta GitHub Pre-release for manual download and testing only. It does not become the Latest Release, has no OTA updates, and does not update the stable `latest.json` or the fixed download links on the product page.
- An interactive Linux shell that Windows-native HERDR starts through `wsl.exe` is not guaranteed to let HERDR see its hidden Linux descendant Agent processes. Yuzora does not replace what HERDR reports with Agent identities it guesses.
- Official macOS installers get Developer ID signing and notarization from a fail-closed workflow. Windows Authenticode is not enabled yet, so SmartScreen may still show a prompt on first launch.

## [0.0.8] - 2026-08-15

### Added

- The HERDR runtime replaces the former AgentZone. The workspace sidebar now shows Spaces, named Sessions, Attention, and Agents directly, and opens a chosen HERDR terminal as a page that can sit alongside files.
- The HERDR terminal page supports creating, focusing, renaming, moving, and closing tabs, plus splitting, zooming, swapping, closing, and adjusting the layout of panes. It starts in observe mode, and you can explicitly take over input control when needed.
- A new read-only Agent Inspector shows an agent's status and output without changing the HERDR runtime while you view it.
- New workspace trust confirmation. Running commands, Git writes, previews, and remote file operations re-check authorization, paths, and resource limits in the native backend.
- The Git branch picker gains Local, Remote, and Tags categories, search, and status markers. It supports detached checkout, creating a branch from a tag, and multi-select and batch actions for large sets of changes.
- Regular dialogs in the app can be resized to suit their purpose and remember your preference. Alert-style confirmation windows stay compact.

### Improved

- Git, SSH/SFTP, database, LSP, and preview operations that take a long time get stronger cancellation, timeout, input size, and concurrency limits. This lowers the risk of large workspaces or abnormal remote responses slowing down the interface.
- Git revision and file operations use stricter OID, literal path, and repository boundary checks. This keeps branches, paths, or symlinks from being mistaken for command options or escaping the workspace.
- LSP downloads are now verified against a built-in integrity catalog for source, content, and provenance. If no trusted artifact is found, the install is refused instead of continuing with unverified content.
- Markdown preview is now a linked page that sits next to its source document. Tab drag reordering, file drag and drop, image and SVG preview, and state recovery after switching workspaces also improve.
- The Git change list, diff, log, and branch operations load, select, and virtualize better with large amounts of data, and guard against stale responses.
- Updates Tauri, React, Vite, CodeMirror, the database and SSH libraries, and other development dependencies.
- The product website and its Chinese and English navigation assets are rebuilt with the current Yuzora logo. They now introduce the current workspace with a leaner ADE × HERDR story, a light/dark theme, responsive feature showcases, and new Git and Terminal videos.
- On Windows, the HERDR public API and event subscription now use the named-pipe transport of HERDR 0.8.0. When the server, protocol, and schema are compatible, snapshots, runtime mutations, and live events are available. They are no longer disabled just because of the operating system.
- The macOS and Windows installers bundle a Yuzora-managed HERDR 0.8.0 compatible binary pinned by SHA-256. By default, a global install found on PATH is preferred, and the bundled version is used automatically when none is detected. Windows also bundles the official protocol-19 preview package and its ConPTY runtime.

### Fixed

- Fixed an error that Tauri event listener cleanup could throw on development-mode reloads, which skipped the backend unlisten and left stray listeners behind.
- Fixed HERDR terminal streaming that could stop updating after the first flush, and snapshot or capability failures that could not recover or hid the error.
- Fixed a race where older asynchronous results could overwrite the current repository state during Git diff, log, branch checkout/cherry-pick, and workspace switching.
- Fixed several cleanup, timeout, and error-reporting problems in database query cancellation, SSH/SFTP transfers, askpass, and preview resource lifecycles.
- Fixed the next MSSQL query possibly failing after paged results were released early because a helper control message was interrupted, and a stored procedure that returns no rows being mistaken for the end of the stream.
- Fixed the PostgreSQL/MSSQL query helpers failing to start on macOS because the system rejects a limited process memory rlimit. A short-interval resident-memory watchdog now keeps the isolation and limit protection.
- Fixed Windows workspace trust and path capabilities using an unstable metadata identity API, which stopped the app from building on stable Rust. The stable identity now comes from the already-open file handle.
- Fixed HERDR and Markdown pseudo pages being saved as regular file tabs, so the app tried to restore them as files after a restart.
- Fixed the `\\?\` prefix of Windows canonical paths showing up in HERDR diagnostics and the workspace trust UI. Displayed paths are normalized, but the backend still uses the original canonical identity for authorization and revocation.

### Known limitations

- The Yuzora desktop app currently ships only macOS and Windows installers. Linux is used only as a CI and test host and is not a supported desktop release platform.
- HERDR features depend on the version, protocol, and schema of the binary actually in use. If it is incompatible or lacks a required method, the matching action is disabled and the reason is shown.
- On macOS, quitting with ⌘Q does not warn about unsaved changes. Close the window instead to exit.
- The macOS and Windows installers do not have OS code signing yet, so Gatekeeper or SmartScreen may show a prompt on first launch.

## [0.0.7] - 2026-07-26

### Improved

- All remaining background operations (opening a workspace, reading file lists, logging, language server settings access, Agent runtime detection, search startup, and so on) move off the main thread. This removes the last sources of brief UI stutter. Together with the workspace switching improvements in 0.0.6, all background work except the native preview window operations, which must run on the main thread, no longer occupies the UI thread.

### Known limitations

- On macOS, quitting with ⌘Q does **not** warn about unsaved changes. This path is limited by the upstream framework, which gives the app no event it can intercept. Close the window instead to exit.
- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.

## [0.0.6] - 2026-07-26

### Added

- Closing a window with unsaved changes first asks whether to save, discard, or cancel. If you choose save and the write fails, the window stays open and your content is not lost.
- When AgentZone cannot find a usable JavaScript runtime, it now shows installation guidance before launching the Agent. It lists the detection result and install link for Bun, Deno, and Node.js one by one, instead of a single error message.
- Logs export can optionally remove identifying information. It strips full paths, hosts and IPs, accounts, SSH fingerprints, and credentials before you hand it over, which makes it easy to attach to a public bug report.
- Diagnostic logs can be viewed and filtered grouped by "Each launch". Exports also include a summary of each launch (version, platform, start and end times, performance peaks), so you can line up a report with what was happening at the time.

### Improved

- The interface no longer stutters when you switch workspaces. The window stays usable during the switch and does not freeze.
- When you switch back to a workspace you opened before, the Git panel no longer starts blank. It shows what it had when you left, then swaps in the latest state once the background update finishes.
- The file tree's expansion state and scroll position are kept across workspace switches, so you don't have to expand it level by level again.
- Switching back to a workspace restores the editor tabs that were open before.
- On the first open of a workspace, panels load in parallel instead of one by one.
- Under high-speed terminal output (for example `yes` or `find /`), far fewer events are sent to the UI, and input echo latency drops noticeably. When output exceeds the limit, a clear truncation notice is shown instead of silently dropping it.
- The status bar's system usage now shows the total for "the app itself and the background processes it manages". Hover to see the breakdown between the app and its child processes.
- Terminal tabs now have a total limit. At the limit, a clear message says where to close tabs, instead of failing silently.
- When an Agent's first launch needs to download an external adapter, it is no longer mistaken for a connection timeout.
- Built-in and community ACP adapters are now pinned to specific versions, so each launch does not fetch a different version and behave inconsistently.

### Fixed

- On Windows, executables whose path contains spaces (for example tools installed under `Program Files`) no longer fail to launch because of quote handling.
- After an Agent connection fails, the session still in use is no longer closed by mistake. Retries now wait progressively longer and no longer repeat forever.
- Fixed diagnostic log export aborting and emptying the target file when it hit non-ASCII content (Chinese, arrows, emoji).
- Fixed an Agent launch failure that could write an environment variable's value into the log as the runtime name.

### Known limitations

- On macOS, quitting with ⌘Q does **not** warn about unsaved changes. This path is limited by the upstream framework, which gives the app no event it can intercept. Close the window instead to exit.
- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.
- On Windows, external child processes that were force-terminated or detached from the app's management may still need the operating system to finish reclaiming them.

## [0.0.5] - 2026-07-23

### Added

- Terminal font size can be adjusted in settings. Open local and SSH terminals apply it immediately, and the setting is kept.

### Improved

- A hidden Terminal limits its buffered output and fills in the content when it is shown again. This reduces UI stutter caused by several background terminals that keep producing output.
- The Terminal content area no longer shows a right-click menu, so mouse actions can go to interactive TUIs. The rename and close menu on Terminal tabs stays.
- External ACP processes are prepared only when you actually enter the Agent feature. Merely opening or switching workspaces no longer starts them automatically.

### Fixed

- When you quit the app, it now actively ends the background processes of Terminal, Agent, LSP, Git, SSH, and preview services. This reduces cases where processes remain after closing on Windows.
- Background process and WSL profile probing on Windows no longer pops up extra command-line windows.

### Known limitations

- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.
- On Windows, external child processes that were force-terminated or detached from the app's management may still need the operating system to finish reclaiming them.

## [0.0.4] - 2026-07-23

### Added

- On Windows, you can choose Command Prompt, Windows PowerShell, PowerShell 7, the WSL default environment, or an installed WSL distribution in Terminal settings.
- A new Terminal profile dropdown. When you create a terminal, you can temporarily use a different profile without changing the default setting.
- A custom Terminal profile sets the executable, the startup arguments (one per line), and the native Windows or WSL working directory strategy separately.

### Improved

- Improves Chinese input method problems in Terminal on Windows: lost input, duplicate sends, and an incorrect composition position.
- On Windows, you can switch between cursor-following and fixed TUI input box modes, so the Chinese composition position stays visible in full-screen terminal programs.
- When you open a workspace from a WSL network path, Terminal starts with the matching distribution and Linux working directory.

### Known limitations

- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.
- The Chinese input method candidate window on Windows can still be affected by the input method and the WebView2 version. If positioning is unstable, switch to the TUI input box in Terminal settings.

## [0.0.3] - 2026-07-22

### Added

- The AI agent chat interface is fully redesigned. Tool activity is grouped into expandable step chains, messages support full Markdown such as tables and nested lists, and code blocks can be copied in one click.
- The built-in Pi runtime ships with the app, so you don't need to install the community adapter separately. You can switch back to the community version in settings in one click, and existing conversations can continue in either runtime.
- Agent questions (options, confirmations, text input) appear as interactive forms that you can answer directly, instead of only being cancelled or getting stuck.
- Sub-agent calls have a dedicated card that shows the agent type and the task. The tool steps of a Claude sub-agent appear indented and nested.
- Context usage and cumulative cost show live in the conversation. Above the input box, a summary shows the number of changed file lines for this turn.
- The model and thinking depth menus are now searchable and can be switched while a run is in progress. While the agent is running, you can send the next follow-up instruction directly.
- Agent conversations support Pi built-in commands (/compact, /session, /name, and others).
- "About and updates" now lists the main changes in the current version and in available updates.

### Improved

- The same release notes show in the app, on the GitHub download page, and in update notifications.
- The Terminal input method composition window follows the actual input position.
- The changelog is now written in plain language about features, improvements, and known limitations, and no longer lists internal development details.

### Known limitations

- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.
- The AI agent feature on Windows has known issues that will be addressed in a later version.

## [0.0.2] - 2026-07-16

### Added

- You can check for, download, and install a new version directly from settings.
- "About and updates" shows the version the app is actually running.

### Improved

- Download progress shows while an update downloads. After a failure you can retry directly without checking for the version again.
- Before installing, the app reminds you to save unfinished documents and restarts only after you confirm.
- Windows automatic updates now all use the MSI installer format, which reduces problems from mixing different install methods.
- The update entry appears only under "About and updates" in settings, so it does not interrupt your daily work.

### Known limitations

- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.

## [0.0.1] - 2026-07-13

### Added

- Use AI agents, a code editor, Terminal, Git, SSH, and database tools in the same workspace.
- Supports code diagnostics, completion, and formatting, plus live Markdown preview.
- Recent workspaces and agent conversations are saved, so you can reopen and keep working.
- You can view Git changes, commit history, and branches, and get a warning before risky operations.
- Provides local website preview, dev server detection, and app Logs.

### Fixed

- Improves the problem where an AI agent could not get login information when opened from Finder or the Dock.
- Improves path compatibility for Windows workspaces and language tools.
- When an SSH connection fails, a record is kept for troubleshooting.

### Known limitations

- The macOS and Windows installers do not have OS code signing yet, so a security prompt may appear on first launch.
