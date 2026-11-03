# Cooperative stop leaves USB sharing intact and marks the run as interrupted.
param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$directory=(Resolve-Path -LiteralPath $OutputDirectory).Path
$manifest=Get-Content -LiteralPath (Join-Path $directory 'manifest.local.json') -Raw|ConvertFrom-Json
if($manifest.schema-ne 1-or $manifest.role-notin @('Server','Client')){throw 'Not a qualification run directory.'}
[IO.File]::WriteAllText((Join-Path $directory 'stop.request'),'stop')
'Stop requested. Wait for result.local.json; USB sharing remains active.'
