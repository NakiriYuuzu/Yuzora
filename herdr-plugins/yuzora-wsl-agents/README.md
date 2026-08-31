# Yuzora WSL Agents (Experimental)

Windows-only Herdr plugin that opens **plugin-managed** WSL panes and installs a
Pi adapter. The adapter reports live identity/state to **Windows-native Herdr**
through `herdr.exe` (WSLInterop). Yuzora remains a snapshot/events consumer.

This plugin is **Experimental**. Herdr’s Windows plugin surface is still
preview. Do not treat it as Stable support.

## Scope (v0.1.0)

- Host runtime: Windows-native Herdr `v0.8.2` / protocol 20 only
- Source: `yuzora:wsl:pi` (does **not** impersonate `herdr:pi`)
- Reports: `working` / `idle` / `blocked` / `unknown` and `release-agent`
- Native Pi session id: adapter log only, redacted, **not resumable**
- Claude / Codex: deferred
- Arbitrary existing `wsl.exe` panes: not supported

## Windows beta bundle

Yuzora `0.0.9-beta.3` Windows MSI/NSIS installers bundle this runtime package at:

```text
<Yuzora resource root>\herdr-plugins\yuzora-wsl-agents
```

Bundling does **not** silently register the plugin or modify any WSL distro. With
Yuzora running, resolve the exact installed path and explicitly link it:

```powershell
$yuzoraExe = (Get-Process yuzora | Select-Object -First 1).Path
$appRoot = Split-Path -Parent $yuzoraExe
$pluginRoot = Join-Path $appRoot 'herdr-plugins\yuzora-wsl-agents'
$helper = Join-Path $pluginRoot 'scripts\manage-bundled-plugin.ps1'
$herdr = Join-Path $appRoot 'herdr\windows-x86_64\herdr.exe'

powershell.exe -NoProfile -ExecutionPolicy Bypass -File $helper -Action status -HerdrPath $herdr
powershell.exe -NoProfile -ExecutionPolicy Bypass -File $helper -Action link -HerdrPath $herdr
```

The helper prefers a PATH-installed Herdr and otherwise uses the adjacent
Yuzora-managed `herdr.exe`. It is idempotent for this exact bundled root and
fails closed if another local or GitHub plugin already owns the
`yuzora-wsl-agents` id.

Use the adjacent managed CLI for setup and actions even when `herdr` is not on
PATH:

```powershell
& $herdr plugin config-dir yuzora-wsl-agents
& $herdr plugin action invoke yuzora-wsl-agents.install-pi
& $herdr plugin action invoke yuzora-wsl-agents.open-pi
```

Optional `config.json` in the printed config directory:

```json
{
  "schemaVersion": 1,
  "defaultDistro": "Ubuntu",
  "distros": ["Ubuntu"],
  "enabledAgents": ["pi"],
  "linuxCwdPolicy": "workspace"
}
```

Then confirm Herdr snapshot shows a Pi agent with source-backed live state.

## Source checkout development

GitHub subdir install would clone the whole Yuzora repository. During local
development, link the checkout instead:

```text
herdr plugin link C:\path\to\yuzora\herdr-plugins\yuzora-wsl-agents
herdr plugin config-dir yuzora-wsl-agents
```

## Rollback

1. Close plugin-managed WSL panes.
2. Invoke **Uninstall Pi WSL adapter**.
3. Run the bundled helper with `-Action unlink`.

The helper refuses to unlink a registration that points at another root.
Adapter uninstall removes only plugin-owned files (`yuzora-herdr-wsl.ts`,
reporter, marker). Official `herdr-agent-state.ts` is left untouched.

## Known limitations

- A running Herdr 0.8.0 / protocol-19 default or named server is not stopped by
  Yuzora. Herdr 0.8.2 rejects it as incompatible; save work, then stop and
  restart every affected session with the new binary before using this plugin.
- PowerShell starts with `-NoProfile -ExecutionPolicy Bypass`. A machine GPO
  that forbids Bypass will block the plugin.
- `HERDR_SOCKET_PATH` is a Windows named-pipe marker. The launcher strips every
  inherited `WSLENV` entry of that name. WSL receives `YUZORA_HERDR_SOCKET_PATH`
  instead. Only the Windows `herdr.exe` child gets `HERDR_SOCKET_PATH/w`.
- Each target distro needs `python3` (fcntl) or `flock` so the reporter can take
  a process-owned seq lock. Without either, `status` reports
  `missing-prerequisite`, `install` fails closed, and the reporter no-ops
  instead of creating a crash-stale mkdir lock.
- Custom source cannot create a persisted Herdr `agent_session`. Do not claim
  resume.
- Windows 11 / WSL2 acceptance (P8) is user-owned. Automated tests on macOS do
  **not** prove ConPTY, named pipes, or WSLInterop.

## Tests

```text
bun test herdr-plugins/yuzora-wsl-agents/tests
bash -n herdr-plugins/yuzora-wsl-agents/adapters/common/herdr-wsl-report
bash -n herdr-plugins/yuzora-wsl-agents/adapters/install.sh
```

On Windows, `tests/run-windows.ps1` is a helper only. It does not replace P8.
