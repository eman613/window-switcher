[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$helperDirectory = Join-Path $PSScriptRoot 'performance'
$sources = @('CycleContracts.cs', 'WindowProbe.cs', 'CycleRunner.cs', 'tests/FakeCycleTarget.cs') |
    ForEach-Object { Join-Path $helperDirectory $_ }
. (Join-Path $helperDirectory 'LoadHelpers.ps1')
$helperIdentity = Import-WindowSwitcherPerformanceHelpers -SourcePaths $sources
. (Join-Path $helperDirectory 'Report.ps1')
$script:performanceTestCount = 0

function Assert-PerformanceTest {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw $Message }
}

function Invoke-PerformanceTest {
    param([string] $Name, [scriptblock] $Body)
    & $Body
    $script:performanceTestCount++
    Write-Host "PASS $Name"
}

function New-PerformanceTestOptions {
    $options = [WindowSwitcher.Performance.CycleOptions]::new()
    $options.RequestedCycles = 3
    $options.WarmupCycles = 0
    $options.VisibleMilliseconds = 0
    $options.HiddenMilliseconds = 0
    $options.StepTimeoutMilliseconds = 30
    $options.MaxRunMilliseconds = 2000
    $options.CheckpointEvery = 2
    $options.CooldownMilliseconds = 0
    $options
}

function Wait-PerformanceTestRunner {
    param([object] $Runner)
    Assert-PerformanceTest ($Runner.Completion.Wait(5000)) 'The worker did not complete within its test deadline.'
}

Invoke-PerformanceTest 'helper assembly is bound to the current source manifest' {
    Assert-PerformanceTest ($helperIdentity.SourceSha256 -match '^[0-9a-f]{64}$') 'Missing helper source hash.'
    Assert-PerformanceTest ($helperIdentity.AssemblySha256 -match '^[0-9a-f]{64}$') 'Missing helper assembly hash.'
    $moduleId = [guid]::Empty
    Assert-PerformanceTest ([guid]::TryParse($helperIdentity.ModuleVersionId, [ref]$moduleId)) 'Missing helper module identity.'
}

Invoke-PerformanceTest 'log diagnostics ignore severity words inside messages' {
    $logPath = [IO.Path]::GetTempFileName()
    try {
        [IO.File]::WriteAllLines($logPath, @(
            '[00:00:00.000] (abc) INFO   configured path contains ERROR and WARN'
            '[00:00:00.001] (abc) WARN   expected warning'
            '[00:00:00.002] (abc) ERROR  expected error'
        ), [Text.UTF8Encoding]::new($false))
        $diagnostics = Get-WindowSwitcherApplicationLogDiagnostics -Path $logPath
        Assert-PerformanceTest ($diagnostics.Warnings.Count -eq 1 -and $diagnostics.Errors.Count -eq 1) 'Log severity parsing matched message text.'
    } finally {
        if (Test-Path -LiteralPath $logPath -PathType Leaf) { [IO.File]::Delete($logPath) }
    }
}

Invoke-PerformanceTest 'keyboard metrics preserve sequence stages and p99' {
    $lines = @(
        '[00:00:00.001] (abc) INFO   perf stage=keyboard_hook sequence=7 elapsed_us=4 dropped_metrics=0',
        '[00:00:00.001] (abc) INFO   perf stage=keyboard_enqueue sequence=7 elapsed_us=2 dropped_metrics=0',
        '[00:00:00.002] (abc) INFO   perf stage=keyboard_queue_wait sequence=7 elapsed_us=15 dropped_metrics=0',
        '[00:00:00.003] (abc) INFO   perf stage=keyboard_ui_dispatch sequence=7 elapsed_us=8 dropped_metrics=0',
        '[00:00:00.004] (abc) INFO   perf stage=keyboard_end_to_end sequence=7 elapsed_us=20 dropped_metrics=0',
        '[00:00:00.005] (abc) INFO   perf stage=keyboard_hook sequence=8 elapsed_us=5 dropped_metrics=0',
        '[00:00:00.005] (abc) INFO   perf stage=keyboard_enqueue sequence=8 elapsed_us=3 dropped_metrics=0',
        '[00:00:00.006] (abc) INFO   perf stage=keyboard_queue_wait sequence=8 elapsed_us=10 dropped_metrics=0',
        '[00:00:00.007] (abc) INFO   perf stage=keyboard_ui_dispatch sequence=8 elapsed_us=9 dropped_metrics=0',
        '[00:00:00.008] (abc) INFO   perf stage=keyboard_end_to_end sequence=8 elapsed_us=30 dropped_metrics=0'
    )
    $metrics = Get-WindowSwitcherKeyboardMetrics -Lines $lines
    Assert-PerformanceTest $metrics.Measured 'Keyboard metrics were not marked as measured.'
    Assert-PerformanceTest ($metrics.SequenceCount -eq 2) 'Keyboard sequence count was not deduplicated.'
    Assert-PerformanceTest ($metrics.MissingStages.Count -eq 0) 'A complete keyboard stage set was marked incomplete.'
    Assert-PerformanceTest ($metrics.StageCounts.keyboard_hook -eq 2 -and
        $metrics.StageCounts.keyboard_end_to_end -eq 2) 'Keyboard stage counts were incorrect.'
    Assert-PerformanceTest ($metrics.EndToEndP99Microseconds -eq 30) 'Keyboard end-to-end P99 was incorrect.'
}

