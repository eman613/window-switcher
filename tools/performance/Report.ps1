function Get-WindowSwitcherSampleMedian {
    param([AllowEmptyCollection()][double[]] $Values)

    if ($Values.Count -eq 0) { return $null }
    $ordered = @($Values | Sort-Object)
    $middle = [int][math]::Floor($ordered.Count / 2)
    if ($ordered.Count % 2 -eq 1) { return $ordered[$middle] }
    ($ordered[$middle - 1] + $ordered[$middle]) / 2
}

function Get-WindowSwitcherResourceTrend {
    param([AllowEmptyCollection()][object[]] $Samples, [AllowEmptyCollection()][object[]] $Checkpoints)

    $baseline = $Checkpoints | Where-Object { $_.Phase -eq 'Baseline' } | Select-Object -First 1
    $settled = $Checkpoints | Where-Object { $_.Phase -eq 'Settled' } | Select-Object -Last 1
    if ($null -eq $baseline) { $baseline = $Samples | Select-Object -First 1 }
    if ($null -eq $settled) { $settled = $Samples | Select-Object -Last 1 }
    if ($null -eq $baseline -or $null -eq $settled) { return @() }
    $closed = @($Checkpoints | Where-Object { $_.Phase -eq 'Closed' } | Sort-Object CompletedCycles)
    $windowCount = [int][math]::Min(10, [math]::Floor($closed.Count / 2))
    $fields = @('PrivateMemoryBytes', 'WorkingSetBytes', 'HandleCount', 'GdiObjects', 'UserObjects', 'ThreadCount')
    foreach ($field in $fields) {
        $firstMedian = $null
        $lastMedian = $null
        $slope = $null
        if ($windowCount -gt 0) {
            $firstMedian = Get-WindowSwitcherSampleMedian -Values @($closed | Select-Object -First $windowCount | ForEach-Object { [double] $_.$field })
            $lastMedian = Get-WindowSwitcherSampleMedian -Values @($closed | Select-Object -Last $windowCount | ForEach-Object { [double] $_.$field })
        }
        if ($closed.Count -gt 1) {
            $meanX = ($closed | Measure-Object CompletedCycles -Average).Average
            $meanY = ($closed | Measure-Object $field -Average).Average
            $numerator = 0.0
            $denominator = 0.0
            foreach ($point in $closed) {
                $offset = $point.CompletedCycles - $meanX
                $numerator += $offset * ([double] $point.$field - $meanY)
                $denominator += $offset * $offset
            }
            if ($denominator -gt 0) { $slope = [math]::Round(1000 * $numerator / $denominator, 3) }
        }
        [pscustomobject]@{
            Metric = $field
            Baseline = $baseline.$field
            Settled = $settled.$field
            Delta = [long] $settled.$field - [long] $baseline.$field
            Peak = ($Samples | Measure-Object $field -Maximum).Maximum
            FirstClosedMedian = $firstMedian
            LastClosedMedian = $lastMedian
            ClosedMedianDelta = if ($null -ne $firstMedian) { $lastMedian - $firstMedian } else { $null }
            SlopePer1000Cycles = $slope
        }
    }
}

function Get-WindowSwitcherResourceTrendStatus {
    param(
        [AllowEmptyCollection()][object[]] $Trend,
        [long] $DroppedSamples = 0
    )

    $limits = [ordered]@{
        PrivateMemoryBytes = 4MB
        WorkingSetBytes = 8MB
        HandleCount = 32
        GdiObjects = 16
        UserObjects = 16
        ThreadCount = 4
    }
    if ($DroppedSamples -gt 0) {
        return [pscustomobject]@{
            Status = 'Incomplete'
            Failures = @("$DroppedSamples resource samples were dropped.")
            Limits = $limits
        }
    }
    if ($Trend.Count -eq 0) {
        return [pscustomobject]@{
            Status = 'Incomplete'
            Failures = @('No resource trend checkpoints were captured.')
            Limits = $limits
        }
    }

    $failures = @()
    foreach ($point in $Trend) {
        if (-not $limits.Contains($point.Metric)) {
            $failures += "Unknown resource trend metric: $($point.Metric)"
            continue
        }
        if ($null -eq $point.ClosedMedianDelta -or $null -eq $point.SlopePer1000Cycles) {
            $failures += "Resource trend is incomplete for $($point.Metric)."
            continue
        }
        $medianDelta = [double]$point.ClosedMedianDelta
        $slope = [double]$point.SlopePer1000Cycles
        if ([double]::IsNaN($medianDelta) -or [double]::IsInfinity($medianDelta) -or
            [double]::IsNaN($slope) -or [double]::IsInfinity($slope)) {
            $failures += "Resource trend contains a non-finite value for $($point.Metric)."
            continue
        }
        $limit = [double]$limits[$point.Metric]
        if ($medianDelta -gt $limit) {
            $failures += "$($point.Metric) closed median growth $medianDelta exceeds $limit."
        }
        if ($slope -gt $limit) {
            $failures += "$($point.Metric) slope $slope per 1000 cycles exceeds $limit."
        }
    }
    [pscustomobject]@{
        Status = if ($failures.Count -eq 0) { 'Passed' } else { 'Failed' }
        Failures = $failures
        Limits = $limits
    }
}

