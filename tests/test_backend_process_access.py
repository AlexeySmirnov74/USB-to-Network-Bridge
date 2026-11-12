"""Exercise real WinPS 5 wrapper binding without opening or changing a process."""
import json
import os
from pathlib import Path
import subprocess
import unittest


@unittest.skipUnless(os.name == "nt", "Windows PowerShell is required")
class BackendProcessAccessTests(unittest.TestCase):
    def test_unsigned_parameters_and_identity_rejection_before_process_access(self):
        helper = Path(__file__).resolve().parents[1] / "Set-BackendProcessAccess.ps1"
        powershell = Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        script = r'''
$ErrorActionPreference='Stop'
. $env:BRIDGE_PROCESS_ACCESS_TEST_HELPER
$checks=@()
foreach($number in @([UInt32]1,[UInt32]::MaxValue)){
 try{
  Grant-BridgeBackendProcessQuery -ProcessId $number -ExpectedExecutable 'C:\not-a-backend.exe' -ExpectedFileTime ([UInt64]134389391023321562)
  throw 'An unrelated executable was accepted.'
 }catch{
  if($_.Exception.Message-notlike '*Only usbbridge-server.exe may receive*'){throw}
  $checks+=$true
 }
}
foreach($arguments in @(@{ProcessId=0;ExpectedFileTime=1},@{ProcessId=1;ExpectedFileTime=0})){
 $rejected=$false
 try{Grant-BridgeBackendProcessQuery @arguments -ExpectedExecutable 'C:\usbbridge-server.exe'|Out-Null}catch{$rejected=$true}
 if(-not $rejected){throw 'A zero process identity was accepted.'}
 $checks+=$true
}
if([USBtoNetworkBridge.BackendProcessAccessV1]::QueryAccess-ne 0x1000){throw 'The query permission was broadened.'}
ConvertTo-Json -InputObject $checks -Compress
'''
        env = dict(os.environ, BRIDGE_PROCESS_ACCESS_TEST_HELPER=str(helper))
        result = subprocess.run(
            [str(powershell), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script],
            env=env, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))
        self.assertEqual(json.loads(result.stdout.decode("utf-8-sig")), [True] * 4)


if __name__ == "__main__":
    unittest.main()
