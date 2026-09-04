[CmdletBinding()]
param(
    [Parameter()]
    [string] $ExecutablePath = (Join-Path $PSScriptRoot '..\target\release\window-switcher.exe'),

    [Parameter()]
    [ValidateRange(1, 3600)]
    [int] $DurationSeconds = 30,

    [Parameter()]
    [ValidateRange(50, 10000)]
    [int] $SampleIntervalMilliseconds = 250,

    [Parameter()]
    [string] $OutputDirectory = (Join-Path $PSScriptRoot '..\target\release\performance'),

    [Parameter()]
    [bool] $StopExisting = $true,

    [Parameter()]
    [ValidateRange(0, 10000)]
    [int] $AutomatedSwitchCount = 0,

    [Parameter()]
    [ValidateSet('none', 'alpha', 'blur', 'acrylic', 'mica', 'auto')]
    [string] $Backdrop = 'none',

    [Parameter()]
    [ValidateRange(0, 100)]
    [int] $BackgroundOpacity = 100,

    [Parameter()]
    [ValidatePattern('^(?:auto|#[0-9A-Fa-f]{6})$')]
    [string] $BackgroundColor = 'auto',

    [Parameter()]
    [ValidateSet('alpha', 'solid')]
    [string] $BackdropFallback = 'alpha'
)

$ErrorActionPreference = 'Stop'

$resolvedExecutable = [System.IO.Path]::GetFullPath($ExecutablePath)
if (-not (Test-Path -LiteralPath $resolvedExecutable -PathType Leaf)) {
    throw "Executable not found: $resolvedExecutable"
}

$resolvedOutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$null = New-Item -ItemType Directory -Path $resolvedOutputDirectory -Force
$timestamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$counterPath = Join-Path $resolvedOutputDirectory "performance-$timestamp.csv"
$summaryPath = Join-Path $resolvedOutputDirectory "performance-$timestamp.json"
$metricsLogPath = Join-Path $resolvedOutputDirectory "performance-$timestamp.log"
$configPath = Join-Path (Split-Path -Parent $resolvedExecutable) 'window-switcher.ini'

if (-not ('WindowSwitcher.NativeMethods' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

namespace WindowSwitcher
{
    public static class NativeMethods
    {
        [DllImport("user32.dll", SetLastError = true)]
        public static extern uint GetGuiResources(IntPtr process, uint flags);

        [DllImport("user32.dll", EntryPoint = "PostMessageW", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);

        private delegate bool EnumWindowsProc(IntPtr window, IntPtr parameter);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);

        [DllImport("user32.dll", SetLastError = true)]
        private static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);

        [DllImport("user32.dll", EntryPoint = "GetWindowTextW", CharSet = CharSet.Unicode)]
        private static extern int GetWindowText(IntPtr window, StringBuilder text, int maximumCount);

        public static IntPtr FindMessageWindow(uint expectedProcessId)
        {
            IntPtr result = IntPtr.Zero;
            EnumWindows(delegate (IntPtr window, IntPtr parameter)
            {
                uint processId;
                GetWindowThreadProcessId(window, out processId);
                if (processId != expectedProcessId)
                {
                    return true;
                }

                StringBuilder title = new StringBuilder(64);
                GetWindowText(window, title, title.Capacity);
                if (title.ToString() != "Window Switcher")
                {
                    return true;
                }

                result = window;
                return false;
            }, IntPtr.Zero);
            return result;
        }
    }
}
'@
}

$configExisted = Test-Path -LiteralPath $configPath -PathType Leaf
$originalConfig = if ($configExisted) {
    [System.IO.File]::ReadAllBytes($configPath)
} else {
    $null
}
$previousPerfValue = [Environment]::GetEnvironmentVariable('WINDOW_SWITCHER_PERF', 'Process')
$startedProcess = $null
$samples = [System.Collections.Generic.List[object]]::new()
$startupStopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$inputIdleReached = $false
$switcherWindow = [IntPtr]::Zero
$switchMessagesSent = 0
$switchAppsMessage = 6010
$cancelSwitchAppsMessage = 6012
$postCancelCounters = $null

