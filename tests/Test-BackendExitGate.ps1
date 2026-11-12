param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile((Join-Path $SourceDirectory 'Switch-Backend.ps1'),[ref]$tokens,[ref]$errors)
if($errors){throw $errors[0].Message}
foreach($name in @('Get-BridgeBackendExitState','Test-BridgeManagedBackendExit')){
 $function=$ast.Find({param($node) $node-is [Management.Automation.Language.FunctionDefinitionAst]-and $node.Name-eq $name},$true)
 if(-not $function){throw 'Missing backend hold gate function.'}
 . ([scriptblock]::Create($function.Extent.Text))
}
$root=Join-Path ([IO.Path]::GetTempPath()) ('backend-hold-'+[guid]::NewGuid().ToString('N'))
$app=Join-Path $root 'app';$config=Join-Path $app 'data\Server\host.local.json';$profile=Join-Path $app 'data\Server\server.local.psd1'
$global:BackendGateFixture=@{machine=('a'*64);held=$false;queries=0;marker=[pscustomobject]@{schema=2;role='Server';initialized=$true;machine_id=('a'*64);config=$config}}
function Get-BridgeBackendExitState {param($Cli,$Config) $global:BackendGateFixture.queries++;$global:BackendGateFixture.held}
$checks=0
function Check($Actual,$Expected,[string]$Name){if($Actual-cne $Expected){throw ('Backend exit gate failed: '+$Name)};$script:checks++}
try {
 New-Item -ItemType Directory -Path (Split-Path -Parent $config) -Force|Out-Null
 Check (Test-BridgeManagedBackendExit $app $profile) $false 'standalone installation without role identity remains usable'
 $identity=@'
function Get-BridgeMachineMarker {param($Role,$Runtime) $global:BackendGateFixture.marker}
function Get-BridgeMachineIdentity {$global:BackendGateFixture.machine}
function Read-BridgeJson {param($Path) Get-Content -LiteralPath $Path -Raw -Encoding UTF8|ConvertFrom-Json}
function Resolve-BridgeConfigPaths {param($Value,$Config,$RuntimeDirectory) $Value}
function Test-BridgeSamePath {param($First,$Second) [string]::Equals([IO.Path]::GetFullPath($First),[IO.Path]::GetFullPath($Second),[StringComparison]::OrdinalIgnoreCase)}
'@
 [IO.File]::WriteAllText((Join-Path $app 'FirstRun-Identity.ps1'),$identity)
 [IO.File]::WriteAllText((Join-Path $app 'machine-server.local.json'),'{}')
 [IO.File]::WriteAllText($config,(@{runtime=$app;profile=$profile}|ConvertTo-Json))
 Check (Test-BridgeManagedBackendExit $app $profile) $false 'no hold file adds no CLI query'
 Check $global:BackendGateFixture.queries 0 'no hold file skips subprocess'
 [IO.File]::WriteAllText([IO.Path]::ChangeExtension($config,'application-stop.local.json'),'fixture')
 Check (Test-BridgeManagedBackendExit $app (Join-Path $app 'standalone.psd1')) $false 'standalone profile remains usable alongside managed server'
 Check $global:BackendGateFixture.queries 0 'unrelated profile never reads managed hold'
 $global:BackendGateFixture.held=$true
 Check (Test-BridgeManagedBackendExit $app $profile) $true 'current boot exit prevents delayed backend restart'
 $global:BackendGateFixture.held=$false
 Check (Test-BridgeManagedBackendExit $app $profile) $false 'old boot marker permits backend startup'
 $global:BackendGateFixture.machine='b'*64
 $failed=$false;try{$null=Test-BridgeManagedBackendExit $app $profile}catch{$failed=$true}
 Check $failed $true 'foreign machine marker cannot authorize managed restart'
 [pscustomobject]@{checks=$checks;success=$true;live_processes_changed=0;registry_writes=0}|ConvertTo-Json
}finally{Remove-Variable BackendGateFixture -Scope Global -ErrorAction SilentlyContinue;if(Test-Path -LiteralPath $root){Remove-Item -LiteralPath $root -Recurse -Force}}
