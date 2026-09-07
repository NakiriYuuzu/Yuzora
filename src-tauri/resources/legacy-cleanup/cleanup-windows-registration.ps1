param(
    [Parameter(Mandatory = $true)][string]$LegacyResourceRoot,
    [Parameter(Mandatory = $true)][string]$HerdrPath,
    [switch]$Apply
)
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
# Only the known Yuzora plugin registration is eligible. No server start/stop,
# filesystem recursion, session deletion, hooks, or external runtime removal.
$pluginRoot = [IO.Path]::GetFullPath((Join-Path $LegacyResourceRoot 'herdr-plugins\yuzora-wsl-agents'))
$manifest = Join-Path $pluginRoot 'herdr-plugin.toml'
if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) { throw 'Legacy Yuzora manifest is unavailable; ownership cannot be verified' }
if ((Get-Item -LiteralPath $manifest).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Reparse-point manifest rejected' }
if ((Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'fde979b251f8d3e471a5994e0d151a03b56e1905114234506bb780fbcc6f3415') { throw 'Manifest differs from the released Yuzora plugin; preserved' }
function Invoke-HerdrJson([string[]]$Arguments) {
    $output = & $HerdrPath @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'HERDR is unavailable; existing server state was preserved' }
    return (($output -join "`n") | ConvertFrom-Json)
}
$status = Invoke-HerdrJson @('status', '--json')
if (-not $status.server.running) { throw 'The legacy HERDR server is stopped; cleanup will not start it' }
$list = Invoke-HerdrJson @('plugin', 'list', '--plugin', 'yuzora-wsl-agents', '--json')
$plugins = @($list.result.plugins)
if ($plugins.Count -eq 0) { Write-Output 'absent: Yuzora WSL plugin registration'; exit 0 }
if ($plugins.Count -ne 1) { throw 'Ambiguous plugin registration; preserved' }
$registered = [IO.Path]::GetFullPath([string]$plugins[0].plugin_root).TrimEnd('\', '/')
if (-not [string]::Equals($registered, $pluginRoot.TrimEnd('\', '/'), [StringComparison]::OrdinalIgnoreCase)) { throw 'Plugin registration belongs to another root; preserved' }
if ($Apply) {
    & $HerdrPath plugin unlink yuzora-wsl-agents
    if ($LASTEXITCODE -ne 0) { throw 'Plugin unlink failed; do not replay automatically' }
    Write-Output 'removed: Yuzora WSL plugin registration'
} else { Write-Output 'would remove: Yuzora WSL plugin registration' }
