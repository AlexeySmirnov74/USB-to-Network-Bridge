param([string]$RuntimeDirectory)
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
if(-not $RuntimeDirectory){$RuntimeDirectory=if(Test-Path -LiteralPath (Join-Path $root 'usbbridge-server.exe')){$root}else{Join-Path $root 'target\release'}}
$logs=Join-Path $root 'logs'
New-Item -ItemType Directory -Path $logs -Force|Out-Null
$server=Start-Process -FilePath (Join-Path $RuntimeDirectory 'usbbridge-server.exe') -ArgumentList '--mock','--export','all','--listen','127.0.0.1:13240' -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $logs 'transport-server.log') -RedirectStandardError (Join-Path $logs 'transport-server-errors.log')
try {
    Start-Sleep -Milliseconds 500
    if($server.HasExited){throw 'Test server failed to bind 127.0.0.1:13240.'}
    & (Join-Path $RuntimeDirectory 'usbbridge-client.exe') list 127.0.0.1:13240
    if($LASTEXITCODE-ne 0){throw 'Device-list test failed.'}
    foreach($direction in @('out','in')){
        Start-Sleep -Milliseconds 100
        & (Join-Path $RuntimeDirectory 'usbbridge-client.exe') bench 127.0.0.1:13240 256 262144 32 $direction
        if($LASTEXITCODE-ne 0){throw 'Transport benchmark failed.'}
    }
} finally {if(-not $server.HasExited){Stop-Process -Id $server.Id -Force}}
