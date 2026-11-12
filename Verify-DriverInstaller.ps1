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
