# Elevated local-only driver setup shared by first run and camera receivers.
param(
 [ValidateSet('Server','Client')][string]$Role,
 [string]$RuntimeDirectory=$PSScriptRoot,
 [switch]$FunctionsOnly
)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
function Get-BridgeMissingClientRuntime {
 $system=Join-Path $env:SystemRoot 'System32'
 @('MSVCP140.dll','MSVCP140_ATOMIC_WAIT.dll','VCRUNTIME140.dll','VCRUNTIME140_1.dll')|Where-Object {-not(Test-Path -LiteralPath (Join-Path $system $_) -PathType Leaf)}
}
function Ensure-BridgeProductionDrivers {
 param([Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,[string]$RuntimeDirectory=$PSScriptRoot)
 $runtime=[IO.Path]::GetFullPath($RuntimeDirectory)
 . (Join-Path $runtime 'FirstRun-Identity.ps1')
 $readiness=Get-BridgeDriverReadiness $Role $runtime
 if($readiness.Ready){return [pscustomobject]@{RestartRequired=$false;Changed=$false;Ready=$true}}
 $installerDirectory=Join-Path $runtime 'tools\driver-installers'
 $restart=$false
 if(-not $readiness.Partial){
  $driver=@(& (Join-Path $runtime 'Install-ProductionDrivers.ps1') -Role $Role -InstallerDirectory $installerDirectory -AllowedPeer @('127.0.0.1'))
  if(-not $?){throw 'Bundled signed driver installation failed.'}
  $restart=@($driver|Where-Object {$_.PSObject.Properties['RestartRequired']-and $_.RestartRequired}).Count-gt 0
 }else{
  if($Role-eq 'Server'-and -not(Get-Service usbipd -ErrorAction SilentlyContinue)){throw 'An existing VirtualBox capture driver conflicts with automatic setup. Repair the supported usbipd-win installation before retrying.'}
  $verified=@(& (Join-Path $runtime 'Verify-DriverInstaller.ps1') -Role $Role -InstallerDirectory $installerDirectory)
  if(-not $?-or $verified.Count-ne 1-or -not $verified[0].OfflineVerified){throw 'Bundled signed driver verification failed. No download was attempted.'}
  $installer=[string]$verified[0].Path
  if($Role-eq 'Server'){
   $operation=Start-Process msiexec.exe -ArgumentList '/i',('"'+$installer+'"'),'/qn','/norestart','REINSTALL=ALL','REINSTALLMODE=vomus' -WindowStyle Hidden -Wait -PassThru
  }else{
   $operation=Start-Process $installer -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/RESTARTEXITCODE=3010','/SP-','/NOICONS','/TYPE=custom','/COMPONENTS=main,client','/TASKS=vcredist' -WindowStyle Hidden -Wait -PassThru
  }
  if($operation.ExitCode-notin @(0,3010)){throw ('Bundled upstream driver repair failed: '+$operation.ExitCode)}
  $restart=$operation.ExitCode-eq 3010
 }
 if($Role-eq 'Client'){
  $missingRuntime=@(Get-BridgeMissingClientRuntime)
  if($missingRuntime.Count){throw ('The bundled Visual C++ runtime prerequisite did not become available: '+($missingRuntime-join ', ')+'. Driver setup is incomplete; inspect the installer log. Windows was not restarted.')}
 }
 $after=Get-BridgeDriverReadiness $Role $runtime
 if(-not $after.Ready){$restart=$true}
 [pscustomobject]@{RestartRequired=$restart;Changed=$true;Ready=[bool]$after.Ready}
}
if($FunctionsOnly){return}
if(-not $Role){throw 'Role is required.'}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){throw 'Signed driver setup requires Administrator rights.'}
Ensure-BridgeProductionDrivers -Role $Role -RuntimeDirectory $RuntimeDirectory
