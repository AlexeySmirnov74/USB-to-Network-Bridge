# Optional local toolchain setup. No drivers are installed by this script.
param([switch]$InstallBuildTools)
$ErrorActionPreference = 'Stop'
$toolRoot = Join-Path $PSScriptRoot 'tools'
New-Item -ItemType Directory -Path $toolRoot -Force | Out-Null
if ($InstallBuildTools) {
    $setup = Join-Path $toolRoot 'vs_BuildTools.exe'
    if (-not (Test-Path -LiteralPath $setup)) {
        Invoke-WebRequest 'https://aka.ms/vs/stable/vs_BuildTools.exe' -OutFile $setup -UseBasicParsing
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $setup
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Microsoft Corporation') {
        throw 'Microsoft Build Tools installer signature is invalid.'
    }
    $installation = Start-Process $setup -ArgumentList @(
        '--quiet', '--wait', '--norestart', '--nocache',
        '--installPath', ('"' + (Join-Path $toolRoot 'buildtools') + '"'),
        '--add', 'Microsoft.VisualStudio.Workload.VCTools', '--includeRecommended'
    ) -WindowStyle Hidden -PassThru -Wait
    if ($installation.ExitCode -notin @(0, 3010)) { throw ('Build Tools failed: ' + $installation.ExitCode) }
    if ($installation.ExitCode -eq 3010) { Write-Output 'Build Tools requests a manual Windows restart.' }
}
$env:RUSTUP_HOME = Join-Path $toolRoot 'rustup'
$env:CARGO_HOME = Join-Path $toolRoot 'cargo'
$env:RUSTUP_INIT_SKIP_PATH_CHECK = 'yes'
if (-not (Test-Path -LiteralPath (Join-Path $env:CARGO_HOME 'bin\cargo.exe'))) {
    $rustSetup = Join-Path $toolRoot 'rustup-init.exe'
    if (-not (Test-Path -LiteralPath $rustSetup)) {
        Invoke-WebRequest 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' -OutFile $rustSetup -UseBasicParsing
    }
    & $rustSetup -y --no-modify-path --profile minimal --default-toolchain stable-x86_64-pc-windows-msvc --default-host x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Rust installation failed.' }
}
& (Join-Path $env:CARGO_HOME 'bin\rustup.exe') component add rustfmt clippy
if ($LASTEXITCODE -ne 0) { throw 'Rust component installation failed.' }
Write-Output 'Rust is ready. Building Windows applications also requires MSVC and the Windows SDK.'
