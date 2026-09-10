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
    foreach ($candidate in $existing) {
        try { $originalPath = [WindowSwitcher.Performance.WindowProbe]::GetProcessImagePath($candidate.Id) }
        catch {
            if ($existing.Count -ne 1 -or -not $ResumeExecutablePath) {
                throw 'Cannot determine the existing executable path. Supply ResumeExecutablePath before stopping it.'
            }
            $originalPath = (Resolve-Path -LiteralPath $ResumeExecutablePath -ErrorAction Stop).ProviderPath
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
            Id = $candidate.Id; Path = $originalPath; StopRequested = $false
            Resumed = $false; ConfigurationHashes = $configurationHashes
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
        Stop-WindowSwitcherPerformanceProcess -ProcessId $original.Id
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
    $window = [IntPtr]::Zero
    while ($startup.Elapsed.TotalSeconds -lt 15 -and $window -eq [IntPtr]::Zero) {
        if ($Session.Process.HasExited) { throw "The tested process exited during startup: $($Session.Process.ExitCode)" }
        $window = [WindowSwitcher.Performance.WindowProbe]::FindReadyWindow($Session.Process.Id)
        if ($window -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 25 }
    }
    if ($window -eq [IntPtr]::Zero) { throw 'The initialized switcher window was not found within 15 seconds.' }
    $Session.InputIdleMilliseconds = $startup.Elapsed.TotalMilliseconds
    $Session.Probe = [WindowSwitcher.Performance.WindowProbe]::new($Session.Process, $window)
}

function Stop-WindowSwitcherPerformanceProcess {
    param([int] $ProcessId)

    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -eq $process) { return }
    if ($process.ProcessName -cne 'window-switcher') { throw 'The process ID no longer belongs to Window Switcher.' }
    try { [WindowSwitcher.Performance.WindowProbe]::RequestExit($ProcessId, 500) }
    catch { Write-Verbose "Graceful shutdown unavailable: $($_.Exception.Message)" }
    $wait = [Diagnostics.Stopwatch]::StartNew()
    while ($wait.ElapsedMilliseconds -lt 3000 -and (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
        Start-Sleep -Milliseconds 50
    }
    $remaining = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -ne $remaining) {
        if ($remaining.ProcessName -cne 'window-switcher') { throw 'Process ownership changed before termination.' }
        Stop-Process -Id $ProcessId -Force -ErrorAction Stop
        $wait.Restart()
        while ($wait.ElapsedMilliseconds -lt 3000 -and (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
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
            if (-not $Session.Process.HasExited) { Stop-WindowSwitcherPerformanceProcess -ProcessId $Session.Process.Id }
            $Session.Process.Dispose()
        } catch { $Session.CleanupErrors.Add("Test process cleanup: $($_.Exception.Message)") }
    }
    foreach ($original in $Session.Originals) {
        if ($original.StopRequested) {
            try {
                if (@(Get-Process -Name window-switcher -ErrorAction SilentlyContinue).Count -eq 0) {
                    $null = Start-Process -FilePath $original.Path -WorkingDirectory (Split-Path -Parent $original.Path) -WindowStyle Hidden -PassThru
                    Start-Sleep -Seconds 2
                }
                if (@(Get-Process -Name window-switcher -ErrorAction SilentlyContinue).Count -eq 0) { throw 'The original application did not remain running.' }
                $original.Resumed = $true
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