function Get-WindowSwitcherApplicationLogDiagnostics {
    param([Parameter(Mandatory)][string] $Path)

    $present = Test-Path -LiteralPath $Path -PathType Leaf
    if (-not $present) {
        return [pscustomobject]@{
            Present = $false
            LineCount = 0
            Warnings = @()
            Errors = @()
            MetricDropCount = 0
            KeyboardMetrics = Get-WindowSwitcherKeyboardMetrics -Lines @()
        }
    }
    $lines = @(Get-Content -LiteralPath $Path -Encoding UTF8)
    $levelPrefix = '^\[[^\]]+\]\s+\([^)]+\)\s+'
    $warnings = @($lines | Where-Object { $_ -cmatch ($levelPrefix + 'WARN\b') })
    $errors = @($lines | Where-Object { $_ -cmatch ($levelPrefix + 'ERROR\b') })
    $metricDropCount = 0L
    foreach ($line in $lines) {
        $match = [regex]::Match($line, '\bdropped_metrics=(?<count>\d+)\b')
        if ($match.Success) {
            $metricDropCount = [long]$match.Groups['count'].Value
        }
    }
    [pscustomobject]@{
        Present = $true
        LineCount = $lines.Count
        Warnings = $warnings
        Errors = $errors
        MetricDropCount = $metricDropCount
        KeyboardMetrics = Get-WindowSwitcherKeyboardMetrics -Lines $lines
    }
}

function Get-WindowSwitcherKeyboardMetrics {
    param([AllowEmptyCollection()][string[]] $Lines)

    $requiredStages = @(
        'keyboard_hook', 'keyboard_enqueue', 'keyboard_queue_wait',
        'keyboard_ui_dispatch', 'keyboard_end_to_end'
    )
    $stageSamples = @{}
    foreach ($stage in $requiredStages) { $stageSamples[$stage] = [Collections.Generic.List[double]]::new() }
    $sequences = [Collections.Generic.HashSet[long]]::new()
    $pattern = '\bperf stage=(?<stage>keyboard_[a-z_]+)\s+sequence=(?<sequence>\d+)\s+elapsed_us=(?<elapsed>\d+)\b'
    foreach ($line in $Lines) {
        $match = [regex]::Match($line, $pattern)
        if (-not $match.Success) { continue }
        $stage = $match.Groups['stage'].Value
        if (-not $stageSamples.ContainsKey($stage)) { continue }
        $sequence = [long]$match.Groups['sequence'].Value
        $elapsed = [double]$match.Groups['elapsed'].Value
        $null = $stageSamples[$stage].Add($elapsed)
        $null = $sequences.Add($sequence)
    }
    $p99 = $null
    $endToEnd = @($stageSamples['keyboard_end_to_end'])
    if ($endToEnd.Count -gt 0) {
        $ordered = @($endToEnd | Sort-Object)
        $index = [int][math]::Ceiling($ordered.Count * 0.99) - 1
        $p99 = $ordered[[math]::Max(0, $index)]
    }
    $missingStages = @($requiredStages | Where-Object { $stageSamples[$_].Count -eq 0 })
    [pscustomobject]@{
        Measured = $stageSamples['keyboard_hook'].Count -gt 0
        SequenceCount = $sequences.Count
        StageCounts = [ordered]@{
            keyboard_hook = $stageSamples['keyboard_hook'].Count
            keyboard_enqueue = $stageSamples['keyboard_enqueue'].Count
            keyboard_queue_wait = $stageSamples['keyboard_queue_wait'].Count
            keyboard_ui_dispatch = $stageSamples['keyboard_ui_dispatch'].Count
            keyboard_end_to_end = $stageSamples['keyboard_end_to_end'].Count
        }
        MissingStages = $missingStages
        EndToEndP99Microseconds = $p99
    }
}

