# Helper for Windows hosts. Does not constitute P8 acceptance.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Write-Host 'yuzora-wsl-agents Windows helper'
Write-Host "plugin root: $root"
Write-Host 'P8 Windows 11/WSL2 acceptance remains user-owned. This script does not claim PASS.'

$ps = @(
    (Join-Path $root 'scripts\common.ps1'),
    (Join-Path $root 'scripts\open-pane.ps1'),
    (Join-Path $root 'scripts\manage-adapters.ps1'),
    (Join-Path $root 'scripts\check-status.ps1')
)
foreach ($file in $ps) {
    $tokens = $null
    $errs = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($file, [ref]$tokens, [ref]$errs)
    if ($errs -and $errs.Count -gt 0) {
        throw "parse failed: $file"
    }
    Write-Host "parsed $file"
}

$wsl = Join-Path $env:SystemRoot 'System32\wsl.exe'
if (Test-Path -LiteralPath $wsl) {
    $reporter = Join-Path $root 'adapters\common\herdr-wsl-report'
    $linuxReporter = (& $wsl --exec wslpath -u -- $reporter)
    if ($LASTEXITCODE -ne 0) { throw "wslpath failed for $reporter" }
    & $wsl --exec sh -n ([string]$linuxReporter).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'in-distro reporter syntax check failed' }
    Write-Host 'wsl is present; in-distro checks still require the user P8 matrix'
} else {
    Write-Host 'wsl.exe not found; skipped in-distro syntax check'
}

. (Join-Path $root 'scripts\common.ps1')
$script:attempts = @()
function Invoke-InDistroInstaller {
    param([string]$Action, [string]$Distro)
    $script:attempts += $Distro
    if ($Distro -eq 'offline-distro') { throw 'offline' }
    Write-Host "ok $Distro"
}
try {
    Invoke-AdapterActionAcrossDistros -Action install -Targets @('offline-distro', 'Ubuntu')
    throw 'expected aggregate failure after the first distro failed'
} catch {
    if ($_.Exception.Message -notmatch 'offline-distro') { throw }
}
if (($script:attempts -join ',') -ne 'offline-distro,Ubuntu') {
    throw "expected both distros to run; got $($script:attempts -join ',')"
}
Write-Host 'multi-distro continue: PASS'
