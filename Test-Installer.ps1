param([switch]$Elevate,[switch]$Prepared)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$root=$PSScriptRoot;$logs=Join-Path $root 'logs\installer-qualification';New-Item -ItemType Directory -Path $logs -Force|Out-Null
if(-not $Prepared){
 foreach($entry in @(@('0.2.0','020'),@('0.2.1','021'),@('0.2.2','022'))){& (Join-Path $root 'Build-Installer.ps1') -TestFixture -VersionOverride $entry[0] -OutputPath (Join-Path $root ('dist\installer-smoke-v'+$entry[1]+'.msi'))|Out-Null;if(-not $?){throw 'Test MSI build failed.'}}
}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 if(-not $Elevate){throw 'Installer qualification requires Administrator rights.'}
 $p=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Prepared' -PassThru
 $p.WaitForExit();exit $p.ExitCode
}
$installed=Join-Path $env:ProgramFiles 'USB Bridge Installer Qualification'
$meta0=Get-Content (Join-Path $root 'dist\installer-smoke-v020.msi.json') -Raw|ConvertFrom-Json
$meta1=Get-Content (Join-Path $root 'dist\installer-smoke-v021.msi.json') -Raw|ConvertFrom-Json
$meta2=Get-Content (Join-Path $root 'dist\installer-smoke-v022.msi.json') -Raw|ConvertFrom-Json
if(Test-Path -LiteralPath $installed){throw 'An old qualification directory exists; inspect it before rerunning.'}
$beforeSessions=if(Test-Path (Join-Path $root 'production\native-sessions.local.json')){[IO.File]::ReadAllText((Join-Path $root 'production\native-sessions.local.json'))}else{$null}
$checks=New-Object 'Collections.Generic.List[object]'
function Msi([string]$Name,[string]$Arguments,[int]$Expected=0){
 $log=Join-Path $logs ($Name+'.log')
 $p=Start-Process msiexec.exe -ArgumentList ($Arguments+' /qn /norestart REBOOT=ReallySuppress /l*v "'+$log+'"') -WindowStyle Hidden -Wait -PassThru
 if($p.ExitCode-ne $Expected){throw ($Name+' returned '+$p.ExitCode+' (expected '+$Expected+'). Log: '+$log)}
 $checks.Add([ordered]@{test=$Name;exit_code=$p.ExitCode;passed=$true})
}
function Assert-Application{
 foreach($file in @('usbbridge.exe','usbbridge-server.exe','usbbridge-client.exe','usbbridge-agent.exe')){
  if((Get-FileHash -LiteralPath (Join-Path $installed $file)).Hash-ne (Get-FileHash -LiteralPath (Join-Path $root ('target\release\'+$file))).Hash){throw ('Installed binary differs from candidate: '+$file)}
 }
 foreach($file in @('Open-Installation.ps1','Configure-Installation.ps1','Maintain-Installation.ps1')){
  if((Get-FileHash -LiteralPath (Join-Path $installed $file)).Hash-ne (Get-FileHash -LiteralPath (Join-Path $root $file)).Hash){throw ('Installed setup helper differs from candidate: '+$file)}
 }
}
try{
 Msi 'fresh-install' ('/i "'+(Join-Path $root 'dist\installer-smoke-v020.msi')+'"')
 Assert-Application
 # Keep the data outside the managed application tree, as real role configurations are.
 $saved=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge\InstallerQualification\preserve.local.json'
 New-Item -ItemType Directory -Path (Split-Path -Parent $saved) -Force|Out-Null
 [IO.File]::WriteAllText($saved,'{"retain":true}');$savedHash=(Get-FileHash $saved).Hash
 [IO.File]::WriteAllText((Join-Path $installed 'usbbridge.exe'),'repair fixture')
 Msi 'repair' ('/fa '+$meta0.product_code)
 Assert-Application
 Msi 'upgrade' ('/i "'+(Join-Path $root 'dist\installer-smoke-v021.msi')+'"')
 Assert-Application
 Msi 'rollback-injected-failure' ('/i "'+(Join-Path $root 'dist\installer-smoke-v022.msi')+'" USB_BRIDGE_TEST_FAIL_AFTER_FILES=1') 1603
 Assert-Application
 Msi 'downgrade-refused' ('/i "'+(Join-Path $root 'dist\installer-smoke-v020.msi')+'"') 1603
 Assert-Application
 Msi 'uninstall' ('/x '+$meta1.product_code)
 if(Test-Path -LiteralPath (Join-Path $installed 'usbbridge-agent.exe')){throw 'Application executable remained after uninstall.'}
 if((Get-FileHash $saved).Hash-ne $savedHash){throw 'User data was modified.'}
 if($beforeSessions-and [IO.File]::ReadAllText((Join-Path $root 'production\native-sessions.local.json'))-ne $beforeSessions){throw 'The unrelated development USB session changed.'}
 [ordered]@{schema=1;passed=$true;checks=$checks;configuration_preserved=$true;unrelated_usb_session_preserved=$true;role_activation_tested=$false;application_signing='unsigned';finished_utc=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json -Depth 6|Set-Content (Join-Path $logs 'result.local.json') -Encoding utf8
}catch{[ordered]@{schema=1;passed=$false;checks=$checks;error=$_.Exception.Message;line=$_.InvocationInfo.ScriptLineNumber;finished_utc=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json -Depth 6|Set-Content (Join-Path $logs 'result.local.json') -Encoding utf8;throw}
