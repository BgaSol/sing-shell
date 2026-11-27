$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'This desktop preparation is limited to disposable GitHub runners.' }
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class SingShellTaskbar {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindow(string className, string windowName);
}
'@
$sessionId = (Get-Process -Id $PID).SessionId
$taskbarBefore = [SingShellTaskbar]::FindWindow('Shell_TrayWnd', $null)
@{ sessionId = $sessionId; taskbarBefore = $taskbarBefore.ToInt64(); explorers = @(Get-Process explorer -ErrorAction SilentlyContinue | Select-Object Id, SessionId) } | ConvertTo-Json -Compress
if ($taskbarBefore -eq [IntPtr]::Zero) {
    Start-Process -FilePath (Join-Path $env:WINDIR 'explorer.exe') -WindowStyle Hidden
}
$deadline = [DateTime]::UtcNow.AddSeconds(25)
while ([SingShellTaskbar]::FindWindow('Shell_TrayWnd', $null) -eq [IntPtr]::Zero) {
    if ([DateTime]::UtcNow -ge $deadline) { throw 'The native Windows runner has no taskbar notification area in this session.' }
    Start-Sleep -Milliseconds 250
}
@{ sessionId = $sessionId; taskbarReady = $true; taskbarHandle = [SingShellTaskbar]::FindWindow('Shell_TrayWnd', $null).ToInt64() } | ConvertTo-Json -Compress
