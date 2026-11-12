# Developer/CI-only retrieval. Runtime setup has no network fallback.
param([string]$Destination=$PSScriptRoot)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$pin=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'unity-capture.json') -Raw|ConvertFrom-Json
if($pin.schema-ne 1-or $pin.file-cne 'components/unity-capture/UnityCaptureFilter64.dll'-or $pin.architecture-cne 'x64'-or $pin.sha256-notmatch '^[A-Fa-f0-9]{64}$'){throw 'Invalid reviewed Unity Capture pins.'}
$file=Join-Path $Destination $pin.file
New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force|Out-Null
if(-not(Test-Path -LiteralPath $file)){
 $partial=$file+'.partial'
 try{
  Invoke-WebRequest -UseBasicParsing -Uri $pin.url -OutFile $partial
  if((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash-ne $pin.sha256){throw 'Downloaded Unity Capture hash mismatch.'}
  Move-Item -LiteralPath $partial -Destination $file -Force
 }finally{if(Test-Path -LiteralPath $partial){Remove-Item -LiteralPath $partial -Force}}
}
if((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash-ne $pin.sha256){throw 'Unity Capture hash mismatch.'}
Write-Output 'Pinned offline Unity Capture component verified.'