function Write-WindowSwitcherPerformanceReport {
    param(
        [object] $Session, [object] $Settings, [object] $Runner,
        [AllowEmptyCollection()][object[]] $Samples, [string] $RunError
    )

    $checkpoints = if ($null -ne $Runner) { @($Runner.Checkpoints) } else { @() }
    $completed = if ($null -ne $Runner) { $Runner.CompletedCycles } else { 0 }
    $logDiagnostics = Get-WindowSwitcherApplicationLogDiagnostics -Path $Session.LogPath
    $warnings = @($logDiagnostics.Warnings)
    $errors = @($logDiagnostics.Errors)
    $logPresent = $logDiagnostics.Present
    $logLineCount = $logDiagnostics.LineCount
    $metricDropCount = $logDiagnostics.MetricDropCount
    $keyboardMetrics = $logDiagnostics.KeyboardMetrics
    $droppedSamples = if ($Samples.Count -gt 0) {
        [long](($Samples | Measure-Object DroppedSamples -Maximum).Maximum)
    } else { 0 }
    $trend = @(Get-WindowSwitcherResourceTrend -Samples $Samples -Checkpoints $checkpoints)
    $trendStatus = if ($null -eq $Runner) {
        [pscustomobject]@{ Status = 'NotRun'; Failures = @(); Limits = [ordered]@{} }
    } else {
        Get-WindowSwitcherResourceTrendStatus -Trend $trend -DroppedSamples ($droppedSamples + $metricDropCount)
    }
    $cycleStatus = if ($null -eq $Runner) { 'NotRun' }
        elseif ($Runner.Phase -ceq 'Completed') { 'Passed' }
        else { 'Failed' }
    $diagnosticFailures = @()
    if (-not $logPresent -or $logLineCount -eq 0) {
        $diagnosticFailures += "Application log is missing or empty: $($Session.LogPath)"
    }
    if ($errors.Count -gt 0) { $diagnosticFailures += "$($errors.Count) application error log entries were found." }
    if ($metricDropCount -gt 0) { $diagnosticFailures += "$metricDropCount performance metric samples were dropped." }
    $diagnosticsStatus = if ($diagnosticFailures.Count -eq 0) { 'Passed' } else { 'Failed' }
    $status = if ($RunError -or $Session.CleanupErrors.Count -gt 0 -or
        $cycleStatus -eq 'Failed' -or $trendStatus.Status -ne 'Passed' -and $null -ne $Runner -or
        $diagnosticsStatus -eq 'Failed') { 'Failed' } else { 'Completed' }
    $counterPath = Join-Path $Session.RunDirectory 'resources.csv'
    $checkpointPath = Join-Path $Session.RunDirectory 'closed-checkpoints.csv'
    if ($Samples.Count -gt 0) { $Samples | Export-Csv -LiteralPath $counterPath -NoTypeInformation -Encoding UTF8 }
    if ($checkpoints.Count -gt 0) { $checkpoints | Export-Csv -LiteralPath $checkpointPath -NoTypeInformation -Encoding UTF8 }
    $summary = [pscustomobject]@{
        Status = $status
        CycleStatus = $cycleStatus
        ResourceTrendStatus = $trendStatus.Status
        ResourceTrendFailures = @($trendStatus.Failures)
        DiagnosticsStatus = $diagnosticsStatus
        DiagnosticsFailures = $diagnosticFailures
        SourceExecutable = $Session.SourcePath
        ExecutableSha256 = $Session.SourceSha256
        ProductVersion = $Session.ProductVersion
        Settings = $Settings
        RequestedCycles = $Settings.AutomatedCycleCount
        WarmupCompleted = if ($null -ne $Runner) { $Runner.WarmupCompleted } else { 0 }
        CompletedCycles = $completed
        OpenRequested = if ($null -ne $Runner) { $Runner.OpenRequested } else { 0 }
        CloseRequested = if ($null -ne $Runner) { $Runner.CloseRequested } else { 0 }
        OpenAcknowledged = if ($null -ne $Runner) { $Runner.OpenAcknowledged } else { 0 }
        CloseAcknowledged = if ($null -ne $Runner) { $Runner.CloseAcknowledged } else { 0 }
        OpenedVerified = if ($null -ne $Runner) { $Runner.OpenedVerified } else { 0 }
        ClosedVerified = if ($null -ne $Runner) { $Runner.ClosedVerified } else { 0 }
        SampleCount = $Samples.Count
        DroppedSampleCount = $droppedSamples
        PerformanceMetricDropCount = $metricDropCount
        CheckpointCount = $checkpoints.Count
        InitializedWindowReadyMilliseconds = $Session.InputIdleMilliseconds
        ElapsedMilliseconds = if ($null -ne $Runner) { $Runner.ElapsedMilliseconds } elseif ($Samples.Count -gt 0) { $Samples[-1].ElapsedMilliseconds } else { 0 }
        ResourceTrend = $trend
        ResourceTrendLimits = $trendStatus.Limits
        ApplicationLogPresent = $logPresent
        ApplicationLogLineCount = $logLineCount
        ApplicationWarningCount = $warnings.Count
        ApplicationErrorCount = $errors.Count
        ApplicationDiagnostics = @($errors + $warnings | Select-Object -First 20)
        Error = $RunError
        RunnerError = if ($null -ne $Runner) { $Runner.Error } else { $null }
        RunnerCleanupError = if ($null -ne $Runner) { $Runner.CleanupError } else { $null }
        CleanupErrors = @($Session.CleanupErrors.ToArray())
        OriginalConfigurationsPreserved = $Session.OriginalConfigurationsPreserved
        Originals = @($Session.Originals.ToArray())
        IsolatedRuntimeRemoved = $Session.RuntimeRemoved
        KeyboardHookMeasured = $keyboardMetrics.Measured
        KeyboardMetrics = $keyboardMetrics
        PhysicalFrameLatencyMeasured = $false
        CounterPath = $counterPath
        CheckpointPath = $checkpointPath
        MetricsLogPath = $Session.LogPath
    }
    $summary | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath (Join-Path $Session.RunDirectory 'summary.json') -Encoding UTF8
    $summary
}
