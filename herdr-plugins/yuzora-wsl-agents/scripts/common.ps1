Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Write-YuzoraError {
    param([string]$Message)
    [Console]::Error.WriteLine("yuzora-wsl-agents: $Message")
}

function Fail-Closed {
    param([string]$Message)
    Write-YuzoraError $Message
    throw "yuzora-wsl-agents: $Message"
}

# StrictMode 2.0 throws on missing PSCustomObject members. JSON configs and
# plugin context snapshots are partial by contract, so optional fields must
# be read through PSObject.Properties rather than direct member access.
function Get-OptionalProperty {
    param(
        [AllowNull()]
        $Object,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )
    if ($null -eq $Object) { return $null }
    $psobject = $Object.PSObject
    if ($null -eq $psobject) { return $null }
    $prop = $psobject.Properties[$Name]
    if ($null -eq $prop) { return $null }
    return $prop.Value
}

function Get-WslExe {
    $systemRoot = $env:SystemRoot
    if (-not $systemRoot) { $systemRoot = 'C:\Windows' }
    $path = Join-Path $systemRoot 'System32\wsl.exe'
    if (-not (Test-Path -LiteralPath $path)) {
        Fail-Closed "wsl.exe not found at $path"
    }
    return $path
}

function ConvertFrom-WslListBytes {
    param([byte[]]$Bytes)
    if ($null -eq $Bytes -or $Bytes.Length -eq 0) { return @() }
    $looksUtf16 = $false
    if ($Bytes.Length -ge 2 -and $Bytes[0] -eq 0xFF -and $Bytes[1] -eq 0xFE) {
        $looksUtf16 = $true
    } else {
        $limit = [Math]::Min($Bytes.Length, 64)
        for ($i = 1; $i -lt $limit; $i += 2) {
            if ($Bytes[$i] -eq 0) { $looksUtf16 = $true; break }
        }
    }
    if ($looksUtf16) {
        $text = [System.Text.Encoding]::Unicode.GetString($Bytes)
    } else {
        $text = [System.Text.Encoding]::UTF8.GetString($Bytes)
    }
    $names = New-Object System.Collections.Generic.List[string]
    foreach ($line in ($text -split "(`r`n|`n|`r)")) {
        $trimmed = $line.Trim().Trim([char]0xFEFF, [char]0, [char]' ', [char]"`t", [char]"`r")
        if ($trimmed.Length -gt 0) { [void]$names.Add($trimmed) }
    }
    return $names.ToArray()
}

function Get-WslDistroInventory {
    $wsl = Get-WslExe
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $wsl
    $psi.Arguments = '--list --quiet'
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    [void]$proc.Start()
    $stdout = New-Object System.IO.MemoryStream
    try {
        $stderrTask = $proc.StandardError.ReadToEndAsync()
        $proc.StandardOutput.BaseStream.CopyTo($stdout)
        [void]$proc.WaitForExit()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        if ($proc.ExitCode -ne 0) {
            Fail-Closed "wsl.exe --list --quiet failed with exit $($proc.ExitCode): $($stderr.Trim())"
        }
        return @(ConvertFrom-WslListBytes -Bytes $stdout.ToArray())
    } finally {
        $stdout.Dispose()
        $proc.Dispose()
    }
}

