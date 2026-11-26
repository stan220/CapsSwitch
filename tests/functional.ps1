[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PackageDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not ('CapsSwitch.NativeMethods' -as [type])) {
    Add-Type -Namespace CapsSwitch -Name NativeMethods -MemberDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class NativeMethods
{
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindow(string className, string windowName);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

    [DllImport("user32.dll")]
    public static extern IntPtr GetKeyboardLayout(uint threadId);

    [DllImport("user32.dll")]
    public static extern short GetKeyState(int virtualKey);

    [DllImport("user32.dll")]
    public static extern void keybd_event(byte virtualKey, byte scanCode, uint flags, UIntPtr extraInfo);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr window);

    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll")]
    public static extern bool ShowWindowAsync(IntPtr window, int command);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr window, StringBuilder text, int maximumLength);
}
'@
}

function Wait-Until {
    param(
        [scriptblock]$Condition,
        [string]$Description,
        [int]$TimeoutSeconds = 10
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (& $Condition) {
            return
        }
        Start-Sleep -Milliseconds 100
    }

    throw "Timed out waiting for $Description."
}

function Assert-True {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Test-ScheduledTask {
    & schtasks.exe /Query /TN CapsSwitch 2>$null | Out-Null
    return $LASTEXITCODE -eq 0
}

$packagePath = (Resolve-Path $PackageDirectory).Path
$executablePath = Join-Path $packagePath 'caps-switch.exe'
$dllPath = Join-Path $packagePath 'caps_switch_hook.dll'
$configPath = Join-Path $packagePath 'config.toml'

foreach ($requiredFile in $executablePath, $dllPath, $configPath) {
    Assert-True (Test-Path $requiredFile -PathType Leaf) "Required package file is missing: $requiredFile"
}

$script:app = $null
$notepad = $null
$originalLanguages = $null
$originalConfig = $null

function Start-CapsSwitch {
    $previous = $script:app
    $next = Start-Process -FilePath $executablePath -WorkingDirectory $packagePath -PassThru

    if ($null -ne $previous) {
        Wait-Until { $previous.HasExited } 'the previous CapsSwitch instance to exit'
    }

    $script:app = $next
    Wait-Until {
        if ($script:app.HasExited) {
            throw "CapsSwitch exited during startup with code $($script:app.ExitCode)."
        }
        [CapsSwitch.NativeMethods]::FindWindow('CapsSwitchMessageTarget', $null) -ne [IntPtr]::Zero
    } 'CapsSwitch to initialize its hooks'

    Wait-Until {
        @(Get-Process -Name caps-switch -ErrorAction SilentlyContinue).Count -eq 1
    } 'exactly one CapsSwitch process'
}

function Get-NotepadLayout {
    $processId = [uint32]0
    $threadId = [CapsSwitch.NativeMethods]::GetWindowThreadProcessId($notepad.MainWindowHandle, [ref]$processId)
    return [CapsSwitch.NativeMethods]::GetKeyboardLayout($threadId)
}

try {
    $originalConfig = [IO.File]::ReadAllText($configPath)
    $originalLanguages = Get-WinUserLanguageList

    $testLanguages = New-WinUserLanguageList en-US
    $testLanguages.Add('ru-RU')
    Set-WinUserLanguageList -LanguageList $testLanguages -Force
    Start-Sleep -Seconds 1

    [IO.File]::WriteAllText($configPath, "method = `"winapi`"`noverlay = true`nautostart = true`n")

    $notepad = Start-Process notepad.exe -PassThru
    Wait-Until {
        $notepad.Refresh()
        -not $notepad.HasExited -and $notepad.MainWindowHandle -ne [IntPtr]::Zero
    } 'Notepad to open its window'

    [CapsSwitch.NativeMethods]::ShowWindowAsync($notepad.MainWindowHandle, 9) | Out-Null
    [CapsSwitch.NativeMethods]::SetForegroundWindow($notepad.MainWindowHandle) | Out-Null
    Wait-Until {
        [CapsSwitch.NativeMethods]::GetForegroundWindow() -eq $notepad.MainWindowHandle
    } 'Notepad to become the foreground window'

    Write-Host 'Checking startup and autostart creation'
    Start-CapsSwitch
    Wait-Until { Test-ScheduledTask } 'the CapsSwitch scheduled task to be created'

    $task = & schtasks.exe /Query /TN CapsSwitch /V /FO CSV | ConvertFrom-Csv
    Assert-True ($task.'Task To Run'.Contains($executablePath, [StringComparison]::OrdinalIgnoreCase)) 'The scheduled task does not point to the installed executable.'

    Write-Host 'Checking autostart removal and single-instance replacement'
    [IO.File]::WriteAllText($configPath, "method = `"winapi`"`noverlay = true`nautostart = false`n")
    Start-CapsSwitch
    Wait-Until { -not (Test-ScheduledTask) } 'the CapsSwitch scheduled task to be removed'

    $firstProcessId = $script:app.Id
    Start-CapsSwitch
    Assert-True ($script:app.Id -ne $firstProcessId) 'The replacement CapsSwitch process did not start.'
    Assert-True (@(Get-Process -Name caps-switch -ErrorAction SilentlyContinue).Count -eq 1) 'More than one CapsSwitch process is running.'

    Start-Sleep -Milliseconds 500
    [CapsSwitch.NativeMethods]::ShowWindowAsync($notepad.MainWindowHandle, 9) | Out-Null
    [CapsSwitch.NativeMethods]::SetForegroundWindow($notepad.MainWindowHandle) | Out-Null
    Wait-Until {
        [CapsSwitch.NativeMethods]::GetForegroundWindow() -eq $notepad.MainWindowHandle
    } 'Notepad to return to the foreground'

    Write-Host 'Checking Caps Lock layout switching and suppression'
    Assert-True (([CapsSwitch.NativeMethods]::GetKeyState(0x14) -band 1) -eq 0) 'Caps Lock was already enabled before the test.'
    $before = Get-NotepadLayout

    [CapsSwitch.NativeMethods]::keybd_event(0x14, 0, 0, [UIntPtr]::Zero)
    [CapsSwitch.NativeMethods]::keybd_event(0x14, 0, 2, [UIntPtr]::Zero)

    Wait-Until { (Get-NotepadLayout) -ne $before } 'Caps Lock to switch the input language'
    Assert-True (([CapsSwitch.NativeMethods]::GetKeyState(0x14) -band 1) -eq 0) 'Caps Lock changed the Caps Lock state.'

    Write-Host 'Checking the language overlay'
    $overlay = [IntPtr]::Zero
    Wait-Until {
        $script:overlay = [CapsSwitch.NativeMethods]::FindWindow('CapsSwitchOverlay', $null)
        $script:overlay -ne [IntPtr]::Zero -and [CapsSwitch.NativeMethods]::IsWindowVisible($script:overlay)
    } 'the language overlay to appear'

    $title = [Text.StringBuilder]::new(256)
    [void][CapsSwitch.NativeMethods]::GetWindowText($script:overlay, $title, $title.Capacity)
    Assert-True ($title.ToString() -in @('ENG', 'РУС')) "Unexpected language overlay text: $($title.ToString())"

    Wait-Until {
        -not [CapsSwitch.NativeMethods]::IsWindowVisible($script:overlay)
    } 'the language overlay to disappear'

    Write-Host 'All functional checks passed.'
}
finally {
    if ($null -ne $script:app) {
        Stop-Process -Id $script:app.Id -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $notepad) {
        Stop-Process -Id $notepad.Id -Force -ErrorAction SilentlyContinue
    }
    & schtasks.exe /Delete /TN CapsSwitch /F 2>$null | Out-Null

    if ($null -ne $originalConfig) {
        [IO.File]::WriteAllText($configPath, $originalConfig)
    }
    if ($null -ne $originalLanguages) {
        Set-WinUserLanguageList -LanguageList $originalLanguages -Force
    }
}
