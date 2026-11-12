param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot),[string]$MaintenanceFile=(Join-Path $SourceDirectory 'Maintain-Installation.ps1'))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Relocate-Installation.ps1') -FunctionsOnly
# Extract only the pure maintenance functions, so an older fixture helper can
# be tested without executing its installer entry point or scheduling tasks.
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($MaintenanceFile,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Maintenance helper has syntax errors.'}
$pure=@('Convert-MaintenancePath','Get-MaintenanceConfigArgument','Convert-MaintenanceArguments','Get-MaintenanceExec','Convert-MaintenanceTaskXml','Set-MaintenanceTaskEnabledXml','Test-MaintenanceTaskEnabled')
foreach($name in $pure){
 $definition=$ast.Find({param($node)$node-is [Management.Automation.Language.FunctionDefinitionAst]-and $node.Name-eq $name},$true)
 if(-not $definition){throw ('Missing pure maintenance function: '+$name)}
 . ([scriptblock]::Create($definition.Extent.Text))
}
$checks=0
function Assert-Equal($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Relocation assertion failed: '+$Name+'; actual='+$Actual)};$script:checks++}
function Assert-Throws([scriptblock]$Action,[string]$Name){$caught=$false;try{& $Action|Out-Null}catch{$caught=$true};if(-not $caught){throw ('Relocation accepted an unsafe fixture: '+$Name)};$script:checks++}
$old='C:\Project A\app';$new='D:\Relocated & Bridge\app'
$binding=[pscustomobject]@{ServerConfig='@programdata/USB to Network Bridge/Server/host.local.json';ClientConfig='data/Client/client.local.json'}
$allowed=Get-RelocationAllowedConfigs $binding $old
Assert-Equal ($allowed.GetType().Name) 'HashSet`1' 'allowed configuration set is not unwrapped'
Assert-Equal (Resolve-RelocationBindingConfig '@app/data/Client/client.local.json' $old) 'C:\Project A\app\data\Client\client.local.json' 'app locator'
Assert-Equal (Resolve-RelocationBindingConfig 'data\Client\client.local.json' $old) 'C:\Project A\app\data\Client\client.local.json' 'relative role locator'
Assert-Equal (Resolve-RelocationBindingConfig $binding.ServerConfig $old) (Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'USB to Network Bridge\Server\host.local.json') 'programdata base is not duplicated'
Assert-Throws {Resolve-RelocationBindingConfig '@app/../outside.json' $old} 'app traversal'
Assert-Throws {Resolve-RelocationBindingConfig '@programdata/../outside.json' $old} 'programdata traversal'
Assert-Throws {Resolve-RelocationBindingConfig '@unknown/private.key' $old} 'unknown locator'
$external=[pscustomobject]@{ClientConfig='@external';ClientExternalConfig='E:\Selected Config\client.local.json'}
Assert-Equal (Get-RelocationRoleConfig $external 'Client' $old) 'E:\Selected Config\client.local.json' 'external locator remains in protected registry'
Assert-Throws {Get-RelocationRoleConfig ([pscustomobject]@{ClientConfig='@external';ClientExternalConfig='relative.json'}) 'Client' $old} 'external registration requires an absolute path'
$agentXml=@'
<Task xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><Principals><Principal id="Author"><UserId>S-1-5-18</UserId><LogonType>ServiceAccount</LogonType></Principal></Principals><Settings><Enabled>false</Enabled></Settings><Actions Context="Author"><Exec><Command>C:\Project A\app\usbbridge-agent.exe</Command><Arguments>--supervise --client --config "C:\Project A\app\data\Client\client.local.json"</Arguments><WorkingDirectory>C:\Project A\app</WorkingDirectory></Exec></Actions></Task>
'@
Assert-Equal (Assert-RelocationOwnedTask $agentXml $old $allowed) 'C:\Project A\app\data\Client\client.local.json' 'owned role task'
$rebased=Convert-MaintenanceTaskXml $agentXml $old $new
$exec=Get-MaintenanceExec $rebased
Assert-Equal ([string]$exec.Command) 'D:\Relocated & Bridge\app\usbbridge-agent.exe' 'XML escaped executable'
Assert-Equal (Get-MaintenanceConfigArgument ([string]$exec.Arguments)) 'D:\Relocated & Bridge\app\data\Client\client.local.json' 'configuration argument rebase'
Assert-Equal ([string]$exec.WorkingDirectory) $new 'working directory rebase'
Assert-Equal (Test-MaintenanceTaskEnabled $rebased) $false 'disabled tasks remain disabled'
Assert-Equal ([string]$exec.OwnerDocument.Task.Principals.Principal.UserId) 'S-1-5-18' 'principal remains unchanged'
Assert-Throws {Assert-RelocationOwnedTask ($agentXml.Replace('usbbridge-agent.exe','other.exe')) $old $allowed} 'different executable'
Assert-Throws {Assert-RelocationOwnedTask ($agentXml.Replace('data\Client\client.local.json','other.local.json')) $old $allowed} 'different configuration'
Assert-Throws {Assert-RelocationOwnedTask ($agentXml.Replace('--supervise --client','--unrecognized')) $old $allowed} 'unrecognized role'
Assert-Throws {Assert-RelocationOwnedTask ($agentXml.Replace('</Arguments>',' --config "C:\Project A\app\data\Client\client.local.json"</Arguments>')) $old $allowed} 'duplicate configuration arguments'
Assert-Throws {Assert-RelocationOwnedTask ($agentXml.Replace('</Actions>','<Exec><Command>cmd.exe</Command></Exec></Actions>')) $old $allowed} 'multi-action task'
$videoXml=$agentXml.Replace('usbbridge-agent.exe','usbbridge-camera.exe').Replace('--supervise --client','receive').Replace('client.local.json','client.video-receive.local.json')
Assert-Equal (Assert-RelocationOwnedTask $videoXml $old $allowed) 'C:\Project A\app\data\Client\client.video-receive.local.json' 'owned receiver task'
Assert-Throws {Assert-RelocationOwnedTask ($videoXml.Replace('receive --config','detach-local --config')) $old $allowed} 'unrecognized camera action'
$deployment='00112233-4455-6677-8899-aabbccddeeff';$machine='a'*64
$ownedMarker=[pscustomobject]@{schema=2;machine_id=$machine;deployment_id=$deployment;role='Client';initialized=$true}
Assert-Equal (Test-RelocationRoleMarker $ownedMarker 'Client' $machine $deployment $binding) $true 'same-machine bound role is owned'
$foreignMarker=[pscustomobject]@{schema=1;machine_id=('b'*64);role='Server';initialized=$true}
Assert-Equal (Test-RelocationRoleMarker $foreignMarker 'Server' $machine $deployment $binding) $false 'foreign other-role marker is ignored'
Assert-Equal (Test-RelocationRoleMarker $ownedMarker 'Client' $machine $deployment ([pscustomobject]@{ServerConfig='@app/host.local.json'})) $false 'role absent from protected binding is ignored'
Assert-Throws {Test-RelocationRoleMarker $ownedMarker 'Client' $machine ([guid]::NewGuid().ToString('D')) $binding} 'same-machine bound marker with different deployment'
$journal=[pscustomobject]@{schema=1;deployment_id=$deployment;machine_id=$machine;old_root='C:\Project A';old_runtime=$old;new_root='D:\Relocated & Bridge';new_runtime=$new;tasks=@([pscustomobject]@{path='\USBBridge\';name='USBBridge.Client';xml=$agentXml});firewall=@()}
Assert-RelocationRecovery $journal $deployment $machine 'C:\Project A' $old 'D:\Relocated & Bridge' $new $allowed;$checks++
Assert-Throws {Assert-RelocationRecovery $journal $deployment ('b'*64) 'C:\Project A' $old 'D:\Relocated & Bridge' $new $allowed} 'different recovery machine'
Assert-Throws {Assert-RelocationRecovery $journal $deployment $machine 'C:\Project A' $old 'D:\Other' $new $allowed} 'different recovery destination'
$journal.tasks[0].path='\Unrelated\'
Assert-Throws {Assert-RelocationRecovery $journal $deployment $machine 'C:\Project A' $old 'D:\Relocated & Bridge' $new $allowed} 'different recovery task folder'
Write-Output ('Relocation focused fixtures passed: '+$checks)
