# Build the small DirectShow bridge linked into the Rust camera test window.
param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$root=$PSScriptRoot
if(-not(Get-Command cl.exe -ErrorAction SilentlyContinue)){
    $vcvars=Join-Path $root 'tools\buildtools\VC\Auxiliary\Build\vcvars64.bat'
    if(-not(Test-Path -LiteralPath $vcvars)){
        $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if(-not(Test-Path -LiteralPath $vswhere)){throw 'MSVC C++ Build Tools and Windows SDK are required.'}
        $installation=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if(-not $installation){throw 'MSVC C++ Build Tools were not found.'}
        $vcvars=Join-Path $installation 'VC\Auxiliary\Build\vcvars64.bat'
    }
    $environment=& cmd.exe /d /c ('"'+$vcvars+'" >nul && set')
    if($LASTEXITCODE-ne 0){throw 'Cannot initialize C++ compiler environment.'}
    foreach($line in $environment){if($line-match '^(PATH|INCLUDE|LIB|LIBPATH)=(.*)$'){[Environment]::SetEnvironmentVariable($Matches[1],$Matches[2],'Process')}}
}
$OutputDirectory=[IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $OutputDirectory -Force|Out-Null
$object=Join-Path $OutputDirectory 'camera_native.obj'
& cl.exe /nologo /c /EHsc /O2 /MT /std:c++20 /utf-8 /W4 /WX /DWIN32_LEAN_AND_MEAN /DNOMINMAX (Join-Path $root 'src\camera_native.cpp') ('/Fo:'+$object)
if($LASTEXITCODE-ne 0){throw 'Camera bridge compilation failed.'}
& lib.exe /nologo ('/OUT:'+(Join-Path $OutputDirectory 'usbbridge_camera.lib')) $object
if($LASTEXITCODE-ne 0){throw 'Camera bridge archive failed.'}
