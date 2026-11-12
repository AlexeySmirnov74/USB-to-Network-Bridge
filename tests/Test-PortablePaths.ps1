param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Portable-Paths.ps1')
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge путь '+[guid]::NewGuid().ToString('N'))
$runtime=Join-Path $testRoot 'app';$config=Join-Path $runtime 'data\Client\client.local.json'
$originalLocation=Get-Location
function Assert-Equal($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Portable path assertion failed: '+$Name)}}
try{
 New-Item -ItemType Directory -Path (Split-Path -Parent $config) -Force|Out-Null
 Set-Location -LiteralPath $env:SystemRoot
 Assert-Equal (Resolve-BridgeConfigPath 'camera key.bin' $config $runtime) (Join-Path (Split-Path -Parent $config) 'camera key.bin') 'config relative independent of cwd'
 Assert-Equal (Resolve-BridgeConfigPath '@app/video/key.bin' $config $runtime) (Join-Path $runtime 'video\key.bin') 'app token'
 Assert-Equal (Resolve-BridgeConfigPath '@app' $config) $SourceDirectory 'helper runtime default'
 Assert-Equal (Resolve-BridgeConfigPath '//server/share/key.bin' $config $runtime) '\\server\share\key.bin' 'qualified UNC readable'
 Assert-Equal (Resolve-BridgeConfigPath '@programdata/USB to Network Bridge/Server/host.local.json' $config $runtime) (Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge\Server\host.local.json') 'programdata token'
 Assert-Equal (ConvertTo-BridgePortablePath $runtime $config $runtime -RuntimePath) '@app' 'runtime serialization'
 Assert-Equal (ConvertTo-BridgePortablePath (Join-Path $runtime 'shared key.bin') $config $runtime) '@app/shared key.bin' 'app serialization from nested config'
 Assert-Equal (ConvertTo-BridgePortablePath (Join-Path (Split-Path -Parent $config) 'video-credentials\camera key.bin') $config $runtime) 'video-credentials\camera key.bin' 'relative secret serialization'
 $programDataConfig=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge\Server\host.local.json'
 Assert-Equal (ConvertTo-BridgePortablePath (Join-Path $runtime 'server.local.psd1') $programDataConfig $runtime) '@app/server.local.psd1' 'external configuration follows app instead of former project name'
 Assert-Equal (Get-BridgeRelativePath $runtime $runtime) '.' 'same directory is dot'
 Assert-Equal (Get-BridgeRelativePath $testRoot $runtime) '..' 'parent directory is dot dot'
 $renamedRoot=$testRoot+' renamed'
 Assert-Equal (Get-BridgeRelativePath $renamedRoot (Join-Path $renamedRoot 'app')) '..' 'renamed project keeps parent reference'
 $otherVolume=if([IO.Path]::GetPathRoot($testRoot)-eq 'Z:\'){'Y:\'}else{'Z:\'}
 $external=$otherVolume+'outside\legacy.key'
 Assert-Equal (Resolve-BridgeConfigPath $external $config $runtime) $external 'legacy cross volume path readable'
 $blocked=$false
 try{$null=ConvertTo-BridgePortablePath $external $config $runtime}catch{$blocked=$true}
 Assert-Equal $blocked $true 'cross volume absolute save rejected'
 $inputConfig=Join-Path $runtime 'input\request.json'
 $input=[pscustomobject]@{key_file='video.key';camera=[pscustomobject]@{serial='UBC-same'}}
 $resolvedInput=Resolve-BridgeConfigPaths $input $inputConfig $runtime
 $storedInput=ConvertTo-BridgePortableConfig $resolvedInput $config $runtime
 Assert-Equal $storedInput.key_file '@app/input/video.key' 'request owner base before destination save'
 Assert-Equal (Resolve-BridgeConfigPath $storedInput.key_file $config $runtime) (Join-Path $runtime 'input\video.key') 'destination refers to original request resource'
 $original=[pscustomobject]@{runtime=$runtime;profile=(Join-Path (Split-Path -Parent $config) 'server.local.psd1');server='192.0.2.1:3240';identity='USB\VID_1209&PID_F002\same';serial='UBC-unchanged';source='@device:sw:{source}';nested=@([pscustomobject]@{credential_file=(Join-Path (Split-Path -Parent $config) 'proxy-credentials\id.credential')},$null,@(1,2));limit=[UInt64]::MaxValue}
 $copy=ConvertTo-BridgePortableConfig $original $config $runtime
 Assert-Equal $copy.runtime '@app' 'clone runtime'
 Assert-Equal $copy.profile 'server.local.psd1' 'clone profile'
 Assert-Equal $copy.nested[0].credential_file 'proxy-credentials\id.credential' 'nested credential reference'
 Assert-Equal $original.runtime $runtime 'source object not mutated'
 foreach($field in @('server','identity','serial','source','limit')){Assert-Equal $copy.$field $original.$field ('preserved '+$field)}
 Assert-Equal $copy.nested.Count 3 'null and nested array positions preserved'
 Assert-Equal $copy.nested[2].Count 2 'nested array preserved'
 $resolved=Resolve-BridgeConfigPaths $copy $config $runtime
 Assert-Equal $resolved.runtime $runtime 'clone resolution'
 Assert-Equal $resolved.nested[0].credential_file $original.nested[0].credential_file 'secret path roundtrip'
 $blocked=$false
 try{$null=Resolve-BridgeConfigPath '@app/../outside.bin' $config $runtime}catch{$blocked=$true}
 Assert-Equal $blocked $true 'token escape rejected'
 $blocked=$false
 try{$null=Resolve-BridgeConfigPath 'C:relative.bin' $config $runtime}catch{$blocked=$true}
 Assert-Equal $blocked $true 'drive relative rejected'
 foreach($invalid in @('@external/secret.key','C:','/without-volume')){
  $blocked=$false
  try{$null=Resolve-BridgeConfigPath $invalid $config $runtime}catch{$blocked=$true}
  Assert-Equal $blocked $true ('ambiguous path rejected '+$invalid)
 }
 foreach($file in @('Portable-Paths.ps1','Configure-Server.ps1','Set-ServerState.ps1','Publish-Device.ps1','Set-Autostart.ps1','Set-VideoMode.ps1')){
  $tokens=$null;$errors=$null;$null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory $file),[ref]$tokens,[ref]$errors)
  if($errors.Count){throw ('PowerShell parsing failed: '+$file)}
 }
 [pscustomobject]@{passed=$true;scope='portable paths, config clones, WinPS parser';cases=35}|ConvertTo-Json -Compress
}finally{
 Set-Location -LiteralPath $originalLocation.Path
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected test cleanup path.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
