param(
    [Parameter()]
    [ValidateSet('shell','pi','open-shell','open-pi')]
    [string]$Kind = ''
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\common.ps1"

function Open-PluginPane {
    param([string]$Entrypoint)
    $herdr = $env:HERDR_BIN_PATH
    if ([string]::IsNullOrWhiteSpace($herdr)) { Fail-Closed 'HERDR_BIN_PATH is missing' }
    $pluginId = $env:HERDR_PLUGIN_ID
    if ([string]::IsNullOrWhiteSpace($pluginId)) { $pluginId = 'yuzora-wsl-agents' }
    & $herdr plugin pane open --plugin $pluginId --entrypoint $Entrypoint --placement tab --focus
    if ($LASTEXITCODE -ne 0) {
        Fail-Closed "herdr plugin pane open failed for $Entrypoint"
    }
}

if ($Kind -eq 'open-shell') { Open-PluginPane 'wsl-shell'; return }
if ($Kind -eq 'open-pi') { Open-PluginPane 'wsl-pi'; return }

$entry = $env:HERDR_PLUGIN_ENTRYPOINT_ID
if ([string]::IsNullOrWhiteSpace($Kind)) {
    if ($entry -eq 'wsl-pi') { $Kind = 'pi' }
    elseif ($entry -eq 'wsl-shell') { $Kind = 'shell' }
    else { Fail-Closed "unknown pane entrypoint: $entry" }
}

$config = Get-YuzoraPluginConfig
$plan = Resolve-LaunchPlan -Config $config
$wsl = Get-WslExe

if ($env:HERDR_SOCKET_PATH) {
    $env:YUZORA_HERDR_SOCKET_PATH = $env:HERDR_SOCKET_PATH
}
if ($plan.distro) {
    $env:YUZORA_WSL_DISTRO = $plan.distro
}

$env:WSLENV = Merge-WslEnv -Existing $env:WSLENV -Additions (Get-Win32ToWslEnvEntries)
if ($env:WSLENV -match '(?i)(^|:)HERDR_SOCKET_PATH(/|$)') {
    Fail-Closed 'WSLENV still contains HERDR_SOCKET_PATH after deny-list merge'
}

$argList = @()
if ($plan.distro) { $argList += @('--distribution', $plan.distro) }
if ($plan.linuxCwd) { $argList += @('--cd', $plan.linuxCwd) }
# Pi is often installed through Linuxbrew or another profile-managed toolchain.
# A direct `--exec pi` bypasses the login profile and cannot resolve it. The
# command string is fixed (no user input); exec replaces bash with Pi.
if ($Kind -eq 'pi') { $argList += @('--exec', 'bash', '-lic', 'exec pi') }

& $wsl @argList
exit $LASTEXITCODE
