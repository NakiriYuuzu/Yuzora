$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\common.ps1"

# Startup and Status action: non-destructive. Never writes WSL home files.
& "$PSScriptRoot\manage-adapters.ps1" -Action status
exit $LASTEXITCODE
