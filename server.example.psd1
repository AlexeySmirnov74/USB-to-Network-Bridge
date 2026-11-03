@{
    Listen = '0.0.0.0:3240'
    DiscoveryPort = 3241          # UDP discovery; use the same value in the client.
    AllowedPeers = @('192.0.2.10') # Replace with actual client IP addresses.
    Export = @()                 # Start without capture; publish devices in the GUI.
    Debug = $false
    # Optional bus ID -> full Windows instance ID to detect replacement devices.
    ExpectedInstanceIds = @{}
}
