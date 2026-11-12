param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot),[string]$PathsSourceDirectory=$SourceDirectory)
$ErrorActionPreference='Stop'
. (Join-Path $PathsSourceDirectory 'Portable-Paths.ps1')
. (Join-Path $SourceDirectory 'Update-PortableSettings.ps1') -FunctionsOnly
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge settings '+[guid]::NewGuid().ToString('N'))
$project=Join-Path $testRoot 'project Old';$runtime=Join-Path $project 'app'
function Assert-Equal($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Portable settings assertion failed: '+$Name)}}
function Write-FixtureJson([string]$Path,$Value){[IO.File]::WriteAllText($Path,($Value|ConvertTo-Json -Depth 24),[Text.UTF8Encoding]::new($false))}
function Read-FixtureJson([string]$Path){[IO.File]::ReadAllText($Path)|ConvertFrom-Json}
try{
 New-Item -ItemType Directory -Path $runtime -Force|Out-Null
 Copy-Item -LiteralPath (Join-Path $PathsSourceDirectory 'Portable-Paths.ps1') -Destination (Join-Path $runtime 'Portable-Paths.ps1')
 $hostConfig=Join-Path $runtime 'host.local.json';$oldRuntime=Join-Path $testRoot 'previous\app'
 Write-FixtureJson $hostConfig ([ordered]@{version=1;runtime=$oldRuntime;profile=(Join-Path $oldRuntime 'server.local.psd1');server_id='725f1463-7864-4242-9e97-c85ffdb7b0d6';enabled=$true;name='FIXTURE'})
 [IO.File]::WriteAllText((Join-Path $runtime 'server.local.psd1'),"@{Listen='0.0.0.0:3240'}")
 [IO.File]::WriteAllText((Join-Path $runtime 'video.key'),'fixture-internal-key')
 $videoConfig=Join-Path $runtime 'host.video-receive.local.json'
 Write-FixtureJson $videoConfig ([ordered]@{enabled=$true;key_file=(Join-Path $oldRuntime 'video.key');camera=@{serial='UBC-preserved';name='4k web camera';microphone=$true}})
 $autostart=Join-Path $runtime 'autostart-server.local.json'
 Write-FixtureJson $autostart @{Success=$true;Config=(Join-Path $oldRuntime 'host.local.json');Task='\USBBridge\USBBridge.Server'}
 $marker=Join-Path $runtime 'machine-server.local.json';[IO.File]::WriteAllText($marker,'fixture-marker-unchanged')
 $result=Convert-BridgeRoleSettings -Role Server -Runtime $runtime -Config $hostConfig -OldRuntime $oldRuntime -InstallRoot $project
 $settings=Read-FixtureJson $hostConfig;$video=Read-FixtureJson $videoConfig
 Assert-Equal $settings.runtime '@app' 'server runtime portable'
 Assert-Equal $settings.profile 'server.local.psd1' 'server profile portable'
 Assert-Equal $settings.server_id '725f1463-7864-4242-9e97-c85ffdb7b0d6' 'server id preserved'
 Assert-Equal $settings.enabled $true 'enabled preserved'
 Assert-Equal $video.key_file 'video.key' 'legacy video key rebased'
 Assert-Equal $video.camera.serial 'UBC-preserved' 'camera serial preserved'
 Assert-Equal $video.camera.microphone $true 'silent microphone preserved'
 Assert-Equal (Read-FixtureJson $autostart).Config 'host.local.json' 'autostart metadata portable'
 Assert-Equal ([IO.File]::ReadAllText($marker)) 'fixture-marker-unchanged' 'library never changes marker'
 Assert-Equal $result.files_changed 3 'only owned three configurations changed'
 $external=Join-Path $testRoot 'external';New-Item -ItemType Directory -Path $external|Out-Null
 $externalKey=Join-Path $external 'source.key';$externalProxy=Join-Path $external 'source.credential'
 [IO.File]::WriteAllBytes($externalKey,[byte[]](1,2,3,4,5));[IO.File]::WriteAllBytes($externalProxy,[byte[]](5,4,3,2,1))
 $clientConfig=Join-Path $runtime 'client.local.json';$clientVideo=Join-Path $runtime 'client.video-send.local.json'
 Write-FixtureJson $clientConfig @{version=1;server='192.0.2.25:3240';devices=@(@{identity='USB\VID_1234&PID_5678\serial';enabled=$true});proxy=@{kind='socks5';credential_file=$externalProxy};servers=@(@{server='192.0.2.26:3240';proxy=@{kind='http';credential_file=$externalProxy}})}
 Write-FixtureJson $clientVideo @{enabled=$true;key_file=$externalKey;source='@device:sw:{fixture-unchanged}'}
 $result=Convert-BridgeRoleSettings -Role Client -Runtime $runtime -Config $clientConfig
 $client=Read-FixtureJson $clientConfig;$sender=Read-FixtureJson $clientVideo
 Assert-Equal $result.credentials_copied 2 'external references copied once each'
 Assert-Equal $client.server '192.0.2.25:3240' 'server address preserved'
 Assert-Equal $client.devices[0].identity 'USB\VID_1234&PID_5678\serial' 'usb identity preserved'
 Assert-Equal $client.proxy.credential_file $client.servers[0].proxy.credential_file 'shared credential reference deduplicated'
 Assert-Equal $sender.source '@device:sw:{fixture-unchanged}' 'capture moniker preserved'
 $copiedKey=Resolve-BridgeConfigPath $sender.key_file $clientVideo $runtime
 $copiedProxy=Resolve-BridgeConfigPath $client.proxy.credential_file $clientConfig $runtime
 Assert-Equal (Get-FileHash -LiteralPath $copiedKey).Hash (Get-FileHash -LiteralPath $externalKey).Hash 'video key bytes preserved'
 Assert-Equal (Get-FileHash -LiteralPath $copiedProxy).Hash (Get-FileHash -LiteralPath $externalProxy).Hash 'proxy credential bytes preserved'
 Assert-Equal (Test-Path -LiteralPath $externalKey) $true 'external source key retained'
 $acl=Get-Acl -LiteralPath $copiedKey
 Assert-Equal $acl.AreAccessRulesProtected $true 'credential acl protected'
 $sids=@($acl.Access|ForEach-Object {$_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value})
 foreach($sid in @([Security.Principal.WindowsIdentity]::GetCurrent().User.Value,'S-1-5-18','S-1-5-32-544')){Assert-Equal ($sid-in $sids) $true 'required explicit private principal'}
 Assert-Equal (@($sids|Where-Object {$_-in @('S-1-1-0','S-1-5-11','S-1-5-32-545')}).Count) 0 'no broad private access'
 $digest=(Get-FileHash -LiteralPath $clientConfig).Hash
 $repeat=Convert-BridgeRoleSettings -Role Client -Runtime $runtime -Config $clientConfig
 Assert-Equal $repeat.files_changed 0 'normalization idempotent'
 Assert-Equal $repeat.credentials_copied 0 'normalization does not duplicate keys'
 Assert-Equal (Get-FileHash -LiteralPath $clientConfig).Hash $digest 'idempotent config bytes unchanged'
 $failureDir=Join-Path $runtime 'failure';New-Item -ItemType Directory -Path $failureDir|Out-Null
 $failureConfig=Join-Path $failureDir 'client.local.json';$failureVideo=Join-Path $failureDir 'client.video-send.local.json'
 Write-FixtureJson $failureConfig @{proxy=@{credential_file=$externalProxy}}
 Write-FixtureJson $failureVideo @{key_file=(Join-Path $external 'missing.key')}
 $digest=(Get-FileHash -LiteralPath $failureConfig).Hash;$blocked=$false
 try{$null=Convert-BridgeRoleSettings -Role Client -Runtime $runtime -Config $failureConfig}catch{$blocked=$true}
 Assert-Equal $blocked $true 'missing external key refuses normalization'
 Assert-Equal (Get-FileHash -LiteralPath $failureConfig).Hash $digest 'failed normalization retains original config'
 Assert-Equal (@(Get-ChildItem -LiteralPath (Join-Path $failureDir 'portable-credentials') -File).Count) 0 'failed normalization removes new copies'
 $oversized=Join-Path $failureDir 'oversized.json';[IO.File]::WriteAllText($oversized,('x'*1048577));$blocked=$false
 try{$null=Read-BridgeNormalizationJson $oversized}catch{$blocked=$true}
 Assert-Equal $blocked $true 'json size bound enforced'
 $renamedProject=Join-Path $testRoot 'project Renamed'
 foreach($candidate in @($project,$renamedProject)){if(-not [IO.Path]::GetFullPath($candidate).StartsWith([IO.Path]::GetFullPath($testRoot)+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture rename target.'}}
 [IO.Directory]::Move($project,$renamedProject)
 $runtime=Join-Path $renamedProject 'app';$clientVideo=Join-Path $runtime 'client.video-send.local.json';$sender=Read-FixtureJson $clientVideo
 Assert-Equal (Test-Path -LiteralPath (Resolve-BridgeConfigPath $sender.key_file $clientVideo $runtime)) $true 'copied key still resolves after root rename'
 Assert-Equal (Resolve-BridgeConfigPath (Read-FixtureJson (Join-Path $runtime 'host.local.json')).runtime (Join-Path $runtime 'host.local.json') $runtime) $runtime 'server runtime follows root rename'
 $tokens=$null;$errors=$null;$null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory 'Update-PortableSettings.ps1'),[ref]$tokens,[ref]$errors)
 Assert-Equal $errors.Count 0 'WinPS parser'
 [pscustomobject]@{passed=$true;scope='role settings normalization; private credential copies; root rename';cases=33}|ConvertTo-Json -Compress
}finally{
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
