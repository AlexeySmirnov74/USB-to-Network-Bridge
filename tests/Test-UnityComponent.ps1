# The default probe never registers a component. Persistent registration is an
# explicit isolated-CI operation, followed by unregistration of only our DLL.
param(
 [string]$RuntimeDirectory=(Split-Path -Parent $PSScriptRoot),
 [string]$ControlExe,
 [switch]$AllowMachineRegistration
)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
if(-not [Environment]::Is64BitProcess){throw 'Use 64-bit Windows PowerShell.'}
$runtime=[IO.Path]::GetFullPath($RuntimeDirectory)
. (Join-Path $runtime 'Ensure-VideoPrerequisites.ps1') -FunctionsOnly
$manifest=Get-BridgeUnityManifest $runtime
$source=Join-Path $runtime ([string]$manifest.file).Replace('/','\')
$source=Assert-BridgeUnityPinnedDll $source $manifest

if(-not ('BridgeUnityComponentProbe' -as [type])){
 Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class BridgeUnityComponentProbe {
 [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
 private static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
 [DllImport("kernel32.dll", CharSet=CharSet.Ansi, ExactSpelling=true)]
 private static extern IntPtr GetProcAddress(IntPtr module, string name);
 [DllImport("kernel32.dll", SetLastError=true)]
 private static extern bool FreeLibrary(IntPtr module);
 public static int Check(string path) {
  // Absolute pinned DLL; search only Windows System32 for its dependencies.
  IntPtr module=LoadLibraryExW(path, IntPtr.Zero, 0x800);
  if(module==IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
  try {
   string[] exports={"DllRegisterServer","DllUnregisterServer","DllInstall","DllGetClassObject","DllCanUnloadNow"};
   foreach(string name in exports)
    if(GetProcAddress(module,name)==IntPtr.Zero) throw new InvalidOperationException("Missing export: "+name);
   return exports.Length;
  } finally { FreeLibrary(module); }
 }
}
'@
}
$lock=[IO.File]::Open($source,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
try{$exports=[BridgeUnityComponentProbe]::Check($source)}finally{$lock.Dispose()}
if(-not $AllowMachineRegistration){
 [ordered]@{pe='x64';sha256=[string]$manifest.sha256;exports=$exports;machine_registration=$false}|ConvertTo-Json -Compress
 exit 0
}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){throw 'The isolated registration fixture requires an already elevated runner; it does not request UAC.'}
function Get-FixtureUnityEntries {
 $entries=@();$prefix='^\{5c2cd55c-92ad-4999-8666-912bd3e7'
 foreach($view in @([Microsoft.Win32.RegistryView]::Registry64,[Microsoft.Win32.RegistryView]::Registry32)){
  foreach($hive in @([Microsoft.Win32.RegistryHive]::LocalMachine,[Microsoft.Win32.RegistryHive]::CurrentUser)){
   $base=[Microsoft.Win32.RegistryKey]::OpenBaseKey($hive,$view)
   try{
    $classes=$base.OpenSubKey('SOFTWARE\Classes\CLSID',$false)
    if(-not $classes){continue}
    try{
     foreach($name in $classes.GetSubKeyNames()){
      if($name-notmatch $prefix){continue}
      $key=$classes.OpenSubKey($name,$false);if(-not $key){continue}
      try{
       $server=$key.OpenSubKey('InprocServer32',$false)
       try{$path=if($server){[string]$server.GetValue('','')}else{''}}finally{if($server){$server.Dispose()}}
       $entries+=[pscustomobject]@{Hive=$hive.ToString();View=$view.ToString();Kind='Class';Guid=$name;Name=[string]$key.GetValue('','');Path=$path;Clsid=$name}
      }finally{$key.Dispose()}
     }
     $category=$classes.OpenSubKey('{860BB310-5D01-11d0-BD3B-00A0C911CE86}\Instance',$false)
     if($category){
      try{
       foreach($name in $category.GetSubKeyNames()){
        $key=$category.OpenSubKey($name,$false);if(-not $key){continue}
        try{
         $friendly=[string]$key.GetValue('FriendlyName','');$clsid=[string]$key.GetValue('CLSID','')
         if($name-match $prefix-or $clsid-match $prefix-or $friendly-match 'Unity(?: Video)? Capture'){
          $entries+=[pscustomobject]@{Hive=$hive.ToString();View=$view.ToString();Kind='Instance';Guid=$name;Name=$friendly;Path='';Clsid=$clsid}
         }
        }finally{$key.Dispose()}
       }
      }finally{$category.Dispose()}
     }
    }finally{$classes.Dispose()}
   }finally{$base.Dispose()}
  }
 }
 $entries
}
if(@(Get-FixtureUnityEntries).Count){throw 'Refusing registration: this runner already has Unity Capture registrations.'}
$stableRoot=Get-BridgeUnityStableRoot
$common=Split-Path -Parent (Split-Path -Parent $stableRoot)
if(Test-Path -LiteralPath $common){throw 'Refusing registration: this fixture requires a fresh isolated project ProgramData directory.'}
if(-not $ControlExe){
 $ControlExe=Join-Path $runtime 'usbbridge.exe'
 if(-not(Test-Path -LiteralPath $ControlExe -PathType Leaf)){$ControlExe=Join-Path $runtime 'target\release\usbbridge.exe'}
}
$control=[IO.Path]::GetFullPath($ControlExe)
if(-not(Test-Path -LiteralPath $control -PathType Leaf)){throw 'Build usbbridge.exe before the registration fixture.'}
Assert-BridgeUnityPlainPath $control
$expected=Join-Path (Join-Path $stableRoot ([string]$manifest.sha256).ToLowerInvariant()) 'UnityCaptureFilter64.dll'
$temporary=Join-Path ([IO.Path]::GetTempPath()) ('bridge-unity-fixture-'+[guid]::NewGuid().ToString('N'))
$attempted=$false;$cleaned=$false;$cameras=@()
try{
 $attempted=$true
 $result=Ensure-BridgeVideoPrerequisites -Role Client -RuntimeDirectory $runtime
 if(-not $result.Ready-or -not $result.Changed-or -not(Test-BridgeUnitySamePath $result.Filter $expected)){throw 'Fresh setup did not produce the expected owned component.'}
 $null=Assert-BridgeUnityPinnedDll $expected $manifest
 New-Item -ItemType Directory -Path $temporary|Out-Null
 $config=Join-Path $temporary 'client.local.json'
 [IO.File]::WriteAllText($config,'{}',[Text.UTF8Encoding]::new($false))
 $output=@(& $control --config $config client cameras)
 if($LASTEXITCODE-ne 0){throw ('DirectShow enumeration failed: '+$LASTEXITCODE)}
 $cameras=@(($output-join [Environment]::NewLine)|ConvertFrom-Json)
 $selected=@()
 foreach($slot in @(Get-BridgeUnitySlots)){
  $matching=@($cameras|Where-Object {$_.name-ceq $slot.Name})
  if($matching.Count-ne 1-or -not [string]$matching[0].path-or [string]$matching[0].path-match 'foo:bar'){throw ('Missing or ambiguous DirectShow source: '+$slot.Name)}
  $selected+=$matching[0]
 }
 if([string]::Equals([string]$selected[0].path,[string]$selected[1].path,[StringComparison]::OrdinalIgnoreCase)){throw 'The two Unity sources have the same moniker.'}
}finally{
 if($attempted){
  $entries=@(Get-FixtureUnityEntries);$slots=@(Get-BridgeUnitySlots)
  # The pinned upstream DLL also registers its shared x64 property-page class.
  $ownedClasses=@($slots.Guid)+@('{5C2CD55C-92AD-4999-8666-912BD3E70011}')
  foreach($entry in $entries){
   $owned=if($entry.Kind-eq 'Class'){$entry.Guid-in $ownedClasses-and (Test-BridgeUnitySamePath $entry.Path $expected)}else{$entry.Guid-in $slots.Guid-and $entry.Clsid-eq $entry.Guid}
   if($entry.Hive-ne 'LocalMachine'-or $entry.View-ne 'Registry64'-or -not $owned){throw 'Cleanup refused: a Unity registration no longer belongs to this isolated fixture.'}
  }
  if($entries.Count){
   $null=Assert-BridgeUnityPinnedDll $expected $manifest
   $lock=[IO.File]::Open($expected,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
   try{
    $operation=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\regsvr32.exe') -ArgumentList @('/u','/s',('"'+$expected+'"')) -WindowStyle Hidden -Wait -PassThru
    if($operation.ExitCode-ne 0){throw ('Owned unregistration failed: '+$operation.ExitCode)}
   }finally{$lock.Dispose()}
  }
  if(@(Get-FixtureUnityEntries).Count){throw 'Owned Unity registrations remain after unregistration.'}
  $cleaned=$true
  # Never recurse through the shared project directory. Remove only our pinned
  # file, then directories created by this fresh fixture if they remain empty.
  if(Test-Path -LiteralPath $expected){$null=Assert-BridgeUnityPinnedDll $expected $manifest;Remove-Item -LiteralPath $expected -Force}
  foreach($directory in @((Split-Path -Parent $expected),$stableRoot,(Split-Path -Parent $stableRoot),$common)){
   if(Test-Path -LiteralPath $directory){Assert-BridgeUnityPlainPath $directory;if(@(Get-ChildItem -LiteralPath $directory -Force).Count-eq 0){Remove-Item -LiteralPath $directory -Force}}
  }
 }
 if(Test-Path -LiteralPath $temporary){
  $resolved=[IO.Path]::GetFullPath($temporary)
  $tempRoot=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
  if(-not $resolved.StartsWith($tempRoot,[StringComparison]::OrdinalIgnoreCase)-or [IO.Path]::GetFileName($resolved)-notlike 'bridge-unity-fixture-*'){throw 'Unsafe fixture cleanup path.'}
  Assert-BridgeUnityPlainPath $resolved
  Remove-Item -LiteralPath $resolved -Recurse -Force
 }
}
[ordered]@{pe='x64';sha256=[string]$manifest.sha256;exports=$exports;machine_registration=$true;sources=@($selected|ForEach-Object {$_.name});unique_monikers=$true;unregistered=$cleaned}|ConvertTo-Json -Compress
