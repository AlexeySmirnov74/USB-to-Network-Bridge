# Verify qualification first; preparing a release never publishes it.
param([Parameter(Mandatory=$true)][string]$ServerEvidence,[Parameter(Mandatory=$true)][string]$ClientEvidence,[Parameter(Mandatory=$true)][string]$CheckReport,[string]$CandidateDirectory,[ValidateSet('Full','ShortObservation')][string]$ValidationProfile='Full')
$ErrorActionPreference='Stop';$root=$PSScriptRoot
$policy=Get-Content -LiteralPath (Join-Path $root 'release-policy.json') -Raw|ConvertFrom-Json
if($policy.application_signing-ne 'unsigned'){throw 'Unexpected release signing policy.'}
if(-not $CandidateDirectory){$CandidateDirectory=Join-Path $root 'dist'}
$checks=Get-Content -LiteralPath $CheckReport -Raw|ConvertFrom-Json
$requiredChecks=if($ValidationProfile-eq 'Full'){@($policy.required_checks)}else{@($policy.short_observation.required_checks)}
foreach($name in $requiredChecks){
 $entry=@($checks.checks|Where-Object test -eq $name)
 if($entry.Count-ne 1-or $entry[0].passed-isnot [bool]-or -not $entry[0].passed){throw ('Required release check is missing or failed: '+$name)}
}
$pending=@($policy.required_checks|Where-Object {$name=$_; -not @($checks.checks|Where-Object {$_.test-eq $name-and $_.passed-is [bool]-and $_.passed}).Count})
$observations=@()
foreach($role in @('Server','Client')){
 $directory=if($role-eq 'Server'){$ServerEvidence}else{$ClientEvidence}
 $evidenceManifest=Get-Content -LiteralPath (Join-Path $directory 'manifest.local.json') -Raw|ConvertFrom-Json
 if($role-eq 'Client'-and $evidenceManifest.minimum_fps-lt $policy.minimum_camera_fps){throw 'Client evidence used a lower camera FPS threshold than release policy.'}
 $report=Join-Path $directory 'analysis.local.json'
 $minimumHours=if($ValidationProfile-eq 'Full'){$policy.minimum_soak_hours}else{$policy.short_observation.minimum_seconds/3600.0}
 $analysisArgs=@('-3',(Join-Path $root 'Analyze-Qualification.py'),$directory,'--minimum-hours',$minimumHours.ToString([Globalization.CultureInfo]::InvariantCulture),'--output',$report)
 if($ValidationProfile-eq 'ShortObservation'){$analysisArgs+='--allow-operator-stop'}
 & py @analysisArgs|Out-Null
 if($LASTEXITCODE-ne 0){throw ($role+' qualification is not complete or has failed.')}
 $result=Get-Content -LiteralPath $report -Raw|ConvertFrom-Json
 if($result.role-ne $role){throw 'Qualification role mismatch.'}
 $observations+=@([ordered]@{role=$role;elapsed_seconds=$result.elapsed_seconds;original_phase=$result.original_phase;operator_stop_accepted=$result.operator_stop_accepted;full_48h_soak_passed=$result.full_48h_soak_passed;resources=$result.resources;camera=$result.camera_summary})
 $required=if($role-eq 'Server'){@('usbbridge-server.exe')}else{@('usbbridge.exe','usbbridge-client.exe','usbbridge-agent.exe')}
 foreach($name in $required){if(@($result.binaries|Where-Object file -eq $name).Count-ne 1){throw ('Missing or duplicate qualification binary: '+$name)}}
 foreach($binary in $result.binaries){
  if($role-eq 'Server'-and $binary.file-ne 'usbbridge-server.exe'){continue}
  if($role-eq 'Client'-and $binary.file-notin @('usbbridge.exe','usbbridge-client.exe','usbbridge-agent.exe')){continue}
  $current=Join-Path $root ('target\release\'+$binary.file)
  if((Get-FileHash -LiteralPath $current).Hash-ne $binary.sha256){throw ('Tested binary differs from candidate: '+$binary.file)}
 }
}
$git=Join-Path $root 'tools\mingit\cmd\git.exe';if(-not(Test-Path $git)){$git=(Get-Command git -ErrorAction Stop).Source}
$status=& $git -C $root status --porcelain
if($status){throw 'Commit the reviewed source first; releases require a clean working tree.'}
$commit=(& $git -C $root rev-parse HEAD).Trim()
$files=@(Get-ChildItem -LiteralPath $CandidateDirectory -File|Where-Object {$_.Name-like 'USB-to-Network-Bridge-*-windows-x64.msi'-or $_.Name-like 'USB-to-Network-Bridge-*-windows-x64.zip'-or $_.Name-like 'USB-to-Network-Bridge-*-source.zip'})
if($files.Count-ne 3){throw 'The candidate directory must contain exactly one MSI, portable ZIP and source ZIP.'}
$portable=@($files|Where-Object Extension -eq '.zip'|Where-Object Name -like '*-windows-x64.zip')[0].FullName
$source=@($files|Where-Object Name -like '*-source.zip')[0].FullName
$msi=@($files|Where-Object Extension -eq '.msi')[0].FullName
& (Join-Path $root 'Verify-ReleaseArtifacts.ps1') -Portable $portable -Source $source -Msi $msi -Commit $commit
if(-not $?){throw 'Candidate artifact verification failed.'}
foreach($launcher in @('USB-to-Network-Bridge-Server.exe','USB-to-Network-Bridge-Client.exe')){
 $artifact=Get-Item -LiteralPath (Join-Path $CandidateDirectory $launcher)
 if((Get-FileHash -LiteralPath $artifact.FullName).Hash-ne (Get-FileHash -LiteralPath (Join-Path $root ('target\release\'+$launcher))).Hash){throw ('Launcher differs from the built candidate: '+$launcher)}
 $files+=@($artifact)
}
$review=[ordered]@{schema=1;source_commit=$commit;application_signing='unsigned';validation_profile=$ValidationProfile;observation_passed=$true;full_qualification_passed=($ValidationProfile-eq 'Full');full_soak_hours_required=$policy.minimum_soak_hours;not_performed_checks=$pending;observations=$observations;created_utc=[DateTime]::UtcNow.ToString('o');artifacts=@($files|ForEach-Object {[ordered]@{file=$_.Name;bytes=$_.Length;sha256=(Get-FileHash -LiteralPath $_.FullName).Hash}})}
New-Item -ItemType Directory -Path (Join-Path $root 'logs') -Force|Out-Null
$review|ConvertTo-Json -Depth 8|Set-Content -LiteralPath (Join-Path $root 'logs\release-review.local.json') -Encoding utf8
$manifest=[ordered]@{schema=1;source_commit=$commit;application_signing='unsigned';created_utc=$review.created_utc;artifacts=$review.artifacts}
$manifest|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $CandidateDirectory 'release-manifest.json') -Encoding utf8
$manifest.artifacts|ForEach-Object {$_.sha256.ToLowerInvariant()+'  '+$_.file}|Set-Content -LiteralPath (Join-Path $CandidateDirectory 'SHA256SUMS.txt') -Encoding ascii
'Selected validation profile passed. Full qualification status is recorded explicitly. Artifacts are prepared locally; nothing was published.'
