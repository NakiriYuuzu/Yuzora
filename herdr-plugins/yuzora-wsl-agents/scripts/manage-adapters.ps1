param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('install','status','uninstall')]
    [string]$Action,

    [switch]$AllInstalledDistros
)

$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\common.ps1"

if ($AllInstalledDistros) {
    if ($Action -eq 'install') {
        Fail-Closed 'AllInstalledDistros is allowed only for status or uninstall'
    }
    $inventory = @(Get-WslDistroInventory)
    if ($inventory.Count -eq 0) {
        Write-Output 'absent'
        return
    }
    Invoke-AdapterActionAcrossDistros -Action $Action -Targets $inventory
    return
}

$config = Get-YuzoraPluginConfig
$targets = @(Get-ConfiguredDistroNames -Config $config)

if ($targets.Count -eq 0) {
    Write-Output 'No distros listed in config; using the WSL default distro.'
    Invoke-InDistroInstaller -Action $Action -Distro $null
    return
}

Invoke-AdapterActionAcrossDistros -Action $Action -Targets $targets
