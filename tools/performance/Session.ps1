function New-WindowSwitcherPerformanceSession {
    param([string] $ExecutablePath, [string] $OutputDirectory)

    $source = (Resolve-Path -LiteralPath $ExecutablePath -ErrorAction Stop).ProviderPath
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw 'The source executable is not a file.' }
    $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($source)
    if ($version.ProductName -cne 'Window Switcher') { throw 'The source file is not a Window Switcher executable.' }
    $runId = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
    $runDirectory = Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) $runId
    $runtimeDirectory = Join-Path ([IO.Path]::GetTempPath()) ('window-switcher-performance-' + [Guid]::NewGuid().ToString('N'))
    [pscustomobject]@{
        SourcePath = $source
        SourceSha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
        ProductVersion = $version.ProductVersion
        RunDirectory = $runDirectory
        RuntimeDirectory = $runtimeDirectory
        ExecutablePath = Join-Path $runtimeDirectory 'window-switcher.exe'
        ConfigurationPath = Join-Path $runtimeDirectory 'window-switcher.ini'
        LogPath = Join-Path $runDirectory 'application.log'
        Process = $null
        ProcessIdentity = $null
        Probe = $null
        Originals = [Collections.Generic.List[object]]::new()
        CleanupErrors = [Collections.Generic.List[string]]::new()
        InputIdleMilliseconds = $null
        OriginalConfigurationsPreserved = $true
        RuntimeRemoved = $false
    }
}

function Start-WindowSwitcherPerformanceSession {
    param(
        [object] $Session, [bool] $StopExisting, [string] $ResumeExecutablePath,
        [bool] $Automated, [bool] $EnableStageTiming,
        [string] $Backdrop, [int] $BackgroundOpacity, [string] $BackgroundColor,
        [string] $BackdropFallback
    )

    $existing = @(Get-Process -Name window-switcher -ErrorAction SilentlyContinue)
    if ($existing.Count -gt 0 -and -not $StopExisting) {
        throw 'A switcher is already running. The single-instance mutex requires it to exit before testing.'
    }
    if ($existing.Count -gt 1 -and [string]::IsNullOrWhiteSpace($ResumeExecutablePath)) {
        throw 'Multiple Window Switcher processes exist. Supply ResumeExecutablePath to identify the exact instance.'
    }
    $expectedResumePath = $null
    if (-not [string]::IsNullOrWhiteSpace($ResumeExecutablePath)) {
        $expectedResumePath = (Resolve-Path -LiteralPath $ResumeExecutablePath -ErrorAction Stop).ProviderPath
    }
    foreach ($candidate in $existing) {
        try { $identity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($candidate) }
        catch { throw "Cannot capture the existing process identity (pid=$($candidate.Id)): $($_.Exception.Message)" }
        $originalPath = $identity.ImagePath
        if ($null -ne $expectedResumePath -and
            -not [StringComparer]::OrdinalIgnoreCase.Equals($originalPath, (Resolve-Path -LiteralPath $expectedResumePath).ProviderPath)) {
            throw "The existing process path does not match ResumeExecutablePath: $originalPath"
        }
        if ($null -eq $expectedResumePath -and
            -not [StringComparer]::OrdinalIgnoreCase.Equals($originalPath, $Session.SourcePath)) {
            throw "The existing Window Switcher path is not the test executable. Supply ResumeExecutablePath: $originalPath"
        }
        if (-not (Test-Path -LiteralPath $originalPath -PathType Leaf)) { throw 'The resume executable does not exist.' }
        $configurationHashes = @{}
        foreach ($name in @('window-switcher.ini', 'windows-switcher.ini')) {
            $path = Join-Path (Split-Path -Parent $originalPath) $name
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                $configurationHashes[$path] = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
            }
        }
        $Session.Originals.Add([pscustomobject]@{
            Id = $identity.ProcessId; Path = $originalPath; Identity = $identity; StopRequested = $false
            Resumed = $false; ResumedIdentity = $null; ConfigurationHashes = $configurationHashes
        })
    }

    $null = New-Item -ItemType Directory -Path $Session.RunDirectory
    $null = New-Item -ItemType Directory -Path $Session.RuntimeDirectory
    Copy-Item -LiteralPath $Session.SourcePath -Destination $Session.ExecutablePath
    if ((Get-FileHash -LiteralPath $Session.ExecutablePath -Algorithm SHA256).Hash -cne $Session.SourceSha256) {
        throw 'The isolated executable differs from the source.'
    }
    $windowHotkey = if ($Automated) { 'win+f12' } else { 'alt+`' }
    $applicationHotkeyEnabled = if ($Automated) { 'no' } else { 'yes' }
    $configuration = @(
        'config_version = 1', 'trayicon = yes', '', '[startup]', 'run_as_admin = no', '',
        '[appearance]', 'monitor = primary', 'use_work_area = yes',
        "background_color = $BackgroundColor", "background_opacity = $BackgroundOpacity",
        "backdrop = $Backdrop", "backdrop_fallback = $BackdropFallback", '',
        '[localization]', 'language = en-US', '', '[performance]', 'icon_cache_limit = 256',
        'render_scale = auto', 'config_reload = restart', '', '[switch-windows]',
        "hotkey = $windowHotkey", 'blacklist =', 'ignore_minimal = no', 'only_current_desktop = auto', '',
        '[switch-apps]', "enable = $applicationHotkeyEnabled", 'hotkey = alt+tab',
        'ignore_minimal = no', 'only_current_desktop = auto', '', '[log]', 'level = info', "path = $($Session.LogPath)"
    )
    [IO.File]::WriteAllLines($Session.ConfigurationPath, $configuration, [Text.UTF8Encoding]::new($false))
    Copy-Item -LiteralPath $Session.ConfigurationPath -Destination (Join-Path $Session.RunDirectory 'test-configuration.ini')

    foreach ($original in $Session.Originals) {
        $original.StopRequested = $true
        Stop-WindowSwitcherPerformanceProcess -ProcessId $original.Id -ExpectedIdentity $original.Identity
    }
    $startInfo = [Diagnostics.ProcessStartInfo]::new($Session.ExecutablePath)
    $startInfo.WorkingDirectory = $Session.RuntimeDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $startInfo.Environment['WINDOW_SWITCHER_PERF'] = if ($EnableStageTiming) { '1' } else { '0' }
    $startup = [Diagnostics.Stopwatch]::StartNew()
    $Session.Process = [Diagnostics.Process]::Start($startInfo)
    if ($null -eq $Session.Process) { throw 'Window Switcher did not start.' }
    $ready = Wait-WindowSwitcherPerformanceProcess -Process $Session.Process -TimeoutMilliseconds 15000
    $Session.InputIdleMilliseconds = $startup.Elapsed.TotalMilliseconds
    $Session.ProcessIdentity = $ready.Identity
    $Session.Probe = $ready.Probe
}

