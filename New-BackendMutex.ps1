# Shared only by the SYSTEM recovery worker and elevated local administrators.
# Every creator must use the same DACL; default token DACLs differ across accounts.
function New-BridgeBackendMutex([Parameter(Mandatory=$true)][string]$RuntimeDirectory){
    $runtime=[IO.Path]::GetFullPath($RuntimeDirectory).TrimEnd('\')
    $hash=[Security.Cryptography.SHA256]::Create()
    try{$digest=([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($runtime.ToUpperInvariant())))).Replace('-','').Substring(0,24)}finally{$hash.Dispose()}
    $name='Global\USBBridge.Backend.'+$digest
    $security=New-Object Security.AccessControl.MutexSecurity
    $security.SetAccessRuleProtection($true,$false)
    foreach($sid in @('S-1-5-18','S-1-5-32-544')){
        $identity=New-Object Security.Principal.SecurityIdentifier($sid)
        $security.AddAccessRule([Security.AccessControl.MutexAccessRule]::new($identity,[Security.AccessControl.MutexRights]::FullControl,[Security.AccessControl.AccessControlType]::Allow))
    }
    $minimum=[Security.AccessControl.MutexRights]::Synchronize-bor [Security.AccessControl.MutexRights]::Modify
    # An old worker may briefly hold an object created with its default DACL.
    # Open that SAME object with synchronization rights, or wait for it to close;
    # never create a different lock name or bypass the running transaction.
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    do{
        try{
            $created=$false
            return [Threading.Mutex]::new($false,$name,[ref]$created,$security)
        }catch [UnauthorizedAccessException]{
            try{return [Threading.Mutex]::OpenExisting($name,$minimum)}
            catch [Threading.WaitHandleCannotBeOpenedException]{}
            catch [UnauthorizedAccessException]{}
            if([DateTime]::UtcNow-ge $deadline){throw "Cannot access the USB Bridge backend lock as an elevated administrator or SYSTEM: $name"}
            Start-Sleep -Milliseconds 100
        }
    }while($true)
}
