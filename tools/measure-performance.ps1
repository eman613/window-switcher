[CmdletBinding()]
param(
    [string] $ExecutablePath = (Join-Path $PSScriptRoot '../target/release/window-switcher.exe'),
    [ValidateRange(1, 3600)][int] $DurationSeconds = 30,
    [ValidateRange(50, 10000)][int] $SampleIntervalMilliseconds = 250,
    [string] $OutputDirectory = (Join-Path $PSScriptRoot '../target/release/performance'),
    [bool] $StopExisting = $true,
    [string] $ResumeExecutablePath,
    [Alias('AutomatedSwitchCount')][ValidateRange(0, 10000)][int] $AutomatedCycleCount = 0,
    [ValidateRange(0, 10000)][int] $WarmupCycles = 200,
    [ValidateRange(0, 5000)][int] $VisibleMilliseconds = 20,
    [ValidateRange(0, 5000)][int] $HiddenMilliseconds = 20,
    [ValidateRange(1, 10000)][int] $StepTimeoutMilliseconds = 2000,
    [ValidateRange(1, 3600)][int] $MaxRunSeconds = 3600,
    [ValidateRange(1, 10000)][int] $CheckpointEvery = 100,
    [ValidateRange(0, 60)][int] $CooldownSeconds = 10,
    [bool] $EnableStageTiming = $true,
    [ValidateSet('none', 'alpha', 'blur', 'acrylic', 'mica', 'auto')][string] $Backdrop = 'none',
    [ValidateRange(0, 100)][int] $BackgroundOpacity = 100,
    [ValidatePattern('^(?:auto|#[0-9A-Fa-f]{6})$')][string] $BackgroundColor = 'auto',
    [ValidateSet('alpha', 'solid')][string] $BackdropFallback = 'alpha'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'Performance sampling requires Windows.' }

$helperDirectory = Join-Path $PSScriptRoot 'performance'
$nativeSources = @('CycleContracts.cs', 'WindowProbe.cs', 'CycleRunner.cs') |
    ForEach-Object { Join-Path $helperDirectory $_ }
. (Join-Path $helperDirectory 'LoadHelpers.ps1')
$helperIdentity = Import-WindowSwitcherPerformanceHelpers -SourcePaths $nativeSources
. (Join-Path $helperDirectory 'Session.ps1')
. (Join-Path $helperDirectory 'Report.ps1')