function Test-SafeDistroName {
    param([string]$Name)
    if ([string]::IsNullOrWhiteSpace($Name)) { Fail-Closed 'distro name is empty' }
    if ($Name.Contains('\') -or $Name.Contains('/') -or $Name.Contains('..')) {
        Fail-Closed "unsafe distro name: $Name"
    }
}

function Get-YuzoraPluginConfig {
    $dir = $env:HERDR_PLUGIN_CONFIG_DIR
    if ([string]::IsNullOrWhiteSpace($dir)) {
        Fail-Closed 'HERDR_PLUGIN_CONFIG_DIR is missing'
    }
    $path = Join-Path $dir 'config.json'
    if (-not (Test-Path -LiteralPath $path)) {
        return [pscustomobject]@{
            schemaVersion = 1
            defaultDistro = $null
            distros = @()
            enabledAgents = @('pi')
            linuxCwdPolicy = 'workspace'
        }
    }
    $raw = Get-Content -LiteralPath $path -Encoding UTF8 -Raw
    $obj = $raw | ConvertFrom-Json
    foreach ($prop in $obj.PSObject.Properties.Name) {
        if (@('schemaVersion','defaultDistro','distros','enabledAgents','linuxCwdPolicy') -notcontains $prop) {
            Fail-Closed "unknown plugin config key: $prop"
        }
    }
    $schema = 1
    $schemaRaw = Get-OptionalProperty $obj 'schemaVersion'
    if ($null -ne $schemaRaw -and "$schemaRaw" -ne '') { $schema = [int]$schemaRaw }
    if ($schema -ne 1) { Fail-Closed "unsupported config schemaVersion: $schema" }
    $agents = @('pi')
    $agentsRaw = Get-OptionalProperty $obj 'enabledAgents'
    if ($null -ne $agentsRaw) { $agents = @($agentsRaw) }
    if ($agents.Count -ne 1 -or $agents[0] -ne 'pi') {
        Fail-Closed 'enabledAgents must be exactly ["pi"] in this MVP'
    }
    $policy = 'workspace'
    $policyRaw = Get-OptionalProperty $obj 'linuxCwdPolicy'
    if ($null -ne $policyRaw -and "$policyRaw" -ne '') {
        $policy = [string]$policyRaw
    }
    if ($policy -ne 'workspace' -and $policy -ne 'home') {
        Fail-Closed "invalid linuxCwdPolicy: $policy"
    }
    $distros = @()
    $distrosRaw = Get-OptionalProperty $obj 'distros'
    if ($null -ne $distrosRaw) {
        foreach ($name in @($distrosRaw)) {
            Test-SafeDistroName $name
            $distros += $name
        }
    }
    $defaultDistro = $null
    $defaultRaw = Get-OptionalProperty $obj 'defaultDistro'
    if ($null -ne $defaultRaw -and "$defaultRaw" -ne '') {
        Test-SafeDistroName ([string]$defaultRaw)
        $defaultDistro = [string]$defaultRaw
    }
    return [pscustomobject]@{
        schemaVersion = 1
        defaultDistro = $defaultDistro
        distros = $distros
        enabledAgents = @('pi')
        linuxCwdPolicy = $policy
    }
}

function Get-ConfiguredDistroNames {
    param($Config)
    if ($Config.distros -and $Config.distros.Count -gt 0) {
        return @($Config.distros)
    }
    if ($Config.defaultDistro) {
        return @($Config.defaultDistro)
    }
    return @()
}

function Resolve-TargetDistros {
    param($Config, [string[]]$Inventory)
    $wanted = @(Get-ConfiguredDistroNames -Config $Config)
    if ($wanted.Count -eq 0) {
        return @()
    }
    $resolved = @()
    foreach ($name in $wanted) {
        $match = $Inventory | Where-Object { $_.ToLower() -eq $name.ToLower() } | Select-Object -First 1
        if (-not $match) { Fail-Closed "WSL distro not installed: $name" }
        $resolved += $match
    }
    return $resolved
}

# Pane launch honors defaultDistro only. Adapter install/status still uses the
# full distros list; a stale secondary target must not block Open WSL Pi.
function Resolve-LaunchDistro {
    param($Config, [string[]]$Inventory)
    $wanted = $null
    $defaultRaw = Get-OptionalProperty $Config 'defaultDistro'
    if (-not [string]::IsNullOrWhiteSpace("$defaultRaw")) {
        $wanted = [string]$defaultRaw
    } else {
        $distrosRaw = Get-OptionalProperty $Config 'distros'
        $distros = @()
        if ($null -ne $distrosRaw) { $distros = @($distrosRaw) }
        if ($distros.Count -gt 0 -and -not [string]::IsNullOrWhiteSpace([string]$distros[0])) {
            $wanted = [string]$distros[0]
        }
    }
    if ([string]::IsNullOrWhiteSpace($wanted)) {
        return $null
    }
    Test-SafeDistroName $wanted
    $match = @($Inventory | Where-Object { $_.ToLower() -eq $wanted.ToLower() } | Select-Object -First 1)
    if ($match.Count -eq 0 -or [string]::IsNullOrWhiteSpace([string]$match[0])) {
        Fail-Closed "WSL distro not installed: $wanted"
    }
    return [string]$match[0]
}

function Invoke-AdapterActionAcrossDistros {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Action,
        [AllowEmptyCollection()]
        [string[]]$Targets
    )
    if ($null -eq $Targets -or $Targets.Count -eq 0) {
        Invoke-InDistroInstaller -Action $Action -Distro $null
        return
    }
    $failed = New-Object System.Collections.Generic.List[string]
    foreach ($distro in $Targets) {
        try {
            Invoke-InDistroInstaller -Action $Action -Distro $distro
        } catch {
            Write-YuzoraError "distro=$distro failed: $($_.Exception.Message)"
            [void]$failed.Add([string]$distro)
        }
    }
    if ($failed.Count -gt 0) {
        Fail-Closed ("adapter $Action failed for: " + ($failed -join ', '))
    }
}

