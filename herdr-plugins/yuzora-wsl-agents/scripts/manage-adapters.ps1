param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('install','status','uninstall')]
    [string]$Action
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\common.ps1"

$config = Get-YuzoraPluginConfig
$targets = @(Get-ConfiguredDistroNames -Config $config)

if ($targets.Count -eq 0) {
    Write-Output 'No distros listed in config; using the WSL default distro.'
    Invoke-InDistroInstaller -Action $Action -Distro $null
    return
}

Invoke-AdapterActionAcrossDistros -Action $Action -Targets $targets
