param([Parameter(Mandatory = $true)][string]$BundleDir)
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$targets = @('linux-aarch64', 'linux-x86_64', 'macos-aarch64', 'macos-x86_64')
$sourceRoot = Join-Path $PSScriptRoot '..\src-tauri\resources\host'
$cleanupRoot = Join-Path $PSScriptRoot '..\src-tauri\resources\legacy-cleanup'

function Get-OneFile([string]$Pattern) {
    $files = @(Get-ChildItem -Path $Pattern -File)
    if ($files.Count -ne 1) { throw "Expected one installer at $Pattern" }
    return $files[0]
}

function Assert-Payload([string]$Root) {
    $files = @(Get-ChildItem -LiteralPath $Root -Recurse -File)
    foreach ($file in $files) {
        if ($file.Name -in @('herdr.exe', 'OpenConsole.exe', 'conpty.dll', 'herdr-plugin.toml', 'yuzora-herdr-wsl.ts')) {
            throw "Legacy runtime must not be bundled: $($file.FullName)"
        }
    }
    $manifests = @($files | Where-Object { $_.FullName -match '[\\/]host[\\/]linux-x86_64.json$' })
    if ($manifests.Count -ne 1) { throw 'Expected exactly one host deployment payload' }
    $root = Split-Path -Parent $manifests[0].FullName
    $expected = @('LICENSE-HERDR.txt')
    foreach ($target in $targets) {
        $expected += "$target.json", "$target\yuzora-host", "$target\herdr"
    }
    $actual = @(Get-ChildItem -LiteralPath $root -Recurse -File | ForEach-Object { $_.FullName.Substring($root.Length + 1).Replace('/', '\') })
    if (@(Compare-Object ($expected | Sort-Object) ($actual | Sort-Object) -CaseSensitive).Count -ne 0) { throw 'Unexpected runtime payload inventory' }
    foreach ($relative in $expected) {
        $sourceHash = (Get-FileHash -LiteralPath (Join-Path $sourceRoot $relative) -Algorithm SHA256).Hash
        $bundledHash = (Get-FileHash -LiteralPath (Join-Path $root $relative) -Algorithm SHA256).Hash
        if ($sourceHash -ne $bundledHash) { throw "Runtime payload hash mismatch: $relative" }
    }
    $appRoot = Split-Path -Parent $root
    foreach ($script in @(Get-ChildItem -LiteralPath $cleanupRoot -File)) {
        $bundled = Join-Path (Join-Path $appRoot 'legacy-cleanup') $script.Name
        if ((Get-FileHash -LiteralPath $script.FullName -Algorithm SHA256).Hash -ne (Get-FileHash -LiteralPath $bundled -Algorithm SHA256).Hash) { throw "Cleanup payload mismatch: $($script.Name)" }
    }
}

$msi = Get-OneFile (Join-Path $BundleDir 'msi\*.msi')
$nsis = Get-OneFile (Join-Path $BundleDir 'nsis\*setup.exe')
$sevenZip = Get-Command '7z.exe' -CommandType Application -ErrorAction Stop
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('yuzora-runtime-' + [Guid]::NewGuid().ToString('N'))
$msiRoot = Join-Path $tempRoot 'msi'
$nsisRoot = Join-Path $tempRoot 'nsis'
New-Item -ItemType Directory -Path $msiRoot, $nsisRoot | Out-Null
try {
    $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/a', ('"{0}"' -f $msi.FullName), '/qn', ('TARGETDIR="{0}"' -f $msiRoot)) -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) { throw "MSI administrative extraction failed: $($process.ExitCode)" }
    & $sevenZip.Source x -y ("-o{0}" -f $nsisRoot) $nsis.FullName | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "NSIS extraction failed: $LASTEXITCODE" }
    Assert-Payload $msiRoot
    Assert-Payload $nsisRoot
    Write-Output 'MSI and NSIS contain verified Unix runtime payloads with no Windows HERDR bridge'
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
