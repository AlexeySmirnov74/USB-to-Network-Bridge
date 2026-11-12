# Read-only developer diagnostics. No certificate import, AIA, CRL or root update.
param([Parameter(Mandatory=$true)][string]$InstallerDirectory)
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;
namespace BridgeCertificateDiagnostic {
 public sealed class CertificateInfo {
  public string Subject {get;set;} public string Issuer {get;set;}
  public string Sha256 {get;set;} public string Sha1 {get;set;}
  public string ErrorStatus {get;set;} public string InfoStatus {get;set;}
  public bool SelfIssued {get;set;}
 }
 public sealed class ChainResult {
  public bool ApiSucceeded {get;set;} public int ApiError {get;set;}
  public string Flags {get;set;} public string ErrorStatus {get;set;} public string InfoStatus {get;set;}
  public CertificateInfo Leaf {get;set;} public CertificateInfo[] Certificates {get;set;}
 }
 public static class Native {
  [StructLayout(LayoutKind.Sequential)] private struct EnhancedUsage {public uint Count; public IntPtr Identifiers;}
  [StructLayout(LayoutKind.Sequential)] private struct UsageMatch {public uint Type; public EnhancedUsage Usage;}
  [StructLayout(LayoutKind.Sequential)] private struct ChainParameters {
   public uint Size; public UsageMatch RequestedUsage; public UsageMatch RequestedIssuancePolicy;
   public uint UrlRetrievalTimeout; [MarshalAs(UnmanagedType.Bool)] public bool CheckRevocationFreshness;
   public uint RevocationFreshness; public IntPtr CacheResync; public IntPtr StrongSign; public uint StrongSignFlags;
  }
  [StructLayout(LayoutKind.Sequential)] private struct ChainPrefix {
   public uint Size; public uint Error; public uint Info; public uint Count; public IntPtr Chains;
  }
  [StructLayout(LayoutKind.Sequential)] private struct ElementPrefix {
   public uint Size; public IntPtr Certificate; public uint Error; public uint Info;
  }
  [DllImport("crypt32.dll",SetLastError=true)] [return:MarshalAs(UnmanagedType.Bool)]
  private static extern bool CertGetCertificateChain(IntPtr engine,IntPtr certificate,IntPtr time,IntPtr extraStore,ref ChainParameters parameters,uint flags,IntPtr reserved,out IntPtr chain);
  [DllImport("crypt32.dll")] private static extern void CertFreeCertificateChain(IntPtr chain);
  public static CertificateInfo Describe(X509Certificate2 cert,uint error,uint info) {
   string hash;using(var sha=SHA256.Create()){hash=BitConverter.ToString(sha.ComputeHash(cert.RawData)).Replace("-","");}
   return new CertificateInfo {Subject=cert.Subject,Issuer=cert.Issuer,Sha256=hash,Sha1=cert.Thumbprint,
    ErrorStatus="0x"+error.ToString("X8"),InfoStatus="0x"+info.ToString("X8"),SelfIssued=String.Equals(cert.Subject,cert.Issuer,StringComparison.OrdinalIgnoreCase)};
  }
  public static ChainResult Inspect(string installer) {
   using(var signed=X509Certificate.CreateFromSignedFile(installer))
   using(var cert=new X509Certificate2(signed)) {
    var parameters=new ChainParameters {Size=(uint)Marshal.SizeOf(typeof(ChainParameters))};
    // CACHE_ONLY_URL_RETRIEVAL | DISABLE_AUTH_ROOT_AUTO_UPDATE | DISABLE_AIA.
    // No REVOCATION_CHECK flags: the probe never retrieves revocation material.
    const uint flags=0x00002104;
    IntPtr chain=IntPtr.Zero;
    bool ok=CertGetCertificateChain(IntPtr.Zero,cert.Handle,IntPtr.Zero,IntPtr.Zero,ref parameters,flags,IntPtr.Zero,out chain);
    var result=new ChainResult {ApiSucceeded=ok,ApiError=ok?0:Marshal.GetLastWin32Error(),Flags="0x"+flags.ToString("X8"),Leaf=Describe(cert,0,0)};
    var certificates=new List<CertificateInfo>();
    try {
     if(chain!=IntPtr.Zero) {
      var context=(ChainPrefix)Marshal.PtrToStructure(chain,typeof(ChainPrefix));
      result.ErrorStatus="0x"+context.Error.ToString("X8");result.InfoStatus="0x"+context.Info.ToString("X8");
      if(context.Count>16)throw new InvalidOperationException("Too many diagnostic certificate chains.");
      for(uint index=0;index<context.Count;index++) {
       IntPtr simplePointer=Marshal.ReadIntPtr(context.Chains,(int)index*IntPtr.Size);
       var simple=(ChainPrefix)Marshal.PtrToStructure(simplePointer,typeof(ChainPrefix));
       if(simple.Count>32)throw new InvalidOperationException("Diagnostic certificate chain is too long.");
       for(uint element=0;element<simple.Count;element++) {
        IntPtr elementPointer=Marshal.ReadIntPtr(simple.Chains,(int)element*IntPtr.Size);
        var value=(ElementPrefix)Marshal.PtrToStructure(elementPointer,typeof(ElementPrefix));
        using(var part=new X509Certificate2(value.Certificate)){certificates.Add(Describe(part,value.Error,value.Info));}
       }
      }
     }
    }finally{if(chain!=IntPtr.Zero)CertFreeCertificateChain(chain);}
    result.Certificates=certificates.ToArray();return result;
   }
  }
 }
}
'@
$expected=@(
 @{Name='GlobalSign code-signing intermediate';Sha256='CD0E144DD10BAC221FE2FB901058D16450A0578B3C47C770908F2E9ADA28EF12'},
 @{Name='GlobalSign Code Signing Root R45';Sha256='7B9D553E1C92CB6E8803E137F4F287D4363757F5D44B37D52F9FCA22FB97DF86'},
 @{Name='Microsoft Public RSA Timestamping CA 2020';Sha256='36E731CFA9BFD69DAFB643809F6DEC500902F7197DAEAAD86EA0159A2268A2B8'},
 @{Name='Microsoft Identity Verification Root Certificate Authority 2020';Sha256='5367F20C7ADE0E2BCA790915056D086B720C33C1FA2A2661ACF787E3292E1270'}
)
$stores=@(foreach($location in @('CurrentUser','LocalMachine')){foreach($name in @('Root','CA')){
 $store=New-Object Security.Cryptography.X509Certificates.X509Store($name,[Security.Cryptography.X509Certificates.StoreLocation]::$location)
 try {
  $store.Open([Security.Cryptography.X509Certificates.OpenFlags]::OpenExistingOnly-bor [Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
  $found=@{}
  foreach($cert in $store.Certificates){$info=[BridgeCertificateDiagnostic.Native]::Describe($cert,0,0);$found[$info.Sha256]=$info}
  [pscustomobject]@{Location=$location;Name=$name;Expected=@(foreach($item in $expected){[pscustomobject]@{Name=$item.Name;Sha256=$item.Sha256;Present=$found.ContainsKey($item.Sha256)}})}
 }catch{[pscustomobject]@{Location=$location;Name=$name;Error=$_.Exception.Message}}
 finally{$store.Close()}
}})
$installers=@(foreach($file in @('usbipd-win_5.3.0_x64.msi','USBip-0.9.8.1-x64.exe')){
 try{[pscustomobject]@{File=$file;Chain=[BridgeCertificateDiagnostic.Native]::Inspect((Join-Path $InstallerDirectory $file))}}
 catch{[pscustomobject]@{File=$file;Error=$_.Exception.Message}}
})
[pscustomobject]@{ReadOnly=$true;NetworkAllowed=$false;Probe='Current-time raw chain diagnostic; timestamped Authenticode verdict is separate';Installers=$installers;Stores=$stores}|ConvertTo-Json -Depth 10
