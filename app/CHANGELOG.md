# Herdr changelog

<!-- yuzora: Yuzora's entries sit above Herdr's; the app shows the first one as its version's notes. -->
<!-- yuzora(L7): each Yuzora entry here has its Traditional Chinese twin in CHANGELOG.zh-TW.md: add a line to both. -->
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
- The app now updates itself from Yuzora's GitHub Releases: it checks every hour, downloads only after the update manifest's signature and each file's SHA-256 check out, and applies the update on the next restart. A beta build only updates to a stable release (0.1.0 and later), never to another beta; download betas by hand.
- Installers: `Yuzora-macos-aarch64.dmg` on macOS (Apple Silicon only, ad-hoc signed; the first time you open it, click "Open Anyway" in Privacy & Security), `Yuzora-windows-x64-setup.exe` on Windows, which needs no admin rights, and a tar.gz with `install` on Linux. The command-line program is called `yuzora`.
- Settings now open from Yuzora's "General" page, with a new "Yuzora" group listing each feature's settings page; the Privacy page explains that Yuzora collects and sends no data.
- General settings: language (follow the system, English, 繁體中文), theme, font and size, zoom, and terminal clipboard writes.
- Agent templates: manage each machine's herdr-core templates in Settings (edit in a form, check before saving, move to the trash), and "Start from template" in a chosen Space and tab.
- Mods management: lists shipped, edited, cloned and personal mods and layouts with their errors; you can clone, show in folder, check, reset and switch layouts, and install from a folder after a trust warning.
- Databases: the sidebar's "Databases" mode connects to PostgreSQL, SQL Server and SQLite (passwords are kept in the system keychain), with a database picker and a schema tree; the table pane has Data (a virtual-scrolling grid, sorting, copying, and editing cells that are written back in a single transaction), Structure (columns, indexes, foreign keys, DDL) and SQL tabs; the SQL pane runs the selection or the whole script, with several result sets, cancelling and a history.
- Files sidebar: follows the current pane's repo, with Git marks and file icons colored by type; create, rename, move to the trash, copy the path, and reveal in Finder; fuzzy file-name search and content search (with ripgrep when it's there, a built-in search otherwise).
- Code editor: tree-sitter highlighting for 14 languages, conflict checks on save, and find and replace; a file changed on disk reloads by itself, and you're told when an agent is editing the same file.
- Markdown preview (kept in sync with the editor as you type, with a switch to the source) and an image / SVG viewer. A repo's SVG is drawn in a separate process with time and memory limits; one past them shows a placeholder and the reason. Files in a remote Space can be browsed and opened read-only.
- Git (modeled on IntelliJ IDEA): the sidebar's Git mode stages, commits (amend, sign-off, skipping hooks, message history and templates) and pushes; the diff pane stages or discards a single hunk or the selected lines; the commit log with its branch graph, filters and context menu; the branch popup (smart checkout, merge, rebase, compare); three-way merge for conflicts; interactive rebase; Annotate; stash, tag, remote, worktree and patch management; a Git settings page.
- Untrusted folders: until you trust a folder in the Git mode, Yuzora runs no Git there that could start a program the repository's own config names (a filter, an fsmonitor hook). The Changes and Files panels and the worktree dialog say the folder isn't trusted yet, the Spaces list reads the branch and ↑↓ from refs only, and on Linux watching such a folder no longer asks Git for its ignored folders.
- Synced herdr-app upstream to f6fe216 (after v0.1.12): optional terminal font ligatures (`font_ligatures` in `settings.json`, off by default), the shortcuts help closes when pressed again, and fixes for the starting size of pane programs, the focus frame, control-key aliases and more.

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
