# Move an existing deployed installation, never a source checkout. No files are deleted.
param([string]$InstallRoot,[string]$PayloadDirectory,[switch]$Elevate,[switch]$ValidateOnly)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
function Convert-BridgeLayoutPath([string]$Value,[string]$OldRoot,[string]$NewRuntime) {
 if([string]::Equals($Value,$OldRoot,[StringComparison]::OrdinalIgnoreCase)){return $NewRuntime}
 if($Value.StartsWith($OldRoot+'\',[StringComparison]::OrdinalIgnoreCase)){return $NewRuntime+$Value.Substring($OldRoot.Length)}
 $Value
}
function Convert-BridgeLayoutValue($Value,[string]$OldRoot,[string]$NewRuntime) {
 if($null-eq $Value){return $null}
 if($Value-is [string]){return (Convert-BridgeLayoutPath $Value $OldRoot $NewRuntime)}
 if($Value-is [System.Collections.IDictionary]){
  foreach($key in @($Value.Keys)){$Value[$key]=Convert-BridgeLayoutValue $Value[$key] $OldRoot $NewRuntime};return $Value
 }
 if($Value-is [System.Collections.IList]){
  $items=@(foreach($item in $Value){Convert-BridgeLayoutValue $item $OldRoot $NewRuntime});return ,$items
 }
 if($Value-is [pscustomobject]){
  foreach($property in $Value.PSObject.Properties){$property.Value=Convert-BridgeLayoutValue $property.Value $OldRoot $NewRuntime}
 }
 $Value
}
function Assert-BridgeLayoutChild([string]$Path,[string]$Parent) {
 $full=[IO.Path]::GetFullPath($Path);$base=[IO.Path]::GetFullPath($Parent).TrimEnd('\')
 if(-not $full.StartsWith($base+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Migration path escapes its installation directory.'}
 $full
}
function Assert-BridgeLayoutTree([string]$Path) {
 $item=Get-Item -LiteralPath $Path -Force
 if($item.Attributes-band [IO.FileAttributes]::ReparsePoint){throw ('Refusing a linked migration path: '+$Path)}
 if($item.PSIsContainer){
  foreach($child in @(Get-ChildItem -LiteralPath $Path -Force -Recurse)){
   if($child.Attributes-band [IO.FileAttributes]::ReparsePoint){throw ('Refusing a linked migration entry: '+$child.FullName)}
  }
 }
}
function Assert-BridgeLayoutPayload([string]$Payload) {
 $manifestName='app/SHA256.json'
 $manifestPath=Join-Path $Payload 'app\SHA256.json'
 if(-not(Test-Path -LiteralPath $manifestPath -PathType Leaf)){throw 'The application package has no app/SHA256.json manifest.'}
 if((Get-Item -LiteralPath $manifestPath).Length-gt 16777216){throw 'The application hash manifest is too large.'}
 $allowedExecutables=@('USB-to-Network-Bridge-Server.exe','USB-to-Network-Bridge-Client.exe','app/usbbridge.exe','app/usbbridge-client.exe','app/usbbridge-server.exe','app/usbbridge-agent.exe','app/usbbridge-camera.exe')
 $expected=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
 $privateDirectories=@('data','logs','credentials')
 foreach($directory in @(Get-ChildItem -LiteralPath $Payload -Directory -Recurse -Force)){
  $relative=$directory.FullName.Substring($Payload.Length+1).Replace('\','/')
  if(@($relative.Split('/')|Where-Object {$_-in $privateDirectories}).Count){throw ('Private data directory in the application package: '+$relative)}
 }
 $manifest=Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8|ConvertFrom-Json
 if($manifest-isnot [Array]-or $manifest.Count-eq 0){throw 'The application hash manifest must be a nonempty array.'}
 foreach($entry in $manifest){
  $name=$entry.File
  if($name-isnot [string]-or -not $name-or $name.Contains('\')-or $name.Contains(':')-or $name.StartsWith('/')-or $name.EndsWith('/')-or $name.Contains('//')){throw 'The package manifest contains an ambiguous or unsafe path.'}
  foreach($segment in $name.Split('/')){
   if($segment-in @('.','..')-or $segment.EndsWith('.')-or $segment.EndsWith(' ')-or $segment-match '^(?i:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)'){throw ('The package manifest contains an unsafe Windows path: '+$name)}
  }
  if($name-eq $manifestName-or -not $expected.Add($name)){throw 'The package manifest lists itself or repeats a case-insensitive file path.'}
  if($name-notin $allowedExecutables-and -not $name.StartsWith('app/',[StringComparison]::Ordinal)){throw ('Unexpected package file outside app: '+$name)}
  $leaf=[IO.Path]::GetFileName($name)
  $extension=[IO.Path]::GetExtension($name)
  if($name-match '(?i)\.local(?:\.|$)' -or $leaf-match '(?i)\.(status|preferences)\.json$|\.(?:agent|supervisor|edit|tray)\.lock$' -or $extension-in @('.key','.pem','.credential','.pfx','.p12','.log','.jsonl','.inventory')){throw ('Private configuration or credential in the application package: '+$name)}
  if(($extension-eq '.exe'-and $name-cnotin $allowedExecutables)-or $extension-in @('.dll','.sys','.cat','.msi','.pyd','.scr','.com','.ocx','.obj','.lib','.pdb')){throw ('Unexpected binary in the application package: '+$name)}
  if(@($name.Split('/')|Where-Object {$_-in $privateDirectories}).Count){throw ('Private data directory in the application package: '+$name)}
  $path=Assert-BridgeLayoutChild (Join-Path $Payload $name) $Payload
  if(-not(Test-Path -LiteralPath $path -PathType Leaf)){throw ('A file listed in the package manifest is missing: '+$name)}
  if(($entry.Bytes-isnot [int]-and $entry.Bytes-isnot [long])-or $entry.Bytes-lt 0-or $entry.SHA256-isnot [string]-or $entry.SHA256-notmatch '^[0-9A-Fa-f]{64}$'){throw ('Invalid size/hash in the package manifest: '+$name)}
  if((Get-Item -LiteralPath $path).Length-ne $entry.Bytes-or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash-ne $entry.SHA256){throw ('Application package hash mismatch: '+$name)}
 }
 foreach($name in $allowedExecutables){if(-not $expected.Contains($name)){throw ('The application package is missing a required executable: '+$name)}}
 foreach($file in @(Get-ChildItem -LiteralPath $Payload -File -Recurse -Force)){
  $relative=$file.FullName.Substring($Payload.Length+1).Replace('\','/')
  if($relative-cne $manifestName-and -not $expected.Contains($relative)){throw ('Unlisted application package file: '+$relative)}
 }
}
if($MyInvocation.InvocationName-eq '.'){return}
if(-not $InstallRoot){throw 'Specify the existing deployed installation with -InstallRoot.'}
if(-not $ValidateOnly-and -not $PayloadDirectory){throw 'Specify -PayloadDirectory with the complete new application package before migrating an active installation.'}
$root=[IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
$app=Assert-BridgeLayoutChild (Join-Path $root 'app') $root
$launchers=@('USB-to-Network-Bridge-Client.exe','USB-to-Network-Bridge-Server.exe')
if(-not(Test-Path -LiteralPath $root -PathType Container)){throw 'The installation directory does not exist.'}
if((Test-Path -LiteralPath (Join-Path $root 'Cargo.toml'))-or (Test-Path -LiteralPath (Join-Path $root '.git'))){throw 'A source checkout must not be migrated as an application installation.'}
Assert-BridgeLayoutTree $root
foreach($name in $launchers){if(-not(Test-Path -LiteralPath (Join-Path $root $name) -PathType Leaf)){throw ('Missing launcher: '+$name)}}
if(Test-Path -LiteralPath $app){throw 'This directory already has an app folder; no legacy files were moved.'}
foreach($name in @('usbbridge-agent.exe','Maintain-Installation.ps1')){if(-not(Test-Path -LiteralPath (Join-Path $root $name) -PathType Leaf)){throw ('Not a complete legacy installation: '+$name)}}
if($PayloadDirectory){
 $PayloadDirectory=[IO.Path]::GetFullPath($PayloadDirectory).TrimEnd('\')
 if($PayloadDirectory.StartsWith($root+'\',[StringComparison]::OrdinalIgnoreCase)-or $PayloadDirectory-eq $root){throw 'The new application package must be staged outside the installation being moved.'}
 Assert-BridgeLayoutTree $PayloadDirectory
 foreach($relative in @($launchers)+@('app\Maintain-Installation.ps1','app\FirstRun-Identity.ps1','app\Portable-Paths.ps1','app\Portable-Installation.ps1','app\Update-PortableSettings.ps1','app\Relocate-Installation.ps1','app\usbbridge-agent.exe')){
  if(-not(Test-Path -LiteralPath (Join-Path $PayloadDirectory $relative) -PathType Leaf)){throw ('Incomplete application package: '+$relative)}
 }
 $unexpected=@(Get-ChildItem -LiteralPath $PayloadDirectory -Force|Where-Object {$_.Name-notin ($launchers+@('app'))})
 if($unexpected.Count){throw 'The package contains unexpected top-level entries.'}
 Assert-BridgeLayoutPayload $PayloadDirectory
}
$entries=@(Get-ChildItem -LiteralPath $root -Force|Where-Object Name -notin $launchers)
foreach($entry in $entries){$null=Assert-BridgeLayoutChild $entry.FullName $root;$null=Assert-BridgeLayoutChild (Join-Path $app $entry.Name) $app}
if($ValidateOnly){[pscustomobject]@{root=$root;runtime=$app;entries_to_move=$entries.Count;payload=$PayloadDirectory;validated=$true};exit 0}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
 if(-not $Elevate){throw 'Moving an active installation requires Administrator rights. Run with -Elevate.'}
 $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-InstallRoot',('"'+$root+'"'))
 if($PayloadDirectory){$arguments+=@('-PayloadDirectory',('"'+$PayloadDirectory+'"'))}
 $child=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -PassThru
 $child.WaitForExit();exit $child.ExitCode
}
$oldAgent=Join-Path $root 'usbbridge-agent.exe';$roles=@()
foreach($task in @(Get-ScheduledTask -ErrorAction Stop)){
 if($task.TaskPath-ne '\USBBridge\'-or $task.TaskName-notin @('USBBridge.Server','USBBridge.Client')){continue}
 if(@($task.Actions).Count-ne 1-or $task.Actions[0].Execute-ne $oldAgent){continue}
 $match=[regex]::Match([string]$task.Actions[0].Arguments,'(?:^|\s)--config\s+(?:"([^"]+)"|(\S+))')
 if(-not $match.Success){throw 'An owned startup task has no configuration path.'}
 $config=if($match.Groups[1].Success){$match.Groups[1].Value}else{$match.Groups[2].Value}
 if(-not(Test-Path -LiteralPath $config -PathType Leaf)){throw 'An owned role configuration is missing.'}
 $roles+=@([pscustomobject]@{role=$task.TaskName.Split('.')[-1];config=$config})
}
if(-not $roles.Count){throw 'No verified local Bridge controller task owns this legacy installation.'}
$maintenance=if($PayloadDirectory){Join-Path $PayloadDirectory 'app\Maintain-Installation.ps1'}else{Join-Path $PSScriptRoot 'Maintain-Installation.ps1'}
$identity=if($PayloadDirectory){Join-Path $PayloadDirectory 'app\FirstRun-Identity.ps1'}else{Join-Path $PSScriptRoot 'FirstRun-Identity.ps1'}
if(-not(Test-Path -LiteralPath $identity -PathType Leaf)){throw 'The current identity helper is required for migration.'}
$powershell=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
& $powershell -NoProfile -ExecutionPolicy Bypass -File $maintenance -Operation Stop -RuntimeDirectory $root
if($LASTEXITCODE-ne 0){throw 'The owned Bridge components could not be stopped; no files were moved.'}
$moved=@();$completed=$false;$backup=$null
try {
 New-Item -ItemType Directory -Path $app|Out-Null
 foreach($entry in $entries){
  $destination=Assert-BridgeLayoutChild (Join-Path $app $entry.Name) $app
  if(Test-Path -LiteralPath $destination){throw 'A migration destination already exists.'}
  Move-Item -LiteralPath $entry.FullName -Destination $destination
  $moved+=@($entry.Name)
 }
 $backup=Join-Path $app ('layout-backups\'+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ'))
 New-Item -ItemType Directory -Path $backup -Force|Out-Null
 # Rewrite active path values, never logs or USB identities. Save originals first.
 $settings=@(Get-ChildItem -LiteralPath $app -File|Where-Object {$_.Name-match '^(host|client|autostart-|machine-|native-).*\.json$'-or $_.Name-eq 'backend-state.json'}|Where-Object {$_.Name-notmatch '\.(status|health\.local|operation\.local|supervisor\.local)\.json$'})
 $dataFolder=Join-Path $app 'data'
 if(Test-Path -LiteralPath $dataFolder){$settings+=@(Get-ChildItem -LiteralPath $dataFolder -Recurse -File -Filter '*.json')}
 foreach($file in $settings){
  if($file.Length-gt 16777216){throw 'An active settings file is too large to migrate.'}
  $original=[IO.File]::ReadAllText($file.FullName)
  $value=$original|ConvertFrom-Json
  $value=Convert-BridgeLayoutValue $value $root $app
  $updated=ConvertTo-Json -InputObject $value -Depth 32
  $relative=$file.FullName.Substring($app.Length+1)
  $saved=Assert-BridgeLayoutChild (Join-Path $backup ('settings\'+$relative)) $backup
  New-Item -ItemType Directory -Path (Split-Path -Parent $saved) -Force|Out-Null
  Copy-Item -LiteralPath $file.FullName -Destination $saved
  [IO.File]::WriteAllText($file.FullName,$updated,[Text.UTF8Encoding]::new($false))
 }
 if($PayloadDirectory){
  foreach($file in @(Get-ChildItem -LiteralPath $PayloadDirectory -Recurse -File)){
   $relative=$file.FullName.Substring($PayloadDirectory.Length+1)
   $target=Assert-BridgeLayoutChild (Join-Path $root $relative) $root
   if(Test-Path -LiteralPath $target){
    $saved=Assert-BridgeLayoutChild (Join-Path $backup ('previous-files\'+$relative)) $backup
    New-Item -ItemType Directory -Path (Split-Path -Parent $saved) -Force|Out-Null
    Copy-Item -LiteralPath $target -Destination $saved
   }
   New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force|Out-Null
   Copy-Item -LiteralPath $file.FullName -Destination $target -Force
  }
 }
 . (Join-Path $app 'FirstRun-Identity.ps1')
 foreach($role in $roles){
  $config=Convert-BridgeLayoutPath $role.config $root $app
  $marker=Write-BridgeMachineMarker -Role $role.role -Runtime $app -Config $config -InstallRoot $root -AdoptExisting
  # Legacy migration preserves the working configuration first. Its first
  # launch normalizes paths and creates the protected OS deployment binding.
  $marker|Add-Member -NotePropertyName paths_version -NotePropertyValue 0 -Force
  Write-BridgeJson (Get-BridgeMachineMarkerPath $role.role $app) $marker
 }
 # Existing shortcuts must launch through the same identity/driver gate.
 $shell=New-Object -ComObject WScript.Shell
 foreach($folder in @([Environment]::GetFolderPath('CommonStartMenu'),[Environment]::GetFolderPath('StartMenu'),[Environment]::GetFolderPath('DesktopDirectory'),[Environment]::GetFolderPath('CommonDesktopDirectory'))|Select-Object -Unique){
  if(-not $folder-or -not(Test-Path -LiteralPath $folder)){continue}
  foreach($link in @(Get-ChildItem -LiteralPath $folder -Filter '*.lnk' -Recurse -File -ErrorAction SilentlyContinue)){
   $shortcut=$shell.CreateShortcut($link.FullName)
   if($shortcut.TargetPath-eq $oldAgent){
    $match=[regex]::Match([string]$shortcut.Arguments,'--role\s+(server|client)\b',[Text.RegularExpressions.RegexOptions]::IgnoreCase)
    if($match.Success){$suffix=if($match.Groups[1].Value-eq 'server'){'Server'}else{'Client'};$shortcut.TargetPath=Join-Path $root ('USB-to-Network-Bridge-'+$suffix+'.exe');$shortcut.Arguments='';$shortcut.WorkingDirectory=$root;$shortcut.Save()}
   }elseif($shortcut.TargetPath-in @($launchers|ForEach-Object {Join-Path $root $_})){$shortcut.WorkingDirectory=$root;$shortcut.Save()}
  }
 }
 & $powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $app 'Maintain-Installation.ps1') -Operation Resume -RuntimeDirectory $app -PreviousRuntimeDirectory $root -AfterInstall
 if($LASTEXITCODE-ne 0){throw 'Files were moved, but startup tasks could not be resumed. The maintenance journal was retained.'}
 $completed=$true
 [ordered]@{success=$true;root=$root;runtime=$app;moved_entries=$moved.Count;roles=@($roles.role);backup=$backup;restart_issued=$false;updated_utc=[DateTime]::UtcNow.ToString('o')}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $app 'layout-migration.local.json') -Encoding UTF8
}catch{
 $failure=$_.Exception.Message
 # An incomplete file move is reversible before settings/package writes begin.
 if(-not $backup){
  foreach($name in @($moved)){
   $source=Assert-BridgeLayoutChild (Join-Path $app $name) $app
   $target=Assert-BridgeLayoutChild (Join-Path $root $name) $root
   if((Test-Path -LiteralPath $source)-and -not(Test-Path -LiteralPath $target)){Move-Item -LiteralPath $source -Destination $target}
  }
  if((Test-Path -LiteralPath $app)-and -not @(Get-ChildItem -LiteralPath $app -Force).Count){Remove-Item -LiteralPath $app}
  & $powershell -NoProfile -ExecutionPolicy Bypass -File $maintenance -Operation Resume -RuntimeDirectory $root
 }
 throw $failure
}finally{if(-not $completed){Write-Warning 'Migration did not finish. Original settings and the owned-task journal were retained for recovery.'}}
