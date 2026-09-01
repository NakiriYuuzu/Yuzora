param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('status', 'link', 'unlink')]
    [string]$Action,

    [string]$HerdrPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$PluginId = 'yuzora-wsl-agents'
$PluginRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))

function Fail-BundledPlugin {
    param([string]$Message)
    [Console]::Error.WriteLine("yuzora-wsl-agents: $Message")
    throw "yuzora-wsl-agents: $Message"
}

function Strip-VerbatimPathPrefix {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path) -or -not $Path.StartsWith('\\?\')) {
        return $Path
    }
    $rest = $Path.Substring(4)
    if ($rest.Length -ge 4 -and $rest.Substring(0, 4).ToUpperInvariant() -eq 'UNC\') {
        return '\\' + $rest.Substring(4)
    }
    return $rest
}

function Normalize-ComparablePath {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return $null }
    $normalized = Strip-VerbatimPathPrefix ([System.IO.Path]::GetFullPath($Path))
    $root = [System.IO.Path]::GetPathRoot($normalized)
    if ($normalized -ne $root) {
        $normalized = $normalized.TrimEnd([char[]]@('\', '/'))
    }
    if ($env:OS -eq 'Windows_NT') {
        return $normalized.ToLowerInvariant()
    }
    return $normalized
}

function Resolve-HerdrExecutable {
    param([string]$ExplicitPath)

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        $explicit = [System.IO.Path]::GetFullPath($ExplicitPath)
        if (-not (Test-Path -LiteralPath $explicit -PathType Leaf)) {
            Fail-BundledPlugin "Herdr executable not found: $explicit"
        }
        return $explicit
    }

    foreach ($name in @('herdr.exe', 'herdr')) {
        $command = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $command) {
            return [System.IO.Path]::GetFullPath($command.Source)
        }
    }

    $resourceRoot = Split-Path -Parent (Split-Path -Parent $PluginRoot)
    $managed = Join-Path $resourceRoot 'herdr\windows-x86_64\herdr.exe'
    if (Test-Path -LiteralPath $managed -PathType Leaf) {
        return [System.IO.Path]::GetFullPath($managed)
    }

    # Source-checkout fallback for maintainers running the bundled helper before packaging.
    $sourceManaged = Join-Path $resourceRoot 'src-tauri\resources\herdr\windows-x86_64\herdr.exe'
    if (Test-Path -LiteralPath $sourceManaged -PathType Leaf) {
        return [System.IO.Path]::GetFullPath($sourceManaged)
    }

    Fail-BundledPlugin 'Herdr was not found on PATH and no adjacent Yuzora-managed herdr.exe exists'
}

