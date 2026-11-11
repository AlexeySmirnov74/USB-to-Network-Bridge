# Optional producer integration. Uses an already installed upstream Unity Capture DLL.
# The two Unity sources are independent of the names of the resulting USB cameras.
param([Parameter(Mandatory=$true)][string]$FilterDll)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
if(-not [Environment]::Is64BitProcess){throw 'Run this helper in 64-bit PowerShell.'}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){throw 'Administrator rights are required to register Unity Capture sources.'}
$FilterDll=[IO.Path]::GetFullPath($FilterDll)
if(-not(Test-Path -LiteralPath $FilterDll -PathType Leaf)){throw 'Select the existing UnityCaptureFilter64.dll from a permanent directory.'}
if([IO.Path]::GetFileName($FilterDll)-ne 'UnityCaptureFilter64.dll'){throw 'The x64 upstream Unity Capture DLL is required.'}
$classes='HKLM:\SOFTWARE\Classes\CLSID'
$category=Join-Path $classes '{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance'
$slots=@(
 [pscustomobject]@{guid='{5C2CD55C-92AD-4999-8666-912BD3E70010}';name='Unity Capture Server'},
 [pscustomobject]@{guid='{5C2CD55C-92AD-4999-8666-912BD3E70012}';name='Unity Capture Client'}
)
foreach($slot in $slots){
 $server=Join-Path $classes ($slot.guid+'\InprocServer32')
 if(Test-Path -LiteralPath $server){
  $owner=(Get-Item -LiteralPath $server).GetValue('')
  if([IO.Path]::GetFullPath($owner)-ne $FilterDll){throw 'A Unity slot belongs to a different installation. Keep its permanent DLL location.'}
 }
}
$registrar=Join-Path $env:SystemRoot 'System32\regsvr32.exe'
$registered=Start-Process -FilePath $registrar -ArgumentList @('/s','/i:UnityCaptureDevices=2',('"'+$FilterDll+'"')) -WindowStyle Hidden -Wait -PassThru
if($registered.ExitCode-ne 0){throw ('Unity Capture registration failed: '+$registered.ExitCode)}
foreach($slot in $slots){
 $class=Join-Path $classes $slot.guid
 $instance=Join-Path $category $slot.guid
 if(-not(Test-Path -LiteralPath $instance)){throw 'Unity Capture did not register both requested slots.'}
 if((Get-Item -LiteralPath ($class+'\InprocServer32')).GetValue('')-ne $FilterDll){throw 'Unexpected Unity Capture registration owner.'}
 # pyvirtualcam reads the CLSID default; DirectShow reads the instance FriendlyName.
 Set-Item -LiteralPath $class -Value $slot.name
 Set-ItemProperty -LiteralPath $instance -Name FriendlyName -Value $slot.name
 if((Get-Item -LiteralPath $class).GetValue('')-ne $slot.name-or (Get-ItemProperty -LiteralPath $instance).FriendlyName-ne $slot.name){throw 'Unity Capture route name verification failed.'}
}
[ordered]@{success=$true;server_source=$slots[0].name;client_source=$slots[1].name;filter=$FilterDll;architecture='x64'}|ConvertTo-Json
