param([Parameter(Mandatory = $true)][string]$BundleDir)
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$targets = @('linux-aarch64', 'linux-x86_64', 'macos-aarch64', 'macos-x86_64')
$sourceRoot = Join-Path $PSScriptRoot '..\src-tauri\resources\host'
$cleanupRoot = Join-Path $PSScriptRoot '..\src-tauri\resources\legacy-cleanup'
$runtimeLock = Get-Content -Raw (Join-Path $PSScriptRoot '..\src-tauri\herdr-runtime.json') | ConvertFrom-Json

function Get-OneFile([string]$Pattern) {
    $files = @(Get-ChildItem -Path $Pattern -File)
    if ($files.Count -ne 1) { throw "Expected one installer at $Pattern" }
    return $files[0]
}

function Assert-Payload([string]$Root) {
    $files = @(Get-ChildItem -LiteralPath $Root -Recurse -File)
    $nativeBinaries = @($files | Where-Object { $_.FullName -match '[\\/]herdr[\\/]windows-x86_64[\\/]herdr.exe$' })
    if ($nativeBinaries.Count -ne 1) { throw 'Expected exactly one native Windows HERDR payload' }
    $nativeRoot = Split-Path -Parent $nativeBinaries[0].FullName
    $nativeExpected = @($runtimeLock.targets.'windows-x86_64'.files | ForEach-Object { $_.path.Replace('/', '\') })
    $nativeActual = @(Get-ChildItem -LiteralPath $nativeRoot -Recurse -File | ForEach-Object { $_.FullName.Substring($nativeRoot.Length + 1).Replace('/', '\') })
    if (@(Compare-Object ($nativeExpected | Sort-Object) ($nativeActual | Sort-Object) -CaseSensitive).Count -ne 0) { throw 'Unexpected native runtime inventory' }
    foreach ($entry in $runtimeLock.targets.'windows-x86_64'.files) {
        if ((Get-FileHash -LiteralPath (Join-Path $nativeRoot $entry.path) -Algorithm SHA256).Hash -ne $entry.sha256) { throw "Native runtime hash mismatch: $($entry.path)" }
    }
    $license = Join-Path (Split-Path -Parent $nativeRoot) 'LICENSE-HERDR.txt'
    if ((Get-FileHash -LiteralPath $license -Algorithm SHA256).Hash -ne $runtimeLock.licenseSha256) { throw 'Native HERDR license mismatch' }
    foreach ($file in $files) {
        $approvedNative = $file.FullName.StartsWith($nativeRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
        if ($file.Name -in @('herdr-plugin.toml', 'yuzora-herdr-wsl.ts') -or (!$approvedNative -and $file.Name -in @('herdr.exe', 'OpenConsole.exe', 'conpty.dll'))) {
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
    Write-Output 'MSI and NSIS contain pinned native Windows HERDR and verified Unix host payloads; no legacy WSL plugin'
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
