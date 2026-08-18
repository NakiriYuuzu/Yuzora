# Windows WSL-native Herdr Provider — implementation issue body

> **Publication status:** prepared local body only. No GitHub Issue was created or updated.

## Back references

- Architecture decision: `.yuuzu/adr/0001-wsl-native-herdr-runtime-provider.html`
- Domain glossary: `.yuuzu/CONTEXT.html`
- Delivery plan: `specs/windows-wsl-native-herdr-provider-plan.html`
- Native characterization: `specs/windows-wsl-native-herdr-provider-baseline.html`
- Windows/WSL acceptance matrix: `.yuuzu/eval/windows-wsl-herdr-provider.html`
- Upstream public stdio proxy proposal patch: `specs/herdr-upstream-api-proxy.patch`

## Objective

Run Herdr in the selected WSL runtime so Linux PTY/process-tree/Agent/Git/worktree ownership stays in WSL. Yuzora Windows projects it through typed IPC and must never open a WSL Unix socket through UNC.

## Accepted constraints

1. Identity is `Runtime Environment + Named Session + resource ID`.
2. Runtime Path, Host Path, and Display Path are distinct values.
3. Windows-native Herdr wrapping `wsl.exe` is not a supported WSL implementation.
4. The long-term bridge is the upstream public `herdr api proxy --stdio`; CLI snapshot polling and official terminal connector form only a degraded fallback.
5. Yuzora cleanup releases only its proxy/connector children; it never stops WSL, Herdr server, or panes.
6. Missing/stopped/incompatible WSL must fail closed and never fall back to Native.

## Acceptance evidence required before public support

- Upstream accepts and ships the documented proxy API in a selected WSL Herdr binary.
- Packaged Windows + WSL2 evidence follows `.yuuzu/eval/windows-wsl-herdr-provider.html`.
- Multi-distro same-ID collision, path conversion, proxy disconnect/reconnect, terminal lifecycle and shutdown behavior are recorded.
- Full Yuzora native/frontend/Rust validation is green at release time.

## Non-goals

No automatic WSL Herdr install/update, WSL lifecycle management, direct Unix-socket-over-UNC access, private bincode protocol, or cross-runtime live pane migration.