function Wait-WindowSwitcherPerformanceProcess {
    param([Diagnostics.Process] $Process, [int] $TimeoutMilliseconds)

    if ($null -eq $Process) { throw 'Cannot wait for a null process.' }
    if ($TimeoutMilliseconds -lt 1) { throw 'The process readiness timeout must be positive.' }
    $wait = [Diagnostics.Stopwatch]::StartNew()
    $lastError = $null
    while ($wait.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
        try {
            $identity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($Process)
            $window = [WindowSwitcher.Performance.WindowProbe]::FindReadyWindow($identity.ProcessId)
            if ($window -ne [IntPtr]::Zero) {
                $probe = [WindowSwitcher.Performance.WindowProbe]::new($Process, $window)
                $null = $probe.IsVisible
                return [pscustomobject]@{ Identity = $identity; Probe = $probe }
            }
        } catch {
            $lastError = $_.Exception.Message
            if ($Process.HasExited) { break }
        }
        Start-Sleep -Milliseconds 25
    }
    if ($Process.HasExited) {
        throw "The Window Switcher process exited before readiness: $($Process.ExitCode)"
    }
    $detail = if ($lastError) { " Last error: $lastError" } else { '' }
    throw "The initialized switcher window was not ready within $TimeoutMilliseconds ms.$detail"
}

function Stop-WindowSwitcherPerformanceProcess {
    param([int] $ProcessId, [object] $ExpectedIdentity)

    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -eq $process) { return }
    if ($process.ProcessName -cne 'window-switcher') { throw 'The process ID no longer belongs to Window Switcher.' }
    $actualIdentity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($process)
    if ($null -eq $ExpectedIdentity -or -not $ExpectedIdentity.Matches($actualIdentity)) {
        throw "The process identity changed before shutdown: expected=$ExpectedIdentity actual=$actualIdentity"
    }
    try { [WindowSwitcher.Performance.WindowProbe]::RequestExit($actualIdentity.ProcessId, 500) }
    catch { Write-Verbose "Graceful shutdown unavailable: $($_.Exception.Message)" }
    $wait = [Diagnostics.Stopwatch]::StartNew()
    while ($wait.ElapsedMilliseconds -lt 3000) {
        $remaining = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
        if ($null -eq $remaining) { break }
        if ($remaining.ProcessName -cne 'window-switcher') { throw 'Process ownership changed before termination.' }
        $remainingIdentity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($remaining)
        if (-not $ExpectedIdentity.Matches($remainingIdentity)) {
            throw "The process identity changed during graceful shutdown: expected=$ExpectedIdentity actual=$remainingIdentity"
        }
        Start-Sleep -Milliseconds 50
    }
    $remaining = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -ne $remaining) {
        if ($remaining.ProcessName -cne 'window-switcher') { throw 'Process ownership changed before termination.' }
        $remainingIdentity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($remaining)
        if (-not $ExpectedIdentity.Matches($remainingIdentity)) {
            throw "The process identity changed before forced termination: expected=$ExpectedIdentity actual=$remainingIdentity"
        }
        Stop-Process -Id $ProcessId -Force -ErrorAction Stop
        $wait.Restart()
        while ($wait.ElapsedMilliseconds -lt 3000) {
            $remaining = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
            if ($null -eq $remaining) { break }
            $remainingIdentity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($remaining)
            if (-not $ExpectedIdentity.Matches($remainingIdentity)) {
                throw "The process identity changed after forced termination: expected=$ExpectedIdentity actual=$remainingIdentity"
            }
            Start-Sleep -Milliseconds 50
        }
        if (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue) { throw 'The switcher process did not exit after termination.' }
    }
    $process.Dispose()
}