Invoke-PerformanceTest 'manual reports tolerate empty resource checkpoints' {
    $trend = @(Get-WindowSwitcherResourceTrend -Samples @([pscustomobject]@{
        PrivateMemoryBytes = 1; WorkingSetBytes = 1; HandleCount = 1
        GdiObjects = 1; UserObjects = 1; ThreadCount = 1
    }) -Checkpoints @())
    Assert-PerformanceTest ($trend.Count -eq 6 -and $null -eq $trend[0].ClosedMedianDelta) 'Manual resource trend did not tolerate empty checkpoints.'
}

Invoke-PerformanceTest 'complete cycles distinguish warmup, acknowledgements and visibility' {
    $target = [WindowSwitcher.Performance.Tests.FakeCycleTarget]::new()
    $options = New-PerformanceTestOptions
    $options.WarmupCycles = 2
    $runner = [WindowSwitcher.Performance.CycleRunner]::new($target, $options)
    try {
        $runner.Start()
        Wait-PerformanceTestRunner $runner
        Assert-PerformanceTest ($runner.Phase -ceq 'Completed') $runner.Error
        Assert-PerformanceTest ($runner.CompletedCycles -eq 3 -and $runner.WarmupCompleted -eq 2) 'Incorrect measured/warmup counts.'
        Assert-PerformanceTest ($runner.OpenRequested -eq 5 -and $runner.CloseRequested -eq 5) 'Incorrect request counts.'
        Assert-PerformanceTest ($runner.OpenAcknowledged -eq 5 -and $runner.CloseAcknowledged -eq 5) 'Incorrect acknowledgement count.'
        Assert-PerformanceTest ($runner.OpenedVerified -eq 5 -and $runner.ClosedVerified -eq 5) 'Incorrect visibility count.'
        Assert-PerformanceTest ($runner.Checkpoints.Count -eq 4) 'Missing baseline, cycle or settled checkpoints.'
        Assert-PerformanceTest (@($runner.Checkpoints | Where-Object WindowVisible).Count -eq 0) 'A checkpoint was not taken in the hidden state.'
    } finally { $runner.Dispose() }
}

foreach ($ignoredTransition in @('IgnoreOpen', 'IgnoreClose')) {
    Invoke-PerformanceTest "$ignoredTransition acknowledgement cannot count as a completed cycle" {
        $target = [WindowSwitcher.Performance.Tests.FakeCycleTarget]::new()
        $target.$ignoredTransition = $true
        $runner = [WindowSwitcher.Performance.CycleRunner]::new($target, (New-PerformanceTestOptions))
        try {
            $runner.Start()
            Wait-PerformanceTestRunner $runner
            Assert-PerformanceTest ($runner.Phase -ceq 'Failed' -and $runner.CompletedCycles -eq 0) 'Unconfirmed visibility was counted as completion.'
            Assert-PerformanceTest ($runner.Error.Contains('visibility')) 'Missing transition timeout diagnosis.'
        } finally { $runner.Dispose() }
    }
}

Invoke-PerformanceTest 'a failed later message preserves only previously completed cycles' {
    $target = [WindowSwitcher.Performance.Tests.FakeCycleTarget]::new()
    $target.FailOnOpen = 2
    $runner = [WindowSwitcher.Performance.CycleRunner]::new($target, (New-PerformanceTestOptions))
    try {
        $runner.Start()
        Wait-PerformanceTestRunner $runner
        Assert-PerformanceTest ($runner.Phase -ceq 'Failed' -and $runner.CompletedCycles -eq 1) 'Failure inflated the completion count.'
        Assert-PerformanceTest ($runner.OpenRequested -eq 2 -and $runner.CloseRequested -eq 1) 'The failed message attempt was not recorded.'
        Assert-PerformanceTest ($runner.OpenAcknowledged -eq 1 -and $runner.CloseAcknowledged -eq 1) 'Failure inflated acknowledgement counts.'
    } finally { $runner.Dispose() }
}

Invoke-PerformanceTest 'cancellation interrupts the hold and cleans the visible panel' {
    $target = [WindowSwitcher.Performance.Tests.FakeCycleTarget]::new()
    $options = New-PerformanceTestOptions
    $options.VisibleMilliseconds = 1000
    $runner = [WindowSwitcher.Performance.CycleRunner]::new($target, $options)
    try {
        $runner.Start()
        $wait = [Diagnostics.Stopwatch]::StartNew()
        while ($runner.OpenedVerified -eq 0 -and $wait.ElapsedMilliseconds -lt 1000) { Start-Sleep -Milliseconds 5 }
        Assert-PerformanceTest ($runner.OpenedVerified -eq 1) 'The cancellation test never reached its visible state.'
        $runner.Cancel()
        Wait-PerformanceTestRunner $runner
        Assert-PerformanceTest ($runner.Phase -ceq 'Cancelled' -and $runner.CompletedCycles -eq 0) 'Cancellation was reported as completion.'
        Assert-PerformanceTest (-not $target.IsVisible) 'Cancellation left the panel visible.'
    } finally { $runner.Dispose() }
}

