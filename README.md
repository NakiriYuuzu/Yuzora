<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="Yuzora icon" />

# Yuzora

**Build with agents. Run on HERDR.**

<samp>AI coding agent workspace, powered by HERDR</samp>

<br />

[![CI](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/ci.yml?style=flat-square&label=CI&labelColor=1b1a17)](https://github.com/NakiriYuuzu/Yuzora/actions/workflows/ci.yml)
[![App CI](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/app-ci.yml?branch=beta2/integration&style=flat-square&label=beta.2%20app&labelColor=1b1a17)](https://github.com/NakiriYuuzu/Yuzora/actions/workflows/app-ci.yml)
![Version](https://img.shields.io/badge/version-0.1.0--beta.1-86b81f?style=flat-square&labelColor=1b1a17)
![Platform](https://img.shields.io/badge/platform-macOS%20·%20Windows-57534b?style=flat-square&labelColor=1b1a17)

<samp>English · <a href="README.zh-TW.md">繁體中文</a> · <a href="https://github.yuuzu.net/Yuzora/en/">Website</a></samp>

</div>

<br />

> **v0.1.0-beta.2 is in development.** Yuzora is becoming a native app built on
> [herdr-app](https://github.com/herdrdev/herdr-app) (Rust · GPUI · libghostty · QuickJS mods) in [`app/`](app/).
> The released app, v0.1.0-beta.1 and older, is the Tauri app in `src-tauri/` and `src/`. It stays in the
> repository as a porting source until beta.2 ships. Architecture: [docs/beta2/architecture.md](docs/beta2/architecture.md).

## v0.1.0-beta.2

Yuzora runs your CLI agents (Claude Code, Codex, Pi…) in herdr-core terminals that keep running when the
window closes, and puts the tools you use around them in the same window:

- **Spaces, Sessions and Agents** from herdr-core, with each agent's state (working, needs you, done) by color and shape.
- **Mixed splits**: Git, files and editor, database and SFTP panes sit next to terminals in one tab.
- **Git** modeled on IntelliJ IDEA Community: staging, commits, branches, the log and branch graph, diff and merge.
- **Databases** (PostgreSQL, SQL Server, SQLite), **SSH / SFTP** with tunnels, a **code editor** with Markdown preview.
- **Mods and templates**: Yuzora's own UI is herdr mods you can clone and change; herdr-core agent templates work as they are.
- English and Traditional Chinese (Taiwan).

Browser, Companion and AgentZone are not part of beta.2.

## Download

Releases are built by GitHub Actions and published on [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases).
Until beta.2 is released, these are the Tauri app.

| Platform | Format | Download |
|:--|:--|:--|
| **macOS** | `.dmg` — Apple Silicon (M series) only | [Yuzora-macos-aarch64.dmg](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-aarch64.dmg) |
| **Windows** | `.msi` — x64 | [Yuzora-windows-x64.msi](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64.msi) |

Starting with v0.0.9, the macOS App requires Apple Silicon. Intel macOS remote Hosts remain supported.

macOS downloads are **not Apple Developer ID signed or notarized**. Gatekeeper may warn or block the first launch.
Download from the official release above, then use macOS **System Settings → Privacy & Security → Open Anyway**
if offered after the first launch attempt. Windows may show SmartScreen because Authenticode signing is not enabled.
Stable and supported prerelease updates verify Tauri updater signatures. Release notes: [CHANGELOG.md](CHANGELOG.md).

## Repository

| Path | What |
|:--|:--|
| [`app/`](app/) | **beta.2 app**: herdr-app brought in with `git subtree`, its own Cargo workspace |
| `app/src/yuzora/` | Yuzora's one integration point in the app: branding, switches, `yuzora:*` modules |
| `app/crates/yuzora-*` | Yuzora's native layer: `yuzora-core` and one crate per lane |
| `app/defaults/mods/yuzora-*` | Yuzora's mods (sidebar modes, tool panes, status bar) |
| `app/defaults/layouts/yuzora.json`, `themes/yuzora-dark.json`, `yuzora/tokens.json` | Default layout, theme and design tokens |
| [`docs/beta2/`](docs/beta2/) | Architecture, lanes, research |
| `src-tauri/`, `src/` | Tauri app (v0.1.0-beta.1 and older), porting source |
| `site/`, `site-remotion/` | Website and README media for the Tauri app |

## Development

### beta.2 app (`app/`)

Needs Rust 1.95 or newer, [Zig 0.16.0](https://ziglang.org/download/) (ghostty) and the ghostty submodule:

```bash
git submodule update --init app/vendor/ghostty
cd app
cargo build                    # target/debug/herdr-app
cargo test --locked            # the app, ghostty-sys and every yuzora-* crate
scripts/yuzora/check-mods      # herdr-app check on each Yuzora mod
```

The app keeps its own target in `app/target`. Never run a bare `target/debug/herdr-app` against your own data:
give it a throwaway home and core session (`YUZORA_APP_HOME`, `HC_SESSION`, see
[docs/beta2/lanes.md](docs/beta2/lanes.md)). On macOS, `scripts/yuzora/bundle-macos <dir>` wraps the binary as
`Yuzora.app`. The Cargo package and binary keep upstream's name, `herdr-app`, for now.

### Tauri app (v0.1.0-beta.1)

```bash
bun install
bun run tauri:dev    # desktop app (dev server :1420)
bun run test         # vitest
bun run build        # typecheck + frontend build
cargo check --locked --all-targets
```

Release steps are in [docs/operations.md](docs/operations.md).

## Lanes

beta.2 is built in parallel lanes, each owning its own directories. Scope and rules: [docs/beta2/lanes.md](docs/beta2/lanes.md).

<!-- yuzora:lane:L1:start -->
### L1 · Shell and workspace

The workspace sidebar (machines → Spaces → worktrees → agents, needs attention, scope chips per machine), the mode
bar with each mode's badge, the tab bar whose "+ New pane" menu mixes tool panes, terminals and agents into one tab,
pane headers (amber while an agent needs a reply) and the status bar. `yuzora:shell` (`app/crates/yuzora-shell`)
holds the sidebar's model, Space badges and a small Git summary. Details: [docs/beta2/shell.md](docs/beta2/shell.md).
<!-- yuzora:lane:L1:end -->

<!-- yuzora:lane:L2:start -->
### L2 · Git

Git modelled on JetBrains IntelliJ IDEA Community, with the staging area:
the sidebar's Git mode (changes, staging, commit, push), a diff pane with
hunk and line staging, the commit log with its branch graph, the branches
popup, push dialog, three-way merge, interactive rebase, annotate, and a
repository pane (stashes, tags, remotes, worktrees, submodules, patches).
All git work runs in Rust (`app/crates/yuzora-git`, the `yuzora:git`
module), never on the UI thread. Design and the IntelliJ checklist:
[docs/beta2/git.md](docs/beta2/git.md).
<!-- yuzora:lane:L2:end -->

<!-- yuzora:lane:L3:start -->
### L3 · Databases

PostgreSQL, SQL Server and SQLite in the sidebar's Database mode: saved connections (passwords in the system keychain), a database picker and the schema tree; a table pane with Data (virtual grid, sorting, copying, editing cells and writing them back in one transaction), Structure (columns, indexes, foreign keys, DDL) and SQL; and a SQL pane with several result sets, cancel and history. Native layer `app/crates/yuzora-db` (`yuzora:db`); design and limits in [docs/beta2/db.md](docs/beta2/db.md).
<!-- yuzora:lane:L3:end -->

<!-- yuzora:lane:L4:start -->
### L4 · SSH / SFTP

Not started.
<!-- yuzora:lane:L4:end -->

<!-- yuzora:lane:L5:start -->
### L5 · Files and editor

Not started.
<!-- yuzora:lane:L5:end -->

<!-- yuzora:lane:L6:start -->
### L6 · Settings, mods and templates

Settings open on Yuzora's own pages. Herdr's settings frame gets a "Yuzora" group listing every mod that declares `"yuzora": {"settings": …}`, and Privacy and Updates speak for Yuzora (it collects and sends nothing). L6's pages:

- **General** (`yuzora-settings-general`): language (system, English, 繁體中文), look (themes, fonts, size, zoom), terminal clipboard writes (the sidebar mode at startup appears once the mode bar applies it).
- **Templates** (`yuzora-settings-templates`): herdr-core agent templates on each machine. List them, edit the frontmatter as a form and the body as text (checked before saving), move one to the trash, or start one in a Space and tab (`yuzora-settings-launch`: `hc run` with `herdr.space`/`herdr.tab` labels).
- **Mods** (`yuzora-settings-mods`): shipped, edited, cloned and personal mods and layouts with their errors. Clone, show in folder, check, reset, switch layout, or install from a folder after a trust warning.

Native side: `app/crates/yuzora-settings` (`yuzora:settings`). Design and decisions: [docs/beta2/settings.md](docs/beta2/settings.md).
<!-- yuzora:lane:L6:end -->

<!-- yuzora:lane:L7:start -->
### L7 · Release and updates

beta.2 packages, made by `app/scripts/yuzora/package` and checked by `.github/workflows/app-package.yml` (nothing is published yet):

| Platform | Download | Installs to |
| --- | --- | --- |
| macOS (Apple Silicon only) | `Yuzora-macos-aarch64.dmg`: drag Yuzora onto Applications | `/Applications/Yuzora.app`; the app adds `~/.local/bin/yuzora` |
| Windows x64 | `Yuzora-windows-x64-setup.exe` (no admin rights needed) | `%LOCALAPPDATA%\Programs\Yuzora`, with `yuzora` on your PATH |
| Linux x86_64 / aarch64 | `yuzora-<version>-linux-<arch>.tar.gz`, then run `yuzora/install` | `~/.local/share/yuzora` and `~/.local/bin/yuzora` |

- **macOS: no Apple Developer ID signature, no notarization** (ad-hoc signature only). Gatekeeper blocks the first open: in System Settings › Privacy & Security, click **Open Anyway**, or run `xattr -dr com.apple.quarantine /Applications/Yuzora.app`. Do this only for a DMG you downloaded from this repository's Releases. After an update, macOS may ask again for microphone, camera or folder access.
- **Windows: no Authenticode signature.** SmartScreen may warn; choose More info › Run anyway.
- **Updates.** An installed Yuzora checks this repository's GitHub Releases every hour, downloads a newer version next to itself and switches to it when it restarts. The update manifest must be signed with the release key and every file must match its SHA-256, or nothing is installed. Beta builds follow the beta channel (pre-releases and stable releases), stable builds the stable one; `"update": {"channel": "stable"}` in `~/.config/yuzora/yuzora/settings.json` picks another.
- **Coming from 0.1.0-beta.1.** When a beta.2 release offers it, beta.1's own updater (Settings › Updates, preview channel) moves you to beta.2; otherwise use the downloads above. beta.1's data stays where it was. Bringing connections and settings over isn't wired up yet, so set them up again for now. On Windows, the installer offers to remove the old all-users install.
- The command line program is `yuzora` (the same commands as `herdr-app`). Yuzora never puts `herdr-app` on your PATH, so an official Herdr install keeps its own.

Design, keys and the release steps: [docs/beta2/release.md](docs/beta2/release.md).
<!-- yuzora:lane:L7:end -->

<!-- yuzora:lane:L8:start -->
### L8 · Resource usage

The chip in the window's bottom-right corner shows what this computer's Yuzora uses (the app, its herdr-core with
the terminals and agents in it, and the tools the app starts) as `{cpu}% · {memory}MB`, and opens the details above
it. `yuzora:perf` (`app/crates/yuzora-perf`) samples on a worker thread every 2 s while the window is in front.
Details: [docs/beta2/perf.md](docs/beta2/perf.md).
<!-- yuzora:lane:L8:end -->

## License

The beta.2 app builds on [herdr-app](https://github.com/herdrdev/herdr-app) (Apache-2.0); its license and
third-party notices are in [`app/LICENSE`](app/LICENSE) and [`app/THIRD-PARTY-NOTICES`](app/THIRD-PARTY-NOTICES).

<br />

---

<div align="center">

**An ADE fused with the HERDR runtime.**

<samp>agent development under the evening sky</samp>

<sub>

[Source](https://github.com/NakiriYuuzu/Yuzora) · [Issues](https://github.com/NakiriYuuzu/Yuzora/issues) · [Releases](https://github.com/NakiriYuuzu/Yuzora/releases) · [Website](https://github.yuuzu.net/Yuzora/)

</sub>

</div>
