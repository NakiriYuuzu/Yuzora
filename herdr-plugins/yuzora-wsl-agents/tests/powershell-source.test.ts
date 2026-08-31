import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { spawnSync } from "node:child_process"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const root = join(dirname(fileURLToPath(import.meta.url)), "..")
const common = readFileSync(join(root, "scripts/common.ps1"), "utf8")
const openPane = readFileSync(join(root, "scripts/open-pane.ps1"), "utf8")
const manage = readFileSync(join(root, "scripts/manage-adapters.ps1"), "utf8")
const manageBundled = readFileSync(join(root, "scripts/manage-bundled-plugin.ps1"), "utf8")
const powershellRuntime = readFileSync(join(root, "tests/powershell-runtime.ps1"), "utf8")
const runWindows = readFileSync(join(root, "tests/run-windows.ps1"), "utf8")
const windowsBundleVerifier = readFileSync(
  join(root, "..", "..", "scripts/verify-windows-bundled-wsl-plugin.ps1"),
  "utf8"
)
const gitAttributes = readFileSync(join(root, "..", "..", ".gitattributes"), "utf8")

describe("PowerShell orchestration source", () => {
  it("fail-closed strips every HERDR_SOCKET_PATH WSLENV entry", () => {
    expect(common).toContain("HERDR_SOCKET_PATH")
    expect(common).toContain("ToUpperInvariant")
    expect(common).toContain("YUZORA_HERDR_SOCKET_PATH/u")
    expect(common).toContain("HERDR_BIN_PATH/up")
    expect(openPane).toContain("YUZORA_HERDR_SOCKET_PATH")
    expect(openPane).toContain("WSLENV still contains HERDR_SOCKET_PATH")
    expect(openPane).not.toContain("HERDR_SOCKET_PATH/w")
  })

  it("launches wsl.exe with argv and in-distro sh, never UNC file writes", () => {
    expect(openPane).toContain("& $wsl @argList")
    expect(openPane).toContain("'--exec', 'bash', '-lic', 'exec pi'")
    expect(openPane).not.toContain("'--exec', 'pi'")
    expect(common).toContain("@('--exec', 'wslpath', '-u', '--', $pathForWsl)")
    expect(common).toContain("$previousOutputEncoding = [Console]::OutputEncoding")
    expect(common).toContain("New-Object System.Text.UTF8Encoding($false)")
    expect(common).toContain("[Console]::OutputEncoding = $previousOutputEncoding")
    expect(common).toContain("@('--exec', 'sh', $installer")
    expect(common).not.toContain("@('--', 'wslpath'")
    expect(common).not.toContain("@('--', 'sh', $installer")
    expect(common).not.toMatch(/Set-Content.*wsl\.localhost/)
    expect(common).not.toMatch(/Out-File.*wsl\$/)
    expect(manage).toContain("Invoke-InDistroInstaller")
    expect(runWindows).toContain("$tokens = $null")
    expect(runWindows).toContain("$errs = $null")
    expect(runWindows).toContain("[ref]$tokens, [ref]$errs")
    expect(runWindows).toContain("--exec wslpath -u -- $reporter")
    expect(runWindows).toContain("--exec sh -n")
    expect(runWindows).toContain("$LASTEXITCODE -ne 0")
  })

  it("passes raw WSL distro-list bytes through the UTF-8/UTF-16 detector", () => {
    expect(common).toContain("$proc.StandardOutput.BaseStream.CopyTo($stdout)")
    expect(common).toContain("$proc.StandardError.ReadToEndAsync()")
    expect(common).toContain("ConvertFrom-WslListBytes -Bytes $stdout.ToArray()")
    expect(common).not.toContain("$psi.StandardOutputEncoding")
    expect(common).not.toContain("Unicode.GetBytes($stdout)")
    expect(powershellRuntime).toContain("Assert-DistroBytes -Label 'UTF-8'")
    expect(powershellRuntime).toContain("Assert-DistroBytes -Label 'UTF-8 BOM'")
    expect(powershellRuntime).toContain("Assert-DistroBytes -Label 'UTF-16LE BOM'")
  })

  it("does not open Claude or Codex entrypoints", () => {
    expect(openPane).not.toMatch(/claude/i)
    expect(openPane).not.toMatch(/codex/i)
    expect(manage).not.toMatch(/claude/i)
    expect(manage).not.toMatch(/codex/i)
  })

  it("makes per-distro failures catchable and continues later distros", () => {
    expect(common).toMatch(/function Fail-Closed[\s\S]*?throw "yuzora-wsl-agents: \$Message"/)
    expect(common).not.toMatch(/function Fail-Closed[\s\S]*?exit 1/)
    expect(common).toContain("function Invoke-AdapterActionAcrossDistros")
    expect(common).toContain("[void]$failed.Add([string]$distro)")
    expect(manage).toContain("Get-ConfiguredDistroNames")
    expect(manage).toContain("Invoke-AdapterActionAcrossDistros")
    expect(manage).toContain("[switch]$AllInstalledDistros")
    expect(manage).toContain("$inventory = @(Get-WslDistroInventory)")
    expect(manage).toContain("Fail-Closed 'AllInstalledDistros is allowed only for status or uninstall'")

    const script = `
$ErrorActionPreference = 'Stop'
. '${join(root, "scripts/common.ps1").replace(/'/g, "''")}'
$script:attempts = @()
function Invoke-InDistroInstaller {
    param([string]$Action, [string]$Distro)
    $script:attempts += $Distro
    if ($Distro -eq 'offline-distro') { throw 'offline' }
    Write-Output "ok $Distro"
}
try {
    Invoke-AdapterActionAcrossDistros -Action install -Targets @('offline-distro','Ubuntu')
    throw 'expected aggregate failure'
} catch {
    if ($_.Exception.Message -notmatch 'offline-distro') { throw }
}
if (($script:attempts -join ',') -ne 'offline-distro,Ubuntu') {
    throw "attempts: $($script:attempts -join ',')"
}
Write-Output 'PASS'
`.trim()
    const file = join(mkdtempSync(join(tmpdir(), "yuzora-wsl-ps-")), "multi-distro-continue.ps1")
    writeFileSync(file, script)
    const pwsh = spawnSync("pwsh", ["-NoProfile", "-File", file], { encoding: "utf8" })
    if (pwsh.error && "code" in pwsh.error && pwsh.error.code === "ENOENT") {
      expect(script).toContain("offline-distro")
      expect(script).toContain("Ubuntu")
      expect(script).toContain("expected aggregate failure")
      return
    }
    expect(pwsh.status, pwsh.stderr).toBe(0)
    expect(pwsh.stdout).toContain("PASS")
  }, 20_000)

  it("reads optional config and context fields without StrictMode errors", () => {
    expect(common).toContain("function Get-OptionalProperty")
    expect(common).toContain("$psobject.Properties[$Name]")
    expect(common).toContain("Get-OptionalProperty $obj 'linuxCwdPolicy'")
    expect(common).toContain("Get-OptionalProperty $ctx 'workspace_cwd'")
    expect(common).toContain("Get-OptionalProperty $ctx 'focused_pane_cwd'")

    const work = mkdtempSync(join(tmpdir(), "yuzora-wsl-ps-optional-"))
    const cfgDir = join(work, "config")
    writeFileSync(
      join(work, "run.ps1"),
      `
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Path '${cfgDir.replace(/'/g, "''")}' | Out-Null
Set-Content -LiteralPath '${join(cfgDir, "config.json").replace(/'/g, "''")}' -Value '{"defaultDistro":"Ubuntu"}' -Encoding utf8
. '${join(root, "scripts/common.ps1").replace(/'/g, "''")}'
$env:HERDR_PLUGIN_CONFIG_DIR = '${cfgDir.replace(/'/g, "''")}'
$cfg = Get-YuzoraPluginConfig
if ($cfg.linuxCwdPolicy -ne 'workspace') { throw "policy $($cfg.linuxCwdPolicy)" }
if ($cfg.defaultDistro -ne 'Ubuntu') { throw "distro $($cfg.defaultDistro)" }
if (($cfg.enabledAgents -join ',') -ne 'pi') { throw 'agents' }
$env:HERDR_PLUGIN_CONTEXT_JSON = @{ workspace_cwd = 'C:\\repo' } | ConvertTo-Json -Compress
$fromWorkspace = Get-WorkspacePathFromContext
if ($fromWorkspace -ne 'C:\\repo') { throw "workspace $fromWorkspace" }
$env:HERDR_PLUGIN_CONTEXT_JSON = @{ focused_pane_cwd = 'C:\\pane' } | ConvertTo-Json -Compress
$fromPane = Get-WorkspacePathFromContext
if ($fromPane -ne 'C:\\pane') { throw "pane $fromPane" }
Write-Output 'PASS'
`.trim()
    )
    const pwsh = spawnSync("pwsh", ["-NoProfile", "-File", join(work, "run.ps1")], {
      encoding: "utf8"
    })
    if (pwsh.error && "code" in pwsh.error && pwsh.error.code === "ENOENT") {
      expect(common).toContain("Get-OptionalProperty")
      return
    }
    expect(pwsh.status, `${pwsh.stdout}\n${pwsh.stderr}`).toBe(0)
    expect(pwsh.stdout).toContain("PASS")
  }, 20_000)

  it("strips a Windows verbatim prefix before converting the plugin root", () => {
    expect(common).toMatch(
      /function Convert-PluginRootToLinux[\s\S]*?Strip-VerbatimWindowsPrefix \$root[\s\S]*?Convert-WindowsPathToLinux/
    )
    expect(common).toContain("function Trim-WindowsPathTrailingSeparators")
    expect(common).toContain("$pathForWsl = Trim-WindowsPathTrailingSeparators $WindowsPath")

    const script = `
$ErrorActionPreference = 'Stop'
. '${join(root, "scripts/common.ps1").replace(/'/g, "''")}'
function Convert-WindowsPathToLinux {
    param([string]$Wsl, [string]$Distro, [string]$WindowsPath)
    return $WindowsPath
}
$trimmed = Trim-WindowsPathTrailingSeparators 'C:\\Users\\Yuuzu\\Path With Space\\'
if ($trimmed -ne 'C:\\Users\\Yuuzu\\Path With Space') { throw "trimmed $trimmed" }
$driveRoot = Trim-WindowsPathTrailingSeparators 'C:\\'
if ($driveRoot -ne 'C:\\') { throw "drive root $driveRoot" }
$env:HERDR_PLUGIN_ROOT = '\\\\?\\C:\\Users\\Yuuzu\\Downloads\\yuzora-herdr-wsl-p8\\plugin'
$actual = Convert-PluginRootToLinux -Wsl 'wsl.exe' -Distro 'Ubuntu-26.04'
$expected = 'C:\\Users\\Yuuzu\\Downloads\\yuzora-herdr-wsl-p8\\plugin'
if ($actual -ne $expected) { throw "plugin root $actual" }
Write-Output 'PASS'
`.trim()
    const file = join(mkdtempSync(join(tmpdir(), "yuzora-wsl-ps-root-")), "plugin-root.ps1")
    writeFileSync(file, script)
    const pwsh = spawnSync("pwsh", ["-NoProfile", "-File", file], { encoding: "utf8" })
    if (pwsh.error && "code" in pwsh.error && pwsh.error.code === "ENOENT") {
      expect(common).toContain("$windowsRoot = Strip-VerbatimWindowsPrefix $root")
      return
    }
    expect(pwsh.status, `${pwsh.stdout}\n${pwsh.stderr}`).toBe(0)
    expect(pwsh.stdout).toContain("PASS")
  }, 20_000)

  it("opens panes on defaultDistro even when distros[0] is unavailable", () => {
    expect(common).toContain("function Resolve-LaunchDistro")
    expect(common).toContain("Resolve-LaunchDistro -Config $Config -Inventory $inventory")
    expect(common).not.toMatch(
      /function Resolve-LaunchPlan[\s\S]*?Resolve-TargetDistros/
    )

    const script = `
$ErrorActionPreference = 'Stop'
. '${join(root, "scripts/common.ps1").replace(/'/g, "''")}'
$config = [pscustomobject]@{
    defaultDistro = 'Ubuntu'
    distros = @('Debian','Ubuntu')
    linuxCwdPolicy = 'workspace'
}
$launch = Resolve-LaunchDistro -Config $config -Inventory @('Ubuntu')
if ($launch -ne 'Ubuntu') { throw "launch $launch" }
$both = Resolve-LaunchDistro -Config $config -Inventory @('Debian','Ubuntu')
if ($both -ne 'Ubuntu') { throw "both $both" }
try {
    [void](Resolve-TargetDistros -Config $config -Inventory @('Ubuntu'))
    throw 'adapter resolve should fail on missing Debian'
} catch {
    if ($_.Exception.Message -notmatch 'Debian') { throw }
}
$empty = Resolve-LaunchDistro -Config ([pscustomobject]@{ defaultDistro = $null; distros = @() }) -Inventory @('Ubuntu')
if ($null -ne $empty -and "$empty" -ne '') { throw "empty $empty" }
Write-Output 'PASS'
`.trim()
    const file = join(mkdtempSync(join(tmpdir(), "yuzora-wsl-ps-launch-")), "launch-distro.ps1")
    writeFileSync(file, script)
    const pwsh = spawnSync("pwsh", ["-NoProfile", "-File", file], { encoding: "utf8" })
    if (pwsh.error && "code" in pwsh.error && pwsh.error.code === "ENOENT") {
      expect(common).toContain("function Resolve-LaunchDistro")
      expect(script).toContain("defaultDistro = 'Ubuntu'")
      expect(script).toContain("'Debian','Ubuntu'")
      return
    }
    expect(pwsh.status, `${pwsh.stdout}\n${pwsh.stderr}`).toBe(0)
    expect(pwsh.stdout).toContain("PASS")
  }, 20_000)

  it("links only the bundled root and ownership-safely unlinks it", () => {
    expect(manageBundled).toContain("[ValidateSet('status', 'link', 'unlink')]")
    expect(manageBundled).toContain("herdr\\windows-x86_64\\herdr.exe")
    expect(manageBundled).toContain("plugin id $PluginId is already registered from another root")
    expect(manageBundled).toContain("refusing to unlink plugin id $PluginId")
    expect(manageBundled).toContain("Test-OwnsRegistration")
    expect(manageBundled).toContain("ConvertTo-Json -Compress")
    expect(manageBundled).not.toMatch(/Remove-Item|Delete\(/)
    expect(windowsBundleVerifier).toContain("function Invoke-NativeCommand")
    expect(windowsBundleVerifier).toContain("$ErrorActionPreference = 'Continue'")
    expect(windowsBundleVerifier).toContain("Start-IsolatedHerdrServer")
    expect(windowsBundleVerifier).toContain("Stop-IsolatedHerdrServer")
    expect(windowsBundleVerifier).toContain("$ExpectedHerdrVersion = '0.8.2'")
    expect(windowsBundleVerifier).toContain("$ExpectedHerdrProtocol = 20")
    expect(windowsBundleVerifier).toContain("function Assert-LfOnlyWslFiles")
    expect(windowsBundleVerifier).toContain("WSL-consumed file contains a CR byte")
    expect(windowsBundleVerifier).toContain("'adapters\\install.sh'")
    expect(windowsBundleVerifier).toContain("'adapters\\common\\herdr-wsl-report'")
    expect(windowsBundleVerifier).toContain("'adapters\\pi\\yuzora-herdr-wsl.ts'")
    expect(gitAttributes).toContain(
      "herdr-plugins/yuzora-wsl-agents/adapters/install.sh text eol=lf"
    )
    expect(gitAttributes).toContain(
      "herdr-plugins/yuzora-wsl-agents/adapters/common/herdr-wsl-report text eol=lf"
    )
    expect(gitAttributes).toContain(
      "herdr-plugins/yuzora-wsl-agents/adapters/pi/yuzora-herdr-wsl.ts text eol=lf"
    )
    expect(windowsBundleVerifier).toContain("'status', 'client', '--json'")
    expect(windowsBundleVerifier).toContain("packaged Herdr version is not $ExpectedHerdrVersion")
    expect(windowsBundleVerifier).toContain("packaged Herdr protocol is not $ExpectedHerdrProtocol")
    expect(windowsBundleVerifier).toContain("'XDG_CONFIG_HOME'")
    expect(windowsBundleVerifier).toContain("'HERDR_SOCKET_PATH'")
    expect(windowsBundleVerifier).not.toContain("& powershell.exe")

    if (process.platform === "win32") return
    const work = mkdtempSync(join(tmpdir(), "yuzora-wsl-bundled-helper-"))
    const registry = join(work, "registry.txt")
    const fakeHerdr = join(work, "herdr")
    writeFileSync(
      fakeHerdr,
      `#!/usr/bin/env bash
set -euo pipefail
registry=${JSON.stringify(registry)}
if [[ "$1" == "plugin" && "$2" == "list" ]]; then
  if [[ -s "$registry" ]]; then
    root="$(cat "$registry")"
    printf '{"result":{"plugins":[{"plugin_root":"%s","enabled":true,"version":"0.1.0"}]}}\\n' "$root"
  else
    printf '{"result":{"plugins":[]}}\\n'
  fi
elif [[ "$1" == "plugin" && "$2" == "link" ]]; then
  printf '%s' "$3" > "$registry"
  printf '{"result":{}}\\n'
elif [[ "$1" == "plugin" && "$2" == "enable" ]]; then
  printf '{"result":{}}\\n'
elif [[ "$1" == "plugin" && "$2" == "unlink" ]]; then
  : > "$registry"
  printf '{"result":{}}\\n'
else
  printf 'unexpected args: %s\\n' "$*" >&2
  exit 2
fi
`
    )
    chmodSync(fakeHerdr, 0o755)

    const helper = join(root, "scripts/manage-bundled-plugin.ps1")
    const run = (action: "status" | "link" | "unlink") =>
      spawnSync(
        "pwsh",
        ["-NoProfile", "-File", helper, "-Action", action, "-HerdrPath", fakeHerdr],
        { encoding: "utf8" }
      )

    const status = run("status")
    if (status.error && "code" in status.error && status.error.code === "ENOENT") return
    expect(status.status, `${status.stdout}\n${status.stderr}`).toBe(0)
    expect(JSON.parse(status.stdout)).toMatchObject({ linked: false, ownsRegistration: false })

    const linked = run("link")
    expect(linked.status, `${linked.stdout}\n${linked.stderr}`).toBe(0)
    expect(JSON.parse(linked.stdout)).toMatchObject({ linked: true, ownsRegistration: true })

    const unlinked = run("unlink")
    expect(unlinked.status, `${unlinked.stdout}\n${unlinked.stderr}`).toBe(0)
    expect(JSON.parse(unlinked.stdout)).toMatchObject({ linked: false, ownsRegistration: false })

    writeFileSync(registry, join(work, "foreign-plugin-root"))
    const refusedLink = run("link")
    expect(refusedLink.status).not.toBe(0)
    expect(refusedLink.stderr).toContain("already registered from another root")
    const refusedUnlink = run("unlink")
    expect(refusedUnlink.status).not.toBe(0)
    expect(refusedUnlink.stderr).toContain("refusing to unlink")
  }, 20_000)
})
