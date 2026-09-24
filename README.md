<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="Yuzora icon" />

# Yuzora

**Build with agents. Run on HERDR.**

<samp>AI coding agent workspace for macOS and Windows, powered by HERDR</samp>

<br />

[![CI](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/ci.yml?style=flat-square&label=CI&labelColor=1b1a17)](https://github.com/NakiriYuuzu/Yuzora/actions/workflows/ci.yml)
[![Pages](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/deploy-pages.yml?style=flat-square&label=pages&labelColor=1b1a17)](https://github.yuuzu.net/Yuzora/)
![Version](https://img.shields.io/badge/version-0.0.16-86b81f?style=flat-square&labelColor=1b1a17)
![Platform](https://img.shields.io/badge/platform-macOS%20·%20Windows-57534b?style=flat-square&labelColor=1b1a17)
![Tauri](https://img.shields.io/badge/Tauri-2-24c8db?style=flat-square&logo=tauri&logoColor=white&labelColor=1b1a17)

<samp>English · <a href="README.zh-TW.md">繁體中文</a> · <a href="https://github.yuuzu.net/Yuzora/en/">Website</a></samp>

<br />
<br />

<img src="docs/readme/hero-en.gif" width="880" alt="Yuzora v0.0.13 demo tour: Spaces, HERDR terminal, Git diff, SQL results and appearance settings" />

</div>

<br />

> Yuzora is an **AI coding agent workspace for macOS and Windows**, built with Tauri.
> Run your CLI agents, such as Claude Code, Codex or Pi, in persistent HERDR terminal Sessions.
> Keep your code editor, Git diffs, SSH/SFTP, WSL, SQL databases and HTML preview in one
> local-first desktop app. Agent CLIs and their accounts are managed by you.

<br />

## Features

### What's new in v0.0.16

- HERDR tools for Worktrees, pane moves, Agents, Integrations, Sessions and Plugins, plus opt-in Agent notifications and the Open Herdr Window view.
- Windows x86_64 SSH hosts with their own helper, named pipes and native paths.
- Redesigned database workbench with database selection after connecting, explicit cell editing and a table structure editor.
- Background search for large documents, virtualized database rows, stable Spaces/Agents ordering and resource cleanup.
- SSH password compatibility, Git unstage fixes for new repositories, clearer branch notices and responsive graph metadata.

### What's new in v0.0.15

- Configurable tab shortcuts, including Ctrl+Tab and direct selection of tabs 1–9.
- Preview saved workspace HTML from local, WSL and SSH folders; select browser elements and copy context for AI edits.
- HERDR 0.9.1 integration, terminal focus and rendering fixes, plus Git operation and WSL save fixes.

### What's new in v0.0.14

- Drag Spaces to reorder them within HERDR Sessions that expose `workspace.move`.
- Recover the same HERDR terminal Session automatically after uncertain input delivery.
- Smoother WSL and HERDR scrolling with capability-gated scrollbars and burst coalescing.

### What's new in v0.0.13

- Stable and Preview update checks now select the newest signed release by SemVer, with compatibility fallback for older Yuzora hosts.
- WSL folder reveal now falls back to the Windows Explorer `\\wsl.localhost`／`\\wsl$` namespace when shell selection cannot foreground Explorer.

### What's new in v0.0.10

- A Files | GIT workspace-tools card with new-file/new-folder actions, full-path copying, and Finder or Explorer reveal.
- Dynamic-width Git Graph with horizontal scrolling, large-document continuous rendering, and directly operable HERDR, diff, and minimap scrollbars.
- Configurable keyboard shortcuts and GitHub, Yuzora, and One syntax themes across common language and file-extension mappings.

- Faster Git status, branch lists and diff loading; smoother switching between open HERDR terminals while keeping their output and connections.
- A persistent Bot animations switch under Appearance; lower-spec devices default to static companions, with system reduced motion respected.
- Themed editor and diff scrollbars, a resizable database sidebar, and a clear commit history / branch graph button.
- Whole-block multiline paste, optional copy on selection, and Option/Alt+V image paste to the terminal's host.
- Native Windows HERDR, opt-in WSL, and safer workspace trust, paths and reconnection across local and SSH hosts.
- Updated branding and an [interactive browser demo](https://github.yuuzu.net/Yuzora/demo/) deployed with the website through GitHub Actions Pages.

See the [Changelog](CHANGELOG.md) for the complete release notes and limitations.

<table>
<tr>
<td valign="middle" width="38%">

<sub><samp>01 · ADE × HERDR</samp></sub>

### From Space to agent terminal

The Space and Agent sidebar projects HERDR Spaces, named Sessions, Attention and Agents. Selecting an agent focuses its owning Session and Space, then opens the corresponding HERDR terminal page. Each Yuzora page maps to one HERDR tab and recursively renders its BSP panes. Mutating actions are capability-gated.

<code>Spaces</code> <code>named Sessions</code> <code>BSP terminal</code>

</td>
<td valign="middle" width="62%">

<img src="docs/readme/ade-herdr-en.png" alt="Yuzora v0.0.9 AppShell demo with Spaces and Agents, HERDR terminal and workspace tools" />

</td>
</tr>
</table>

<table>
<tr>
<td valign="middle" width="62%">

<img src="docs/readme/remote-db-en.png" alt="Database panel: browse tables, run SQL, inspect schemas" />

</td>
<td valign="middle" width="38%">

<sub><samp>02 · SSH & DATABASES</samp></sub>

### Remote feels local

Browse and edit files over SSH with SFTP transfer; query tables, run SQL and inspect schemas in the database panel. Connections are managed in one place — known hosts and credentials stay on your machine.

<code>SSH / SFTP</code> <code>PostgreSQL</code> <code>SQL Server</code> <code>SQLite</code>

</td>
</tr>
</table>

<table>
<tr>
<td valign="middle" width="38%">

<sub><samp>03 · TERMINAL & GIT</samp></sub>

### Built-in terminal & git tools

HERDR terminal pages provide xterm-powered input, output and split panes; the git panel shows history and diffs, with cherry-pick straight from commit details. Log query and export keep debugging inside the workbench.

<code>xterm + HERDR</code> <code>git log / cherry-pick</code> <code>log query</code>

</td>
<td valign="middle" width="62%">

<img src="docs/readme/terminal-git-en.png" alt="Yuzora v0.0.9 split Git diff with the commit history and branch graph button" />

</td>
</tr>
</table>

<br />

## Download

Stable releases are built by GitHub Actions and published on [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases). Unpublished PR candidates are available only as Actions artifacts.

| Platform | Format | Download |
|:--|:--|:--|
| **macOS** | `.dmg` — Apple Silicon (M series) only | [Yuzora-macos-aarch64.dmg](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-aarch64.dmg) |
| **Windows** | `.exe` (NSIS) — x64 | [Yuzora-windows-x64-setup.exe](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe) |

The Windows `.msi` installer and past versions live on [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases). Linux is used as a CI/test host only and is not a supported Yuzora desktop release platform.

Starting with v0.0.9, the macOS App requires Apple Silicon. Intel macOS remote Hosts remain supported.

macOS downloads are **not Apple Developer ID signed or notarized**. Gatekeeper may warn or block the first launch. Download from the official release above, then use macOS **System Settings → Privacy & Security → Open Anyway** if offered after the first launch attempt. Windows may show SmartScreen because Authenticode signing is not enabled. Stable and supported prerelease updates verify Tauri updater signatures.

### Update channels

In **Settings → About & Updates → Update channel**, choose:

- **Automatic (installed version)**: stable installations check stable releases; prerelease installations check newer preview and stable releases.
- **Stable**: only stable releases, even when the installed version is a prerelease.
- **Preview and stable**: the highest newer semantic version with signed updater metadata, including prereleases. Switching channels never downgrades.

Older betas, including **v0.0.9-beta.3**, shipped with automatic updates disabled. Install a newer version supporting update channels manually once; existing published installers are not modified. Preview updates become available as new signed prereleases are published. PR acceptance candidates keep automatic updates disabled.

## Tech stack

| Layer | Tech |
|:--|:--|
| Desktop shell | [Tauri 2](https://tauri.app) (Rust) |
| Frontend | React + TypeScript + Vite |
| Agent runtime | HERDR public API + official terminal session connector |
| Terminal | xterm.js + HERDR terminal pages |
| Toolchain | Bun · Vitest · Cargo |

Yuzora bundles HERDR 0.9.1 and retains compatibility with 0.9.0 through private protocol 22 and schema checks. In Settings → HERDR, each host can use a Yuzora-managed, installed or custom binary with compatibility checks and diagnostics. Windows uses native HERDR by default; WSL is opt-in, and each workspace runs on its selected local, WSL or SSH host. Pure SFTP connections do not require a runtime. Closing Yuzora releases its own helpers and connectors while preserving HERDR servers and agents. Existing host paths are retained on upgrade; update the selected source explicitly in settings.

The sidebar's **HERDR tools** manage Worktrees, pane moves, Agent start/prompt/wait/rename/keys, Integrations, running and stopped Sessions, and Plugin installation, enablement and removal. Background agents can notify through in-app toasts, system notifications and sound when they finish or need input. System notifications are opt-in and require OS permission.

**Open Herdr Window** embeds the official HERDR interface for history search, keyboard Copy mode, Kitty images and plugin popups. With default bindings, press Ctrl+B then [ to enter Copy mode, / to search, v to select and y to copy. Custom HERDR bindings still apply. Normal pane connectors pause while this view is open and reconnect when it closes; Session processes keep running. Windows x86_64 SSH hosts use their own helper and named pipes while preserving native paths. See [operations](docs/operations.md) for deployment and acceptance requirements.

Agents can also be started manually in HERDR terminals. The old WSL Pi plugin, separate local/SSH shells and LSP settings have been removed. The browser opens websites and services you start in a terminal.

### Interactive web demo

The [website](https://github.yuuzu.net/Yuzora/) includes an [interactive demo](https://github.yuuzu.net/Yuzora/demo/) with sample terminals, files, Git diffs and databases. It uses in-memory data and does not connect to your computer or remote hosts. Build it with `bun run demo:build`; GitHub Actions deploys the website and demo together to Pages.

## Development

```bash
bun install          # install dependencies
bun run tauri:dev    # launch the desktop app (dev server :1420)
bun run site:companions # generate website characters
bun run demo:build   # prepare Pages artifact for tests
bun run test         # vitest
bun run build        # frontend build (incl. typecheck)
cd src-tauri
cargo check          # Rust check
```

Build installers from source:

```bash
bun install
bun run tauri:build
```

This local build disables updater artifacts and does not require production secrets. Windows
local builds skip platform signing; macOS keeps the ad-hoc bundle seal while Apple Developer ID
signing and notarization remain disabled. See [the operations guide](docs/operations.md) for
release and verification steps.

> README and [website](https://github.yuuzu.net/Yuzora/) media are rendered by
> [Remotion](https://www.remotion.dev) from recordings of the current AppShell demo.
> The recordings use sample data, not a live host. Sources and repeatable render
> commands are in [`site-remotion/`](site-remotion/).

<br />

---

<div align="center">

**An ADE fused with the HERDR runtime.**

<samp>agent development under the evening sky</samp>

<sub>

[Source](https://github.com/NakiriYuuzu/Yuzora) · [Issues](https://github.com/NakiriYuuzu/Yuzora/issues) · [Releases](https://github.com/NakiriYuuzu/Yuzora/releases) · [Website](https://github.yuuzu.net/Yuzora/)

</sub>

</div>