function Invoke-Herdr {
    param(
        [string]$Executable,
        [string[]]$Arguments
    )
    $output = @(& $Executable @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    $text = ($output | ForEach-Object { [string]$_ }) -join [Environment]::NewLine
    if ($exitCode -ne 0) {
        Fail-BundledPlugin ("Herdr command failed (exit $exitCode): $text")
    }
    return $text.Trim()
}

function Get-RegisteredPlugin {
    param([string]$Executable)
    $raw = Invoke-Herdr -Executable $Executable -Arguments @(
        'plugin', 'list', '--plugin', $PluginId, '--json'
    )
    if ([string]::IsNullOrWhiteSpace($raw)) {
        Fail-BundledPlugin 'Herdr plugin list returned empty output'
    }
    try {
        $response = $raw | ConvertFrom-Json
    } catch {
        Fail-BundledPlugin "Herdr plugin list returned invalid JSON: $($_.Exception.Message)"
    }
    $result = $response.PSObject.Properties['result']
    if ($null -eq $result) { Fail-BundledPlugin 'Herdr plugin list response has no result' }
    $pluginsProperty = $result.Value.PSObject.Properties['plugins']
    if ($null -eq $pluginsProperty) { Fail-BundledPlugin 'Herdr plugin list response has no plugins' }
    $plugins = @($pluginsProperty.Value)
    if ($plugins.Count -gt 1) {
        Fail-BundledPlugin "Herdr returned multiple registrations for plugin id $PluginId"
    }
    if ($plugins.Count -eq 0) { return $null }
    return $plugins[0]
}

function Test-OwnsRegistration {
    param($Plugin)
    if ($null -eq $Plugin) { return $false }
    $rootProperty = $Plugin.PSObject.Properties['plugin_root']
    if ($null -eq $rootProperty -or [string]::IsNullOrWhiteSpace([string]$rootProperty.Value)) {
        return $false
    }
    return (Normalize-ComparablePath ([string]$rootProperty.Value)) -eq
        (Normalize-ComparablePath $PluginRoot)
}

function Write-StatusJson {
    param(
        [string]$Executable,
        [string]$RequestedAction
    )
    $plugin = Get-RegisteredPlugin -Executable $Executable
    $linked = $null -ne $plugin
    $linkedPath = $null
    $enabled = $false
    $version = $null
    if ($linked) {
        $rootProperty = $plugin.PSObject.Properties['plugin_root']
        if ($null -ne $rootProperty) { $linkedPath = [string]$rootProperty.Value }
        $enabledProperty = $plugin.PSObject.Properties['enabled']
        if ($null -ne $enabledProperty) { $enabled = [bool]$enabledProperty.Value }
        $versionProperty = $plugin.PSObject.Properties['version']
        if ($null -ne $versionProperty) { $version = [string]$versionProperty.Value }
    }
    $status = [ordered]@{
        action = $RequestedAction
        pluginId = $PluginId
        bundledPath = $PluginRoot
        herdrPath = $Executable
        linked = $linked
        enabled = $enabled
        linkedPath = $linkedPath
        ownsRegistration = (Test-OwnsRegistration -Plugin $plugin)
        version = $version
    }
    Write-Output ($status | ConvertTo-Json -Compress)
}

$ResolvedHerdrPath = Resolve-HerdrExecutable -ExplicitPath $HerdrPath
$existing = Get-RegisteredPlugin -Executable $ResolvedHerdrPath

switch ($Action) {
    'status' {
        Write-StatusJson -Executable $ResolvedHerdrPath -RequestedAction $Action
        break
    }
    'link' {
        if ($null -ne $existing -and -not (Test-OwnsRegistration -Plugin $existing)) {
            $foreignRoot = [string]$existing.PSObject.Properties['plugin_root'].Value
            Fail-BundledPlugin "plugin id $PluginId is already registered from another root: $foreignRoot"
        }
        if ($null -eq $existing) {
            [void](Invoke-Herdr -Executable $ResolvedHerdrPath -Arguments @(
                'plugin', 'link', $PluginRoot
            ))
        } else {
            $enabledProperty = $existing.PSObject.Properties['enabled']
            $enabled = $false
            if ($null -ne $enabledProperty) { $enabled = [bool]$enabledProperty.Value }
            if (-not $enabled) {
                [void](Invoke-Herdr -Executable $ResolvedHerdrPath -Arguments @(
                    'plugin', 'enable', $PluginId
                ))
            }
        }
        Write-StatusJson -Executable $ResolvedHerdrPath -RequestedAction $Action
        break
    }
    'unlink' {
        if ($null -ne $existing -and -not (Test-OwnsRegistration -Plugin $existing)) {
            $foreignRoot = [string]$existing.PSObject.Properties['plugin_root'].Value
            Fail-BundledPlugin "refusing to unlink plugin id $PluginId registered from another root: $foreignRoot"
        }
        if ($null -ne $existing) {
            [void](Invoke-Herdr -Executable $ResolvedHerdrPath -Arguments @(
                'plugin', 'unlink', $PluginId
            ))
        }
        Write-StatusJson -Executable $ResolvedHerdrPath -RequestedAction $Action
        break
    }
}