try {
    if ($StopExisting) {
        Get-Process -Name 'window-switcher' -ErrorAction SilentlyContinue |
            Stop-Process -Force -ErrorAction Stop
    }

    $temporaryConfig = @(
        'trayicon = yes'
        ''
        '[startup]'
        'run_as_admin = no'
        ''
        '[appearance]'
        "background_color = $BackgroundColor"
        "background_opacity = $BackgroundOpacity"
        "backdrop = $Backdrop"
        "backdrop_fallback = $BackdropFallback"
        ''
        '[switch-windows]'
        'hotkey = alt+`'
        'blacklist ='
        'ignore_minimal = no'
        'only_current_desktop = auto'
        ''
        '[switch-apps]'
        'enable = yes'
        'hotkey = alt+tab'
        'ignore_minimal = no'
        'override_icons ='
        'only_current_desktop = auto'
        ''
        '[log]'
        'level = info'
        "path = $metricsLogPath"
    )
    [System.IO.File]::WriteAllLines(
        $configPath,
        $temporaryConfig,
        [System.Text.UTF8Encoding]::new($false)
    )

    [Environment]::SetEnvironmentVariable('WINDOW_SWITCHER_PERF', '1', 'Process')
    $startedProcess = Start-Process -FilePath $resolvedExecutable -PassThru
    try {
        $inputIdleReached = $startedProcess.WaitForInputIdle(5000)
    } catch [System.InvalidOperationException] {
        $inputIdleReached = $false
    }
    $startupStopwatch.Stop()

    if ($AutomatedSwitchCount -gt 0) {
        for ($attempt = 0; $attempt -lt 100 -and $switcherWindow -eq [IntPtr]::Zero; $attempt++) {
            $switcherWindow = [WindowSwitcher.NativeMethods]::FindMessageWindow($startedProcess.Id)
            if ($switcherWindow -eq [IntPtr]::Zero) {
                Start-Sleep -Milliseconds 50
            }
        }
        if ($switcherWindow -eq [IntPtr]::Zero) {
            throw 'Window Switcher message window was not found'
        }
        Write-Host "采样已开始。脚本将自动执行 $AutomatedSwitchCount 次应用切换消息。"
    } else {
        Write-Host '采样已开始。请在采样期间触发 Alt+Tab 和 Alt+`，覆盖首次显示与连续切换。'
    }

    $samplingStopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    while ($samplingStopwatch.Elapsed.TotalSeconds -lt $DurationSeconds) {
        if ($startedProcess.HasExited) {
            throw "window-switcher exited unexpectedly with code $($startedProcess.ExitCode)"
        }

        $nextSwitchMilliseconds = if ($AutomatedSwitchCount -gt 0) {
            $DurationSeconds * 1000.0 * $switchMessagesSent / $AutomatedSwitchCount
        } else {
            [double]::PositiveInfinity
        }
        if ($switchMessagesSent -lt $AutomatedSwitchCount -and
            $samplingStopwatch.Elapsed.TotalMilliseconds -ge $nextSwitchMilliseconds) {
            $posted = [WindowSwitcher.NativeMethods]::PostMessage(
                $switcherWindow,
                $switchAppsMessage,
                [IntPtr]::Zero,
                [IntPtr]::Zero
            )
            if (-not $posted) {
                throw "Failed to post automated switch message $switchMessagesSent"
            }
            $switchMessagesSent++
        }

        $startedProcess.Refresh()
        $samples.Add([pscustomobject]@{
            TimestampUtc = [DateTime]::UtcNow.ToString('O')
            ElapsedMilliseconds = [math]::Round($samplingStopwatch.Elapsed.TotalMilliseconds, 3)
            WorkingSetBytes = $startedProcess.WorkingSet64
            PrivateMemoryBytes = $startedProcess.PrivateMemorySize64
            CpuSeconds = [math]::Round($startedProcess.TotalProcessorTime.TotalSeconds, 6)
            HandleCount = $startedProcess.HandleCount
            GdiObjects = [WindowSwitcher.NativeMethods]::GetGuiResources($startedProcess.Handle, 0)
            UserObjects = [WindowSwitcher.NativeMethods]::GetGuiResources($startedProcess.Handle, 1)
            ThreadCount = $startedProcess.Threads.Count
            Responding = $startedProcess.Responding
        })
        Start-Sleep -Milliseconds $SampleIntervalMilliseconds
    }

    if ($switcherWindow -ne [IntPtr]::Zero) {
        $null = [WindowSwitcher.NativeMethods]::PostMessage(
            $switcherWindow,
            $cancelSwitchAppsMessage,
            [IntPtr]::Zero,
            [IntPtr]::Zero
        )
        Start-Sleep -Milliseconds ([math]::Max(250, $SampleIntervalMilliseconds))
        if (-not $startedProcess.HasExited) {
            $startedProcess.Refresh()
            $postCancelCounters = [pscustomobject]@{
                WorkingSetBytes = $startedProcess.WorkingSet64
                PrivateMemoryBytes = $startedProcess.PrivateMemorySize64
                HandleCount = $startedProcess.HandleCount
                GdiObjects = [WindowSwitcher.NativeMethods]::GetGuiResources($startedProcess.Handle, 0)
                UserObjects = [WindowSwitcher.NativeMethods]::GetGuiResources($startedProcess.Handle, 1)
                ThreadCount = $startedProcess.Threads.Count
            }
        }
    }

    $samples | Export-Csv -LiteralPath $counterPath -NoTypeInformation -Encoding UTF8
    $summary = [pscustomobject]@{
        ExecutablePath = $resolvedExecutable
        ExecutableBytes = (Get-Item -LiteralPath $resolvedExecutable).Length
        Sha256 = (Get-FileHash -LiteralPath $resolvedExecutable -Algorithm SHA256).Hash
        InputIdleReached = $inputIdleReached
        InputIdleMilliseconds = [math]::Round($startupStopwatch.Elapsed.TotalMilliseconds, 3)
        DurationSeconds = $DurationSeconds
        SampleIntervalMilliseconds = $SampleIntervalMilliseconds
        SampleCount = $samples.Count
        AutomatedSwitchMessages = $switchMessagesSent
        PeakWorkingSetBytes = ($samples | Measure-Object -Property WorkingSetBytes -Maximum).Maximum
        PeakPrivateMemoryBytes = ($samples | Measure-Object -Property PrivateMemoryBytes -Maximum).Maximum
        PeakHandleCount = ($samples | Measure-Object -Property HandleCount -Maximum).Maximum
        PeakGdiObjects = ($samples | Measure-Object -Property GdiObjects -Maximum).Maximum
        PeakUserObjects = ($samples | Measure-Object -Property UserObjects -Maximum).Maximum
        PostCancelCounters = $postCancelCounters
        CounterPath = $counterPath
        MetricsLogPath = $metricsLogPath
    }
    $summary | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $summaryPath -Encoding UTF8
    $summary | Format-List
} finally {
    if ($null -ne $startedProcess -and -not $startedProcess.HasExited) {
        Stop-Process -Id $startedProcess.Id -Force -ErrorAction SilentlyContinue
        $startedProcess.WaitForExit()
    }
    [Environment]::SetEnvironmentVariable(
        'WINDOW_SWITCHER_PERF',
        $previousPerfValue,
        'Process'
    )
    if ($configExisted) {
        [System.IO.File]::WriteAllBytes($configPath, $originalConfig)
    } else {
        Remove-Item -LiteralPath $configPath -Force -ErrorAction SilentlyContinue
    }
}
