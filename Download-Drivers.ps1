param(
    [ValidateSet('Server', 'Client', 'Both')][string]$Role = 'Both',
    [string]$Destination = (Join-Path $PSScriptRoot 'tools\driver-installers')
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'driver-installers.json') -Raw | ConvertFrom-Json
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
$roles = if ($Role -eq 'Both') { @('Server', 'Client') } else { @($Role) }
foreach ($selected in $roles) {
    $package = $manifest.$selected
    $file = Join-Path $Destination $package.file
    if (-not (Test-Path -LiteralPath $file)) {
        $partial = $file + '.partial'
        Invoke-WebRequest $package.url -OutFile $partial -UseBasicParsing
        if ((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash -ne $package.sha256) {
            throw ('Downloaded installer hash mismatch: ' + $selected)
        }
        Move-Item -LiteralPath $partial -Destination $file -Force
    }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $package.sha256) {
        throw ('Installer hash mismatch: ' + $file)
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch [regex]::Escape($package.signer)) {
        throw ('Installer Authenticode verification failed: ' + $file)
    }
    Write-Output ($selected + ' ' + $package.version + ' verified: ' + (Resolve-Path -LiteralPath $file).Path)
}
