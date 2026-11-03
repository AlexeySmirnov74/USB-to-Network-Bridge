# A normal launch never elevates or reconfigures an already initialized role.
param([Parameter(Mandatory=$true)][ValidateSet('Server','Client')][string]$Role)
$ErrorActionPreference='Stop'
$data=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) ('USB to Network Bridge\'+$Role)
$config=Join-Path $data $(if($Role-eq 'Server'){'host.local.json'}else{'client.local.json'})
$marker=Join-Path $data 'installation.local.json'
$ready=$false
if((Test-Path -LiteralPath $config)-and (Test-Path -LiteralPath $marker)){
 try{$state=Get-Content -LiteralPath $marker -Raw|ConvertFrom-Json;$ready=($state.initialized-and $state.runtime-eq $PSScriptRoot)}catch{}
}
if(-not $ready){
 & (Join-Path $PSScriptRoot 'Configure-Installation.ps1') -Role $Role -Elevate
 exit $LASTEXITCODE
}
Start-Process -FilePath (Join-Path $PSScriptRoot 'usbbridge-agent.exe') -ArgumentList '--gui','--role',$Role.ToLowerInvariant(),'--config',('"'+$config+'"') -WindowStyle Hidden
