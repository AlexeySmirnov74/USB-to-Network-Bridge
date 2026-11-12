# Local-only verification. Runtime setup must never fetch an installer or a CRL.
param(
 [ValidateSet('Server','Client','Both')][string]$Role='Both',
 [string]$InstallerDirectory=(Join-Path $PSScriptRoot 'tools\driver-installers'),
 [string]$ManifestPath=(Join-Path $PSScriptRoot 'driver-installers.json'),
 [switch]$FunctionsOnly
)
$ErrorActionPreference='Stop'
function Get-BridgeOfflineDriverSignature([string]$Path,[string]$ExpectedSha256,[string]$ExpectedSigner) {
 if(-not ('BridgeOfflineInstaller.Signature'-as [type])){
  Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
namespace BridgeOfflineInstaller {
 public sealed class VerifiedInstaller {
  public string Sha256 {get;set;}
  public string Signer {get;set;}
 }
 public static class Signature {
  private const string ClientInstallerSha256="38CAD6D4432B52D5BB9409D9AD03B72FDFFC4ADA4CD3A48FBECA1A2752A8518A";
  private const string ClientIntermediateSha256="CD0E144DD10BAC221FE2FB901058D16450A0578B3C47C770908F2E9ADA28EF12";
  // Public intermediate from the pinned signer's CA Issuers extension:
  // https://secure.globalsign.com/cacert/gsgccr45evcodesignca2020.crt
  private const string ClientIntermediateBase64="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";
  private static string CertificateDigest(byte[] encoded) {
   using(var hash=SHA256.Create()) {return BitConverter.ToString(hash.ComputeHash(encoded)).Replace("-", "");}
  }
  private static X509Certificate2 DecodePinnedClientIntermediate(byte[] encoded) {
   if(!String.Equals(CertificateDigest(encoded),ClientIntermediateSha256,StringComparison.OrdinalIgnoreCase))
    throw new InvalidDataException("Bundled client intermediate certificate SHA256 mismatch.");
   var certificate=new X509Certificate2(encoded);
   try {
    X509BasicConstraintsExtension constraints=null;
    X509KeyUsageExtension usage=null;
    foreach(X509Extension extension in certificate.Extensions) {
     if(extension.Oid.Value=="2.5.29.19") constraints=new X509BasicConstraintsExtension(extension,extension.Critical);
     if(extension.Oid.Value=="2.5.29.15") usage=new X509KeyUsageExtension(extension,extension.Critical);
    }
    if(certificate.HasPrivateKey || constraints==null || !constraints.CertificateAuthority || usage==null ||
       (usage.KeyUsages & X509KeyUsageFlags.KeyCertSign)==0 ||
       String.Equals(certificate.Subject,certificate.Issuer,StringComparison.OrdinalIgnoreCase))
     throw new InvalidDataException("The bundled client intermediate must be a non-self-issued certificate-signing CA.");
    return certificate;
   } catch {certificate.Dispose();throw;}
  }
  private static void EnsurePinnedClientIntermediate(string installerDigest) {
   // The upstream Client EXE embeds only its leaf signer. Supply its exact
   // missing intermediate only after the complete installer digest matched.
   if(!String.Equals(installerDigest,ClientInstallerSha256,StringComparison.OrdinalIgnoreCase))return;
   using(var certificate=DecodePinnedClientIntermediate(Convert.FromBase64String(ClientIntermediateBase64)))
   using(var store=new X509Store(StoreName.CertificateAuthority,StoreLocation.CurrentUser)) {
    store.Open(OpenFlags.ReadWrite);
    bool present=false;
    foreach(X509Certificate2 existing in store.Certificates) {
     using(existing) {
      if(String.Equals(CertificateDigest(existing.RawData),ClientIntermediateSha256,StringComparison.OrdinalIgnoreCase))present=true;
     }
    }
    if(!present)store.Add(certificate);
   }
   // CA is the intermediate store, never Root/TrustedPeople/TrustedPublisher.
   // This adds no trust anchor. The unchanged full WinVerifyTrust FILE policy
   // below still validates PE digest, timestamp and a Windows-trusted root.
  }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  private struct FileInfo {
   public uint Size;
   [MarshalAs(UnmanagedType.LPWStr)] public string Path;
   public IntPtr Handle;
   public IntPtr KnownSubject;
  }
  [StructLayout(LayoutKind.Sequential)]
  private struct TrustData {
   public uint Size;
   public IntPtr PolicyCallbackData;
   public IntPtr SIPClientData;
   public uint UIChoice;
   public uint RevocationChecks;
   public uint UnionChoice;
   public IntPtr File;
   public uint StateAction;
   public IntPtr StateData;
   public IntPtr URLReference;
   public uint ProviderFlags;
   public uint UIContext;
   public IntPtr SignatureSettings;
  }
  [StructLayout(LayoutKind.Sequential)]
  private struct ProviderCertificatePrefix {
   public uint Size;
   public IntPtr Certificate;
  }
  [DllImport("wintrust.dll", ExactSpelling=true)]
  private static extern int WinVerifyTrust(IntPtr window, ref Guid action, ref TrustData data);
  [DllImport("wintrust.dll", ExactSpelling=true)]
  private static extern IntPtr WTHelperProvDataFromStateData(IntPtr state);
  [DllImport("wintrust.dll", ExactSpelling=true)]
  private static extern IntPtr WTHelperGetProvSignerFromChain(IntPtr provider, uint signer, [MarshalAs(UnmanagedType.Bool)] bool counterSigner, uint counterIndex);
  [DllImport("wintrust.dll", ExactSpelling=true)]
  private static extern IntPtr WTHelperGetProvCertFromChain(IntPtr signer, uint index);
  public static VerifiedInstaller Verify(string path, string expectedSha256, string expectedSigner) {
   // FileShare.Read prevents modification/deletion between digest and trust checks.
   using(var input=new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read)) {
    string digest;
    using(var hash=SHA256.Create()) {digest=BitConverter.ToString(hash.ComputeHash(input)).Replace("-", "");}
    if(!String.Equals(digest, expectedSha256, StringComparison.OrdinalIgnoreCase))
     throw new InvalidDataException("Bundled driver installer SHA256 mismatch. Extract a fresh complete release; no download was attempted.");
    EnsurePinnedClientIntermediate(digest);
    input.Position=0;
    var file=new FileInfo {Size=(uint)Marshal.SizeOf(typeof(FileInfo)), Path=path, Handle=input.SafeFileHandle.DangerousGetHandle()};
    IntPtr filePointer=Marshal.AllocHGlobal(Marshal.SizeOf(typeof(FileInfo)));
    var trust=new TrustData {
     Size=(uint)Marshal.SizeOf(typeof(TrustData)), UIChoice=2, RevocationChecks=0,
     UnionChoice=1, File=filePointer, StateAction=1,
     // WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE.
     // This forbids certificate/revocation URL fetches even on a new offline PC.
     ProviderFlags=0x00001010
    };
    var action=new Guid("00AAC56B-CD44-11D0-8CC2-00C04FC295EE");
    bool fileMarshaled=false;
    try {
     Marshal.StructureToPtr(file, filePointer, false);fileMarshaled=true;
     int status=WinVerifyTrust(new IntPtr(-1), ref action, ref trust);
     if(status!=0)
      throw new InvalidDataException("Bundled driver installer offline signature verification failed (0x"+unchecked((uint)status).ToString("X8")+"). Windows must trust the upstream publisher locally; no download was attempted.");
     IntPtr provider=WTHelperProvDataFromStateData(trust.StateData);
     IntPtr signer=provider==IntPtr.Zero ? IntPtr.Zero : WTHelperGetProvSignerFromChain(provider,0,false,0);
     IntPtr certificate=signer==IntPtr.Zero ? IntPtr.Zero : WTHelperGetProvCertFromChain(signer,0);
     if(certificate==IntPtr.Zero) throw new InvalidDataException("The verified installer has no publisher certificate.");
     var prefix=(ProviderCertificatePrefix)Marshal.PtrToStructure(certificate,typeof(ProviderCertificatePrefix));
     if(prefix.Certificate==IntPtr.Zero) throw new InvalidDataException("The verified installer publisher certificate is unavailable.");
     using(var cert=new X509Certificate2(prefix.Certificate)) {
      string name=cert.GetNameInfo(X509NameType.SimpleName,false);
      if(!String.Equals(name,expectedSigner,StringComparison.OrdinalIgnoreCase))
       throw new InvalidDataException("Bundled driver installer publisher mismatch (expected '"+expectedSigner+"', received '"+name+"'). No download was attempted.");
      return new VerifiedInstaller {Sha256=digest,Signer=name};
     }
    } finally {
     if(trust.StateData!=IntPtr.Zero) {trust.StateAction=2;WinVerifyTrust(new IntPtr(-1),ref action,ref trust);}
     if(fileMarshaled) Marshal.DestroyStructure(filePointer,typeof(FileInfo));
     Marshal.FreeHGlobal(filePointer);
    }
   }
  }
 }
}
'@
 }
 [BridgeOfflineInstaller.Signature]::Verify($Path,$ExpectedSha256,$ExpectedSigner)
}
function Assert-BridgeDriverInstaller {
 param(
  [Parameter(Mandatory=$true)][ValidateSet('Server','Client','Both')][string]$Role,
  [Parameter(Mandatory=$true)][string]$InstallerDirectory,
  [string]$ManifestPath=(Join-Path $PSScriptRoot 'driver-installers.json')
 )
 if(-not(Test-Path -LiteralPath $ManifestPath -PathType Leaf)){throw 'The driver installer manifest is missing. Extract the complete release; no download was attempted.'}
 $manifestFile=Get-Item -LiteralPath $ManifestPath
 if([UInt64]$manifestFile.Length-gt [UInt64]65536){throw 'The driver installer manifest exceeds the size limit.'}
 $manifest=[IO.File]::ReadAllText($manifestFile.FullName)|ConvertFrom-Json
 $roles=if($Role-eq 'Both'){@('Server','Client')}else{@($Role)}
 foreach($selected in $roles){
  $package=$manifest.$selected
  if(-not $package-or [string]$package.sha256-notmatch '^[a-fA-F0-9]{64}$'-or [string]::IsNullOrWhiteSpace([string]$package.signer)-or [string]::IsNullOrWhiteSpace([string]$package.file)-or [IO.Path]::GetFileName([string]$package.file)-cne [string]$package.file){throw 'The bundled driver installer manifest is invalid.'}
  $file=Join-Path $InstallerDirectory ([string]$package.file)
  if(-not(Test-Path -LiteralPath $file -PathType Leaf)){throw ('Bundled '+$selected+' driver installer is missing: '+$package.file+'. Extract the complete release including app\tools\driver-installers. No download was attempted.')}
  $item=Get-Item -LiteralPath $file
  if($item.Attributes-band [IO.FileAttributes]::ReparsePoint){throw 'A bundled driver installer must not be a symlink or junction.'}
  $expectedSigner=if($package.PSObject.Properties['signer_name']){[string]$package.signer_name}else{[string]$package.signer}
  if([string]::IsNullOrWhiteSpace($expectedSigner)){throw 'The bundled driver installer publisher name is invalid.'}
  $verified=Get-BridgeOfflineDriverSignature $item.FullName ([string]$package.sha256) $expectedSigner
  [pscustomobject]@{Role=$selected;Path=$item.FullName;Version=[string]$package.version;Sha256=$verified.Sha256;Signer=$verified.Signer;OfflineVerified=$true}
 }
}
if($FunctionsOnly){return}
Assert-BridgeDriverInstaller -Role $Role -InstallerDirectory $InstallerDirectory -ManifestPath $ManifestPath
