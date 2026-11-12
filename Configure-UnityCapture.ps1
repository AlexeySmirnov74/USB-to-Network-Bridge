# Offline repair of the two producer sources. Existing working slots are reused.
param([string]$FilterDll,[switch]$Elevate)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$arguments=@{Role='Client';RuntimeDirectory=$PSScriptRoot;Elevate=$Elevate}
if($FilterDll){$arguments.FilterDll=$FilterDll}
& (Join-Path $PSScriptRoot 'Ensure-VideoPrerequisites.ps1') @arguments
exit $LASTEXITCODE
