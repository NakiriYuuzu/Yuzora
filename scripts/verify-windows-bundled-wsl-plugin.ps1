param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [string]$SourcePluginRoot
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$PluginId = 'yuzora-wsl-agents'
$ExpectedFiles = @(
    'README.md',
    'adapters\common\herdr-wsl-report',
    'adapters\install.sh',
    'adapters\pi\yuzora-herdr-wsl.ts',
    'herdr-plugin.toml',
    'scripts\check-status.ps1',
    'scripts\common.ps1',
    'scripts\manage-adapters.ps1',
    'scripts\manage-bundled-plugin.ps1',
    'scripts\open-pane.ps1'
) | Sort-Object

function Fail-Verification {
    param([string]$Message)
    throw "Windows bundled WSL plugin verification failed: $Message"
}

function Get-ExactlyOneFile {
    param(
        [string]$Label,
        [string]$Pattern
    )
    $matches = @(Get-ChildItem -Path $Pattern -File -ErrorAction SilentlyContinue)
    if ($matches.Count -ne 1) {
        Fail-Verification "$Label expected exactly one file at $Pattern; found $($matches.Count)"
    }
    return $matches[0]
}

function Get-RelativeFileInventory {
    param([string]$Root)
    $prefix = $Root.TrimEnd([char[]]@('\', '/')) + [System.IO.Path]::DirectorySeparatorChar
    return @(
        Get-ChildItem -LiteralPath $Root -Recurse -File |
            ForEach-Object {
                $_.FullName.Substring($prefix.Length).Replace('/', '\')
            } |
            Sort-Object
    )
}

function Assert-ExactInventory {
    param(
        [string]$Label,
        [string[]]$Actual
    )
    $delta = @(Compare-Object -ReferenceObject $ExpectedFiles -DifferenceObject $Actual -CaseSensitive)
    if ($delta.Count -gt 0) {
        $detail = @($delta | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }) -join ', '
        Fail-Verification "$Label payload differs from the exact allowlist: $detail"
    }
}

function Assert-PowerShellSyntax {
    param(
        [string]$Label,
        [string]$PluginRoot
    )
    foreach ($script in @(Get-ChildItem -LiteralPath (Join-Path $PluginRoot 'scripts') -File -Filter '*.ps1')) {
        $tokens = $null
        $parseErrors = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile(
            $script.FullName,
            [ref]$tokens,
            [ref]$parseErrors
        )
        if ($null -ne $parseErrors -and $parseErrors.Count -gt 0) {
            $messages = @($parseErrors | ForEach-Object { $_.Message }) -join '; '
            Fail-Verification "$Label PowerShell syntax error in $($script.Name): $messages"
        }
    }
}

function Assert-PluginPayload {
    param(
        [string]$Label,
        [string]$ExtractedRoot,
        [string]$SourceRoot
    )
    $manifests = @(
        Get-ChildItem -LiteralPath $ExtractedRoot -Recurse -File -Filter 'herdr-plugin.toml' |
            Where-Object {
                $_.FullName -match '[\\/]herdr-plugins[\\/]yuzora-wsl-agents[\\/]herdr-plugin\.toml$'
            }
    )
    if ($manifests.Count -ne 1) {
        Fail-Verification "$Label expected exactly one bundled $PluginId manifest; found $($manifests.Count)"
    }
    $pluginRoot = Split-Path -Parent $manifests[0].FullName
    $actualFiles = @(Get-RelativeFileInventory -Root $pluginRoot)
    Assert-ExactInventory -Label $Label -Actual $actualFiles
    Assert-PowerShellSyntax -Label $Label -PluginRoot $pluginRoot

    foreach ($relativePath in $ExpectedFiles) {
        $sourcePath = Join-Path $SourceRoot $relativePath
        $bundledPath = Join-Path $pluginRoot $relativePath
        $sourceHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash
        $bundledHash = (Get-FileHash -LiteralPath $bundledPath -Algorithm SHA256).Hash
        if ($sourceHash -ne $bundledHash) {
            Fail-Verification "$Label hash mismatch for $relativePath"
        }
    }

    $appRoot = Split-Path -Parent (Split-Path -Parent $pluginRoot)
    $herdrPath = Join-Path $appRoot 'herdr\windows-x86_64\herdr.exe'
    if (-not (Test-Path -LiteralPath $herdrPath -PathType Leaf)) {
        Fail-Verification "$Label is missing adjacent Yuzora-managed herdr.exe"
    }
    return [pscustomobject]@{
        label = $Label
        pluginRoot = $pluginRoot
        helperPath = (Join-Path $pluginRoot 'scripts\manage-bundled-plugin.ps1')
        herdrPath = $herdrPath
        requiredFiles = $ExpectedFiles.Count
    }
}

function Invoke-BundledHelper {
    param(
        $Payload,
        [ValidateSet('status', 'link', 'unlink')]
        [string]$Action,
        [bool]$ExpectSuccess
    )
    $output = @(
        & powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File $Payload.helperPath `
            -Action $Action -HerdrPath $Payload.herdrPath 2>&1
    )
    $exitCode = $LASTEXITCODE
    $text = @($output | ForEach-Object { [string]$_ }) -join [Environment]::NewLine
    if ($ExpectSuccess -and $exitCode -ne 0) {
        Fail-Verification "$($Payload.label) helper $Action failed (exit $exitCode): $text"
    }
    if (-not $ExpectSuccess -and $exitCode -eq 0) {
        Fail-Verification "$($Payload.label) helper $Action unexpectedly succeeded"
    }
    if (-not $ExpectSuccess) { return $text }
    try {
        return ($text | ConvertFrom-Json)
    } catch {
        Fail-Verification "$($Payload.label) helper $Action returned invalid JSON: $text"
    }
}

function Assert-OwnedStatus {
    param(
        [string]$Label,
        $Status,
        [bool]$Linked
    )
    if ([bool]$Status.linked -ne $Linked) {
        Fail-Verification "$Label linked status mismatch"
    }
    if ($Linked -and -not [bool]$Status.ownsRegistration) {
        Fail-Verification "$Label did not own its linked registration"
    }
}

if ($env:OS -ne 'Windows_NT') {
    Fail-Verification 'this verifier must run on Windows PowerShell'
}
if ($PSVersionTable.PSVersion.Major -ne 5) {
    Fail-Verification "expected Windows PowerShell 5.1, got $($PSVersionTable.PSVersion)"
}

$resolvedBundleDir = [System.IO.Path]::GetFullPath($BundleDir)
if (-not (Test-Path -LiteralPath $resolvedBundleDir -PathType Container)) {
    Fail-Verification "bundle directory not found: $resolvedBundleDir"
}
if ([string]::IsNullOrWhiteSpace($SourcePluginRoot)) {
    $repositoryRoot = Split-Path -Parent $PSScriptRoot
    $SourcePluginRoot = Join-Path $repositoryRoot 'herdr-plugins\yuzora-wsl-agents'
}
$resolvedSourceRoot = [System.IO.Path]::GetFullPath($SourcePluginRoot)
if (-not (Test-Path -LiteralPath $resolvedSourceRoot -PathType Container)) {
    Fail-Verification "source plugin root not found: $resolvedSourceRoot"
}
Assert-ExactInventory -Label 'source plugin runtime' -Actual @(
    $ExpectedFiles | Where-Object { Test-Path -LiteralPath (Join-Path $resolvedSourceRoot $_) -PathType Leaf }
)

$msi = Get-ExactlyOneFile -Label 'MSI' -Pattern (Join-Path $resolvedBundleDir 'msi\*.msi')
$nsis = Get-ExactlyOneFile -Label 'NSIS' -Pattern (Join-Path $resolvedBundleDir 'nsis\*setup.exe')
$sevenZip = Get-Command '7z.exe' -CommandType Application -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($null -eq $sevenZip) {
    Fail-Verification '7z.exe is required to inspect the NSIS candidate'
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("yuzora-wsl-plugin-" + [Guid]::NewGuid().ToString('N'))
$msiRoot = Join-Path $tempRoot 'msi'
$nsisRoot = Join-Path $tempRoot 'nsis'
New-Item -ItemType Directory -Path $msiRoot, $nsisRoot -Force | Out-Null
$msiPayload = $null
$nsisPayload = $null

try {
    $msiArgs = @(
        '/a',
        ('"{0}"' -f $msi.FullName),
        '/qn',
        ('TARGETDIR="{0}"' -f $msiRoot)
    )
    $msiProcess = Start-Process -FilePath 'msiexec.exe' -ArgumentList $msiArgs -Wait -PassThru
    if ($msiProcess.ExitCode -ne 0 -and $msiProcess.ExitCode -ne 3010) {
        Fail-Verification "MSI administrative extraction exited $($msiProcess.ExitCode)"
    }

    & $sevenZip.Source x -y ("-o{0}" -f $nsisRoot) $nsis.FullName | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Fail-Verification "NSIS 7-Zip extraction exited $LASTEXITCODE"
    }

    $msiPayload = Assert-PluginPayload -Label 'MSI' -ExtractedRoot $msiRoot -SourceRoot $resolvedSourceRoot
    $nsisPayload = Assert-PluginPayload -Label 'NSIS' -ExtractedRoot $nsisRoot -SourceRoot $resolvedSourceRoot

    $initial = Invoke-BundledHelper -Payload $msiPayload -Action status -ExpectSuccess $true
    Assert-OwnedStatus -Label 'initial registry' -Status $initial -Linked $false

    $msiLinked = Invoke-BundledHelper -Payload $msiPayload -Action link -ExpectSuccess $true
    Assert-OwnedStatus -Label 'MSI link' -Status $msiLinked -Linked $true

    $foreignLink = Invoke-BundledHelper -Payload $nsisPayload -Action link -ExpectSuccess $false
    if ($foreignLink -notmatch 'already registered from another root') {
        Fail-Verification "NSIS helper did not explain foreign-root link refusal: $foreignLink"
    }
    $foreignUnlink = Invoke-BundledHelper -Payload $nsisPayload -Action unlink -ExpectSuccess $false
    if ($foreignUnlink -notmatch 'refusing to unlink') {
        Fail-Verification "NSIS helper did not explain foreign-root unlink refusal: $foreignUnlink"
    }

    $msiUnlinked = Invoke-BundledHelper -Payload $msiPayload -Action unlink -ExpectSuccess $true
    Assert-OwnedStatus -Label 'MSI unlink' -Status $msiUnlinked -Linked $false

    $nsisLinked = Invoke-BundledHelper -Payload $nsisPayload -Action link -ExpectSuccess $true
    Assert-OwnedStatus -Label 'NSIS link' -Status $nsisLinked -Linked $true
    $nsisUnlinked = Invoke-BundledHelper -Payload $nsisPayload -Action unlink -ExpectSuccess $true
    Assert-OwnedStatus -Label 'NSIS unlink' -Status $nsisUnlinked -Linked $false

    Write-Output (@($msiPayload, $nsisPayload) | ConvertTo-Json -Compress)
} finally {
    foreach ($payload in @($msiPayload, $nsisPayload)) {
        if ($null -eq $payload) { continue }
        try {
            $status = Invoke-BundledHelper -Payload $payload -Action status -ExpectSuccess $true
            if ([bool]$status.ownsRegistration) {
                [void](Invoke-BundledHelper -Payload $payload -Action unlink -ExpectSuccess $true)
            }
        } catch {
            [Console]::Error.WriteLine("cleanup warning: $($_.Exception.Message)")
        }
    }
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
