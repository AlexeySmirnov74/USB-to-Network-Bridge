param([string]$SourceDirectory=(Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference='Stop'
. (Join-Path $SourceDirectory 'Verify-DriverInstaller.ps1') -FunctionsOnly
$testRoot=Join-Path ([IO.Path]::GetTempPath()) ('bridge-intermediate-'+[guid]::NewGuid().ToString('N'))
$script:checks=0
function Assert-Intermediate([bool]$Condition,[string]$Label){if(-not $Condition){throw ('Intermediate fixture failed: '+$Label)};$script:checks++}
try {
 New-Item -ItemType Directory -Path $testRoot -Force|Out-Null
 $unsigned=Join-Path $testRoot 'USBip-0.9.8.1-x64.exe'
 [IO.File]::WriteAllText($unsigned,'Unsigned fixture; no executable content.')
 $digest=(Get-FileHash -LiteralPath $unsigned -Algorithm SHA256).Hash
 # Compile the real verifier using an unsigned fixture whose digest cannot
 # select the pinned installer branch. This never opens a certificate store.
 $blocked=$false
 try{$null=Get-BridgeOfflineDriverSignature $unsigned $digest 'Fixture Publisher'}catch{$blocked=$_.Exception.Message-match 'offline signature verification failed'}
 Assert-Intermediate $blocked 'a matching digest and installer filename do not permit an unsigned file'

 $type='BridgeOfflineInstaller.Signature'-as [type]
 $flags=[Reflection.BindingFlags]'Static,NonPublic'
 $field=$type.GetField('ClientIntermediateBase64',$flags)
 $decode=$type.GetMethod('DecodePinnedClientIntermediate',$flags)
 $bytes=[Convert]::FromBase64String([string]$field.GetValue($null))
 $sha=[Security.Cryptography.SHA256]::Create()
 try{$certificateDigest=([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-','')}finally{$sha.Dispose()}
 Assert-Intermediate ($certificateDigest-eq 'CD0E144DD10BAC221FE2FB901058D16450A0578B3C47C770908F2E9ADA28EF12') 'the embedded public intermediate has the reviewed exact DER digest'
 # Test only the pure decoder, independently of Root contents or a working
 # signer chain. Do not call EnsurePinnedClientIntermediate or modify stores.
 $certificate=$decode.Invoke($null,[object[]]@(,$bytes))
 try {
  Assert-Intermediate ($certificate.Subject-eq 'CN=GlobalSign GCC R45 EV CodeSigning CA 2020, O=GlobalSign nv-sa, C=BE') 'the intermediate belongs to the pinned client issuer'
  Assert-Intermediate ($certificate.Subject-ne $certificate.Issuer-and -not $certificate.HasPrivateKey) 'the certificate is non-self-issued and contains no private key'
  $constraints=$certificate.Extensions|Where-Object {$_.Oid.Value-eq '2.5.29.19'}
  $usage=$certificate.Extensions|Where-Object {$_.Oid.Value-eq '2.5.29.15'}
  Assert-Intermediate ($constraints.CertificateAuthority-and ($usage.KeyUsages-band [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::KeyCertSign)) 'the certificate is a CA allowed to sign certificates'
 }finally{$certificate.Dispose()}

 $changed=[byte[]]$bytes.Clone();$changed[$changed.Length-1]=$changed[$changed.Length-1]-bxor 1
 $blocked=$false
 try{$unexpected=$decode.Invoke($null,[object[]]@(,$changed));$unexpected.Dispose()}catch{$blocked=$_.Exception.ToString()-match 'intermediate certificate SHA256 mismatch'}
 Assert-Intermediate $blocked 'a modified embedded certificate is rejected before it can be used'
 $blocked=$false
 try{$unexpected=$decode.Invoke($null,[object[]]@(,[byte[]]@(1,2,3)));$unexpected.Dispose()}catch{$blocked=$_.Exception.ToString()-match 'intermediate certificate SHA256 mismatch'}
 Assert-Intermediate $blocked 'unreviewed certificate bytes cannot enter the intermediate store'
 [pscustomobject]@{passed=$true;checks=$script:checks;certificate_stores_modified=$false;root_store_required=$false}|ConvertTo-Json -Compress
}finally{
 $full=[IO.Path]::GetFullPath($testRoot);$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
 if(-not $full.StartsWith($temp+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected fixture cleanup target.'}
 if(Test-Path -LiteralPath $full){Remove-Item -LiteralPath $full -Recurse -Force}
}
