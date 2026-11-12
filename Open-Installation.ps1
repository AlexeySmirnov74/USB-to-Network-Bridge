# Launchers always pass through this identity/driver gate, including --config.
param(
 [Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,
 [switch]$Auto,[string]$Config,[string]$InstallRoot
)
$ErrorActionPreference='Stop'
try {
 . (Join-Path $PSScriptRoot 'FirstRun-Identity.ps1')
 $runtime=$PSScriptRoot
 if(-not $InstallRoot){$InstallRoot=if((Split-Path -Leaf $runtime)-eq 'app'){Split-Path -Parent $runtime}else{$runtime}}
 $InstallRoot=[IO.Path]::GetFullPath($InstallRoot)
 $agent=Join-Path $runtime 'usbbridge-agent.exe'
 if(-not(Test-Path -LiteralPath $agent -PathType Leaf)){throw 'Extract the complete application folder; app\usbbridge-agent.exe is missing.'}
 $data=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) ('USB to Network Bridge\'+$Role)
 $machine=Get-BridgeMachineIdentity
 $marker=Get-BridgeMachineMarker -Role $Role -Runtime $runtime
 # A folder move changes Windows startup/firewall registration, not identity.
 # Repair that registration before a single-instance GUI can activate the old copy.
 $binding=Get-BridgeDeploymentBinding -Runtime $runtime
 if($binding-and
    ((-not(Test-BridgeSamePath $binding.RuntimeDirectory $runtime))-or $binding.RelocationPending)){
  & (Join-Path $runtime 'Relocate-Installation.ps1') -InstallRoot $InstallRoot -DeploymentId (Get-BridgeDeploymentId $runtime) -Elevate
  if(-not $?){throw 'The moved installation could not be registered for startup.'}
  $marker=Get-BridgeMachineMarker -Role $Role -Runtime $runtime
  $binding=Get-BridgeDeploymentBinding -Runtime $runtime
 }
 $installed=-not $Auto
 if($Auto){
  $property=$Role+'Directory'
  $registration=Get-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge' -Name $property -ErrorAction SilentlyContinue
  $directory=if($registration){[string]$registration.$property}else{''}
  $installed=$directory-and ((Test-BridgeSamePath $directory $InstallRoot)-or (Test-BridgeSamePath $directory $runtime))
 }
 if($Config){
  $Config=[IO.Path]::GetFullPath($Config)
  if(-not(Test-Path -LiteralPath $Config -PathType Leaf)){throw ('Configuration file not found: '+$Config)}
 }else{
  $filename=if($Role-eq 'Server'){'host.local.json'}else{'client.local.json'}
  $configDirectory=if($installed-or $Role-eq 'Server'){$data}else{Join-Path $runtime 'data\Client'}
  $Config=Join-Path $configDirectory $filename
  if($marker-and $marker.machine_id-eq $machine-and $marker.config-and (Test-Path -LiteralPath $marker.config)-and (Test-BridgeSamePath $marker.runtime $runtime)){$Config=[string]$marker.config}
  elseif($binding-and $binding.($Role+'Config')){
   $locator=[string]$binding.($Role+'Config')
   $Config=if($locator-eq '@external'){[string]$binding.($Role+'ExternalConfig')}else{Resolve-BridgeConfigPath $locator (Join-Path $runtime 'deployment.local.json') $runtime}
  }
  elseif(-not $installed-and $Role-eq 'Server'-and (Test-Path -LiteralPath (Join-Path $runtime $filename))){$Config=Join-Path $runtime $filename}
  elseif(-not(Test-Path -LiteralPath $Config)){
   $legacy=Join-Path $runtime $filename
   if(Test-Path -LiteralPath $legacy -PathType Leaf){$Config=$legacy}
  }
 }
 $owned=(Test-BridgeOwnedRoleTask $Role $runtime $Config)-or (Test-BridgeOwnedRoleProcess $Role $runtime $Config)
 $disposition=Get-BridgeMachineDisposition -Role $Role -Runtime $runtime -Config $Config -MachineIdentity $machine -OwnedTask:$owned
 # A legacy ProgramData marker belongs to this Windows installation, unlike
 # a copied portable marker. Adopt it only at its own role config/runtime.
 if($installed-and $disposition-in @('Unverified','New')){
  $legacy=Read-BridgeJson (Join-Path $data 'installation.local.json')
  if((Test-BridgeInstalledRoleMarker -Marker $legacy -Role $Role -Runtime $runtime -InstallRoot $InstallRoot -Config $Config -MarkerPath (Join-Path $data 'installation.local.json'))-and (Test-Path -LiteralPath $Config)){$disposition='Adopt'}
 }
 # Upgrade legacy absolute settings once. Normal launches do not request UAC.
 if($disposition-in @('Current','Adopt')-and
    ((-not $marker)-or $marker.schema-ne 2-or $marker.paths_version-ne 1-or -not $binding)){
  & (Join-Path $runtime 'Update-PortableSettings.ps1') -InstallRoot $InstallRoot -Role $Role -Config $Config -Elevate
  if(-not $?){throw 'Portable settings could not be updated.'}
  $marker=Get-BridgeMachineMarker -Role $Role -Runtime $runtime
 }
 $driver=Get-BridgeDriverReadiness $Role $runtime
 $needsSetup=$disposition-notin @('Current','Adopt')-or -not(Test-Path -LiteralPath $Config)-or -not $driver.Ready
 if($needsSetup){
  $choice='New'
  $adopt=$disposition-in @('Current','Adopt')
  if($Role-eq 'Client'-and -not $adopt-and (Test-Path -LiteralPath $Config)){
   Add-Type -AssemblyName System.Windows.Forms
   $ru=[Globalization.CultureInfo]::CurrentUICulture.TwoLetterISOLanguageName-eq 'ru'
   $message=if($ru){"Эта копия ещё не настроена для данного компьютера.`n`nДа — сохранить подключения к серверам и выбранные USB-устройства.`nНет — создать новые настройки.`nОтмена — выйти без изменений.`n`nСохранённые пароли прокси и локальную камеру потребуется настроить заново."}else{"This copy has not been configured for this computer.`n`nYes — keep server connections and selected USB devices.`nNo — create new settings.`nCancel — exit without changes.`n`nProxy passwords and the local camera must be configured again."}
   $answer=[Windows.Forms.MessageBox]::Show($message,'USB to Network Bridge','YesNoCancel','Question')
   if($answer-eq [Windows.Forms.DialogResult]::Cancel){exit 0}
   $choice=if($answer-eq [Windows.Forms.DialogResult]::Yes){'Keep'}else{'New'}
  }
  $arguments=@{Role=$Role;Elevate=$true;Config=$Config;InstallRoot=$InstallRoot;ClientChoice=$choice}
  if($adopt){$arguments.AdoptExisting=$true}
  & (Join-Path $runtime 'Configure-Installation.ps1') @arguments
  exit $LASTEXITCODE
 }
 # Reopening resumes an intentionally exited role. Health checks do not enter here.
 $stateText=& (Join-Path $runtime 'usbbridge.exe') --config $Config $Role.ToLowerInvariant() application-state status
 if($LASTEXITCODE-ne 0){throw 'Could not inspect the application exit state.'}
 $state=($stateText|Out-String)|ConvertFrom-Json
 if($state.stopped){
  & (Join-Path $runtime 'Quit-Application.ps1') -Role $Role -Config $Config -RuntimeDirectory $runtime -Resume -Elevate
  if(-not $?){throw 'The application could not resume after its previous exit.'}
 }
 Start-Process -FilePath $agent -ArgumentList '--gui','--role',$Role.ToLowerInvariant(),'--config',('"'+$Config+'"') -WorkingDirectory $runtime -WindowStyle Hidden
}catch{
 Add-Type -AssemblyName System.Windows.Forms
 [Windows.Forms.MessageBox]::Show($_.Exception.Message,'USB to Network Bridge','OK','Error')|Out-Null
 exit 1
}