$session = New-WindowSwitcherPerformanceSession -ExecutablePath $ExecutablePath -OutputDirectory $OutputDirectory
$runner = $null
$runError = $null
$summary = $null
$samples = [Collections.Generic.List[object]]::new()
$settings = [ordered]@{
    AutomatedCycleCount = $AutomatedCycleCount
    WarmupCycles = if ($AutomatedCycleCount -gt 0) { $WarmupCycles } else { 0 }
    DurationSeconds = if ($AutomatedCycleCount -eq 0) { $DurationSeconds } else { $null }
    MaxRunSeconds = $MaxRunSeconds
    SampleIntervalMilliseconds = $SampleIntervalMilliseconds
    VisibleMilliseconds = $VisibleMilliseconds
    HiddenMilliseconds = $HiddenMilliseconds
    StepTimeoutMilliseconds = $StepTimeoutMilliseconds
    CheckpointEvery = $CheckpointEvery
    CooldownSeconds = $CooldownSeconds
    EnableStageTiming = $EnableStageTiming
    Backdrop = $Backdrop
    BackgroundOpacity = $BackgroundOpacity
    BackgroundColor = $BackgroundColor
    BackdropFallback = $BackdropFallback
    WindowSet = 'Current interactive desktop; application windows are not pinned'
    Confirmation = 'Synchronous message return, valid owner, visible nonempty window, then hidden window'
    PowerShellVersion = $PSVersionTable.PSVersion.ToString()
    OperatingSystem = [Environment]::OSVersion.VersionString
    ProcessorCount = [Environment]::ProcessorCount
    RunnerArchitecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    HelperSourceSha256 = $helperIdentity.SourceSha256
    HelperSourceEntries = $helperIdentity.SourceEntries
    HelperAssemblySha256 = $helperIdentity.AssemblySha256
    HelperAssemblyFullName = $helperIdentity.AssemblyFullName
    HelperModuleVersionId = $helperIdentity.ModuleVersionId
    HarnessElevated = ([Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    Dpi = $null
    TargetProcessIdentity = $null
    SampleDropCount = 0
}
try {
    $startParameters = @{
        Session = $session; StopExisting = $StopExisting; ResumeExecutablePath = $ResumeExecutablePath
        Automated = ($AutomatedCycleCount -gt 0); EnableStageTiming = $EnableStageTiming
        Backdrop = $Backdrop; BackgroundOpacity = $BackgroundOpacity
        BackgroundColor = $BackgroundColor; BackdropFallback = $BackdropFallback
    }
    Start-WindowSwitcherPerformanceSession @startParameters
    $settings.Dpi = $session.Probe.Dpi
    $settings.TargetProcessIdentity = $session.ProcessIdentity
    $cancellationPath = Join-Path $session.RunDirectory 'cancel.request'
    Write-Host "Run directory: $($session.RunDirectory)"
    Write-Host "Cancellation file: $cancellationPath"

    if ($AutomatedCycleCount -gt 0) {
        $options = [WindowSwitcher.Performance.CycleOptions]::new()
        $options.RequestedCycles = $AutomatedCycleCount
        $options.WarmupCycles = $WarmupCycles
        $options.VisibleMilliseconds = $VisibleMilliseconds
        $options.HiddenMilliseconds = $HiddenMilliseconds
        $options.StepTimeoutMilliseconds = $StepTimeoutMilliseconds
        $options.MaxRunMilliseconds = $MaxRunSeconds * 1000
        $options.CheckpointEvery = $CheckpointEvery
        $options.CooldownMilliseconds = $CooldownSeconds * 1000
        $runner = [WindowSwitcher.Performance.CycleRunner]::new($session.Probe, $options)
        $runner.Start()
        Write-Host "Requested complete cycles: $AutomatedCycleCount; warmup: $WarmupCycles"
    } else {
        Write-Host 'Manual sampling started; use the configured application/window hotkeys.'
    }

    $sampling = [Diagnostics.Stopwatch]::StartNew()
    $nextSampleAt = 0.0
    $nextProgressAt = 0.0
    $sampleSequence = 0L
    $droppedSamples = 0L
    do {
        if (Test-Path -LiteralPath $cancellationPath) {
            if ($null -ne $runner) { $runner.Cancel() }
            throw 'Performance sampling was cancelled by request.'
        }
        if ($sampling.Elapsed.TotalMilliseconds -ge $nextSampleAt) {
            $sampleElapsed = $sampling.Elapsed.TotalMilliseconds
            if ($sampleSequence -gt 0 -and $sampleElapsed -ge ($nextSampleAt + $SampleIntervalMilliseconds)) {
                $droppedSamples += [long][math]::Floor(
                    ($sampleElapsed - $nextSampleAt) / $SampleIntervalMilliseconds
                )
            }
            $sample = $session.Probe.Capture()
            $sampleSequence++
            $sample.SampleSequence = $sampleSequence
            $sample.DroppedSamples = $droppedSamples
            $sample.ElapsedMilliseconds = $sampleElapsed
            $sample.Phase = if ($null -ne $runner) { $runner.Phase } else { 'Manual' }
            $sample.CompletedCycles = if ($null -ne $runner) { $runner.CompletedCycles } else { 0 }
            $samples.Add($sample)
            if ($null -ne $runner) { $runner.ValidateResources($sample) }
            $nextSampleAt = $sampleElapsed + $SampleIntervalMilliseconds
            if ($sampleElapsed -ge $nextProgressAt) {
                $progress = [pscustomobject]@{
                    Phase = $sample.Phase; CompletedCycles = $sample.CompletedCycles
                    RequestedCycles = $AutomatedCycleCount
                    WarmupCompleted = if ($null -ne $runner) { $runner.WarmupCompleted } else { 0 }
                    ElapsedSeconds = [math]::Round($sampleElapsed / 1000, 1)
                    PrivateMiB = [math]::Round($sample.PrivateMemoryBytes / 1MB, 2)
                    GdiObjects = $sample.GdiObjects; UserObjects = $sample.UserObjects
                    HandleCount = $sample.HandleCount; ThreadCount = $sample.ThreadCount
                }
                $progress | ConvertTo-Json -Compress |
                    Set-Content -LiteralPath (Join-Path $session.RunDirectory 'progress.json') -Encoding UTF8
                Write-Host ($progress | ConvertTo-Json -Compress)
                $nextProgressAt = $sampleElapsed + 30000
            }
        }
        Start-Sleep -Milliseconds 20
        $continueSampling = if ($null -ne $runner) { -not $runner.Completion.IsCompleted }
            else { $sampling.Elapsed.TotalSeconds -lt $DurationSeconds }
    } while ($continueSampling)

    if ($null -ne $runner) {
        $runner.Completion.GetAwaiter().GetResult()
        $expectedTotal = $AutomatedCycleCount + $WarmupCycles
        if ($runner.Phase -cne 'Completed' -or $runner.CompletedCycles -ne $AutomatedCycleCount -or
            $runner.OpenRequested -ne $expectedTotal -or $runner.CloseRequested -ne $expectedTotal -or
            $runner.WarmupCompleted -ne $WarmupCycles -or $runner.OpenedVerified -ne $expectedTotal -or
            $runner.ClosedVerified -ne $expectedTotal -or $runner.OpenAcknowledged -ne $expectedTotal -or
            $runner.CloseAcknowledged -ne $expectedTotal -or $runner.CleanupError) {
            throw "Incomplete cycle run: phase=$($runner.Phase), completed=$($runner.CompletedCycles), error=$($runner.Error), cleanup=$($runner.CleanupError)"
        }
    } elseif ($session.Probe.IsVisible) {
        $session.Probe.Close($StepTimeoutMilliseconds)
    }
    $settings.SampleDropCount = $droppedSamples
} catch {
    $runError = $_.Exception.Message
} finally {
    if ($null -ne $runner) {
        try { $runner.Dispose() }
        catch { $session.CleanupErrors.Add("Cycle worker cleanup: $($_.Exception.Message)") }
    }
    try { Close-WindowSwitcherPerformanceSession -Session $session }
    catch { $session.CleanupErrors.Add("Session cleanup: $($_.Exception.Message)") }
    if (Test-Path -LiteralPath $session.RunDirectory) {
        $summary = Write-WindowSwitcherPerformanceReport -Session $session -Settings $settings -Runner $runner -Samples $samples.ToArray() -RunError $runError
    }
}
if ($null -ne $summary) {
    $summary | Select-Object Status, RequestedCycles, CompletedCycles, SampleCount, CheckpointCount,
        ApplicationWarningCount, ApplicationErrorCount, OriginalConfigurationsPreserved, IsolatedRuntimeRemoved | Format-List
    Write-Host "Summary: $(Join-Path $session.RunDirectory 'summary.json')"
}
if ($runError -or $session.CleanupErrors.Count -gt 0 -or ($null -ne $summary -and $summary.Status -ne 'Completed')) {
    throw "Performance validation failed. $runError $($session.CleanupErrors -join '; ')"
}
