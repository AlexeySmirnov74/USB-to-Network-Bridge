param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Verify-DriverInstaller.ps1') -FunctionsOnly
. (Join-Path $SourceDirectory 'Ensure-ProductionDrivers.ps1') -FunctionsOnly
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge offline fixture '+[guid]::NewGuid().ToString('N'))
$script:assertions=0
function Assert-OfflineFixture([bool]$Condition,[string]$Name){if(-not $Condition){throw ('Offline driver assertion failed: '+$Name)};$script:assertions++}
function Write-OfflineFixtureJson([string]$Path,$Value){[IO.File]::WriteAllText($Path,($Value|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($true))}
function Reset-OfflineFixture {
 $global:OfflineDriverFixture=@{calls=0;before=$false;after=$true;partial=$false;fresh=0;verify=0;process=0;download=0;network=0;missingRuntime=@();arguments=@();failVerify=$false}
}
try {
 New-Item -ItemType Directory -Path $testRoot -Force|Out-Null
 $manifestPath=Join-Path $testRoot 'driver-installers.json';$fixtureFile=Join-Path $testRoot 'fixture.exe'
 [IO.File]::WriteAllText($fixtureFile,'unsigned fixture, not executable')
 $digest=(Get-FileHash -LiteralPath $fixtureFile -Algorithm SHA256).Hash
 $package=@{file='missing.exe';sha256=$digest;signer='Fixture Publisher';version='fixture'}
 Write-OfflineFixtureJson $manifestPath @{Client=$package;Server=$package}
 Reset-OfflineFixture
 function Invoke-WebRequest {$global:OfflineDriverFixture.network++;throw 'Unexpected network call.'}
 function Invoke-RestMethod {$global:OfflineDriverFixture.network++;throw 'Unexpected network call.'}
 $blocked=$false;$message='';try{$null=Assert-BridgeDriverInstaller Client $testRoot $manifestPath}catch{$blocked=$true;$message=$_.Exception.Message}
 Assert-OfflineFixture $blocked 'missing installer rejected'
 Assert-OfflineFixture ($message-match 'No download was attempted') 'missing installer reports local-only recovery'
 $package.file='fixture.exe';$package.sha256='0'*64;Write-OfflineFixtureJson $manifestPath @{Client=$package;Server=$package}
 $blocked=$false;try{$null=Assert-BridgeDriverInstaller Client $testRoot $manifestPath}catch{$blocked=$_.Exception.Message-match 'SHA256 mismatch'}
 Assert-OfflineFixture $blocked 'corrupt installer rejected before trust check'
 $package.sha256=$digest;Write-OfflineFixtureJson $manifestPath @{Client=$package;Server=$package}
 $blocked=$false;try{$null=Assert-BridgeDriverInstaller Client $testRoot $manifestPath}catch{$blocked=$_.Exception.Message-match 'offline signature verification failed'}
 Assert-OfflineFixture $blocked 'correct digest does not permit unsigned installer'
 $package.file='..\fixture.exe';Write-OfflineFixtureJson $manifestPath @{Client=$package;Server=$package}
 $blocked=$false;try{$null=Assert-BridgeDriverInstaller Client $testRoot $manifestPath}catch{$blocked=$_.Exception.Message-match 'manifest is invalid'}
 Assert-OfflineFixture $blocked 'manifest cannot escape installer directory'
 Assert-OfflineFixture ($global:OfflineDriverFixture.network-eq 0) 'local verification never invokes network helpers'
 [IO.File]::WriteAllText((Join-Path $testRoot 'FirstRun-Identity.ps1'),@'
function Get-BridgeDriverReadiness {param($Role,$Runtime) $global:OfflineDriverFixture.calls++;[pscustomobject]@{Ready=$(if($global:OfflineDriverFixture.calls-eq 1){$global:OfflineDriverFixture.before}else{$global:OfflineDriverFixture.after});Partial=$global:OfflineDriverFixture.partial;Missing=@('fixture')}}
'@)
 [IO.File]::WriteAllText((Join-Path $testRoot 'Install-ProductionDrivers.ps1'),@'
param($Role,$InstallerDirectory,$AllowedPeer) $global:OfflineDriverFixture.fresh++;[pscustomobject]@{RestartRequired=$false}
'@)
 [IO.File]::WriteAllText((Join-Path $testRoot 'Verify-DriverInstaller.ps1'),@'
param($Role,$InstallerDirectory) $global:OfflineDriverFixture.verify++;if($global:OfflineDriverFixture.failVerify){throw 'Bundled fixture installer missing.'};[pscustomobject]@{Path='fixture-installer-not-run.exe';OfflineVerified=$true}
'@)
 [IO.File]::WriteAllText((Join-Path $testRoot 'Download-Drivers.ps1'),'$global:OfflineDriverFixture.download++;throw ''Runtime download forbidden.''')
 function Get-BridgeMissingClientRuntime {@($global:OfflineDriverFixture.missingRuntime)}
 function Get-Service {param($Name,$ErrorAction) [pscustomobject]@{Name='usbipd';Status='Stopped'}}
 function Start-Process {param($FilePath,$ArgumentList,$WindowStyle,[switch]$Wait,[switch]$PassThru) $global:OfflineDriverFixture.process++;$global:OfflineDriverFixture.arguments=@($ArgumentList);[pscustomobject]@{ExitCode=0}}
 Reset-OfflineFixture;$global:OfflineDriverFixture.before=$true
 $result=Ensure-BridgeProductionDrivers Client $testRoot
 Assert-OfflineFixture ($result.Ready-and -not $result.Changed-and $global:OfflineDriverFixture.fresh-eq 0-and $global:OfflineDriverFixture.process-eq 0) 'ready driver leaves installation untouched'
 Reset-OfflineFixture
 $result=Ensure-BridgeProductionDrivers Client $testRoot
 Assert-OfflineFixture ($result.Ready-and $result.Changed-and $global:OfflineDriverFixture.fresh-eq 1) 'missing driver uses bundled installation path'
 Reset-OfflineFixture;$global:OfflineDriverFixture.partial=$true
 $result=Ensure-BridgeProductionDrivers Client $testRoot
 Assert-OfflineFixture ($result.Ready-and $global:OfflineDriverFixture.verify-eq 1-and $global:OfflineDriverFixture.process-eq 1-and $global:OfflineDriverFixture.arguments-contains '/TASKS=vcredist') 'repair verifies local installer and enables embedded VC prerequisite'
 Assert-OfflineFixture ($global:OfflineDriverFixture.download-eq 0-and $global:OfflineDriverFixture.network-eq 0) 'repair never downloads'
 Reset-OfflineFixture;$global:OfflineDriverFixture.partial=$true;$global:OfflineDriverFixture.failVerify=$true
 $blocked=$false;try{$null=Ensure-BridgeProductionDrivers Client $testRoot}catch{$blocked=$true}
 Assert-OfflineFixture ($blocked-and $global:OfflineDriverFixture.process-eq 0-and $global:OfflineDriverFixture.download-eq 0) 'missing repair package fails before installer launch without fallback'
 Reset-OfflineFixture;$global:OfflineDriverFixture.missingRuntime=@('MSVCP140.dll')
 $blocked=$false;try{$null=Ensure-BridgeProductionDrivers Client $testRoot}catch{$blocked=$_.Exception.Message-match 'Visual C\+\+ runtime prerequisite'}
 Assert-OfflineFixture $blocked 'failed VC prerequisite is reported as incomplete setup'
 Reset-OfflineFixture;$global:OfflineDriverFixture.after=$false
 $result=Ensure-BridgeProductionDrivers Client $testRoot
 Assert-OfflineFixture ($result.RestartRequired-and -not $result.Ready) 'pending controller requires manual restart'
 Reset-OfflineFixture;$global:OfflineDriverFixture.partial=$true
 $result=Ensure-BridgeProductionDrivers Server $testRoot
 Assert-OfflineFixture ($result.Ready-and $global:OfflineDriverFixture.process-eq 1) 'stopped stock service is present for supported local repair'
 foreach($name in @('Configure-Installation.ps1','Install-ProductionDrivers.ps1','Ensure-ProductionDrivers.ps1','Set-VideoMode.ps1')){
  $source=[IO.File]::ReadAllText((Join-Path $SourceDirectory $name))
  Assert-OfflineFixture ($source-notmatch 'Download-Drivers\.ps1|Invoke-WebRequest|Invoke-RestMethod') ('no runtime HTTP fetch in '+$name)
 }
 $source=[IO.File]::ReadAllText((Join-Path $SourceDirectory 'Set-VideoMode.ps1'))
 $ensure=$source.IndexOf("Join-Path `$runtime 'Ensure-ProductionDrivers.ps1'")
 Assert-OfflineFixture ($ensure-gt $source.IndexOf('The video firewall rule belongs to another application.')-and $ensure-lt $source.IndexOf('$taskStateChanged=$true')) 'receiver setup follows ownership checks and precedes endpoint mutations'
 $source=[IO.File]::ReadAllText((Join-Path $SourceDirectory 'Verify-DriverInstaller.ps1'))
 Assert-OfflineFixture ($source-match 'ProviderFlags=0x00001010'-and $source-notmatch 'Get-AuthenticodeSignature') 'native trust verification explicitly forbids certificate URL retrieval'
 $source=[IO.File]::ReadAllText((Join-Path $SourceDirectory 'Install-ProductionDrivers.ps1'))
 Assert-OfflineFixture ($source-match "'/TASKS=vcredist'") 'fresh install selects embedded VC prerequisite'
 $source=[IO.File]::ReadAllText((Join-Path $SourceDirectory 'Configure-Installation.ps1'))
 Assert-OfflineFixture ($source-match "Extension-notin @\([^\r\n]*'\.msi'") 'setup removes broad explicit MSI write permissions before privileged execution'
 foreach($name in @('Verify-DriverInstaller.ps1','Ensure-ProductionDrivers.ps1','Download-Drivers.ps1','Configure-Installation.ps1','Install-ProductionDrivers.ps1','Set-VideoMode.ps1')){
  $tokens=$null;$errors=$null;$null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory $name),[ref]$tokens,[ref]$errors)
  Assert-OfflineFixture ($errors.Count-eq 0) ('WinPS parser '+$name)
 }
 [pscustomobject]@{passed=$true;assertions=$script:assertions;scope='offline installer integrity; no HTTP fallback; receiver preflight; VC runtime'}|ConvertTo-Json -Compress
}finally{
 Remove-Variable -Name OfflineDriverFixture -Scope Global -ErrorAction SilentlyContinue
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
