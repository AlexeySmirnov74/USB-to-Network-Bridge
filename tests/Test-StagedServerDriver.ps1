param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'FirstRun-Identity.ps1')
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('USB Bridge staged driver fixture '+[guid]::NewGuid().ToString('N'))
$script:count=0
function Assert-StagedDriver([bool]$Condition,[string]$Name){if(-not $Condition){throw ('Staged driver assertion failed: '+$Name)};$script:count++}
try {
 $script:source=Join-Path $testRoot 'installed\Drivers';$script:storeRoot=Join-Path $testRoot 'Windows\System32\DriverStore\FileRepository';$script:store=Join-Path $script:storeRoot 'vboxusb.inf_amd64_fixture';$script:publishedRoot=Join-Path $testRoot 'Windows\INF';$script:published=Join-Path $script:publishedRoot 'oem123.inf'
 foreach($directory in @($script:source,$script:store,$script:publishedRoot)){New-Item -ItemType Directory -Path $directory -Force|Out-Null}
 foreach($name in @('VBoxUSB.inf','VBoxUSB.sys','VBoxUSB.cat','VBoxUSBMon.sys')){[IO.File]::WriteAllText((Join-Path $script:source $name),('trusted fixture bytes '+$name));if($name-ne 'VBoxUSBMon.sys'){Copy-Item -LiteralPath (Join-Path $script:source $name) -Destination (Join-Path $script:store $name)}}
 Copy-Item -LiteralPath (Join-Path $script:source 'VBoxUSB.inf') -Destination $script:published
 $script:stagedImplementation=${function:Test-BridgeStagedVBoxUsbDriver}
 function Test-BridgeStagedVBoxUsbDriver([string]$MonitorPath){& $script:stagedImplementation -MonitorPath $MonitorPath -DriverStoreRoot $script:storeRoot -PublishedInfRoot $script:publishedRoot}
 $script:publishedMode='normal';$script:vboxMode='Absent';$script:monitorMode='Manual';$script:stockPresent=$true
 function Get-BridgePublishedVBoxUsbInf([string]$DriverStoreInf){
  if($script:publishedMode-eq 'unpublished'){return $null}
  [pscustomobject]@{StoreInf=$(if($script:publishedMode-eq 'wrong-store'){Join-Path $script:storeRoot 'other\VBoxUSB.inf'}else{$DriverStoreInf});PublishedInf=$(if($script:publishedMode-eq 'outside'){Join-Path $testRoot 'oem123.inf'}elseif($script:publishedMode-eq 'basename'){'oem123.inf'}else{$script:published})}
 }
 function Get-Service {param($Name,$ErrorAction) if($script:stockPresent){[pscustomobject]@{Name='usbipd';Status='Stopped'}}}
 function Get-CimInstance {
  param($ClassName,$Filter,$ErrorAction)
  if($Filter-eq "Name='VBoxUSBMon'"){return [pscustomobject]@{PathName=(Join-Path $script:source 'VBoxUSBMon.sys');StartMode=$script:monitorMode}}
  if($Filter-eq "Name='VBoxUSB'"-and $script:vboxMode-ne 'Absent'){return [pscustomobject]@{PathName=(Join-Path $script:source 'VBoxUSB.sys');StartMode=$script:vboxMode}}
  if($Filter-eq "Name='usbip2_ude'"){return [pscustomobject]@{PathName=(Join-Path $script:source 'VBoxUSB.sys');StartMode='Manual'}}
 }
 $result=Get-BridgeDriverReadiness Server
 Assert-StagedDriver $result.Ready 'fresh staged package is ready before the VBoxUSB device service is created'
 Assert-StagedDriver ($result.Partial-and $result.Missing.Count-eq 0) 'fresh staged package retains present driver ownership'
 $script:vboxMode='Disabled';Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'existing disabled VBoxUSB service is never bypassed';$script:vboxMode='Absent'
 $script:monitorMode='Disabled';Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'disabled monitor remains unready';$script:monitorMode='Manual'
 $script:stockPresent=$false;Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'missing usbipd service remains unready';$script:stockPresent=$true
 $script:publishedMode='unpublished';Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'loose package without Windows publication is rejected';$script:publishedMode='normal'
 $file=Join-Path $script:store 'VBoxUSB.cat';Remove-Item -LiteralPath $file;Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'missing staged catalog is rejected';Copy-Item -LiteralPath (Join-Path $script:source 'VBoxUSB.cat') -Destination $file
 foreach($name in @('VBoxUSB.sys','VBoxUSB.inf','VBoxUSB.cat')){
  $file=Join-Path $script:store $name;[IO.File]::WriteAllText($file,'modified fixture');Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) ('modified staged '+$name+' is rejected');Copy-Item -LiteralPath (Join-Path $script:source $name) -Destination $file -Force
 }
 $file=Join-Path $script:source 'VBoxUSB.cat';[IO.File]::WriteAllText($file,'modified installed fixture');Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'installed and staged publisher package bytes must match';Copy-Item -LiteralPath (Join-Path $script:store 'VBoxUSB.cat') -Destination $file -Force
 [IO.File]::WriteAllText($script:published,'modified published fixture');Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'published OEM INF must match the installed package';Copy-Item -LiteralPath (Join-Path $script:source 'VBoxUSB.inf') -Destination $script:published -Force
 $script:publishedMode='outside';Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'published INF outside Windows INF root is rejected'
 $script:publishedMode='wrong-store';Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'SetupAPI store roundtrip must match the exact candidate'
 $script:publishedMode='basename';Assert-StagedDriver (Get-BridgeDriverReadiness Server).Ready 'SetupAPI OEM basename resolves against Windows INF directory';$script:publishedMode='normal'
 $file=Join-Path $script:source 'VBoxUSB.inf';Remove-Item -LiteralPath $file;Assert-StagedDriver (-not(Get-BridgeDriverReadiness Server).Ready) 'missing installed INF is rejected';Copy-Item -LiteralPath (Join-Path $script:store 'VBoxUSB.inf') -Destination $file
 $script:vboxMode='Manual';$script:publishedMode='unpublished';Assert-StagedDriver (Get-BridgeDriverReadiness Server).Ready 'existing enabled device service retains the established readiness path'
 Assert-StagedDriver (Get-BridgeDriverReadiness Client).Ready 'client readiness path is unchanged'
 $tokens=$null;$errors=$null;$null=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory 'FirstRun-Identity.ps1'),[ref]$tokens,[ref]$errors)
 Assert-StagedDriver ($errors.Count-eq 0) 'WinPS parser'
 [pscustomobject]@{passed=$true;assertions=$script:count;scope='fresh server staged-driver readiness; published OEM identity; missing and modified package rejection'}|ConvertTo-Json -Compress
}finally{
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
