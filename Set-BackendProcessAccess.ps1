# Dot-source only. Grants metadata queries on one verified native backend process.
# No USB operation, task change, network request, or process restart is performed.
function Initialize-BridgeBackendProcessAccess {
 if('USBtoNetworkBridge.BackendProcessAccessV1'-as [type]){return}
 Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;

namespace USBtoNetworkBridge {
 public static class BackendProcessAccessV1 {
  public const uint QueryAccess = 0x00001000;
  public const uint OpenAccess = 0x00061000; // READ_CONTROL | WRITE_DAC | QUERY_LIMITED_INFORMATION
  private const uint KernelObject = 6;
  private const uint DaclInformation = 4;
  private const uint GrantAccess = 1;
  private const uint NoInheritance = 0;
  private const uint TrusteeIsSid = 0;
  private const uint TrusteeIsAlias = 4;
  [StructLayout(LayoutKind.Sequential)]
  private struct Trustee {
   public IntPtr MultipleTrustee;
   public uint MultipleTrusteeOperation;
   public uint Form;
   public uint Type;
   public IntPtr Name;
  }
  [StructLayout(LayoutKind.Sequential)]
  private struct ExplicitAccess {
   public uint Permissions;
   public uint Mode;
   public uint Inheritance;
   public Trustee Trustee;
  }
  [DllImport("kernel32.dll", SetLastError=true)]
  private static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  private static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags, StringBuilder name, ref uint size);
  [DllImport("kernel32.dll", SetLastError=true)]
  private static extern bool GetProcessTimes(IntPtr process, out long created, out long exited, out long kernel, out long user);
  [DllImport("kernel32.dll")]
  private static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll")]
  private static extern IntPtr LocalFree(IntPtr memory);
  [DllImport("advapi32.dll")]
  private static extern uint GetSecurityInfo(IntPtr handle, uint objectType, uint information, out IntPtr owner, out IntPtr group, out IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode)]
  private static extern uint SetEntriesInAclW(uint count, ref ExplicitAccess entries, IntPtr oldAcl, out IntPtr newAcl);
  [DllImport("advapi32.dll")]
  private static extern uint SetSecurityInfo(IntPtr handle, uint objectType, uint information, IntPtr owner, IntPtr group, IntPtr dacl, IntPtr sacl);

  public static string ValidateExpectedExecutable(string expected) {
   if(String.IsNullOrEmpty(expected) || expected.IndexOf('\0') >= 0 ||
      !((expected.Length >= 3 && Char.IsLetter(expected[0]) && expected[1] == ':' && (expected[2] == '\\' || expected[2] == '/')) || expected.StartsWith(@"\\", StringComparison.Ordinal)))
    throw new ArgumentException("ExpectedExecutable must be an absolute backend executable path.");
   string full = Path.GetFullPath(expected);
   if(!String.Equals(Path.GetFileName(full), "usbbridge-server.exe", StringComparison.OrdinalIgnoreCase))
    throw new ArgumentException("Only usbbridge-server.exe may receive the query permission.");
   return full;
  }
  public static void ValidateIdentity(string actual, ulong created, string expected, ulong expectedTime) {
   string full = ValidateExpectedExecutable(expected);
   if(expectedTime == 0 || created != expectedTime || !String.Equals(actual, full, StringComparison.OrdinalIgnoreCase))
    throw new InvalidOperationException("Backend image or creation time does not match the owned process.");
  }
  public static string Grant(uint processId, string expected, ulong expectedTime) {
   string full = ValidateExpectedExecutable(expected);
   if(processId == 0 || expectedTime == 0) throw new ArgumentException("A nonzero owned PID and creation time are required.");
   IntPtr process = IntPtr.Zero, descriptor = IntPtr.Zero, newAcl = IntPtr.Zero, sidMemory = IntPtr.Zero;
   try {
    process = OpenProcess(OpenAccess, false, processId);
    if(process == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot open the owned backend security descriptor.");
    var actual = new StringBuilder(32768);
    uint length = (uint)actual.Capacity;
    if(!QueryFullProcessImageNameW(process, 0, actual, ref length)) throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot verify the backend executable.");
    long created, exited, kernel, user;
    if(!GetProcessTimes(process, out created, out exited, out kernel, out user)) throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot verify the backend creation time.");
    ValidateIdentity(actual.ToString(), unchecked((ulong)created), full, expectedTime);
    if(exited != 0) throw new InvalidOperationException("The owned backend process has already exited.");
    IntPtr owner, group, oldAcl, sacl;
    uint result = GetSecurityInfo(process, KernelObject, DaclInformation, out owner, out group, out oldAcl, out sacl, out descriptor);
    if(result != 0) throw new Win32Exception((int)result, "Cannot read the backend DACL.");
    // A null DACL already permits queries. Replacing it would remove existing rights.
    if(oldAcl == IntPtr.Zero) return "AlreadyPermissive";
    var users = new SecurityIdentifier(WellKnownSidType.BuiltinUsersSid, null);
    var sid = new byte[users.BinaryLength];
    users.GetBinaryForm(sid, 0);
    sidMemory = Marshal.AllocHGlobal(sid.Length);
    Marshal.Copy(sid, 0, sidMemory, sid.Length);
    var entry = new ExplicitAccess {
     Permissions = QueryAccess, Mode = GrantAccess, Inheritance = NoInheritance,
     Trustee = new Trustee { MultipleTrustee = IntPtr.Zero, MultipleTrusteeOperation = 0, Form = TrusteeIsSid, Type = TrusteeIsAlias, Name = sidMemory }
    };
    // Merge into the current DACL; retain all existing ACEs and explicit denies.
    result = SetEntriesInAclW(1, ref entry, oldAcl, out newAcl);
    if(result != 0) throw new Win32Exception((int)result, "Cannot merge the backend query permission.");
    // Only DACL_SECURITY_INFORMATION: preserve owner, SACL, integrity and protection.
    result = SetSecurityInfo(process, KernelObject, DaclInformation, IntPtr.Zero, IntPtr.Zero, newAcl, IntPtr.Zero);
    if(result != 0) throw new Win32Exception((int)result, "Cannot set the backend query permission.");
    return "Granted";
   } finally {
    if(newAcl != IntPtr.Zero) LocalFree(newAcl);
    if(descriptor != IntPtr.Zero) LocalFree(descriptor);
    if(sidMemory != IntPtr.Zero) Marshal.FreeHGlobal(sidMemory);
    if(process != IntPtr.Zero) CloseHandle(process);
   }
  }
 }
}
'@
}
function Grant-BridgeBackendProcessQuery {
 [CmdletBinding()]
 param(
  [Parameter(Mandatory=$true)][ValidateScript({$_-gt 0})][UInt32]$ProcessId,
  [Parameter(Mandatory=$true)][string]$ExpectedExecutable,
  [Parameter(Mandatory=$true)][ValidateScript({$_-gt 0})][UInt64]$ExpectedFileTime
 )
 Initialize-BridgeBackendProcessAccess
 [USBtoNetworkBridge.BackendProcessAccessV1]::Grant($ProcessId,$ExpectedExecutable,$ExpectedFileTime)
}
