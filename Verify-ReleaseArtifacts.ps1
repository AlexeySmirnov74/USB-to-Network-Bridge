# Extract an administrative MSI image, then compare all payload bytes with the ZIP.
param([Parameter(Mandatory=$true)][string]$Portable,[Parameter(Mandatory=$true)][string]$Source,[Parameter(Mandatory=$true)][string]$Msi,[Parameter(Mandatory=$true)][string]$Commit)
$ErrorActionPreference='Stop'
$stage=Join-Path $PSScriptRoot ('dist\release-verify-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -Force|Out-Null
$log=Join-Path $stage 'extract.log'
$p=Start-Process msiexec.exe -ArgumentList ('/a "'+$Msi+'" /qn /norestart REBOOT=ReallySuppress TARGETDIR="'+$stage+'" /l*v "'+$log+'"') -WindowStyle Hidden -Wait -PassThru
if($p.ExitCode-ne 0){throw ('MSI administrative extraction failed: '+$p.ExitCode+'; '+$log)}
$manifests=@(Get-ChildItem -LiteralPath $stage -Filter SHA256.json -File -Recurse)
if($manifests.Count-ne 1){throw 'MSI did not contain exactly one application hash manifest.'}
& py -3 (Join-Path $PSScriptRoot 'Verify-ReleaseArtifacts.py') --portable $Portable --source $Source --root $PSScriptRoot --commit $Commit --installed $manifests[0].DirectoryName
if($LASTEXITCODE-ne 0){throw 'Release archive verification failed.'}