Invoke-PerformanceTest 'the total time budget bounds a long visibility hold' {
    $options = New-PerformanceTestOptions
    $options.MaxRunMilliseconds = 40
    $options.VisibleMilliseconds = 1000
    $runner = [WindowSwitcher.Performance.CycleRunner]::new([WindowSwitcher.Performance.Tests.FakeCycleTarget]::new(), $options)
    try {
        $runner.Start()
        Wait-PerformanceTestRunner $runner
        Assert-PerformanceTest ($runner.Phase -ceq 'Failed' -and $runner.CompletedCycles -eq 0) 'The total deadline did not stop the cycle.'
        Assert-PerformanceTest ($runner.Error.Contains('time limit')) 'Missing total-deadline diagnosis.'
    } finally { $runner.Dispose() }
}

Invoke-PerformanceTest 'resource safety limits stop the run before measuring cycles' {
    $target = [WindowSwitcher.Performance.Tests.FakeCycleTarget]::new()
    $target.PrivateBytes = 2GB
    $runner = [WindowSwitcher.Performance.CycleRunner]::new($target, (New-PerformanceTestOptions))
    try {
        $runner.Start()
        Wait-PerformanceTestRunner $runner
        Assert-PerformanceTest ($runner.Phase -ceq 'Failed' -and $runner.CompletedCycles -eq 0) 'The memory safety limit was ignored.'
        Assert-PerformanceTest ($runner.Error.Contains('resource safety limit')) 'Missing resource-limit diagnosis.'
    } finally { $runner.Dispose() }
}

Invoke-PerformanceTest 'the native adapter rejects an invalid window' {
    $process = [Diagnostics.Process]::GetCurrentProcess()
    $rejected = $false
    try {
        $identity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($process)
        $sameIdentity = [WindowSwitcher.Performance.ProcessIdentity]::new(
            $identity.ProcessId, $identity.CreationTimeUtcTicks, $identity.ImagePath
        )
        $differentIdentity = [WindowSwitcher.Performance.ProcessIdentity]::new(
            $identity.ProcessId, $identity.CreationTimeUtcTicks + 1, $identity.ImagePath
        )
        Assert-PerformanceTest ($identity.Matches($sameIdentity)) 'Stable process identity did not compare equal.'
        Assert-PerformanceTest (-not $identity.Matches($differentIdentity)) 'Creation identity changes were ignored.'
        try { $null = [WindowSwitcher.Performance.WindowProbe]::new($process, [IntPtr]::Zero) }
        catch { $rejected = $true }
        Assert-PerformanceTest $rejected 'An invalid window was accepted.'
    } finally { $process.Dispose() }
}

Invoke-PerformanceTest 'closed-state trends use actual completed cycles' {
    $target = [WindowSwitcher.Performance.Tests.FakeCycleTarget]::new()
    $points = @()
    for ($index = 0; $index -le 21; $index++) {
        $point = $target.Capture()
        $point.Phase = if ($index -eq 0) { 'Baseline' } elseif ($index -eq 21) { 'Settled' } else { 'Closed' }
        $point.CompletedCycles = [math]::Min($index, 20) * 100
        $point.GdiObjects = 20 + [math]::Min($index, 20)
        $points += $point
    }
    $trend = @(Get-WindowSwitcherResourceTrend -Samples $points -Checkpoints $points)
    $gdi = $trend | Where-Object Metric -EQ 'GdiObjects'
    $handles = $trend | Where-Object Metric -EQ 'HandleCount'
    Assert-PerformanceTest ($gdi.Delta -eq 20 -and $gdi.ClosedMedianDelta -eq 10 -and $gdi.SlopePer1000Cycles -eq 10) 'The GDI growth calculation is incorrect.'
    Assert-PerformanceTest ($handles.Delta -eq 0 -and $handles.SlopePer1000Cycles -eq 0) 'Stable resources were reported as growing.'
    Assert-PerformanceTest (@(Get-WindowSwitcherResourceTrend -Samples @() -Checkpoints @()).Count -eq 0) 'An empty run produced a false baseline.'
    $gdi.ClosedMedianDelta = 17
    $trendStatus = Get-WindowSwitcherResourceTrendStatus -Trend $trend
    Assert-PerformanceTest ($trendStatus.Status -ceq 'Failed' -and @($trendStatus.Failures | Where-Object { $_ -like 'GdiObjects*' }).Count -gt 0) 'Resource growth was not rejected.'
    $incompleteStatus = Get-WindowSwitcherResourceTrendStatus -Trend $trend -DroppedSamples 1
    Assert-PerformanceTest ($incompleteStatus.Status -ceq 'Incomplete') 'Dropped samples did not invalidate the resource trend.'
}

Write-Host "Performance harness regression tests passed: $script:performanceTestCount"
