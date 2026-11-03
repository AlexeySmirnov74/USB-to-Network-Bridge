$ErrorActionPreference='Stop'
$root=$PSScriptRoot
$vcvars=Join-Path $root 'tools\buildtools\VC\Auxiliary\Build\vcvars64.bat'
if(-not(Get-Command cl.exe -ErrorAction SilentlyContinue)){
    if(-not(Test-Path -LiteralPath $vcvars)){
        $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if(-not(Test-Path -LiteralPath $vswhere)){throw 'MSVC Build Tools and Windows SDK are required.'}
        $installation=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if(-not $installation){throw 'C++ Build Tools were not found.'}
        $vcvars=Join-Path $installation 'VC\Auxiliary\Build\vcvars64.bat'
    }
    $environment=& cmd.exe /d /c ('"'+$vcvars+'" >nul && set')
    if($LASTEXITCODE-ne 0){throw 'Cannot initialize C++ compiler environment.'}
    foreach($line in $environment){if($line-match '^(PATH|INCLUDE|LIB|LIBPATH)=(.*)$'){[Environment]::SetEnvironmentVariable($Matches[1],$Matches[2],'Process')}}
}
New-Item -ItemType Directory -Path (Join-Path $root 'production'),(Join-Path $root 'tools') -Force|Out-Null
& cl.exe /nologo /EHsc /O2 /MT /std:c++20 (Join-Path $root 'tests\camera_probe.cpp') ('/Fe:'+(Join-Path $root 'production\camera-probe.exe')) ('/Fo:'+(Join-Path $root 'tools\camera-probe.obj')) /link mf.lib mfplat.lib mfreadwrite.lib mfuuid.lib ole32.lib
if($LASTEXITCODE-ne 0){throw 'Camera probe build failed.'}
& cl.exe /nologo /EHsc /O2 /MT /std:c++20 (Join-Path $root 'tests\camera_dshow.cpp') ('/Fe:'+(Join-Path $root 'production\camera-dshow.exe')) ('/Fo:'+(Join-Path $root 'tools\camera-dshow.obj')) /link strmiids.lib ole32.lib oleaut32.lib
if($LASTEXITCODE-ne 0){throw 'DirectShow camera probe build failed.'}
