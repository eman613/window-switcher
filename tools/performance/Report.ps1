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

function Write-WindowSwitcherPerformanceReport {
    param(
        [object] $Session, [object] $Settings, [object] $Runner,
        [AllowEmptyCollection()][object[]] $Samples, [string] $RunError
    )

    $checkpoints = if ($null -ne $Runner) { @($Runner.Checkpoints) } else { @() }
    $completed = if ($null -ne $Runner) { $Runner.CompletedCycles } else { 0 }
    $status = if ($RunError -or $Session.CleanupErrors.Count -gt 0) { 'Failed' }
        elseif ($null -ne $Runner) { $Runner.Phase }
        else { 'Completed' }
    $warnings = @()
    $errors = @()
    if (Test-Path -LiteralPath $Session.LogPath) {
        $warnings = @(Select-String -LiteralPath $Session.LogPath -Pattern '\bWARN\b' -CaseSensitive | ForEach-Object { $_.Line })
        $errors = @(Select-String -LiteralPath $Session.LogPath -Pattern '\bERROR\b' -CaseSensitive | ForEach-Object { $_.Line })
    }
    if ($errors.Count -gt 0) { $status = 'Failed' }
    $counterPath = Join-Path $Session.RunDirectory 'resources.csv'
    $checkpointPath = Join-Path $Session.RunDirectory 'closed-checkpoints.csv'
    if ($Samples.Count -gt 0) { $Samples | Export-Csv -LiteralPath $counterPath -NoTypeInformation -Encoding UTF8 }
    if ($checkpoints.Count -gt 0) { $checkpoints | Export-Csv -LiteralPath $checkpointPath -NoTypeInformation -Encoding UTF8 }
    $summary = [pscustomobject]@{
        Status = $status
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
        CheckpointCount = $checkpoints.Count
        InitializedWindowReadyMilliseconds = $Session.InputIdleMilliseconds
        ElapsedMilliseconds = if ($null -ne $Runner) { $Runner.ElapsedMilliseconds } elseif ($Samples.Count -gt 0) { $Samples[-1].ElapsedMilliseconds } else { 0 }
        ResourceTrend = @(Get-WindowSwitcherResourceTrend -Samples $Samples -Checkpoints $checkpoints)
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
        KeyboardHookMeasured = $false
        PhysicalFrameLatencyMeasured = $false
        CounterPath = $counterPath
        CheckpointPath = $checkpointPath
        MetricsLogPath = $Session.LogPath
    }
    $summary | ConvertTo-Json -Depth 9 | Set-Content -LiteralPath (Join-Path $Session.RunDirectory 'summary.json') -Encoding UTF8
    $summary
}
