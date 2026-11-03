$script:BridgeOperationId=[guid]::NewGuid().ToString()
function Write-BridgeEvent([string]$Runtime,[string]$Event,[hashtable]$Fields){
    try{
        $directory=Join-Path $Runtime 'logs';[IO.Directory]::CreateDirectory($directory)|Out-Null
        $file=Join-Path $directory 'server-scripts.debug.jsonl'
        if((Test-Path -LiteralPath $file)-and (Get-Item -LiteralPath $file).Length-ge 33554432){for($i=4;$i-ge 1;$i--){$old=if($i-eq 1){$file}else{$file+'.'+($i-1)};$next=$file+'.'+$i;if(Test-Path -LiteralPath $next){Remove-Item -LiteralPath $next -Force};if(Test-Path -LiteralPath $old){Move-Item -LiteralPath $old -Destination $next}}}
        $row=[ordered]@{time_unix_ms=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds();pid=$PID;operation_id=$script:BridgeOperationId;component='server-scripts';event=$Event;fields=$Fields}|ConvertTo-Json -Depth 10 -Compress
        $bytes=[Text.UTF8Encoding]::new($false).GetBytes($row+[Environment]::NewLine)
        # Separate elevated/SYSTEM helpers can finish at the same time. Serialize
        # each complete JSON line with a short exclusive writer and bounded retry.
        for($attempt=0;$attempt-lt 25;$attempt++){
            $stream=$null
            try{
                $stream=[IO.File]::Open($file,[IO.FileMode]::Append,[IO.FileAccess]::Write,[IO.FileShare]::Read)
                $stream.Write($bytes,0,$bytes.Length)
                break
            }catch [IO.IOException]{if($attempt-eq 24){throw};Start-Sleep -Milliseconds 10}
            finally{if($stream){$stream.Dispose()}}
        }
    }catch{}
}