function Close-WindowSwitcherPerformanceSession {
    param([object] $Session)

    if ($null -ne $Session.Process) {
        try {
            if (-not $Session.Process.HasExited) {
                Stop-WindowSwitcherPerformanceProcess -ProcessId $Session.Process.Id -ExpectedIdentity $Session.ProcessIdentity
            }
            $Session.Process.Dispose()
        } catch { $Session.CleanupErrors.Add("Test process cleanup: $($_.Exception.Message)") }
    }
    foreach ($original in $Session.Originals) {
        if ($original.StopRequested) {
            try {
                $matching = @()
                $samePath = @()
                foreach ($candidate in @(Get-Process -Name window-switcher -ErrorAction SilentlyContinue)) {
                    try {
                        $candidateIdentity = [WindowSwitcher.Performance.WindowProbe]::GetProcessIdentity($candidate)
                        if ([StringComparer]::OrdinalIgnoreCase.Equals($candidateIdentity.ImagePath, $original.Path)) {
                            $samePath += [pscustomobject]@{ Process = $candidate; Identity = $candidateIdentity }
                            if ($original.Identity.Matches($candidateIdentity)) {
                                $matching += [pscustomobject]@{ Process = $candidate; Identity = $candidateIdentity }
                            }
                        }
                    } catch { }
                }
                if ($matching.Count -gt 1) { throw "Multiple restored processes match the original path: $($original.Path)" }
                if ($matching.Count -eq 0 -and $samePath.Count -gt 0) {
                    throw "A different process instance already owns the original path: $($original.Path)"
                }
                if ($matching.Count -eq 0) {
                    $startInfo = [Diagnostics.ProcessStartInfo]::new($original.Path)
                    $startInfo.WorkingDirectory = Split-Path -Parent $original.Path
                    $startInfo.UseShellExecute = $false
                    $startInfo.CreateNoWindow = $true
                    $startInfo.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
                    $restoredProcess = [Diagnostics.Process]::Start($startInfo)
                    if ($null -eq $restoredProcess) { throw 'The original application did not start.' }
                } else {
                    $restoredProcess = $matching[0].Process
                }
                $ready = Wait-WindowSwitcherPerformanceProcess -Process $restoredProcess -TimeoutMilliseconds 15000
                if ($ready.Probe.IsVisible) {
                    $ready.Probe.Close(1000)
                    if ($ready.Probe.IsVisible) { throw 'The restored original panel remained visible.' }
                }
                $original.ResumedIdentity = $ready.Identity
                $original.Resumed = $true
                $restoredProcess.Dispose()
            } catch { $Session.CleanupErrors.Add("Original process restore: $($_.Exception.Message)") }
        }
        foreach ($path in $original.ConfigurationHashes.Keys) {
            try {
                if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or
                    (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -cne $original.ConfigurationHashes[$path]) {
                    throw "An original configuration changed during the test: $path"
                }
            } catch {
                $Session.OriginalConfigurationsPreserved = $false
                $Session.CleanupErrors.Add($_.Exception.Message)
            }
        }
    }
    try {
        if (Test-Path -LiteralPath $Session.RuntimeDirectory) {
            $directory = Get-Item -LiteralPath $Session.RuntimeDirectory -Force
            $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar)
            if ($directory.Parent.FullName -ine $temporaryRoot -or
                -not $directory.Name.StartsWith('window-switcher-performance-') -or
                ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw 'Refusing to clean an unexpected runtime directory.'
            }
            # Only the two files created by this session are removed. Unknown files prevent directory removal.
            [IO.File]::Delete($Session.ConfigurationPath)
            [IO.File]::Delete($Session.ExecutablePath)
            [IO.Directory]::Delete($directory.FullName)
        }
        $Session.RuntimeRemoved = $true
    } catch { $Session.CleanupErrors.Add("Runtime directory cleanup: $($_.Exception.Message)") }
}