function Get-PluginContextObject {
    if ([string]::IsNullOrWhiteSpace($env:HERDR_PLUGIN_CONTEXT_JSON)) {
        return $null
    }
    return $env:HERDR_PLUGIN_CONTEXT_JSON | ConvertFrom-Json
}

function Get-WorkspacePathFromContext {
    $ctx = Get-PluginContextObject
    if ($null -eq $ctx) { return $null }
    $workspace = Get-OptionalProperty $ctx 'workspace_cwd'
    if (-not [string]::IsNullOrWhiteSpace("$workspace")) { return [string]$workspace }
    $paneCwd = Get-OptionalProperty $ctx 'focused_pane_cwd'
    if (-not [string]::IsNullOrWhiteSpace("$paneCwd")) { return [string]$paneCwd }
    return $null
}

function Strip-VerbatimWindowsPrefix {
    param([string]$Path)
    if (-not $Path.StartsWith('\\?\')) { return $Path }
    $rest = $Path.Substring(4)
    if ($rest.Length -ge 4 -and $rest.Substring(0, 4).ToUpper() -eq 'UNC\') {
        return '\\' + $rest.Substring(4)
    }
    if ($rest.Length -ge 3 -and $rest[1] -eq ':' -and ($rest[2] -eq '\' -or $rest[2] -eq '/')) {
        return $rest
    }
    return $Path
}

function Trim-WindowsPathTrailingSeparators {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return $Path }
    if ($Path -match '^[A-Za-z]:[\\/]$') { return $Path }
    return $Path.TrimEnd([char[]]@('\', '/'))
}

function Parse-WslUnc {
    param([string]$Path)
    $normalized = (Strip-VerbatimWindowsPrefix $Path) -replace '/', '\'
    if (-not $normalized.StartsWith('\\')) { return $null }
    $parts = $normalized.Substring(2).Split('\') | Where-Object { $_ -ne '' }
    if ($parts.Count -lt 2) { return $null }
    $server = $parts[0]
    if ($server.ToLower() -ne 'wsl.localhost' -and $server.ToLower() -ne 'wsl$') {
        return $null
    }
    $distro = $parts[1].Trim()
    if ([string]::IsNullOrWhiteSpace($distro)) { return $null }
    $remainder = @()
    if ($parts.Count -gt 2) { $remainder = $parts[2..($parts.Count - 1)] }
    $linuxCwd = '/'
    if ($remainder.Count -gt 0) { $linuxCwd = '/' + ($remainder -join '/') }
    return [pscustomobject]@{ distro = $distro; linuxCwd = $linuxCwd }
}

function Merge-WslEnv {
    param(
        [string]$Existing,
        [string[]]$Additions
    )
    $parts = @()
    if ($Existing) {
        $parts = @($Existing.Split(':') | Where-Object { $_ -and $_.Trim() -ne '' })
    }
    $filtered = @()
    foreach ($entry in $parts) {
        $name = ($entry.Split('/'))[0].ToUpperInvariant()
        if ($name -eq 'HERDR_SOCKET_PATH') { continue }
        $filtered += $entry
    }
    $map = New-Object 'System.Collections.Specialized.OrderedDictionary'
    foreach ($entry in @($filtered + $Additions)) {
        if ([string]::IsNullOrWhiteSpace($entry)) { continue }
        $name = ($entry.Split('/'))[0].ToUpperInvariant()
        if ($map.Contains($name)) {
            $map[$name] = $entry
        } else {
            [void]$map.Add($name, $entry)
        }
    }
    return ($map.Values -join ':')
}

function Get-Win32ToWslEnvEntries {
    return @(
        'HERDR_ENV/u',
        'HERDR_PANE_ID/u',
        'HERDR_TAB_ID/u',
        'HERDR_WORKSPACE_ID/u',
        'HERDR_BIN_PATH/up',
        'YUZORA_HERDR_SOCKET_PATH/u',
        'YUZORA_WSL_DISTRO/u'
    )
}

function Convert-WindowsPathToLinux {
    param(
        [string]$Wsl,
        [string]$Distro,
        [string]$WindowsPath
    )
    $pathForWsl = Trim-WindowsPathTrailingSeparators $WindowsPath
    $argList = @()
    if ($Distro) { $argList += @('--distribution', $Distro) }
    $argList += @('--exec', 'wslpath', '-u', '--', $pathForWsl)
    $previousOutputEncoding = [Console]::OutputEncoding
    try {
        [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
        $out = & $Wsl @argList
        $exitCode = $LASTEXITCODE
    } finally {
        [Console]::OutputEncoding = $previousOutputEncoding
    }
    if ($exitCode -ne 0) {
        Fail-Closed "wslpath failed for $WindowsPath (distro=$Distro)"
    }
    return ([string]$out).Trim()
}

function Resolve-LaunchPlan {
    param($Config)
    $workspacePath = Get-WorkspacePathFromContext
    $inventory = @(Get-WslDistroInventory)
    $distro = Resolve-LaunchDistro -Config $Config -Inventory $inventory

    $linuxCwd = $null
    if ($Config.linuxCwdPolicy -eq 'home' -or [string]::IsNullOrWhiteSpace($workspacePath)) {
        return [pscustomobject]@{ distro = $distro; linuxCwd = $null }
    }

    $unc = Parse-WslUnc $workspacePath
    if ($null -ne $unc) {
        if ($distro -and $unc.distro.ToLower() -ne $distro.ToLower()) {
            Fail-Closed "WSL UNC distro '$($unc.distro)' does not match configured distro '$distro'"
        }
        if (-not $distro) { $distro = $unc.distro }
        return [pscustomobject]@{ distro = $distro; linuxCwd = $unc.linuxCwd }
    }

    $windowsPath = Strip-VerbatimWindowsPrefix $workspacePath
    $wsl = Get-WslExe
    $linuxCwd = Convert-WindowsPathToLinux -Wsl $wsl -Distro $distro -WindowsPath $windowsPath
    return [pscustomobject]@{ distro = $distro; linuxCwd = $linuxCwd }
}

function Convert-PluginRootToLinux {
    param(
        [string]$Wsl,
        [string]$Distro
    )
    $root = $env:HERDR_PLUGIN_ROOT
    if ([string]::IsNullOrWhiteSpace($root)) {
        Fail-Closed 'HERDR_PLUGIN_ROOT is missing'
    }
    $windowsRoot = Strip-VerbatimWindowsPrefix $root
    return Convert-WindowsPathToLinux -Wsl $Wsl -Distro $Distro -WindowsPath $windowsRoot
}

function Invoke-InDistroInstaller {
    param(
        [string]$Action,
        [string]$Distro
    )
    $wsl = Get-WslExe
    $linuxRoot = Convert-PluginRootToLinux -Wsl $wsl -Distro $Distro
    $installer = "$linuxRoot/adapters/install.sh"
    $argList = @()
    if ($Distro) { $argList += @('--distribution', $Distro) }
    $argList += @('--exec', 'sh', $installer, $Action, '--source-root', $linuxRoot)
    Write-Output "distro=$Distro action=$Action"
    & $wsl @argList
    if ($LASTEXITCODE -ne 0) {
        Fail-Closed "in-distro installer failed for distro=$Distro action=$Action"
    }
}
