$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    $localCargo = Join-Path $PSScriptRoot 'tools\cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $localCargo) {
        $env:RUSTUP_HOME = Join-Path $PSScriptRoot 'tools\rustup'
        $env:CARGO_HOME = Join-Path $PSScriptRoot 'tools\cargo'
        $cargo = $localCargo
    } else {
        $cargo = (Get-Command cargo -ErrorAction Stop).Source
    }
    foreach($arguments in @(@('fmt','--all','--','--check'),@('clippy','--locked','--all-targets','--','-D','warnings'),@('test','--locked'),@('build','--release','--locked'))){
        & $cargo @arguments
        if($LASTEXITCODE-ne 0){throw ('Cargo failed: '+($arguments-join ' '))}
    }
} finally {Pop-Location}
