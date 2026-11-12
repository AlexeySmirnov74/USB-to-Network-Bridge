# Build an x64 MSI with native Windows Installer APIs; no third-party compiler.
param([string]$PayloadZip,[string]$OutputPath,[string]$VersionOverride,[switch]$TestFixture)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$root=$PSScriptRoot
$version=[regex]::Match([IO.File]::ReadAllText((Join-Path $root 'Cargo.toml')),'(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
$msiVersion=($version-split '-')[0]
if($VersionOverride){if(-not $TestFixture){throw 'VersionOverride is restricted to installer fixtures.'};$msiVersion=$VersionOverride}
if($msiVersion-notmatch '^\d+\.\d+\.\d+$'){throw 'MSI version needs three numeric components.'}
if(-not $PayloadZip){$PayloadZip=Join-Path $root ('dist\USB-to-Network-Bridge-'+$version+'-windows-x64.zip')}
$PayloadZip=(Resolve-Path -LiteralPath $PayloadZip).Path
if(-not $OutputPath){$OutputPath=Join-Path $root ('dist\USB-to-Network-Bridge-'+$version+'-windows-x64.msi')}
$OutputPath=[IO.Path]::GetFullPath($OutputPath)
$stage=Join-Path $root ('dist\msi-build-'+[guid]::NewGuid().ToString('N'))
$payload=Join-Path $stage 'payload';New-Item -ItemType Directory -Path $payload -Force|Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive=[IO.Compression.ZipFile]::OpenRead($PayloadZip)
try{
 foreach($entry in $archive.Entries){
  $target=[IO.Path]::GetFullPath((Join-Path $payload $entry.FullName))
  if(-not $target.StartsWith($payload+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Package entry escapes payload directory.'}
 }
}finally{$archive.Dispose()}
Expand-Archive -LiteralPath $PayloadZip -DestinationPath $payload
$launchers=@('USB-to-Network-Bridge-Server.exe','USB-to-Network-Bridge-Client.exe')
$visibleFiles=@(Get-ChildItem -LiteralPath $payload -File)
$visibleFolders=@(Get-ChildItem -LiteralPath $payload -Directory)
if($visibleFiles.Count-ne 2-or @($visibleFiles|Where-Object Name -notin $launchers).Count-or $visibleFolders.Count-ne 1-or $visibleFolders[0].Name-cne 'app'){throw 'The package root must contain only the two desktop EXEs and app.'}
$hashes=Get-Content -LiteralPath (Join-Path $payload 'app\SHA256.json') -Raw|ConvertFrom-Json
$expected=@{}
foreach($item in $hashes){
 $target=[IO.Path]::GetFullPath((Join-Path $payload $item.File))
 if(-not $target.StartsWith($payload+'\',[StringComparison]::OrdinalIgnoreCase)-or $expected.ContainsKey($item.File)-or $item.File-eq 'app/SHA256.json'){throw 'Invalid package hash manifest.'}
 if((Get-FileHash -LiteralPath $target).Hash-ne $item.SHA256-or (Get-Item -LiteralPath $target).Length-ne $item.Bytes){throw ('Payload hash mismatch: '+$item.File)}
 $expected[$item.File]=$true
}
$driverManifestPath=Join-Path $root 'driver-installers.json'
$driverManifest=Get-Content -LiteralPath $driverManifestPath -Raw|ConvertFrom-Json
if((Get-FileHash -LiteralPath (Join-Path $payload 'app\driver-installers.json')).Hash-ne (Get-FileHash -LiteralPath $driverManifestPath).Hash){throw 'The packaged driver pins differ from the reviewed manifest.'}
$driverPaths=@{}
foreach($role in @('Server','Client')){$driverPaths['app/tools/driver-installers/'+$driverManifest.$role.file]=[string]$driverManifest.$role.sha256}
& (Join-Path $root 'Verify-DriverInstaller.ps1') -Role Both -InstallerDirectory (Join-Path $payload 'app\tools\driver-installers') -ManifestPath $driverManifestPath|Out-Null
if(-not $?){throw 'MSI payload has invalid offline driver installers.'}
$unityPinPath=Join-Path $root 'unity-capture.json'
if((Get-FileHash -LiteralPath (Join-Path $payload 'app\unity-capture.json')).Hash-ne (Get-FileHash -LiteralPath $unityPinPath).Hash){throw 'The packaged Unity Capture pins differ from the reviewed manifest.'}
$unityPin=Get-Content -LiteralPath $unityPinPath -Raw|ConvertFrom-Json
if($unityPin.schema-ne 1-or $unityPin.file-cne 'components/unity-capture/UnityCaptureFilter64.dll'-or $unityPin.architecture-cne 'x64'){throw 'Invalid Unity Capture pins.'}
$driverPaths['app/'+$unityPin.file]=[string]$unityPin.sha256
if((Get-FileHash -LiteralPath (Join-Path $payload ('app/'+$unityPin.file))).Hash-ne $unityPin.sha256){throw 'Pinned offline Unity Capture hash mismatch.'}
$applicationExecutables=@('USB-to-Network-Bridge-Server.exe','USB-to-Network-Bridge-Client.exe','app/usbbridge.exe','app/usbbridge-client.exe','app/usbbridge-server.exe','app/usbbridge-agent.exe','app/usbbridge-camera.exe')
foreach($item in @(Get-ChildItem -LiteralPath $payload -File -Recurse)){
 $relative=$item.FullName.Substring($payload.Length+1).Replace('\','/')
 if($relative-ne 'app/SHA256.json'-and -not $expected.ContainsKey($relative)){throw ('Unlisted payload file: '+$relative)}
 $pinnedDriver=$driverPaths.ContainsKey($relative)
 if($pinnedDriver-and (Get-FileHash -LiteralPath $item.FullName).Hash-ne $driverPaths[$relative]){throw ('Pinned driver payload hash mismatch: '+$relative)}
 $privateName=$relative.ToLowerInvariant()
 if($privateName.Contains('.local.')-or $privateName-match '\.(status\.json|preferences\.json|agent\.lock|edit\.lock|tray\.lock)$'-or $item.Extension-in @('.pfx','.key','.pem','.credential','.sys','.cat','.jsonl','.log')-or ($item.Extension-in @('.exe','.dll','.msi')-and $relative-cnotin $applicationExecutables-and -not $pinnedDriver)){throw ('Private/runtime/unreviewed binary in package: '+$relative)}
}
foreach($helper in @('Open-Installation.ps1','Configure-Installation.ps1','FirstRun-Identity.ps1','Migrate-PortableLayout.ps1','Maintain-Installation.ps1','Verify-DriverInstaller.ps1','Ensure-ProductionDrivers.ps1','Ensure-VideoPrerequisites.ps1')){if(-not(Test-Path -LiteralPath (Join-Path $payload ('app\'+$helper)))){throw ('Rebuild Package.ps1: missing app/'+$helper)}}
function Com-Call($Object,$Name,$Arguments){
 switch($Name){
  'OpenDatabase' {$Object.OpenDatabase([string]$Arguments[0],[int]$Arguments[1])}
  'OpenView' {$Object.OpenView([string]$Arguments[0])}
  'CreateRecord' {$Object.CreateRecord([int]$Arguments[0])}
  'Execute' {if($Arguments.Count){$Object.Execute($Arguments[0])}else{$Object.Execute()}}
  'Close' {$Object.Close()}
  'SetStream' {$Object.SetStream([int]$Arguments[0],[string]$Arguments[1])}
  'Persist' {$Object.Persist()}
  'Commit' {$Object.Commit()}
  default {throw ('Unknown COM method: '+$Name)}
 }
}
function Com-Set($Object,$Name,$Arguments){
 switch($Name){
  'StringData' {$Object.StringData([int]$Arguments[0])=[string]$Arguments[1]}
  'IntegerData' {$Object.IntegerData([int]$Arguments[0])=[int]$Arguments[1]}
  'Property' {$Object.Property([int]$Arguments[0])=$Arguments[1]}
  default {throw ('Unknown COM property: '+$Name)}
 }
}
$tableColumns=@{}
$installer=New-Object -ComObject WindowsInstaller.Installer
$buildMsi=Join-Path $stage 'package.msi'
$db=Com-Call $installer 'OpenDatabase' @($buildMsi,3)
function Query([string]$Sql){$view=Com-Call $db 'OpenView' @($Sql);try{Com-Call $view 'Execute' @()|Out-Null}finally{Com-Call $view 'Close' @()|Out-Null}}
function Table($Name,$Columns){
 $tableColumns[$Name]=@([regex]::Matches($Columns,'`([^`]+)` (?:CHAR|SHORT|LONG|OBJECT)')|ForEach-Object {$_.Groups[1].Value})
 Query ('CREATE TABLE '+[char]96+$Name+[char]96+' ('+$Columns+')')
}
function Insert($Name,[object[]]$Values){
 $view=Com-Call $db 'OpenView' @('INSERT INTO '+[char]96+$Name+[char]96+' ('+(($tableColumns[$Name]|ForEach-Object {[char]96+$_+[char]96})-join ',')+') VALUES ('+(($Values|ForEach-Object {'?'})-join ',')+')')
 $record=Com-Call $installer 'CreateRecord' @($Values.Count)
 try{
  for($index=0;$index-lt $Values.Count;$index++){
   if($null-eq $Values[$index]){continue}
   $value=$Values[$index]
   if($value-is [int]-or $value-is [long]){Com-Set $record 'IntegerData' @(($index+1),[int]$value)|Out-Null}else{Com-Set $record 'StringData' @(($index+1),[string]$value)|Out-Null}
  }
  Com-Call $view 'Execute' @($record)|Out-Null
 }finally{Com-Call $view 'Close' @()|Out-Null;[Runtime.InteropServices.Marshal]::FinalReleaseComObject($record)|Out-Null}
}
function Guid-For([string]$Text){
 $hash=[Security.Cryptography.MD5]::Create()
 try{$bytes=$hash.ComputeHash([Text.Encoding]::UTF8.GetBytes('USBtoNetworkBridge.MSI.'+$(if($TestFixture){'Smoke.'}else{'Production.'})+$Text));return ([guid]::new($bytes)).ToString('B').ToUpperInvariant()}finally{$hash.Dispose()}
}
Table Property '`Property` CHAR(72) NOT NULL, `Value` CHAR(0) LOCALIZABLE PRIMARY KEY `Property`'
Table Directory '`Directory` CHAR(72) NOT NULL, `Directory_Parent` CHAR(72), `DefaultDir` CHAR(255) NOT NULL LOCALIZABLE PRIMARY KEY `Directory`'
Table Component '`Component` CHAR(72) NOT NULL, `ComponentId` CHAR(38), `Directory_` CHAR(72) NOT NULL, `Attributes` SHORT NOT NULL, `Condition` CHAR(255), `KeyPath` CHAR(72) PRIMARY KEY `Component`'
Table Feature '`Feature` CHAR(38) NOT NULL, `Feature_Parent` CHAR(38), `Title` CHAR(64) LOCALIZABLE, `Description` CHAR(255) LOCALIZABLE, `Display` SHORT, `Level` SHORT NOT NULL, `Directory_` CHAR(72), `Attributes` SHORT NOT NULL PRIMARY KEY `Feature`'
Table FeatureComponents '`Feature_` CHAR(38) NOT NULL, `Component_` CHAR(72) NOT NULL PRIMARY KEY `Feature_`, `Component_`'
Table File '`File` CHAR(72) NOT NULL, `Component_` CHAR(72) NOT NULL, `FileName` CHAR(255) NOT NULL LOCALIZABLE, `FileSize` LONG NOT NULL, `Version` CHAR(72), `Language` CHAR(20), `Attributes` SHORT, `Sequence` SHORT NOT NULL PRIMARY KEY `File`'
Table Media '`DiskId` SHORT NOT NULL, `LastSequence` LONG NOT NULL, `DiskPrompt` CHAR(64) LOCALIZABLE, `Cabinet` CHAR(255), `VolumeLabel` CHAR(32), `Source` CHAR(72) PRIMARY KEY `DiskId`'
Table Registry '`Registry` CHAR(72) NOT NULL, `Root` SHORT NOT NULL, `Key` CHAR(255) NOT NULL LOCALIZABLE, `Name` CHAR(255) LOCALIZABLE, `Value` CHAR(0) LOCALIZABLE, `Component_` CHAR(72) NOT NULL PRIMARY KEY `Registry`'
Table Shortcut '`Shortcut` CHAR(72) NOT NULL, `Directory_` CHAR(72) NOT NULL, `Name` CHAR(128) NOT NULL LOCALIZABLE, `Component_` CHAR(72) NOT NULL, `Target` CHAR(72) NOT NULL, `Arguments` CHAR(255), `Description` CHAR(255) LOCALIZABLE, `Hotkey` SHORT, `Icon_` CHAR(72), `IconIndex` SHORT, `ShowCmd` SHORT, `WkDir` CHAR(72) PRIMARY KEY `Shortcut`'
Table Icon '`Name` CHAR(72) NOT NULL, `Data` OBJECT NOT NULL PRIMARY KEY `Name`'
Table RemoveFile '`FileKey` CHAR(72) NOT NULL, `Component_` CHAR(72) NOT NULL, `FileName` CHAR(255) LOCALIZABLE, `DirProperty` CHAR(72) NOT NULL, `InstallMode` SHORT NOT NULL PRIMARY KEY `FileKey`'
Table Upgrade '`UpgradeCode` CHAR(38) NOT NULL, `VersionMin` CHAR(20), `VersionMax` CHAR(20), `Language` CHAR(255), `Attributes` LONG NOT NULL, `Remove` CHAR(255), `ActionProperty` CHAR(72) NOT NULL PRIMARY KEY `UpgradeCode`, `VersionMin`, `VersionMax`, `Language`, `Attributes`'
Table LaunchCondition '`Condition` CHAR(255) NOT NULL, `Description` CHAR(255) NOT NULL LOCALIZABLE PRIMARY KEY `Condition`'
Table CustomAction '`Action` CHAR(72) NOT NULL, `Type` SHORT NOT NULL, `Source` CHAR(72), `Target` CHAR(0) PRIMARY KEY `Action`'
foreach($name in @('InstallExecuteSequence','InstallUISequence','AdminExecuteSequence')){Table $name '`Action` CHAR(72) NOT NULL, `Condition` CHAR(255), `Sequence` SHORT PRIMARY KEY `Action`'}
$upgrade=Guid-For $(if($TestFixture){'SmokeUpgrade'}else{'ProductionUpgrade'})
$product=Guid-For (([string]$upgrade)+$msiVersion)
$properties=[ordered]@{ProductCode=$product;UpgradeCode=$upgrade;ProductName='USB to Network Bridge';ProductVersion=$msiVersion;ProductLanguage='1033';Manufacturer='USB to Network Bridge contributors';ALLUSERS='1';INSTALLLEVEL='1';ARPNOMODIFY='1';ARPHELPLINK='https://github.com/AlexeySmirnov74/USB-to-Network-Bridge';ARPURLINFOABOUT='https://github.com/AlexeySmirnov74/USB-to-Network-Bridge';ARPPRODUCTICON='AppIcon';SecureCustomProperties='OLDPRODUCTS;NEWERPRODUCTS';DefaultUIFont='DefaultFont'}
if($TestFixture){$properties.ProductName+=' Installer Qualification'}
foreach($p in $properties.GetEnumerator()){Insert Property @($p.Key,$p.Value)}
Insert Directory @('TARGETDIR',$null,'SourceDir')
Insert Directory @('ProgramFiles64Folder','TARGETDIR','.')
Insert Directory @('System64Folder','TARGETDIR','.')
Insert Directory @('ProgramMenuFolder','TARGETDIR','.')
Insert Directory @('INSTALLDIR','ProgramFiles64Folder',$(if($TestFixture){'UBTEST~1|USB Bridge Installer Qualification'}else{'USBBRI~1|USB to Network Bridge'}))
Insert Directory @('AppMenu','ProgramMenuFolder',$(if($TestFixture){'UBTEST~1|USB Bridge Installer Qualification'}else{'USBBRI~1|USB to Network Bridge'}))
Insert Feature @('Core',$null,'Application files','Shared Rust applications, runtime helpers and documentation.',1,1,'INSTALLDIR',16)
Insert Feature @('Server','Core','Server','Server setup and desktop shortcut.',3,1,'INSTALLDIR',0)
Insert Feature @('Client','Core','Client','Client setup and desktop shortcut.',5,1,'INSTALLDIR',0)
$directories=@{''='INSTALLDIR'};$cabLines=New-Object 'Collections.Generic.List[string]'
$cabLines.Add('.OPTION EXPLICIT');$cabLines.Add('.Set Cabinet=on');$cabLines.Add('.Set Compress=on');$cabLines.Add('.Set CompressionType=LZX');$cabLines.Add('.Set CabinetNameTemplate=payload.cab');$cabLines.Add('.Set DiskDirectoryTemplate="'+$stage+'"');$cabLines.Add('.Set MaxDiskSize=0');$cabLines.Add('.Set MaxCabinetSize=0')
$number=0
foreach($file in @(Get-ChildItem -LiteralPath $payload -File -Recurse|Sort-Object FullName)){
 $relative=$file.FullName.Substring($payload.Length+1)
 $folder=Split-Path -Parent $relative
 if(-not $directories.ContainsKey($folder)){
  $parts=$folder-split '\\';$partial=''
  foreach($part in $parts){
   $parent=$partial;$partial=if($partial){$partial+'\'+$part}else{$part}
   if(-not $directories.ContainsKey($partial)){$id='D'+(Guid-For $partial).Replace('-','').Trim('{}');$directories[$partial]=$id;Insert Directory @($id,$directories[$parent],$part)}
  }
 }
 $number++;$id='F'+$number;$component='C'+$number
 Insert Component @($component,(Guid-For $relative),$directories[$folder],256,$null,$id)
 Insert FeatureComponents @('Core',$component)
 $fileVersion=$null;$language=$null
 if($file.Extension-eq '.exe'){$fileVersion=([Diagnostics.FileVersionInfo]::GetVersionInfo($file.FullName).FileVersion-split ' ')[0];$language='1033'}
 Insert File @($id,$component,$file.Name,[int]$file.Length,$fileVersion,$language,512,$number)
 $cabLines.Add('"'+$file.FullName+'" '+$id)
}
foreach($role in @('Server','Client')){
 $component=$role+'Shortcut'
 Insert Component @($component,(Guid-For $component),'INSTALLDIR',260,$null,($role+'Registry'))
 Insert FeatureComponents @($role,$component)
 Insert Registry @(($role+'Registry'),2,$(if($TestFixture){'Software\USBtoNetworkBridgeInstallerQualification'}else{'Software\USBtoNetworkBridge'}),($role+'Installed'),'[ProductVersion]',$component)
 Insert Registry @(($role+'Directory'),2,$(if($TestFixture){'Software\USBtoNetworkBridgeInstallerQualification'}else{'Software\USBtoNetworkBridge'}),($role+'Directory'),'[INSTALLDIR]',$component)
 Insert Shortcut @(($role+'Shortcut'),'AppMenu',('USB to Network Bridge '+$role),$component,('[INSTALLDIR]USB-to-Network-Bridge-'+$role+'.exe'),$null,('Configure or open the '+$role.ToLowerInvariant()),$null,'AppIcon',0,1,'INSTALLDIR')
 Insert RemoveFile @(($role+'MenuFolder'),$component,$null,'AppMenu',2)
}
Insert Media @(1,$number,$null,'#payload.cab',$null,$null)
Insert Upgrade @($upgrade,$null,$msiVersion,$null,1,$null,'OLDPRODUCTS')
Insert Upgrade @($upgrade,$msiVersion,$null,$null,2,$null,'NEWERPRODUCTS')
Insert LaunchCondition @('VersionNT64','This package requires 64-bit Windows.')
Insert LaunchCondition @('NOT NEWERPRODUCTS','A newer version is already installed.')
$baseSequence=@(@('FindRelatedProducts',25),@('LaunchConditions',100),@('ValidateProductID',700),@('CostInitialize',800),@('FileCost',900),@('CostFinalize',1000))
foreach($item in $baseSequence){Insert InstallExecuteSequence @($item[0],$null,$item[1]);Insert InstallUISequence @($item[0],$null,$item[1])}
$execute=@(@('MigrateFeatureStates',1200),@('InstallValidate',1400),@('InstallInitialize',1500),@('RemoveExistingProducts',6550),@('ProcessComponents',1600),@('UnpublishFeatures',1800),@('RemoveShortcuts',3200),@('RemoveFiles',3500),@('RemoveFolders',3600),@('InstallFiles',4000),@('CreateShortcuts',4500),@('WriteRegistryValues',5000),@('RemoveRegistryValues',5100),@('RegisterUser',6000),@('RegisterProduct',6100),@('PublishFeatures',6300),@('PublishProduct',6400),@('InstallExecute',6500),@('InstallFinalize',6600))
foreach($item in $execute){Insert InstallExecuteSequence @($item[0],$null,$item[1])}
Insert CustomAction @('SetPowerShell',51,'MAINTENANCE_EXE','[System64Folder]WindowsPowerShell\v1.0\powershell.exe')
Insert InstallExecuteSequence @('SetPowerShell',$null,1450)
$actionBase='-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "[INSTALLDIR]app\Maintain-Installation.ps1" -RuntimeDirectory "[INSTALLDIR]app" -PreviousRuntimeDirectory "[INSTALLDIR]." -Operation '
# Directory-source EXE actions run in INSTALLDIR. The encoded dispatcher reads
# that working directory as data instead of interpolating an install path into
# PowerShell code. Before InstallFiles, a 0.2.6 upgrade still has only the flat
# helper; repair/removal/rollback of the new layout uses the helper under app.
function Maintenance-Dispatch([ValidateSet('Stop','Resume','Remove')][string]$Operation){
 $code='$ErrorActionPreference=''Stop'';$r=[Environment]::CurrentDirectory;$a=Join-Path $r ''app'';$h=Join-Path $a ''Maintain-Installation.ps1'';if(Test-Path -LiteralPath $h){$runtime=$a}else{$runtime=$r;$h=Join-Path $r ''Maintain-Installation.ps1''};if(Test-Path -LiteralPath $h){& $h -Operation ''OPERATION'' -RuntimeDirectory $runtime;if(-not $?){exit 1}};exit 0'
 $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code.Replace('OPERATION',$Operation)))
 return '"[System64Folder]WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand '+$encoded
}
Insert CustomAction @('RollbackResume',3362,'INSTALLDIR',(Maintenance-Dispatch 'Resume'))
Insert InstallExecuteSequence @('RollbackResume','Installed OR OLDPRODUCTS',1510)
Insert CustomAction @('StopOwned',3106,'INSTALLDIR',(Maintenance-Dispatch 'Stop'))
Insert InstallExecuteSequence @('StopOwned','Installed OR OLDPRODUCTS',1520)
Insert CustomAction @('RemoveOwned',3106,'INSTALLDIR',(Maintenance-Dispatch 'Remove'))
Insert InstallExecuteSequence @('RemoveOwned','REMOVE="ALL" AND NOT UPGRADINGPRODUCTCODE',3300)
Insert CustomAction @('ResumeOwned',3122,'MAINTENANCE_EXE',($actionBase+'Resume -AfterInstall'))
Insert InstallExecuteSequence @('ResumeOwned','NOT REMOVE="ALL"',6570)
if($TestFixture){Insert CustomAction @('InjectFailure',19,$null,'Installer rollback qualification failure.');Insert InstallExecuteSequence @('InjectFailure','USB_BRIDGE_TEST_FAIL_AFTER_FILES="1"',6560)}
# Minimal native feature-selection dialog; /qn remains available for managed installs.
Table TextStyle '`TextStyle` CHAR(72) NOT NULL, `FaceName` CHAR(32) NOT NULL, `Size` SHORT NOT NULL, `Color` LONG, `StyleBits` SHORT PRIMARY KEY `TextStyle`'
Table Dialog '`Dialog` CHAR(72) NOT NULL, `HCentering` SHORT NOT NULL, `VCentering` SHORT NOT NULL, `Width` SHORT NOT NULL, `Height` SHORT NOT NULL, `Attributes` LONG, `Title` CHAR(128) LOCALIZABLE, `Control_First` CHAR(50) NOT NULL, `Control_Default` CHAR(50), `Control_Cancel` CHAR(50) PRIMARY KEY `Dialog`'
Table Control '`Dialog_` CHAR(72) NOT NULL, `Control` CHAR(50) NOT NULL, `Type` CHAR(20) NOT NULL, `X` SHORT NOT NULL, `Y` SHORT NOT NULL, `Width` SHORT NOT NULL, `Height` SHORT NOT NULL, `Attributes` LONG, `Property` CHAR(72), `Text` CHAR(0) LOCALIZABLE, `Control_Next` CHAR(50), `Help` CHAR(50) LOCALIZABLE PRIMARY KEY `Dialog_`, `Control`'
Table ControlEvent '`Dialog_` CHAR(72) NOT NULL, `Control_` CHAR(50) NOT NULL, `Event` CHAR(50) NOT NULL, `Argument` CHAR(255) NOT NULL, `Condition` CHAR(255), `Ordering` SHORT PRIMARY KEY `Dialog_`, `Control_`, `Event`, `Argument`, `Condition`'
Insert TextStyle @('DefaultFont','Segoe UI',10,0,0)
Insert Dialog @('ChooseRoles',50,50,390,260,3,'[ProductName] [ProductVersion]','Features','Install','Cancel')
Insert Control @('ChooseRoles','Heading','Text',20,15,350,22,3,$null,'Choose server, client, or both',$null,$null)
Insert Control @('ChooseRoles','Description','Text',20,43,350,38,3,$null,'Installs the application files. Open a Start menu shortcut afterwards to configure signed USB drivers and Windows startup. No automatic restart.',$null,$null)
Insert Control @('ChooseRoles','Features','SelectionTree',20,90,350,100,3,$null,$null,'Install',$null)
Insert Control @('ChooseRoles','Location','Text',20,198,350,24,3,$null,'Location: [INSTALLDIR]',$null,$null)
Insert Control @('ChooseRoles','Install','PushButton',220,232,72,18,3,$null,'Install','Cancel',$null)
Insert Control @('ChooseRoles','Cancel','PushButton',300,232,72,18,3,$null,'Cancel','Features',$null)
Insert ControlEvent @('ChooseRoles','Install','EndDialog','Return','1',1)
Insert ControlEvent @('ChooseRoles','Cancel','EndDialog','Exit','1',1)
Insert InstallUISequence @('ChooseRoles','NOT Installed',1230)
Insert InstallUISequence @('ExecuteAction',$null,1300)
foreach($item in @(@('CostInitialize',800),@('FileCost',900),@('CostFinalize',1000),@('InstallValidate',1400),@('InstallInitialize',1500),@('InstallAdminPackage',3900),@('InstallFiles',4000),@('InstallFinalize',6600))){Insert AdminExecuteSequence @($item[0],$null,$item[1])}
$ddf=Join-Path $stage 'payload.ddf'
[IO.File]::WriteAllLines($ddf,$cabLines,[Text.Encoding]::Default)
$p=Start-Process -FilePath (Join-Path $env:WINDIR 'System32\makecab.exe') -ArgumentList '/F',('"'+$ddf+'"') -WorkingDirectory $stage -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput (Join-Path $stage 'makecab.log')
if($p.ExitCode-ne 0){throw 'makecab failed; inspect its build log.'}
function Insert-Stream($Table,$Key,$File){
 $view=Com-Call $db 'OpenView' @('INSERT INTO '+[char]96+$Table+[char]96+' (`Name`, `Data`) VALUES (?, ?)')
 $record=Com-Call $installer 'CreateRecord' @(2)
 try{Com-Set $record 'StringData' @(1,$Key)|Out-Null;Com-Call $record 'SetStream' @(2,$File)|Out-Null;Com-Call $view 'Execute' @($record)|Out-Null}finally{Com-Call $view 'Close' @()|Out-Null}
}
Insert-Stream '_Streams' 'payload.cab' (Join-Path $stage 'payload.cab')
Insert-Stream 'Icon' 'AppIcon' (Join-Path $root 'assets\usbbridge.ico')
$summary=$db.SummaryInformation(20)
foreach($item in @(@(1,1252),@(2,'USB to Network Bridge'),@(3,'Windows USB over network'),@(4,'USB to Network Bridge contributors'),@(7,'x64;1033'),@(9,([guid]::NewGuid().ToString('B').ToUpperInvariant())),@(14,500),@(15,2),@(18,'USB to Network Bridge native MSI builder'))){Com-Set $summary 'Property' @($item[0],$item[1])|Out-Null}
Com-Call $summary 'Persist' @()|Out-Null;Com-Call $db 'Commit' @()|Out-Null
[Runtime.InteropServices.Marshal]::FinalReleaseComObject($summary)|Out-Null
[Runtime.InteropServices.Marshal]::FinalReleaseComObject($db)|Out-Null
[Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)|Out-Null
Copy-Item -LiteralPath $buildMsi -Destination $OutputPath -Force
[ordered]@{file=$OutputPath;payload_sha256=(Get-FileHash -LiteralPath $PayloadZip).Hash;sha256=(Get-FileHash -LiteralPath $OutputPath).Hash;product_code=$product;upgrade_code=$upgrade;version=$msiVersion;file_count=$number;test_fixture=[bool]$TestFixture;application_signing='unsigned';driver_signing='upstream production signatures'}|ConvertTo-Json|Set-Content -LiteralPath ($OutputPath+'.json') -Encoding utf8
Get-Content -LiteralPath ($OutputPath+'.json') -Raw
