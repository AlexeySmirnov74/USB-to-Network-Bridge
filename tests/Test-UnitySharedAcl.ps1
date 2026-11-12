# Isolated ACL policy fixture: never writes to ProgramData or a real registry.
param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Ensure-VideoPrerequisites.ps1') -FunctionsOnly
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge shared ACL fixture '+[guid]::NewGuid().ToString('N'))
$script:checks=0;$script:protected=@();$script:aclReads=0
function Assert-UnityAclFixture([bool]$Condition,[string]$Name){if(-not $Condition){throw ('Unity shared ACL assertion failed: '+$Name)};$script:checks++}
function New-SharedAclFixture([string]$Owner='S-1-5-32-544',[string]$CustomRights='Write') {
 $acl=New-Object Security.AccessControl.DirectorySecurity
 $acl.SetAccessRuleProtection($true,$false)
 $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier($Owner)))
 foreach($entry in @(@('S-1-5-18','FullControl'),@('S-1-5-32-544','FullControl'),@('S-1-5-32-545','ReadAndExecute'),@('S-1-5-21-300000-400000-500000-1001',$CustomRights))){
  $sid=New-Object Security.Principal.SecurityIdentifier($entry[0])
  $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,$entry[1],'ContainerInherit,ObjectInherit','None','Allow')))
 }
 $acl
}
try{
 New-Item -ItemType Directory -Path $testRoot -Force|Out-Null
 $common=Join-Path $testRoot 'Bridge shared data'
 New-Item -ItemType Directory -Path (Join-Path $common 'Client'),(Join-Path $common 'Server\video-credentials') -Force|Out-Null
 $config=Join-Path $common 'Client\client.local.json';$key=Join-Path $common 'Server\video-credentials\camera.key'
 [IO.File]::WriteAllText($config,'{"fixture":"private route remains unchanged"}')
 [IO.File]::WriteAllText($key,'fixture key; not a real credential')
 $configHash=(Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash;$keyHash=(Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash
 $script:sharedAcl=New-SharedAclFixture
 function Get-Acl {param($LiteralPath)
  if(-not(Test-BridgeUnitySamePath $LiteralPath $common)){throw 'Unexpected ACL read outside the shared root.'}
  $script:aclReads++;$script:sharedAcl
 }
 function Protect-BridgeUnityDirectory {param($Path)
  $script:protected+=[IO.Path]::GetFullPath($Path)
  New-Item -ItemType Directory -Path $Path -Force|Out-Null
 }
 $before=$script:sharedAcl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::All)
 $root=Join-Path $common 'Components\UnityCapture'
 Initialize-BridgeUnityDirectories $root ('A'*64)
 Assert-UnityAclFixture ($script:aclReads-eq 1-and $script:protected.Count-eq 3) 'existing common root is validated and only component subtree is hardened'
 Assert-UnityAclFixture ($before-ceq $script:sharedAcl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::All)) 'existing custom service-account grants remain byte-for-byte equivalent'
 Assert-UnityAclFixture (@($script:protected|Where-Object {-not $_.StartsWith((Join-Path $common 'Components')+'\',[StringComparison]::OrdinalIgnoreCase)-and -not(Test-BridgeUnitySamePath $_ (Join-Path $common 'Components'))}).Count-eq 0) 'role config and key directories are never selected for DACL modification'
 Assert-UnityAclFixture ((Get-FileHash -LiteralPath $config -Algorithm SHA256).Hash-eq $configHash-and (Get-FileHash -LiteralPath $key -Algorithm SHA256).Hash-eq $keyHash) 'existing config and key bytes are retained'
 $script:protected=@();$script:sharedAcl=New-SharedAclFixture -CustomRights FullControl
 $blocked=$false;try{Initialize-BridgeUnityDirectories $root ('B'*64)}catch{$blocked=$_.Exception.Message-match 'path replacement'}
 Assert-UnityAclFixture ($blocked-and $script:protected.Count-eq 0) 'replaceable shared parent is refused without stripping its existing grants'
 $script:sharedAcl=New-SharedAclFixture -Owner 'S-1-5-21-300000-400000-500000-1001'
 $blocked=$false;try{Initialize-BridgeUnityDirectories $root ('C'*64)}catch{$blocked=$_.Exception.Message-match 'untrusted owner'}
 Assert-UnityAclFixture ($blocked-and $script:protected.Count-eq 0) 'untrusted-owned shared parent is refused without mutation'
 $script:protected=@();$beforeReads=$script:aclReads
 $freshCommon=Join-Path $testRoot 'New shared data';$freshRoot=Join-Path $freshCommon 'Components\UnityCapture'
 Initialize-BridgeUnityDirectories $freshRoot ('D'*64)
 Assert-UnityAclFixture ($script:protected.Count-eq 4-and (Test-BridgeUnitySamePath $script:protected[0] $freshCommon)-and $script:aclReads-eq $beforeReads) 'new common root receives protected permissions before installing components'
 [pscustomobject]@{passed=$true;checks=$script:checks;scope='shared root ACL preservation; role config/key retention; unsafe ancestor refusal; clean parent creation'}|ConvertTo-Json -Compress
}finally{
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
