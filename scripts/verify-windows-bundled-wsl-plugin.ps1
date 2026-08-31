param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [string]$SourcePluginRoot
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$PluginId = 'yuzora-wsl-agents'
$ExpectedHerdrVersion = '0.8.2'
$ExpectedHerdrProtocol = 20
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
    $identityResult = Invoke-NativeCommand -Executable $herdrPath -Arguments @(
        'status', 'client', '--json'
    )
    if ($identityResult.exitCode -ne 0) {
        Fail-Verification "$Label packaged Herdr identity failed (exit $($identityResult.exitCode)): $($identityResult.text)"
    }
    try {
        $identity = $identityResult.text | ConvertFrom-Json
    } catch {
        Fail-Verification "$Label packaged Herdr identity returned invalid JSON: $($identityResult.text)"
    }
    $versionProperty = $identity.PSObject.Properties['version']
    $protocolProperty = $identity.PSObject.Properties['protocol']
    if ($null -eq $versionProperty -or [string]$versionProperty.Value -ne $ExpectedHerdrVersion) {
        Fail-Verification "$Label packaged Herdr version is not $ExpectedHerdrVersion"
    }
    if ($null -eq $protocolProperty -or [int]$protocolProperty.Value -ne $ExpectedHerdrProtocol) {
        Fail-Verification "$Label packaged Herdr protocol is not $ExpectedHerdrProtocol"
    }
    return [pscustomobject]@{
        label = $Label
        pluginRoot = $pluginRoot
        helperPath = (Join-Path $pluginRoot 'scripts\manage-bundled-plugin.ps1')
        herdrPath = $herdrPath
        herdrVersion = [string]$versionProperty.Value
        herdrProtocol = [int]$protocolProperty.Value
        requiredFiles = $ExpectedFiles.Count
    }
}

function Invoke-NativeCommand {
    param(
        [string]$Executable,
        [string[]]$Arguments
    )
    $previousErrorActionPreference = $ErrorActionPreference
    $output = @()
    $exitCode = -1
    try {
        # Windows PowerShell 5.1 turns native stderr into ErrorRecord objects.
        # Expected non-zero commands must remain observable instead of becoming
        # terminating errors under the verifier's fail-closed preference.
        $ErrorActionPreference = 'Continue'
        $output = @(& $Executable @Arguments 2>&1)
        if ($null -ne $LASTEXITCODE) { $exitCode = [int]$LASTEXITCODE }
    } catch {
        $output += $_
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    return [pscustomobject]@{
        exitCode = $exitCode
        text = (@($output | ForEach-Object { [string]$_ }) -join [Environment]::NewLine)
    }
}

function Test-HerdrServerReady {
    param([string]$HerdrPath)
    $result = Invoke-NativeCommand -Executable $HerdrPath -Arguments @('status', '--json')
    if ($result.exitCode -ne 0 -or [string]::IsNullOrWhiteSpace($result.text)) {
        return $false
    }
    try {
        $status = $result.text | ConvertFrom-Json
    } catch {
        return $false
    }
    $server = $status.PSObject.Properties['server']
    if ($null -eq $server) { return $false }
    $running = $server.Value.PSObject.Properties['running']
    return ($null -ne $running -and [bool]$running.Value)
}

function Start-IsolatedHerdrServer {
    param([string]$HerdrPath)
    $process = Start-Process -FilePath $HerdrPath -ArgumentList @('server') `
        -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-HerdrServerReady -HerdrPath $HerdrPath) {
            return $process
        }
        if ($process.HasExited) {
            Fail-Verification "isolated Herdr server exited $($process.ExitCode) before becoming ready"
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    Fail-Verification 'isolated Herdr server did not become ready within 15 seconds'
}

function Stop-IsolatedHerdrServer {
    param(
        [string]$HerdrPath,
        $Process
    )
    $result = Invoke-NativeCommand -Executable $HerdrPath -Arguments @('server', 'stop')
    if ($result.exitCode -ne 0) {
        Fail-Verification "isolated Herdr server stop failed (exit $($result.exitCode)): $($result.text)"
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Test-HerdrServerReady -HerdrPath $HerdrPath)) { break }
        Start-Sleep -Milliseconds 100
    }
    if (Test-HerdrServerReady -HerdrPath $HerdrPath) {
        Fail-Verification 'isolated Herdr server remained ready after stop'
    }
    if ($null -ne $Process -and -not $Process.HasExited -and -not $Process.WaitForExit(5000)) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
        Fail-Verification 'isolated Herdr server process did not exit after stop'
    }
}

function Invoke-BundledHelper {
    param(
        $Payload,
        [ValidateSet('status', 'link', 'unlink')]
        [string]$Action,
        [bool]$ExpectSuccess
    )
    $powershell = Join-Path $PSHOME 'powershell.exe'
    $result = Invoke-NativeCommand -Executable $powershell -Arguments @(
        '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', $Payload.helperPath, '-Action', $Action, '-HerdrPath', $Payload.herdrPath
    )
    $exitCode = $result.exitCode
    $text = $result.text
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
$isolationConfigRoot = Join-Path $tempRoot 'xdg-config'
$isolationStateRoot = Join-Path $tempRoot 'xdg-state'
New-Item -ItemType Directory -Path $msiRoot, $nsisRoot, $isolationConfigRoot, $isolationStateRoot -Force | Out-Null
$msiPayload = $null
$nsisPayload = $null
$serverProcess = $null
$environmentNames = @(
    'XDG_CONFIG_HOME',
    'XDG_STATE_HOME',
    'HERDR_CONFIG_PATH',
    'HERDR_SESSION',
    'HERDR_SOCKET_PATH',
    'HERDR_CLIENT_SOCKET_PATH'
)
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
    $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
[Environment]::SetEnvironmentVariable('XDG_CONFIG_HOME', $isolationConfigRoot, 'Process')
[Environment]::SetEnvironmentVariable('XDG_STATE_HOME', $isolationStateRoot, 'Process')
[Environment]::SetEnvironmentVariable(
    'HERDR_SOCKET_PATH',
    (Join-Path $tempRoot 'isolated-herdr.sock'),
    'Process'
)
foreach ($name in @('HERDR_CONFIG_PATH', 'HERDR_SESSION', 'HERDR_CLIENT_SOCKET_PATH')) {
    [Environment]::SetEnvironmentVariable($name, $null, 'Process')
}

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
    $serverProcess = Start-IsolatedHerdrServer -HerdrPath $msiPayload.herdrPath

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

    Stop-IsolatedHerdrServer -HerdrPath $msiPayload.herdrPath -Process $serverProcess
    $serverProcess = $null
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
    if ($null -ne $serverProcess) {
        try {
            [void](Invoke-NativeCommand -Executable $msiPayload.herdrPath -Arguments @('server', 'stop'))
        } catch {
            [Console]::Error.WriteLine("server cleanup warning: $($_.Exception.Message)")
        }
        if (-not $serverProcess.HasExited) {
            Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
        }
    }
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
