# yuzora(L7): adds or removes Yuzora's folder on the user PATH, for the
# installer (yuzora.nsi):
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File path.ps1 -Add <dir>
#   powershell -NoProfile -ExecutionPolicy Bypass -File path.ps1 -Remove <dir>
#
# The raw user PATH, so %VARIABLES% in it stay unexpanded and keep their type
# (as upstream's distribution/install.ps1 does).
param([string]$Add, [string]$Remove)
$ErrorActionPreference = "Stop"

$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
try {
    $raw = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $kind = if ($key.GetValueNames() -contains "Path") { $key.GetValueKind("Path") } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
    $dir = if ($Add) { $Add } else { $Remove }
    if (-not $dir) { throw "path.ps1: -Add <dir> or -Remove <dir>" }
    $same = { param($entry) $entry.Trim().TrimEnd("\") -ieq $dir.TrimEnd("\") }
    $entries = @($raw -split ";" | Where-Object { $_ -ne "" })
    if ($Add) {
        if ($entries | Where-Object { & $same $_ }) { return }
        $entries += $dir
    } else {
        $kept = @($entries | Where-Object { -not (& $same $_) })
        if ($kept.Count -eq $entries.Count) { return }
        $entries = $kept
    }
    $key.SetValue("Path", ($entries -join ";"), $kind)
    # Setting any user variable tells running programs the environment changed.
    [Environment]::SetEnvironmentVariable("YUZORA_INSTALLING", "1", "User")
    [Environment]::SetEnvironmentVariable("YUZORA_INSTALLING", $null, "User")
} finally {
    $key.Close()
}
