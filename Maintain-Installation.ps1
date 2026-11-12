# Runs only from Windows Installer, elevated. Operates on this exact installation.
param([ValidateSet('Stop','Resume','Remove')][string]$Operation,[string]$RuntimeDirectory,[switch]$AfterInstall,[string]$PreviousRuntimeDirectory,[switch]$FunctionsOnly)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
. (Join-Path $PSScriptRoot 'Portable-Paths.ps1')
function Get-MaintenanceJournalPath([string]$Directory,[string]$Runtime){
 $normalized=[IO.Path]::GetFullPath($Runtime).TrimEnd('\')
 $hash=[Security.Cryptography.SHA256]::Create()
 try{$key=([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($normalized.ToLowerInvariant())))).Replace('-','').Substring(0,24)}finally{$hash.Dispose()}
 Join-Path $Directory ($key+'.local.json')
}
function Convert-MaintenancePath([string]$Value,[string]$PreviousRuntime,[string]$NewRuntime){
 if(-not $Value-or -not [IO.Path]::IsPathRooted($Value)){return $Value}
 $old=[IO.Path]::GetFullPath($PreviousRuntime).TrimEnd('\');$new=[IO.Path]::GetFullPath($NewRuntime).TrimEnd('\')
 $path=[IO.Path]::GetFullPath($Value)
 if($path-eq $new-or $path.StartsWith($new+'\',[StringComparison]::OrdinalIgnoreCase)){return $Value}
 if($path-eq $old){return $new}
 if($path.StartsWith($old+'\',[StringComparison]::OrdinalIgnoreCase)){return $new+$path.Substring($old.Length)}
 $Value
}
function Get-MaintenanceConfigArgument([string]$Arguments){
 $matches=[regex]::Matches($Arguments,'(?i)(?:^|\s)(?:--config|-Config)\s+(?:"(?<quoted>[^"]+)"|(?<bare>[^\s"]+))')
 if($matches.Count-gt 1){throw 'Ambiguous configuration arguments in an owned task.'}
 if(-not $matches.Count){return ''}
 $value=if($matches[0].Groups['quoted'].Success){$matches[0].Groups['quoted'].Value}else{$matches[0].Groups['bare'].Value}
 if(-not [IO.Path]::IsPathRooted($value)){throw 'Owned task configuration must use an absolute path.'}
 [IO.Path]::GetFullPath($value)
}
function Convert-MaintenanceArguments([string]$Arguments,[string]$PreviousRuntime,[string]$NewRuntime){
 $pattern='(?i)(?<prefix>(?:^|\s)(?:--config|--profile|--runtime(?:-directory)?|-Config|-Profile|-RuntimeDirectory)\s+)(?:"(?<quoted>[^"]*)"|(?<bare>[^\s"]+))'
 [regex]::Replace($Arguments,$pattern,[Text.RegularExpressions.MatchEvaluator]{param($match)
  $quoted=$match.Groups['quoted'].Success
  $value=if($quoted){$match.Groups['quoted'].Value}else{$match.Groups['bare'].Value}
  $converted=Convert-MaintenancePath $value $PreviousRuntime $NewRuntime
  if($converted-eq $value){return $match.Value}
  if($quoted-or $converted-match '\s'){$converted='"'+$converted+'"'}
  $match.Groups['prefix'].Value+$converted
 })
}
function Get-MaintenanceExec([string]$TaskXml){
 $xml=New-Object Xml.XmlDocument;$xml.XmlResolver=$null;$xml.LoadXml($TaskXml)
 $actions=@($xml.SelectNodes('/*[local-name()="Task"]/*[local-name()="Actions"]/*'))
 if($actions.Count-ne 1-or $actions[0].LocalName-ne 'Exec'){throw 'Unexpected multi-action startup task.'}
 $actions[0]
}
function Convert-MaintenanceTaskXml([string]$TaskXml,[string]$PreviousRuntime,[string]$NewRuntime){
 $exec=Get-MaintenanceExec $TaskXml
 $owned=@((Join-Path $PreviousRuntime 'usbbridge-agent.exe'),(Join-Path $PreviousRuntime 'usbbridge-camera.exe'))
 if([string]$exec.Command-notin $owned){throw 'Maintenance task XML belongs to another installation.'}
 $exec.Command=Convert-MaintenancePath ([string]$exec.Command) $PreviousRuntime $NewRuntime
 if($exec.Arguments){$exec.Arguments=Convert-MaintenanceArguments ([string]$exec.Arguments) $PreviousRuntime $NewRuntime}
 if($exec.WorkingDirectory){$exec.WorkingDirectory=Convert-MaintenancePath ([string]$exec.WorkingDirectory) $PreviousRuntime $NewRuntime}
 $exec.OwnerDocument.OuterXml
}
function Update-MaintenanceJsonNode($Node,[string]$PreviousRuntime,[string]$NewRuntime,[ref]$Changed){
 if($null-eq $Node-or $Node-is [string]){return}
 if($Node-is [Collections.IDictionary]){$properties=@($Node.Keys|ForEach-Object {[pscustomobject]@{Name=[string]$_;Value=$Node[$_]}})}
 elseif($Node-is [pscustomobject]){$properties=@($Node.PSObject.Properties)}
 elseif($Node-is [Collections.IEnumerable]){foreach($child in $Node){Update-MaintenanceJsonNode $child $PreviousRuntime $NewRuntime $Changed};return}
 else{return}
 foreach($property in $properties){
  if($property.Name-in @('runtime','runtime_directory','profile','config','key_file','credential_file')-and $property.Value-is [string]){
   $converted=Convert-MaintenancePath $property.Value $PreviousRuntime $NewRuntime
   if($converted-cne $property.Value){if($Node-is [Collections.IDictionary]){$Node[$property.Name]=$converted}else{$Node.($property.Name)=$converted};$Changed.Value=$true}
  }else{Update-MaintenanceJsonNode $property.Value $PreviousRuntime $NewRuntime $Changed}
 }
}
function Convert-MaintenanceJsonContent([string]$Content,[string]$PreviousRuntime,[string]$NewRuntime,[string]$Config){
 if($Content.Length-gt 1048576){throw 'Owned configuration exceeds the1MiB limit.'}
 if($Content.TrimStart().StartsWith('[')){$value=[object[]](ConvertFrom-Json -InputObject $Content)}else{$value=$Content|ConvertFrom-Json}
 $changed=[ref]$false;Update-MaintenanceJsonNode $value $PreviousRuntime $NewRuntime $changed
 if($Config){
  $value=ConvertTo-BridgePortableConfig (Resolve-BridgeConfigPaths $value $Config $NewRuntime) $Config $NewRuntime
  return (ConvertTo-Json -InputObject $value -Depth 32)
 }
 if($changed.Value){ConvertTo-Json -InputObject $value -Depth 32}else{$Content}
}
function Convert-MaintenanceProfileContent([string]$Content,[string]$PreviousRuntime,[string]$NewRuntime,[string]$ProfilePath){
 $tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseInput($Content,[ref]$tokens,[ref]$errors)
 if($errors.Count){throw 'Could not parse the owned server profile.'}
 $table=$ast.Find({param($node)$node-is [Management.Automation.Language.HashtableAst]},$true)
 if(-not $table){throw 'Owned server profile has no data table.'}
 $runtimeValues=@($table.KeyValuePairs|Where-Object {$_.Item1-is [Management.Automation.Language.StringConstantExpressionAst]-and $_.Item1.Value-eq 'Runtime'})
 if($runtimeValues.Count-gt 1){throw 'Ambiguous Runtime fields in the owned server profile.'}
 if(-not $runtimeValues.Count){return $Content}
 $pipeline=$runtimeValues[0].Item2
 if($pipeline-isnot [Management.Automation.Language.PipelineAst]-or $pipeline.PipelineElements.Count-ne 1){throw 'Owned server Runtime must be a literal path.'}
 $expression=$pipeline.PipelineElements[0].Expression
 if($expression-isnot [Management.Automation.Language.StringConstantExpressionAst]){throw 'Owned server Runtime must be a literal path.'}
 $converted=Convert-MaintenancePath $expression.Value $PreviousRuntime $NewRuntime
 if($ProfilePath){$converted=ConvertTo-BridgePortablePath $converted $ProfilePath $NewRuntime -RuntimePath}
 if($converted-ceq $expression.Value){return $Content}
 $Content.Substring(0,$expression.Extent.StartOffset)+"'"+$converted.Replace("'","''")+"'"+$Content.Substring($expression.Extent.EndOffset)
}
function Convert-MaintenanceJournal($Journal,[string]$PreviousRuntime,[string]$NewRuntime){
 if($Journal.runtime-ne $PreviousRuntime){throw 'Maintenance journal belongs to another installation.'}
 $copy=$Journal|ConvertTo-Json -Depth 32|ConvertFrom-Json
 foreach($entry in @($copy.tasks)){$entry.xml=Convert-MaintenanceTaskXml $entry.xml $PreviousRuntime $NewRuntime}
 $oldPrograms=@((Join-Path $PreviousRuntime 'usbbridge-agent.exe'),(Join-Path $PreviousRuntime 'usbbridge-camera.exe'),(Join-Path $PreviousRuntime 'usbbridge-server.exe'))
 foreach($entry in @($copy.firewall)){
  if($entry.program-notin $oldPrograms){throw 'Maintenance firewall snapshot belongs to another installation.'}
  $entry.program=Convert-MaintenancePath $entry.program $PreviousRuntime $NewRuntime
 }
 foreach($entry in @($copy.markers|Where-Object {$_})){
  if($entry.role-notin @('Server','Client')){throw 'Invalid maintenance marker role.'}
  $marker=$entry.content|ConvertFrom-Json
  if($marker.runtime-ne $PreviousRuntime){throw 'Maintenance marker belongs to another installation.'}
  $entry.content=Convert-MaintenanceJsonContent $entry.content $PreviousRuntime $NewRuntime
 }
 $copy.runtime=$NewRuntime
 $copy
}
function Write-MaintenanceText([string]$Path,[string]$Content){
 $temporary=$Path+'.'+[guid]::NewGuid().ToString('N')+'.tmp'
 try{[IO.File]::WriteAllText($temporary,$Content,[Text.UTF8Encoding]::new($true));Move-Item -LiteralPath $temporary -Destination $Path -Force}
 finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
function Get-MaintenanceConfigWrites($Journal,[string]$PreviousRuntime,[string]$NewRuntime){
 $paths=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
 foreach($entry in @($Journal.tasks)){
  $exec=Get-MaintenanceExec $entry.xml
  $owned=@((Join-Path $PreviousRuntime 'usbbridge-agent.exe'),(Join-Path $PreviousRuntime 'usbbridge-camera.exe'))
  if([string]$exec.Command-notin $owned){throw 'Configuration discovery refused an unowned task.'}
  $config=Get-MaintenanceConfigArgument ([string]$exec.Arguments)
  if($config){
   [void]$paths.Add($config)
   $stem=[IO.Path]::GetFileNameWithoutExtension($config)-replace '\.local$',''
   foreach($role in @('receive','send','local-send')){
    $candidate=Join-Path ([IO.Path]::GetDirectoryName($config)) ($stem+'.video-'+$role+'.local.json')
    if((Test-Path -LiteralPath (Convert-MaintenancePath $candidate $PreviousRuntime $NewRuntime))-or (Test-Path -LiteralPath $candidate)){[void]$paths.Add($candidate)}
   }
  }
 }
 $writes=New-Object 'Collections.Generic.List[object]'
 $profiles=New-Object 'Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
 foreach($path in $paths){
  $destination=Convert-MaintenancePath $path $PreviousRuntime $NewRuntime
  if(-not(Test-Path -LiteralPath $destination -PathType Leaf)){throw 'Relocate the owned configuration before resuming the new runtime.'}
  if((Get-Item -LiteralPath $destination).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Owned configuration must not be a reparse point.'}
  $content=[IO.File]::ReadAllText($destination)
  $value=(Convert-MaintenanceJsonContent $content $PreviousRuntime $NewRuntime)|ConvertFrom-Json
  $value=Resolve-BridgeConfigPaths $value $destination $NewRuntime
  if($value.PSObject.Properties['runtime']-and $value.runtime-ne $NewRuntime){throw 'Server configuration belongs to another installation.'}
  if($value.PSObject.Properties['profile']-and $value.profile){[void]$profiles.Add([string]$value.profile)}
  $converted=Convert-MaintenanceJsonContent $content $PreviousRuntime $NewRuntime $destination
  if($converted-cne $content){$writes.Add([pscustomobject]@{path=$destination;content=$converted;before=[IO.File]::ReadAllBytes($destination)})}
 }
 foreach($path in $profiles){
  $destination=Convert-MaintenancePath $path $PreviousRuntime $NewRuntime
  if(-not(Test-Path -LiteralPath $destination -PathType Leaf)){throw 'Relocate the owned server profile before resuming the new runtime.'}
  if((Get-Item -LiteralPath $destination).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Owned server profile must not be a reparse point.'}
  $content=[IO.File]::ReadAllText($destination);$converted=Convert-MaintenanceProfileContent $content $PreviousRuntime $NewRuntime $destination
  if($converted-cne $content){$writes.Add([pscustomobject]@{path=$destination;content=$converted;before=[IO.File]::ReadAllBytes($destination)})}
 }
 @($writes.ToArray())
}
function Set-MaintenanceTaskEnabledXml([string]$TaskXml,[bool]$Enabled){
 $exec=Get-MaintenanceExec $TaskXml;$xml=$exec.OwnerDocument
 $settings=$xml.SelectSingleNode('/*[local-name()="Task"]/*[local-name()="Settings"]')
 if(-not $settings){$settings=$xml.CreateElement('Settings',$xml.DocumentElement.NamespaceURI);[void]$xml.DocumentElement.AppendChild($settings)}
 $enabledNode=$settings.SelectSingleNode('*[local-name()="Enabled"]')
 if(-not $enabledNode){$enabledNode=$xml.CreateElement('Enabled',$xml.DocumentElement.NamespaceURI);[void]$settings.AppendChild($enabledNode)}
 $enabledNode.InnerText=$Enabled.ToString().ToLowerInvariant();$xml.OuterXml
}
function Test-MaintenanceTaskEnabled([string]$TaskXml){
 $exec=Get-MaintenanceExec $TaskXml
 $node=$exec.OwnerDocument.SelectSingleNode('/*[local-name()="Task"]/*[local-name()="Settings"]/*[local-name()="Enabled"]')
 (-not $node)-or $node.InnerText-ne 'false'
}
function Invoke-MaintenanceMigrationResume($Journal,[string]$PreviousRuntime,[string]$NewRuntime,[string]$DataDirectory){
 $converted=Convert-MaintenanceJournal $Journal $PreviousRuntime $NewRuntime
 $writes=New-Object 'Collections.Generic.List[object]'
 foreach($write in @(Get-MaintenanceConfigWrites $Journal $PreviousRuntime $NewRuntime)){$writes.Add($write)}
 foreach($marker in @($converted.markers|Where-Object {$_})){
  $path=Join-Path $DataDirectory ($marker.role+'\installation.local.json');$before=$null
  if(Test-Path -LiteralPath $path){
   if((Get-Item -LiteralPath $path).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Installation marker must not be a reparse point.'}
   $current=[IO.File]::ReadAllText($path)|ConvertFrom-Json
   if($current.runtime-notin @($PreviousRuntime,$NewRuntime)){throw 'A different installation now owns a role marker.'}
   $before=[IO.File]::ReadAllBytes($path)
  }
  $writes.Add([pscustomobject]@{path=$path;content=$marker.content;before=$before})
 }
 $owned=@((Join-Path $PreviousRuntime 'usbbridge-agent.exe'),(Join-Path $PreviousRuntime 'usbbridge-camera.exe'),(Join-Path $NewRuntime 'usbbridge-agent.exe'),(Join-Path $NewRuntime 'usbbridge-camera.exe'))
 $taskBackups=New-Object 'Collections.Generic.List[object]'
 foreach($entry in @($converted.tasks)){
  $existing=Get-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue
  if($existing-and (@($existing.Actions).Count-ne 1-or @($existing.Actions|Where-Object {$_.Execute-notin $owned}).Count)){throw 'A different installation now owns a startup task.'}
  $xml=if($existing){Export-ScheduledTask -TaskPath $entry.path -TaskName $entry.name}else{$null}
  $taskBackups.Add([pscustomobject]@{path=$entry.path;name=$entry.name;xml=$xml})
 }
 $firewallBackups=New-Object 'Collections.Generic.List[object]'
 for($index=0;$index-lt @($converted.firewall).Count;$index++){
  $entry=@($converted.firewall)[$index];$oldEntry=@($Journal.firewall)[$index]
  if($entry.name-notlike 'USBBridge-*'){throw 'Unexpected firewall rule name in the maintenance journal.'}
  $rule=Get-NetFirewallRule -Name $entry.name -ErrorAction SilentlyContinue
  if(@($rule).Count-gt 1){throw 'Ambiguous owned firewall rule.'}
  $program=if($rule){[string]($rule|Get-NetFirewallApplicationFilter).Program}else{$null}
  if($rule-and $program-notin @($oldEntry.program,$entry.program)){throw 'A different installation now owns a firewall rule.'}
  $firewallBackups.Add([pscustomobject]@{name=$entry.name;program=$program;existed=[bool]$rule})
 }
 # Everything above is a read-only ownership preflight. Register disabled tasks
 # first so no worker can observe partially migrated configuration or firewall.
 try{
  foreach($write in $writes){
   $parent=[IO.Path]::GetDirectoryName($write.path)
   if(-not(Test-Path -LiteralPath $parent)){New-Item -ItemType Directory -Path $parent -Force|Out-Null}
   Write-MaintenanceText $write.path $write.content
  }
  foreach($entry in @($converted.tasks)){
   $disabledXml=Set-MaintenanceTaskEnabledXml $entry.xml $false
   Register-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -Xml $disabledXml -Force|Out-Null
  }
  foreach($entry in @($converted.firewall)){
   $rule=Get-NetFirewallRule -Name $entry.name -ErrorAction SilentlyContinue
   if($rule){$rule|Get-NetFirewallApplicationFilter|Set-NetFirewallApplicationFilter -Program $entry.program|Out-Null}
   else{
    $profile=if($entry.PSObject.Properties['profile']){$entry.profile}else{'Any'}
    $enabled=if($entry.PSObject.Properties['enabled']){$entry.enabled}else{'True'}
    New-NetFirewallRule -Name $entry.name -DisplayName $entry.display -Direction Inbound -Action Allow -Program $entry.program -Protocol $entry.protocol -LocalPort $entry.port -RemoteAddress $entry.remote -Profile $profile -Enabled $enabled|Out-Null
   }
  }
  foreach($entry in @($converted.tasks)){
   $wasEnabled=Test-MaintenanceTaskEnabled $entry.xml
   if($wasEnabled){Enable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}else{Disable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}
   if($entry.running-and $wasEnabled){Start-ScheduledTask -TaskPath $entry.path -TaskName $entry.name}
  }
 }catch{
  $failure=$_
  # Keep the original journal unchanged for the old helper's rollback Resume.
  foreach($entry in @($converted.tasks)){try{Disable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue|Out-Null;Stop-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue}catch{}}
  foreach($backup in $taskBackups){try{if($backup.xml){Register-ScheduledTask -TaskPath $backup.path -TaskName $backup.name -Xml (Set-MaintenanceTaskEnabledXml $backup.xml $false) -Force|Out-Null}else{Unregister-ScheduledTask -TaskPath $backup.path -TaskName $backup.name -Confirm:$false -ErrorAction SilentlyContinue}}catch{}}
  foreach($backup in $firewallBackups){try{if($backup.existed){Get-NetFirewallRule -Name $backup.name|Get-NetFirewallApplicationFilter|Set-NetFirewallApplicationFilter -Program $backup.program|Out-Null}else{Remove-NetFirewallRule -Name $backup.name -ErrorAction SilentlyContinue}}catch{}}
  foreach($write in $writes){try{if($null-ne $write.before){[IO.File]::WriteAllBytes($write.path,$write.before)}elseif(Test-Path -LiteralPath $write.path){Remove-Item -LiteralPath $write.path -Force}}catch{}}
  throw $failure
 }
}
function Get-MaintenanceRoleConfig($Tasks,[string]$Role,[string]$Fallback,[string]$OwnedAgent){
 $roleTasks=@($Tasks|Where-Object TaskName -eq ('USBBridge.'+$Role))
 if($roleTasks.Count-gt 1){throw 'Ambiguous owned role task.'}
 if(-not $roleTasks.Count){return ''}
 $task=$roleTasks[0]
 if(@($task.Actions).Count-ne 1-or $task.Actions.Execute-ne $OwnedAgent){throw 'Role configuration discovery refused an unowned task.'}
 $config=Get-MaintenanceConfigArgument ([string]$task.Actions.Arguments)
 if($config){$config}else{$Fallback}
}
function Update-MaintenanceDeploymentManifest([string]$Runtime){
 # Windows Installer has replaced the public payload. Refresh only a verified
 # existing deployment's OS registration; never reset or rewrite role settings.
 if(-not(Test-Path -LiteralPath (Join-Path $Runtime 'deployment.local.json'))){return}
 . (Join-Path $Runtime 'FirstRun-Identity.ps1')
 $binding=Get-BridgeDeploymentBinding $Runtime
 if(-not $binding-or -not(Test-BridgeSamePath $binding.RuntimeDirectory $Runtime)){return}
 $agent=Join-Path $Runtime 'usbbridge-agent.exe'
 $owned=@(Get-ScheduledTask -TaskPath '\USBBridge\' -ErrorAction SilentlyContinue|Where-Object {@($_.Actions|Where-Object Execute -eq $agent).Count})
 $roles=@{}
 foreach($role in @('Server','Client')){
  $config=Get-MaintenanceRoleConfig $owned $role '' $agent
  if(-not $config){continue}
  $marker=Get-BridgeMachineMarker -Role $role -Runtime $Runtime
  if($marker-and $marker.schema-eq 2-and $marker.initialized-and $marker.machine_id-eq (Get-BridgeMachineIdentity)-and $marker.deployment_id-eq $binding.DeploymentId-and (Test-BridgeSamePath $marker.config $config)){$roles[$role]=$config}
 }
 if($roles.Count){$null=Register-BridgeDeploymentBinding -Runtime $Runtime -InstallRoot (Split-Path -Parent $Runtime) -RoleConfigs $roles}
}
if($FunctionsOnly){return}
if(-not $Operation-or -not $RuntimeDirectory){throw 'Operation and RuntimeDirectory are required for installer maintenance.'}
$runtime=[IO.Path]::GetFullPath($RuntimeDirectory).TrimEnd('\')
$agent=Join-Path $runtime 'usbbridge-agent.exe'
$camera=Join-Path $runtime 'usbbridge-camera.exe'
$ownedPrograms=@($agent,$camera)
if(-not (Test-Path -LiteralPath (Join-Path $runtime 'Maintain-Installation.ps1'))){exit 0}
if((Get-Item -LiteralPath $runtime).Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'Refusing a reparse-point installation.'}
$data=Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge'
$journalDir=Join-Path $data 'Maintenance'
New-Item -ItemType Directory -Path $journalDir -Force|Out-Null
$acl=New-Object Security.AccessControl.DirectorySecurity;$acl.SetAccessRuleProtection($true,$false)
foreach($id in @('S-1-5-18','S-1-5-32-544')){$sid=New-Object Security.Principal.SecurityIdentifier($id);$acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))}
Set-Acl -LiteralPath $journalDir -AclObject $acl
$journalPath=Get-MaintenanceJournalPath $journalDir $runtime
function Save-Journal($Value){$tmp=$journalPath+'.tmp';$Value|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $tmp -Encoding utf8;Move-Item -LiteralPath $tmp -Destination $journalPath -Force}
if($Operation-eq 'Resume'){
 $migrationSource=$null
 if(-not(Test-Path -LiteralPath $journalPath)){
  if(-not $PreviousRuntimeDirectory){exit 0}
  $previousRuntime=[IO.Path]::GetFullPath($PreviousRuntimeDirectory).TrimEnd('\')
  if($previousRuntime-eq $runtime){exit 0}
  if($runtime-ne (Join-Path $previousRuntime 'app')){throw 'PreviousRuntimeDirectory must be the exact parent of the app runtime.'}
  $previousJournalPath=Get-MaintenanceJournalPath $journalDir $previousRuntime
  if(-not(Test-Path -LiteralPath $previousJournalPath)){exit 0}
  $journalPath=$previousJournalPath;$migrationSource=$previousRuntime
 }
 $journal=Get-Content -LiteralPath $journalPath -Raw -Encoding UTF8|ConvertFrom-Json
 $expectedRuntime=if($migrationSource){$migrationSource}else{$runtime}
 if($journal.runtime-ne $expectedRuntime){throw 'Maintenance journal belongs to another installation.'}
 # A later fresh installation must not resurrect roles explicitly removed earlier.
 if($AfterInstall-and $journal.removal_requested){Remove-Item -LiteralPath $journalPath -Force;exit 0}
 if($migrationSource){
  Invoke-MaintenanceMigrationResume $journal $migrationSource $runtime $data
  Remove-Item -LiteralPath $journalPath -Force
  exit 0
 }
 foreach($marker in @($journal.markers|Where-Object {$_})){
  $path=Join-Path $data ($marker.role+'\installation.local.json')
  if($marker.role-notin @('Server','Client')){throw 'Invalid maintenance marker role.'}
  [IO.File]::WriteAllText($path,$marker.content,[Text.UTF8Encoding]::new($true))
 }
 foreach($entry in @($journal.tasks)){
  $task=Get-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -ErrorAction SilentlyContinue
  if(-not $task){Register-ScheduledTask -TaskPath $entry.path -TaskName $entry.name -Xml $entry.xml|Out-Null}
  elseif(@($task.Actions).Count-ne 1-or @($task.Actions|Where-Object {$_.Execute-notin $ownedPrograms}).Count){throw 'A different installation now owns a startup task.'}
  $xml=[xml]$entry.xml
  $wasEnabled=([string]$xml.Task.Settings.Enabled)-ne 'false'
  if($wasEnabled){Enable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}else{Disable-ScheduledTask -TaskPath $entry.path -TaskName $entry.name|Out-Null}
  if($entry.running-and $wasEnabled){Start-ScheduledTask -TaskPath $entry.path -TaskName $entry.name}
 }
 foreach($entry in @($journal.firewall)){
  if(-not(Get-NetFirewallRule -Name $entry.name -ErrorAction SilentlyContinue)){New-NetFirewallRule -Name $entry.name -DisplayName $entry.display -Direction Inbound -Action Allow -Program $entry.program -Protocol $entry.protocol -LocalPort $entry.port -RemoteAddress $entry.remote -Profile Any|Out-Null}
 }
 if($AfterInstall){Update-MaintenanceDeploymentManifest $runtime}
 Remove-Item -LiteralPath $journalPath -Force
 exit 0
}
$tasks=@(Get-ScheduledTask -ErrorAction Stop|Where-Object {@($_.Actions|Where-Object {$_.Execute-in $ownedPrograms}).Count})
if($Operation-eq 'Stop'-and (Test-Path -LiteralPath $journalPath)){
 $previous=Get-Content -LiteralPath $journalPath -Raw -Encoding UTF8|ConvertFrom-Json
 if($previous.runtime-ne $runtime){throw 'Maintenance journal belongs to another installation.'}
 if($previous.removal_requested){Remove-Item -LiteralPath $journalPath -Force}
}
if($Operation-eq 'Stop'-and -not(Test-Path -LiteralPath $journalPath)){
 $journal=[ordered]@{runtime=$runtime;tasks=@();firewall=@();markers=@();removal_requested=$false}
 foreach($task in $tasks){if(@($task.Actions).Count-ne 1){throw 'Unexpected multi-action startup task.'};$journal.tasks+=@([ordered]@{path=$task.TaskPath;name=$task.TaskName;running=($task.State-eq 'Running');xml=(Export-ScheduledTask -TaskPath $task.TaskPath -TaskName $task.TaskName)})}
 foreach($rule in @(Get-NetFirewallRule -Name 'USBBridge-*' -ErrorAction SilentlyContinue)){
  $app=$rule|Get-NetFirewallApplicationFilter
  if($app.Program-in @($agent,$camera,(Join-Path $runtime 'usbbridge-server.exe'))){$port=$rule|Get-NetFirewallPortFilter;$address=$rule|Get-NetFirewallAddressFilter;$journal.firewall+=@([ordered]@{name=$rule.Name;display=$rule.DisplayName;program=$app.Program;protocol=$port.Protocol;port=$port.LocalPort;remote=$address.RemoteAddress;profile=[string]$rule.Profile;enabled=[string]$rule.Enabled})}
 }
 foreach($role in @('Server','Client')){
  if(@($tasks|Where-Object TaskName -eq ('USBBridge.'+$role)).Count){
   $marker=Join-Path $data ($role+'\installation.local.json')
   if(Test-Path -LiteralPath $marker){$journal.markers+=@([ordered]@{role=$role;content=[IO.File]::ReadAllText($marker)})}
  }
 }
 Save-Journal $journal
}
foreach($task in $tasks){$task|Disable-ScheduledTask|Out-Null}
foreach($task in @($tasks|Where-Object {$_.Actions.Execute-eq $camera-and $_.Actions.Arguments-match '^receive --config "(.+)"$'})){
 $videoConfig=[regex]::Match($task.Actions.Arguments,'^receive --config "(.+)"$').Groups[1].Value
 if(Test-Path -LiteralPath $videoConfig){$video=Get-Content -LiteralPath $videoConfig -Raw -Encoding UTF8|ConvertFrom-Json;& $camera detach-local ('127.0.0.1:'+$video.usb_port)|Out-Null;if($LASTEXITCODE-ne 0){throw "Could not release this installation's virtual USB camera."}}
}
foreach($task in $tasks){$task|Stop-ScheduledTask -ErrorAction SilentlyContinue}
foreach($item in @(Get-CimInstance Win32_Process -Filter "Name='usbbridge-agent.exe' OR Name='usbbridge-camera.exe'"|Where-Object {$_.ExecutablePath-in $ownedPrograms})){
 $p=Get-Process -Id $item.ProcessId -ErrorAction SilentlyContinue;if($p){Stop-Process -Id $p.Id -Force;$p.WaitForExit()}
}
# Pause this installation's remembered imports while preserving its original choices.
$clientConfig=Get-MaintenanceRoleConfig $tasks 'Client' (Join-Path $data 'Client\client.local.json') $agent
if($clientConfig-and (Test-Path -LiteralPath $clientConfig)){
 $original=[IO.File]::ReadAllBytes($clientConfig)
 try{
  $client=Get-Content -LiteralPath $clientConfig -Raw -Encoding UTF8|ConvertFrom-Json
  $servers=@($client)+@($client.servers)
  foreach($entry in $servers){if($entry.server){$selector=if($entry.server_id){$entry.server_id}else{$entry.server};& (Join-Path $runtime 'usbbridge.exe') --config $clientConfig client pause $selector|Out-Null;if($LASTEXITCODE-ne 0){throw 'Could not detach an import owned by this installation.'}}}
 }finally{[IO.File]::WriteAllBytes($clientConfig,$original)}
}
$serverConfig=Get-MaintenanceRoleConfig $tasks 'Server' (Join-Path $data 'Server\host.local.json') $agent
if($serverConfig-and (Test-Path -LiteralPath $serverConfig)){
 $hostData=Get-Content -LiteralPath $serverConfig -Raw -Encoding UTF8|ConvertFrom-Json
 $hostData=Resolve-BridgeConfigPaths $hostData $serverConfig $runtime
 if($hostData.runtime-ne $runtime){throw 'Server configuration belongs to another installation.'}
 $state=Get-Content -LiteralPath (Join-Path $runtime 'backend-state.json') -Raw -Encoding UTF8|ConvertFrom-Json
 & (Join-Path $runtime 'Switch-Backend.ps1') -Backend $state.DesiredBackend -RuntimeDirectory $runtime -Profile $hostData.profile -Stop
 if(-not $?){throw 'Could not stop the owned USB transport.'}
}
if($Operation-eq 'Remove'){
 if(Test-Path -LiteralPath $journalPath){
  $journal=Get-Content -LiteralPath $journalPath -Raw -Encoding UTF8|ConvertFrom-Json
  if($journal.runtime-ne $runtime){throw 'Maintenance journal belongs to another installation.'}
  $journal|Add-Member -NotePropertyName removal_requested -NotePropertyValue $true -Force
  Save-Journal $journal
 }
 foreach($task in $tasks){
  $task|Unregister-ScheduledTask -Confirm:$false
  if($task.TaskName-in @('USBBridge.Server','USBBridge.Client')){
   $role=$task.TaskName.Split('.')[-1]
   $marker=Join-Path $data ($role+'\installation.local.json')
   if(Test-Path -LiteralPath $marker){[ordered]@{schema=1;runtime=$runtime;initialized=$false}|ConvertTo-Json|Set-Content -LiteralPath $marker -Encoding utf8}
  }
 }
 foreach($rule in @(Get-NetFirewallRule -Name 'USBBridge-*' -ErrorAction SilentlyContinue)){
  if(($rule|Get-NetFirewallApplicationFilter).Program-in @($agent,$camera,(Join-Path $runtime 'usbbridge-server.exe'))){$rule|Remove-NetFirewallRule}
 }
}
