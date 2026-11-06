# Normal launches preserve the configured role. -Auto also supports portable copies.
param(
 [Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role,
 [switch]$Auto,
 [string]$Config
)
$ErrorActionPreference='Stop'
try {
 $agent=Join-Path $PSScriptRoot 'usbbridge-agent.exe'
 if(-not(Test-Path -LiteralPath $agent -PathType Leaf)){throw 'Keep the launcher with the complete application package; usbbridge-agent.exe is missing.'}
 $data=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) ('USB to Network Bridge\'+$Role)
 $marker=Join-Path $data 'installation.local.json'
 $state=$null
 if(Test-Path -LiteralPath $marker){try{$state=Get-Content -LiteralPath $marker -Raw|ConvertFrom-Json}catch{}}
 $installed=-not $Auto
 if($Auto){
  $property=$Role+'Directory'
  $registration=Get-ItemProperty -LiteralPath 'HKLM:\Software\USBtoNetworkBridge' -Name $property -ErrorAction SilentlyContinue
  $directory=if($registration){[string]$registration.$property}else{''}
  $installed=($directory-and $directory.TrimEnd('\')-eq $PSScriptRoot.TrimEnd('\'))-or ($state-and $state.runtime-eq $PSScriptRoot)
 }
 if($Config){
  $Config=[IO.Path]::GetFullPath($Config)
  if(-not(Test-Path -LiteralPath $Config -PathType Leaf)){throw ('Configuration file not found: '+$Config)}
 }elseif($installed){
  $Config=Join-Path $data $(if($Role-eq 'Server'){'host.local.json'}else{'client.local.json'})
  $ready=(Test-Path -LiteralPath $Config)-and $state-and $state.initialized-and $state.runtime-eq $PSScriptRoot
  if(-not $ready){
   & (Join-Path $PSScriptRoot 'Configure-Installation.ps1') -Role $Role -Elevate
   exit $LASTEXITCODE
  }
 }else{
  $Config=Join-Path $PSScriptRoot $(if($Role-eq 'Server'){'host.local.json'}else{'client.local.json'})
  if(-not(Test-Path -LiteralPath $Config)){
   if($Role-eq 'Server'){throw 'Complete the portable server setup in README.md first; host.local.json is missing. Then double-click USB-to-Network-Bridge-Server.exe.'}
   $legacy=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'USBBridge\client.json'
   if(Test-Path -LiteralPath $legacy -PathType Leaf){$Config=$legacy}
  }
 }
 Start-Process -FilePath $agent -ArgumentList '--gui','--role',$Role.ToLowerInvariant(),'--config',('"'+$Config+'"') -WorkingDirectory $PSScriptRoot -WindowStyle Hidden
}catch{
 Add-Type -AssemblyName System.Windows.Forms
 [Windows.Forms.MessageBox]::Show($_.Exception.Message,'USB to Network Bridge','OK','Error')|Out-Null
 exit 1
}
