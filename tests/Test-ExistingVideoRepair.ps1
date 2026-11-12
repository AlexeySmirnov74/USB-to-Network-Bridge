# Component-only upgrade policy. No live tasks, COM registration or ACL changes.
param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'FirstRun-Identity.ps1')
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge video repair fixture '+[guid]::NewGuid().ToString('N'))
$script:checks=0
function Assert-VideoRepairFixture([bool]$Condition,[string]$Name){if(-not $Condition){throw ('Existing video repair assertion failed: '+$Name)};$script:checks++}
try{
 New-Item -ItemType Directory -Path $testRoot -Force|Out-Null
 $config=Join-Path $testRoot 'client.local.json';$key=Join-Path $testRoot 'existing-camera.key'
 [IO.File]::WriteAllText($config,'{"server":"192.0.2.25:3240","fixture":"do not reset"}')
 [IO.File]::WriteAllText($key,'fixture credential, never a real key')
 $configHash=(Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash;$keyHash=(Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash
 $configAcl=(Get-Acl -LiteralPath $config).Sddl
 $global:VideoRepairFixture=@{videoReady=$false;driversReady=$true;calls=0;configure=0;fail=$false;withhold=$false;elevated=$false}
 [IO.File]::WriteAllText((Join-Path $testRoot 'Ensure-VideoPrerequisites.ps1'),@'
param($Role,$RuntimeDirectory,[switch]$FunctionsOnly,[switch]$Elevate)
function Test-BridgeUnityCaptureReadiness {param($RuntimeDirectory) [pscustomobject]@{Ready=[bool]$global:VideoRepairFixture.videoReady}}
if($FunctionsOnly){return}
$global:VideoRepairFixture.calls++;$global:VideoRepairFixture.elevated=[bool]$Elevate
if($global:VideoRepairFixture.fail){throw 'Fixture foreign Unity registration refused.'}
if(-not $global:VideoRepairFixture.withhold){$global:VideoRepairFixture.videoReady=$true}
exit 0
'@,[Text.UTF8Encoding]::new($false))
 [IO.File]::WriteAllText((Join-Path $testRoot 'Configure-Installation.ps1'),'$global:VideoRepairFixture.configure++; throw ''Unexpected full role setup.''')
 function Get-BridgeDriverReadiness {param($Role,$Runtime) [pscustomobject]@{Ready=$global:VideoRepairFixture.driversReady;Missing=$(if($global:VideoRepairFixture.driversReady){@()}else{@('fixture controller')});Partial=$true}}
 function Get-ScheduledTask {throw 'Component-only repair must not inspect or rewrite live tasks.'}
 function Set-Acl {throw 'Component-only repair must not reset data ACLs.'}
 $before=Get-BridgeInstallationReadiness Client $testRoot
 Assert-VideoRepairFixture ($before.DriversReady-and -not $before.VideoReady-and -not $before.Ready) 'missing Unity is distinct from ready physical drivers'
 $result=Repair-BridgeExistingVideoPrerequisites Client Current $config $testRoot $before
 Assert-VideoRepairFixture ($result.Ready-and $result.VideoReady-and $global:VideoRepairFixture.calls-eq 1-and $global:VideoRepairFixture.elevated-and $global:VideoRepairFixture.configure-eq 0) 'existing current client uses only elevated component helper and fresh readiness'
 Assert-VideoRepairFixture ((Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash-eq $configHash-and (Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash-eq $keyHash-and (Get-Acl -LiteralPath $config).Sddl-eq $configAcl) 'component-only repair preserves config key and ACL bytes'
 $null=Repair-BridgeExistingVideoPrerequisites Client Current $config $testRoot $result
 Assert-VideoRepairFixture ($global:VideoRepairFixture.calls-eq 1) 'ready current client requests no setup'
 $global:VideoRepairFixture.videoReady=$false
 $before=Get-BridgeInstallationReadiness Client $testRoot
 foreach($disposition in @('New','Foreign','Unverified')){
  $result=Repair-BridgeExistingVideoPrerequisites Client $disposition $config $testRoot $before
  Assert-VideoRepairFixture (-not $result.Ready-and $global:VideoRepairFixture.calls-eq 1) ('new/copied '+$disposition+' client stays on full first-run path')
 }
 $global:VideoRepairFixture.driversReady=$false
 $beforeDrivers=Get-BridgeInstallationReadiness Client $testRoot
 $result=Repair-BridgeExistingVideoPrerequisites Client Current $config $testRoot $beforeDrivers
 Assert-VideoRepairFixture (-not $result.DriversReady-and -not $result.Ready-and $global:VideoRepairFixture.calls-eq 1) 'missing physical driver stays on full signed-driver setup path'
 $global:VideoRepairFixture.driversReady=$true
 $result=Repair-BridgeExistingVideoPrerequisites Client Current (Join-Path $testRoot 'absent.json') $testRoot $before
 Assert-VideoRepairFixture (-not $result.Ready-and $global:VideoRepairFixture.calls-eq 1) 'absent config is never adopted by component-only repair'
 $server=Get-BridgeInstallationReadiness Server $testRoot
 $result=Repair-BridgeExistingVideoPrerequisites Server Current $config $testRoot $server
 Assert-VideoRepairFixture ($result.Ready-and $result.VideoReady-and $global:VideoRepairFixture.calls-eq 1) 'server launch has no Unity producer requirement'
 $global:VideoRepairFixture.fail=$true;$blocked=$false
 try{$null=Repair-BridgeExistingVideoPrerequisites Client Adopt $config $testRoot $before}catch{$blocked=$_.Exception.Message-match 'foreign Unity registration'}
 Assert-VideoRepairFixture ($blocked-and $global:VideoRepairFixture.configure-eq 0) 'foreign Unity refusal does not fall back to full role setup'
 $global:VideoRepairFixture.fail=$false;$global:VideoRepairFixture.withhold=$true;$blocked=$false
 try{$null=Repair-BridgeExistingVideoPrerequisites Client Current $config $testRoot $before}catch{$blocked=$_.Exception.Message-match 'still unavailable'}
 Assert-VideoRepairFixture ($blocked-and $global:VideoRepairFixture.configure-eq 0) 'helper success without fresh sources is reported as failure'
 $global:VideoRepairFixture.withhold=$false
 $result=Repair-BridgeExistingVideoPrerequisites Client Adopt $config $testRoot $before
 Assert-VideoRepairFixture $result.Ready 'verified legacy adopted client can repair just its video components'
 [pscustomobject]@{passed=$true;checks=$script:checks;scope='component-only existing client repair; no role config task ACL reset; full new-PC setup retained'}|ConvertTo-Json -Compress
}finally{
 Remove-Variable -Name VideoRepairFixture -Scope Global -ErrorAction SilentlyContinue
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
