param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot),[string]$PortablePathsFile=(Join-Path $SourceDirectory 'Portable-Paths.ps1'))
$ErrorActionPreference='Stop'
$fixture=Join-Path ([IO.Path]::GetTempPath()) ('USBBridge-Maintenance-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture|Out-Null
$checks=0
function Assert-Equal($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Maintenance assertion failed: '+$Name+'; actual='+$Actual)};$script:checks++}
function Assert-Throws([scriptblock]$Action,[string]$Name){$caught=$false;try{& $Action|Out-Null}catch{$caught=$true};if(-not $caught){throw ('Maintenance accepted unsafe fixture: '+$Name)};$script:checks++}
try{
 $helper=Join-Path $fixture 'helpers';New-Item -ItemType Directory -Path $helper|Out-Null
 Copy-Item -LiteralPath (Join-Path $SourceDirectory 'Maintain-Installation.ps1') -Destination $helper
 Copy-Item -LiteralPath $PortablePathsFile -Destination (Join-Path $helper 'Portable-Paths.ps1')
 . (Join-Path $helper 'Maintain-Installation.ps1') -FunctionsOnly
 # Loading the pure functions requires no MSI operation, tasks or backend state.
 Assert-Equal ([bool](Get-Command Convert-MaintenanceTaskXml -ErrorAction SilentlyContinue)) $true 'functions-only entry point'
 $old=Join-Path $fixture 'Legacy';$new=Join-Path $old 'app';$data=Join-Path $new 'data\Server'
 New-Item -ItemType Directory -Path $data -Force|Out-Null
 $profile=Join-Path $new 'server.local.psd1'
 [IO.File]::WriteAllText($profile,("@{ Runtime = '"+$old.Replace("'","''")+"'; Listen = '0.0.0.0:3240'; Export = @() }"))
 $config=Join-Path $data 'host.local.json';$oldConfig=Join-Path $old 'data\Server\host.local.json'
 $hostValue=[ordered]@{runtime=$old;profile=(Join-Path $old 'server.local.psd1');server_id='preserve-server';name='Notebook';enabled=$true;listen='0.0.0.0:3240'}
 [IO.File]::WriteAllText($config,($hostValue|ConvertTo-Json))
 $xml='<Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Actions><Exec><Command>'+[Security.SecurityElement]::Escape((Join-Path $old 'usbbridge-agent.exe'))+'</Command><Arguments>--supervise --host --config &quot;'+[Security.SecurityElement]::Escape($oldConfig)+'&quot;</Arguments><WorkingDirectory>'+[Security.SecurityElement]::Escape($old)+'</WorkingDirectory></Exec></Actions></Task>'
 $journal=[pscustomobject]@{tasks=@([pscustomobject]@{xml=$xml})}
 $writes=@(Get-MaintenanceConfigWrites $journal $old $new)
 Assert-Equal $writes.Count 2 'host and profile writes discovered'
 $hostWrite=$writes|Where-Object path -eq $config;$saved=$hostWrite.content|ConvertFrom-Json
 Assert-Equal $saved.runtime '@app' 'host runtime is portable'
 Assert-Equal $saved.profile '@app/server.local.psd1' 'profile refers to runtime independently of host data directory'
 Assert-Equal $saved.server_id 'preserve-server' 'server identity preserved'
 Assert-Equal $saved.enabled $true 'enabled state preserved'
 $profileWrite=$writes|Where-Object path -eq $profile
 Assert-Equal ($profileWrite.content.Contains("Runtime = '@app'")) $true 'profile runtime literal is portable'
 Assert-Equal ([Text.Encoding]::UTF8.GetString($hostWrite.before).Contains($old.Replace('\','\\'))) $true 'rollback bytes retained'
 # A subsequent MSI run accepts values already expressed relative to the config.
 [IO.File]::WriteAllText($config,$hostWrite.content);[IO.File]::WriteAllText($profile,$profileWrite.content)
 $again=@(Get-MaintenanceConfigWrites $journal $old $new)
 Assert-Equal $again.Count 0 'portable configuration remains unchanged'
 $bad=$saved|ConvertTo-Json|ConvertFrom-Json;$bad.runtime=Join-Path $fixture 'OtherInstall'
 [IO.File]::WriteAllText($config,($bad|ConvertTo-Json))
 Assert-Throws {Get-MaintenanceConfigWrites $journal $old $new} 'unowned host runtime'
 $legacy='{"runtime":"C:\\Old","key_file":"C:\\Old\\video.key","camera":{"serial":"KEEP-SERIAL"}}'
 $converted=Convert-MaintenanceJsonContent $legacy 'C:\Old' 'D:\Moved' 'D:\Moved\receiver.local.json'|ConvertFrom-Json
 Assert-Equal $converted.runtime '@app' 'JSON resource conversion uses supplied runtime'
 Assert-Equal $converted.key_file 'video.key' 'JSON key remains config relative'
 Assert-Equal $converted.camera.serial 'KEEP-SERIAL' 'camera serial preserved'
 Write-Output ('Maintenance portable fixtures passed: '+$checks)
}finally{
 $absolute=[IO.Path]::GetFullPath($fixture);$temporaryRoot=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $absolute.StartsWith(($temporaryRoot+'\USBBridge-Maintenance-'),[StringComparison]::OrdinalIgnoreCase)){throw 'Refusing to remove a fixture outside the temporary directory.'}
 Remove-Item -LiteralPath $absolute -Recurse -Force
}
