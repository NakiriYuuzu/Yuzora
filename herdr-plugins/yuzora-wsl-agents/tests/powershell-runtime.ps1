Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $root 'scripts\common.ps1')

function Assert-DistroBytes {
    param(
        [string]$Label,
        [byte[]]$Bytes
    )
    $actual = @(ConvertFrom-WslListBytes -Bytes $Bytes)
    $expected = @('Ubuntu-26.04', '測試發行版')
    if ($actual.Count -ne $expected.Count) {
        throw "$Label count mismatch: $($actual.Count)"
    }
    for ($i = 0; $i -lt $expected.Count; $i += 1) {
        if ($actual[$i] -ne $expected[$i]) {
            throw "$Label item $i mismatch: '$($actual[$i])'"
        }
    }
}

$text = "Ubuntu-26.04`r`n測試發行版`r`n"
$utf8 = New-Object System.Text.UTF8Encoding($false)
Assert-DistroBytes -Label 'UTF-8' -Bytes $utf8.GetBytes($text)

$utf8Bom = New-Object System.Text.UTF8Encoding($true)
$utf8BomBytes = [byte[]](@($utf8Bom.GetPreamble()) + @($utf8Bom.GetBytes($text)))
Assert-DistroBytes -Label 'UTF-8 BOM' -Bytes $utf8BomBytes

$utf16 = New-Object System.Text.UnicodeEncoding -ArgumentList $false, $true
$utf16Bytes = [byte[]](@($utf16.GetPreamble()) + @($utf16.GetBytes($text)))
Assert-DistroBytes -Label 'UTF-16LE BOM' -Bytes $utf16Bytes

Write-Output 'Windows PowerShell runtime fixtures PASS'
