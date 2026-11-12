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
if($manifests[0].Directory.Name-ne 'app'){throw 'The application hash manifest must be inside app.'}
$installedRoot=$manifests[0].Directory.Parent.FullName
# Trust the reviewed verifier and pins, never a helper/manifest supplied inside
# the candidate archive. The Python gate below also checks both exact hashes.
& (Join-Path $PSScriptRoot 'Verify-DriverInstaller.ps1') -Role Both -InstallerDirectory (Join-Path $installedRoot 'app\tools\driver-installers') -ManifestPath (Join-Path $PSScriptRoot 'driver-installers.json')|Out-Null
if(-not $?){throw 'Extracted MSI offline driver signatures failed verification.'}
& py -3 (Join-Path $PSScriptRoot 'Verify-ReleaseArtifacts.py') --portable $Portable --source $Source --root $PSScriptRoot --commit $Commit --installed $installedRoot
if($LASTEXITCODE-ne 0){throw 'Release archive verification failed.'}
