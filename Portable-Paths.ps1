# Dot-source only. Serialized paths are relative to their owning configuration.
function Get-BridgeConfigDirectory([string]$Config) {
 if(-not $Config){throw 'A configuration file is required to resolve its paths.'}
 [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($Config))
}
function Test-BridgePathWithin([string]$Path,[string]$Directory) {
 $base=[IO.Path]::GetFullPath($Directory).TrimEnd('\','/')
 $full=[IO.Path]::GetFullPath($Path).TrimEnd('\','/')
 [string]::Equals($full,$base,[StringComparison]::OrdinalIgnoreCase)-or $full.StartsWith($base+'\',[StringComparison]::OrdinalIgnoreCase)
}
function Join-BridgePathToken([string]$Directory,[string]$Suffix) {
 $base=[IO.Path]::GetFullPath($Directory)
 $path=if($Suffix){[IO.Path]::GetFullPath((Join-Path $base $Suffix.Replace('/','\')))}else{$base}
 if(-not(Test-BridgePathWithin $path $base)){throw 'A portable path token must remain within its base directory.'}
 $path
}
function Resolve-BridgeConfigPath {
 param([AllowNull()][AllowEmptyString()][string]$Value,[string]$Config,[string]$RuntimeDirectory=$PSScriptRoot)
 if([string]::IsNullOrEmpty($Value)){return $Value}
 if($Value-match '^@app(?:[\\/](.*))?$'){return (Join-BridgePathToken $RuntimeDirectory $Matches[1])}
 if($Value-match '^@programdata(?:[\\/](.*))?$'){
  return (Join-BridgePathToken ([Environment]::GetFolderPath('CommonApplicationData')) $Matches[1])
 }
 if($Value.StartsWith('@')){throw 'Unknown portable configuration path token.'}
 if($Value-match '^[A-Za-z]:(?:$|[^\\/])' -or ($Value.StartsWith('\')-and -not $Value.StartsWith('\\'))-or ($Value.StartsWith('/')-and -not $Value.StartsWith('//'))){throw 'Drive-relative configuration paths are not supported.'}
 if([IO.Path]::IsPathRooted($Value)){return [IO.Path]::GetFullPath($Value)}
 [IO.Path]::GetFullPath((Join-Path (Get-BridgeConfigDirectory $Config) $Value.Replace('/','\')))
}
function Get-BridgeRelativePath([string]$Path,[string]$Directory) {
 $full=[IO.Path]::GetFullPath($Path);$base=[IO.Path]::GetFullPath($Directory)
 $root=[IO.Path]::GetPathRoot($full);$baseRoot=[IO.Path]::GetPathRoot($base)
 if(-not [string]::Equals($root,$baseRoot,[StringComparison]::OrdinalIgnoreCase)){return $null}
 $separators=[char[]]@('\','/')
 $parts=@($full.Substring($root.Length).Split($separators,[StringSplitOptions]::RemoveEmptyEntries))
 $baseParts=@($base.Substring($baseRoot.Length).Split($separators,[StringSplitOptions]::RemoveEmptyEntries))
 $common=0
 while($common-lt $parts.Count-and $common-lt $baseParts.Count-and [string]::Equals($parts[$common],$baseParts[$common],[StringComparison]::OrdinalIgnoreCase)){$common++}
 $relative=New-Object 'Collections.Generic.List[string]'
 for($index=$common;$index-lt $baseParts.Count;$index++){$relative.Add('..')}
 for($index=$common;$index-lt $parts.Count;$index++){$relative.Add($parts[$index])}
 if(-not $relative.Count){return '.'}
 $relative-join '\'
}
function ConvertTo-BridgePortablePath {
 param([AllowNull()][AllowEmptyString()][string]$Value,[string]$Config,[string]$RuntimeDirectory=$PSScriptRoot,[switch]$RuntimePath)
 if([string]::IsNullOrEmpty($Value)){return $Value}
 $full=Resolve-BridgeConfigPath $Value $Config $RuntimeDirectory
 $runtime=[IO.Path]::GetFullPath($RuntimeDirectory)
 if($RuntimePath-and [string]::Equals($full.TrimEnd('\','/'),$runtime.TrimEnd('\','/'),[StringComparison]::OrdinalIgnoreCase)){return '@app'}
 $directory=Get-BridgeConfigDirectory $Config
 # A config outside the app must follow the app when its folder moves; a
 # relative path through the former project directory would preserve its name.
 if(-not(Test-BridgePathWithin $directory $runtime)-and (Test-BridgePathWithin $full $runtime)){
  $relative=Get-BridgeRelativePath $full $runtime
  if($relative-eq '.'){return '@app'}
  return ('@app/'+$relative.Replace('\','/'))
 }
 $common=[Environment]::GetFolderPath('CommonApplicationData')
 if(-not(Test-BridgePathWithin $directory $common)-and (Test-BridgePathWithin $full $common)){
  $relative=Get-BridgeRelativePath $full $common
  if($relative-eq '.'){return '@programdata'}
  return ('@programdata/'+$relative.Replace('\','/'))
 }
 if(Test-BridgePathWithin $full $directory){return (Get-BridgeRelativePath $full $directory)}
 if(Test-BridgePathWithin $full $runtime){
  $relative=Get-BridgeRelativePath $full $runtime
  if($relative-eq '.'){return '@app'}
  return ('@app/'+$relative.Replace('\','/'))
 }
 if(Test-BridgePathWithin $full $common){
  $relative=Get-BridgeRelativePath $full $common
  if($relative-eq '.'){return '@programdata'}
  return ('@programdata/'+$relative.Replace('\','/'))
 }
 $relative=Get-BridgeRelativePath $full $directory
 if($null-ne $relative){return $relative}
 # Legacy absolute paths remain readable, but new settings must be portable.
 # Never substitute another credential or serialize a drive-specific fallback.
 throw 'A configured resource is on another volume. Copy it into the application folder before saving its settings.'
}
function Convert-BridgeConfigPathNode($Value,[string]$Config,[string]$RuntimeDirectory,[bool]$Portable) {
 if($null-eq $Value){return $null}
 if($Value-is [Collections.IDictionary]){
  $copy=[ordered]@{}
  foreach($key in $Value.Keys){
   $item=$Value[$key]
   if($key-in @('runtime','profile','key_file','credential_file')-and $item-is [string]){
    $copy[$key]=if($Portable){ConvertTo-BridgePortablePath $item $Config $RuntimeDirectory -RuntimePath:($key-eq 'runtime')}else{Resolve-BridgeConfigPath $item $Config $RuntimeDirectory}
   }else{$copy[$key]=Convert-BridgeConfigPathNode $item $Config $RuntimeDirectory $Portable}
  }
  return [pscustomobject]$copy
 }
 if($Value-is [Collections.IEnumerable]-and $Value-isnot [string]){
  $items=New-Object 'Collections.Generic.List[object]'
  foreach($item in $Value){$items.Add((Convert-BridgeConfigPathNode $item $Config $RuntimeDirectory $Portable))}
  return ,$items.ToArray()
 }
 if($Value-is [pscustomobject]){
  $copy=[ordered]@{}
  foreach($property in $Value.PSObject.Properties){
   $key=$property.Name;$item=$property.Value
   if($key-in @('runtime','profile','key_file','credential_file')-and $item-is [string]){
    $copy[$key]=if($Portable){ConvertTo-BridgePortablePath $item $Config $RuntimeDirectory -RuntimePath:($key-eq 'runtime')}else{Resolve-BridgeConfigPath $item $Config $RuntimeDirectory}
   }else{$copy[$key]=Convert-BridgeConfigPathNode $item $Config $RuntimeDirectory $Portable}
  }
  return [pscustomobject]$copy
 }
 $Value
}
function Resolve-BridgeConfigPaths {
 param($Value,[string]$Config,[string]$RuntimeDirectory=$PSScriptRoot)
 Convert-BridgeConfigPathNode $Value $Config $RuntimeDirectory $false
}
function ConvertTo-BridgePortableConfig {
 param($Value,[string]$Config,[string]$RuntimeDirectory=$PSScriptRoot)
 Convert-BridgeConfigPathNode $Value $Config $RuntimeDirectory $true
}
