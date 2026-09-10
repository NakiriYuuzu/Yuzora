<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="Yuzora icon" />

# Yuzora

**Build with agents. Run on HERDR.**

<samp>An open-source desktop ADE fused with the HERDR runtime</samp>

<br />

[![CI](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/ci.yml?style=flat-square&label=CI&labelColor=1b1a17)](https://github.com/NakiriYuuzu/Yuzora/actions/workflows/ci.yml)
[![Pages](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/deploy-pages.yml?style=flat-square&label=pages&labelColor=1b1a17)](https://nakiriyuuzu.github.io/Yuzora/)
![Version](https://img.shields.io/badge/version-0.0.9-86b81f?style=flat-square&labelColor=1b1a17)
![Platform](https://img.shields.io/badge/platform-macOS%20·%20Windows-57534b?style=flat-square&labelColor=1b1a17)
![Tauri](https://img.shields.io/badge/Tauri-2-24c8db?style=flat-square&logo=tauri&logoColor=white&labelColor=1b1a17)

<samp>English · <a href="README.zh-TW.md">繁體中文</a> · <a href="https://nakiriyuuzu.github.io/Yuzora/">Website</a></samp>

<br />
<br />

<img src="docs/readme/hero-en.gif" width="880" alt="Yuzora product tour: ADE and HERDR Spaces, agents, terminal pages, SSH, databases, terminal and git" />

</div>

<br />

> Yuzora is an **Agent Development Environment (ADE)** built around HERDR as its
> execution and terminal runtime. Spaces, named Sessions, Attention and Agents are
> projected into one desktop surface, while editor, git, SSH/SFTP, databases and a
> browser remain close at hand. Built with Tauri and local-first by default.

<br />

## Features

<table>
<tr>
<td valign="middle" width="38%">

<sub><samp>01 · ADE × HERDR</samp></sub>

### From Space to agent terminal

The Space and Agent sidebar projects HERDR Spaces, named Sessions, Attention and Agents. Selecting an agent focuses its owning Session and Space, then opens the corresponding HERDR terminal page. Each Yuzora page maps to one HERDR tab and recursively renders its BSP panes. Mutating actions are capability-gated, and Agent Inspector is read-only.

<code>Spaces</code> <code>named Sessions</code> <code>BSP terminal</code> <code>read-only Inspector</code>

</td>
<td valign="middle" width="62%">

<img src="docs/readme/ade-herdr-en.png" alt="Yuzora ADE with HERDR Spaces rail, named Sessions, agent status, BSP terminal panes and read-only Agent Inspector" />

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

<code>SSH / SFTP</code> <code>PostgreSQL</code> <code>MySQL</code> <code>SQLite</code>

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

Terminal execution is owned by HERDR. Use the browser to open websites or services started in a HERDR terminal.

</td>
</tr>
</table>

<br />

## Download

Every build is produced by GitHub Actions and published on [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases) — the source is open.

| Platform | Format | Download |
|:--|:--|:--|
| **macOS** | `.dmg` — universal (Apple Silicon / Intel) | [Yuzora-macos-universal.dmg](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-universal.dmg) |
| **Windows** | `.exe` (NSIS) — x64 | [Yuzora-windows-x64-setup.exe](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe) |

The Windows `.msi` installer and past versions live on [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases). Linux is used as a CI/test host only and is not a supported Yuzora desktop release platform.

## Tech stack

| Layer | Tech |
|:--|:--|
| Desktop shell | [Tauri 2](https://tauri.app) (Rust) |
| Frontend | React + TypeScript + Vite |
| Agent runtime | HERDR public API + official terminal session connector |
| Terminal | xterm.js + HERDR terminal pages |
| Toolchain | Bun · Vitest · Cargo |

Yuzora bundles HERDR 0.9.0. In Settings → HERDR, each host can use a Yuzora-managed, installed or custom binary with compatibility checks and diagnostics. Windows uses native HERDR by default; WSL is opt-in, and each workspace runs on its selected local, WSL or SSH host. Pure SFTP connections do not require a runtime. Closing Yuzora releases its own helpers and connectors while preserving HERDR servers and agents. Existing host paths are retained on upgrade; update the selected source explicitly in settings.

Agents are started manually in HERDR terminals. The old WSL Pi plugin, separate local/SSH terminals and LSP settings have been removed. The browser opens websites and services you start in a terminal.

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

This local build deliberately disables updater artifacts and release signing, so it does not
require production secrets. Official macOS release installers are Developer ID signed and
notarized only in the protected release workflow; see `docs/operations.md` for the platform gates.

> The product animation and screenshots in this README and on the
> [website](https://nakiriyuuzu.github.io/Yuzora/) are rendered programmatically by the
> [Remotion](https://www.remotion.dev) project in [`site-remotion/`](site-remotion/),
> with design tokens aligned 1:1 to the app itself.

<br />

---

<div align="center">

**An ADE fused with the HERDR runtime.**

<samp>agent development under the evening sky</samp>

<sub>

[Source](https://github.com/NakiriYuuzu/Yuzora) · [Issues](https://github.com/NakiriYuuzu/Yuzora/issues) · [Releases](https://github.com/NakiriYuuzu/Yuzora/releases) · [Website](https://nakiriyuuzu.github.io/Yuzora/)

</sub>

</div>
