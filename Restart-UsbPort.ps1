param(
    [Parameter(Mandatory=$true)][string]$DeviceInstanceId,
    [Parameter(Mandatory=$true)][ValidateRange(1,127)][int]$Port,
    [string]$CapturedInstanceId,
    [switch]$Elevate
)
$ErrorActionPreference='Stop'
if($DeviceInstanceId-notlike 'USB\*'){throw 'An explicit USB instance ID is required.'}
$admin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if(-not $admin){
    if(-not $Elevate){throw 'USB port recovery requires Administrator rights.'}
    $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-DeviceInstanceId',('"'+$DeviceInstanceId+'"'),'-Port',[string]$Port)
    if($CapturedInstanceId){$arguments+=@('-CapturedInstanceId',('"'+$CapturedInstanceId+'"'))}
    $child=Start-Process powershell.exe -Verb RunAs -WindowStyle Hidden -ArgumentList $arguments -Wait -PassThru
    if($child.ExitCode-ne 0){throw ('Targeted USB port recovery failed: '+$child.ExitCode)}
    exit 0
}
$parent=(Get-PnpDeviceProperty -InstanceId $DeviceInstanceId -KeyName DEVPKEY_Device_Parent).Data
$actualPort=(Get-PnpDeviceProperty -InstanceId $DeviceInstanceId -KeyName DEVPKEY_Device_Address -ErrorAction Stop).Data
if([uint32]$actualPort-ne [uint32]$Port){throw 'The selected device moved to another USB port. Refresh its address before cycling.'}
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class UsbBridgePortRecovery {
 [DllImport("cfgmgr32.dll",CharSet=CharSet.Unicode)] static extern uint CM_Locate_DevNodeW(out uint node,string id,uint flags);
 [DllImport("cfgmgr32.dll")] static extern uint CM_Get_DevNode_Status(out uint status,out uint problem,uint node,uint flags);
 [DllImport("cfgmgr32.dll")] static extern uint CM_Setup_DevNode(uint node,uint flags);
 [DllImport("cfgmgr32.dll",CharSet=CharSet.Unicode)] static extern uint CM_Get_Device_Interface_List_SizeW(out uint size,ref Guid guid,string instance,uint flags);
 [DllImport("cfgmgr32.dll",CharSet=CharSet.Unicode)] static extern uint CM_Get_Device_Interface_ListW(ref Guid guid,string instance,[Out] char[] buffer,uint size,uint flags);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr CreateFileW(string path,uint access,uint share,IntPtr security,uint creation,uint attributes,IntPtr template);
 [DllImport("kernel32.dll",SetLastError=true)] [return:MarshalAs(UnmanagedType.Bool)] static extern bool DeviceIoControl(IntPtr file,uint code,ref PortCycle input,uint inputSize,ref PortCycle output,uint outputSize,out uint returned,IntPtr overlapped);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
 [StructLayout(LayoutKind.Sequential)] struct PortCycle {public uint Port,Status;}
 public static void ClearPendingRemoval(string instance) {
  uint node,status,problem;
  if(instance!=null && CM_Locate_DevNodeW(out node,instance,1)==0 && CM_Get_DevNode_Status(out status,out problem,node,0)==0 && problem==21) {
   uint result=CM_Setup_DevNode(node,0);
   if(result!=0)throw new InvalidOperationException("CM_Setup_DevNode failed: "+result);
  }
 }
 public static void Cycle(string hub,uint port) {
  var guid=new Guid("f18a0e88-c30c-11d0-8815-00a0c906bed8");
  uint length;
  uint result=CM_Get_Device_Interface_List_SizeW(out length,ref guid,hub,0);
  if(result!=0 || length<2)throw new InvalidOperationException("USB hub interface missing: "+result);
  var buffer=new char[length];
  result=CM_Get_Device_Interface_ListW(ref guid,hub,buffer,length,0);
  if(result!=0)throw new InvalidOperationException("USB hub interface query failed: "+result);
  int end=Array.IndexOf(buffer,'\0');
  var file=CreateFileW(new string(buffer,0,end),0xc0000000,3,IntPtr.Zero,3,0,IntPtr.Zero);
  if(file==new IntPtr(-1))throw new Win32Exception(Marshal.GetLastWin32Error());
  try {
   var input=new PortCycle{Port=port};var output=new PortCycle();uint returned;
   if(!DeviceIoControl(file,0x220444,ref input,8,ref output,8,out returned,IntPtr.Zero))throw new Win32Exception(Marshal.GetLastWin32Error());
   if(returned!=8||output.Status!=0)throw new InvalidOperationException("USB port cycle status: "+output.Status);
  } finally {CloseHandle(file);}
 }
}
'@
[UsbBridgePortRecovery]::ClearPendingRemoval($CapturedInstanceId)
[UsbBridgePortRecovery]::Cycle($parent,[uint32]$Port)
