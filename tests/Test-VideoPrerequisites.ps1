param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Ensure-VideoPrerequisites.ps1') -FunctionsOnly
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge Unity fixture '+[guid]::NewGuid().ToString('N'))
$script:checks=0
function Assert-VideoFixture([bool]$Condition,[string]$Name){if(-not $Condition){throw ('Video prerequisite assertion failed: '+$Name)};$script:checks++}
function New-VideoFixtureDll([string]$Path,[byte]$Marker=0){
 New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($Path)) -Force|Out-Null
 $bytes=New-Object byte[] 512
 [Array]::Copy([BitConverter]::GetBytes([uint16]0x5a4d),0,$bytes,0,2)
 [Array]::Copy([BitConverter]::GetBytes([uint32]0x80),0,$bytes,0x3c,4)
 [Array]::Copy([BitConverter]::GetBytes([uint32]0x4550),0,$bytes,0x80,4)
 [Array]::Copy([BitConverter]::GetBytes([uint16]0x8664),0,$bytes,0x84,2)
 [Array]::Copy([BitConverter]::GetBytes([uint16]0x2000),0,$bytes,0x96,2)
 [Array]::Copy([BitConverter]::GetBytes([uint16]0x20b),0,$bytes,0x98,2)
 $bytes[511]=$Marker;[IO.File]::WriteAllBytes($Path,$bytes)
}
function Set-VideoFixtureRegistration([string]$Dll,[int]$Count=2){
 $script:entries=@();$slots=@(Get-BridgeUnitySlots)
 for($index=0;$index-lt $Count;$index++){
  $slot=$slots[$index]
  $script:entries+=[pscustomobject]@{Hive='LocalMachine';Kind='Class';Guid=$slot.Guid;Name=$slot.Name;Path=$Dll}
  $script:entries+=[pscustomobject]@{Hive='LocalMachine';Kind='Instance';Guid=$slot.Guid;Name=$slot.Name;Clsid=$slot.Guid;Path=''}
 }
}
function New-VideoFixtureProperties([string]$Dll,[string]$Hive='LocalMachine'){
 [pscustomobject]@{Hive=$Hive;Kind='PropertiesClass';Guid='{5C2CD55C-92AD-4999-8666-912BD3E70011}';Name='Unity Capture Properties';Path=$Dll}
}
try{
 New-Item -ItemType Directory -Path $testRoot -Force|Out-Null
 $source=Join-Path $testRoot 'components\unity-capture\UnityCaptureFilter64.dll'
 New-VideoFixtureDll $source
 $digest=(Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
 $manifest=@{schema=1;file='components/unity-capture/UnityCaptureFilter64.dll';architecture='x64';sha256=$digest}
 $manifestPath=Join-Path $testRoot 'unity-capture.json'
 [IO.File]::WriteAllText($manifestPath,($manifest|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
 Assert-VideoFixture (Test-BridgeUnityX64Dll $source) 'fixture x64 DLL accepted structurally without executing it'
 $x86=Join-Path $testRoot 'x86\UnityCaptureFilter64.dll';New-VideoFixtureDll $x86
 $bytes=[IO.File]::ReadAllBytes($x86);$bytes[0x84]=0x4c;$bytes[0x85]=1;[IO.File]::WriteAllBytes($x86,$bytes)
 Assert-VideoFixture (-not(Test-BridgeUnityX64Dll $x86)) 'x86 DLL rejected before registration'
 $script:entries=@();$script:installs=0;$script:registers=0;$script:registeredPath='';$script:network=0
 function Get-BridgeUnityRegistryState {$script:entries}
 function Get-BridgeUnityStableRoot {Join-Path $testRoot 'protected-owned'}
 function Install-BridgeUnityStableDll {param($Source,$Manifest)
  $script:installs++
  $target=Join-Path (Join-Path (Get-BridgeUnityStableRoot) ([string]$Manifest.sha256).ToLowerInvariant()) 'UnityCaptureFilter64.dll'
  New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($target)) -Force|Out-Null
  Copy-Item -LiteralPath $Source -Destination $target -Force
  $target
 }
 function Register-BridgeUnitySlots {param($FilterDll,$Manifest)
  $script:registers++;$script:registeredPath=$FilterDll
  $null=Assert-BridgeUnityPinnedDll $FilterDll $Manifest
  Set-VideoFixtureRegistration $FilterDll
  $script:entries+=New-VideoFixtureProperties $FilterDll
 }
 function Invoke-WebRequest {$script:network++;throw 'Unexpected network access.'}
 function Invoke-RestMethod {$script:network++;throw 'Unexpected network access.'}
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot
 Assert-VideoFixture ($result.Ready-and $result.Changed-and $result.Disposition-eq 'InstalledOwned'-and $script:installs-eq 1-and $script:registers-eq 1) 'clean PC installs and registers both owned sources'
 Assert-VideoFixture ((Test-BridgeUnitySamePath $result.Filter $script:registeredPath)-and -not(Test-BridgeUnitySamePath $result.Filter $source)) 'registered DLL uses stable owned location instead of portable path'
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot
 Assert-VideoFixture ($result.Ready-and -not $result.Changed-and $script:registers-eq 1) 'second launch does not re-register or interrupt a working source'
 $foreign=Join-Path $testRoot 'existing-other-version\UnityCaptureFilter64.dll';New-VideoFixtureDll $foreign 42
 Set-VideoFixtureRegistration $foreign
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot
 Assert-VideoFixture ($result.Ready-and $result.Disposition-eq 'ReadyExisting'-and -not $result.Changed-and $script:registers-eq 1-and (Test-BridgeUnitySamePath $result.Filter $foreign)) 'complete existing different-version installation reused without executing or adopting its DLL'
 Set-VideoFixtureRegistration $foreign 1
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'another installation'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 1) 'partial foreign registration is never taken over automatically'
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot $foreign}catch{$blocked=$_.Exception.Message-match 'SHA256 mismatch'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 1) 'manual repair cannot execute an arbitrary unpinned DLL'
 $knownForeign=Join-Path $testRoot 'known-foreign\UnityCaptureFilter64.dll';New-VideoFixtureDll $knownForeign
 Set-VideoFixtureRegistration $knownForeign 1
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot $knownForeign
 Assert-VideoFixture ($result.Ready-and $result.Changed-and $script:registers-eq 2-and -not(Test-BridgeUnitySamePath $script:registeredPath $knownForeign)) 'explicit same-hash repair registers only the protected bundled copy'
 $owned=$result.Filter;Set-VideoFixtureRegistration $owned 1
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot
 Assert-VideoFixture ($result.Ready-and $result.Changed-and $script:registers-eq 3) 'partial owned registration is repaired locally'
 $script:entries+=[pscustomobject]@{Hive='LocalMachine';Kind='Instance';Guid='{11111111-1111-1111-1111-111111111111}';Name='Unity Capture Client';Clsid='{11111111-1111-1111-1111-111111111111}';Path=''}
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'ambiguous'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 3) 'duplicate source names fail before registry mutation'
 Set-VideoFixtureRegistration $owned;$script:entries[0].Hive='CurrentUser'
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'per-user'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 3) 'per-user override is not overwritten'
 $script:entries=@();$manifest.sha256='0'*64
 [IO.File]::WriteAllText($manifestPath,($manifest|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'SHA256 mismatch'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 3) 'corrupted package fails closed without local registration'
 $manifest.sha256=$digest;$manifest.file='../UnityCaptureFilter64.dll'
 [IO.File]::WriteAllText($manifestPath,($manifest|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
 $blocked=$false;try{$null=Get-BridgeUnityManifest $testRoot}catch{$blocked=$_.Exception.Message-match 'invalid'}
 Assert-VideoFixture $blocked 'manifest cannot select a DLL outside the pinned component path'
 $manifest.file='components/unity-capture/UnityCaptureFilter64.dll'
 [IO.File]::WriteAllText($manifestPath,($manifest|ConvertTo-Json),[Text.UTF8Encoding]::new($false))
 $script:entries=@(New-VideoFixtureProperties $foreign)
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'another installation'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 3) 'foreign property-page class alone is never overwritten by fresh registration'
 $script:entries=@(New-VideoFixtureProperties $foreign CurrentUser)
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'per-user'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 3) 'per-user property-page override fails before registration'
 Set-VideoFixtureRegistration $owned;$script:entries+=New-VideoFixtureProperties $foreign
 $blocked=$false;try{$null=Ensure-BridgeVideoPrerequisites Client $testRoot}catch{$blocked=$_.Exception.Message-match 'property-page'}
 Assert-VideoFixture ($blocked-and $script:registers-eq 3) 'complete video slots cannot reuse a mismatched foreign property-page DLL'
 Set-VideoFixtureRegistration $foreign;$script:entries+=New-VideoFixtureProperties $foreign
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot
 Assert-VideoFixture ($result.Ready-and -not $result.Changed-and $script:registers-eq 3) 'existing two filters and matching property-page class are reused untouched'
 $script:entries=@(New-VideoFixtureProperties $owned)
 $result=Ensure-BridgeVideoPrerequisites Client $testRoot
 Assert-VideoFixture ($result.Ready-and $result.Changed-and $script:registers-eq 4-and @($script:entries|Where-Object {$_.Kind-in @('Class','PropertiesClass')}).Count-eq 3) 'owned property-page-only partial install repairs all three known classes locally'
 Assert-VideoFixture ($script:network-eq 0) 'all setup and repair paths stay offline'
 foreach($name in @('Ensure-VideoPrerequisites.ps1','Configure-UnityCapture.ps1','FirstRun-Identity.ps1','Configure-Installation.ps1','Open-Installation.ps1')){
  $tokens=$null;$errors=$null;$null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory $name),[ref]$tokens,[ref]$errors)
  Assert-VideoFixture ($errors.Count-eq 0) ('Windows PowerShell parser '+$name)
 }
 $text=[IO.File]::ReadAllText((Join-Path $SourceDirectory 'Ensure-VideoPrerequisites.ps1'))
 Assert-VideoFixture ($text-notmatch 'Invoke-WebRequest|Invoke-RestMethod|https?://|Download-Drivers') 'runtime helper contains no HTTP fallback'
 [pscustomobject]@{passed=$true;checks=$script:checks;scope='clean Windows slots; stable DLL path; existing source reuse; foreign/ambiguous/tampered rejection; local-only'}|ConvertTo-Json -Compress
}finally{
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
