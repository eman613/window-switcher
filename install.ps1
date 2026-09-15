#Requires -Version 7.0
[CmdletBinding(DefaultParameterSetName = 'Download')]
param(
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$')]
    [string]$Repository = 'eman613/window-switcher',
    [ValidateSet('prerelease', 'stable')][string]$Channel = 'prerelease',
    [string]$Tag,
    [string]$Destination,
    [Parameter(Mandatory, ParameterSetName = 'Local')][string]$ArchivePath,
    [Parameter(ParameterSetName = 'Local')][switch]$ValidateOnly,
    [Parameter(ParameterSetName = 'Local')][switch]$RequireUsage,
    [Parameter(ParameterSetName = 'Local')]
    [ValidateSet('windows-64', 'windows-arm64')][string]$Platform,
    [Parameter(Mandatory, ParameterSetName = 'Rollback')][switch]$Rollback,
    [ValidateRange(1, 5)][int]$Attempts = 3,
    [ValidateRange(1, 120)][int]$TimeoutSec = 30
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $IsWindows) { throw 'This installer requires Windows and PowerShell 7.' }

function Assert-WindowSwitcherPath([string]$Path) {
    $full = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)
    $root = [IO.Path]::GetPathRoot($full)
    if ($full.TrimEnd('\') -eq $root.TrimEnd('\')) { throw 'An installation path cannot be a volume root.' }
    for ($ancestor = $full; $ancestor; $ancestor = [IO.Path]::GetDirectoryName($ancestor)) {
        if (Test-Path -LiteralPath $ancestor) {
            $item = Get-Item -LiteralPath $ancestor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Reparse points are not supported in installation paths: $ancestor"
            }
        }
    }
    return $full
}

function Invoke-WindowSwitcherDownload([string]$Uri, [string]$OutFile) {
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        try {
            $options = @{ Uri = $Uri; TimeoutSec = $TimeoutSec; MaximumRedirection = 5; ErrorAction = 'Stop' }
            if ($OutFile) {
                Invoke-WebRequest @options -OutFile $OutFile | Out-Null
                return
            }
            return Invoke-RestMethod @options -Headers @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'window-switcher-installer' }
        } catch {
            if ($attempt -eq $Attempts) { throw "Download failed after $attempt attempt(s): $($_.Exception.Message)" }
            Write-Warning "Download failed; retry $($attempt + 1)/$Attempts."
            Start-Sleep -Milliseconds (200 * $attempt)
        }
    }
}

function Assert-WindowSwitcherExecutable([string]$Path, [string]$ExpectedVersion) {
    if ((Get-Item -LiteralPath $Path).Length -gt 64MB) { throw 'Executable exceeds its size limit.' }
    $image = [IO.File]::ReadAllBytes($Path)
    if ($image.Length -lt 64 -or [BitConverter]::ToUInt16($image, 0) -ne 0x5A4D) { throw 'Invalid executable DOS header.' }
    $offset = [BitConverter]::ToInt32($image, 0x3C)
    if ($offset -lt 64 -or $offset -gt $image.Length - 26 -or [BitConverter]::ToUInt32($image, $offset) -ne 0x4550) {
        throw 'Invalid executable PE header.'
    }
    $machine = if ($Platform -eq 'windows-64') { 0x8664 } else { 0xAA64 }
    if ([BitConverter]::ToUInt16($image, $offset + 4) -ne $machine -or [BitConverter]::ToUInt16($image, $offset + 24) -ne 0x20B) {
        throw "Incorrect executable architecture; expected $Platform."
    }
    $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    if ($version.ProductName -cne 'Window Switcher' -or $version.OriginalFilename -cne 'window-switcher.exe') {
        throw 'The executable is not identified as Window Switcher.'
    }
    if ($ExpectedVersion -and ($version.ProductVersion -cne $ExpectedVersion -or $version.FileVersion -cne "$ExpectedVersion.0")) {
        throw 'Executable version does not match the requested tag.'
    }
}

function Expand-WindowSwitcherPackage([string]$Path, [string]$OutputDirectory) {
    $packageStream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $archive = $null
    try {
        if ($packageStream.Length -gt 128MB -or (Get-Item -LiteralPath "$Path.sha256").Length -gt 512) {
            throw 'Package or checksum exceeds its size limit.'
        }
        $hash = (Get-FileHash -InputStream $packageStream -Algorithm SHA256).Hash.ToLowerInvariant()
        if ((Get-Content -LiteralPath "$Path.sha256" -Raw -Encoding UTF8).Trim() -cne "$hash  $([IO.Path]::GetFileName($Path))") {
            throw 'Package SHA-256 mismatch. Download the archive and checksum again.'
        }
        $packageStream.Position = 0
        $archive = [IO.Compression.ZipArchive]::new($packageStream, [IO.Compression.ZipArchiveMode]::Read, $true)
        $required = @('window-switcher.exe', 'window-switcher.ini', 'LICENSE')
        if ($RequireUsage) { $required += 'USAGE.txt' }
        $names = @($archive.Entries | Select-Object -ExpandProperty FullName)
        $unique = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        if ($names.Count -lt 3 -or $names.Count -gt 4) { throw 'Unexpected number of package files.' }
        foreach ($name in $required) { if ($names -cnotcontains $name) { throw "Missing package file: $name" } }
        $total = 0L
        foreach ($entry in $archive.Entries) {
            if ($entry.FullName -cnotin @('window-switcher.exe', 'window-switcher.ini', 'LICENSE', 'USAGE.txt') -or
                -not $unique.Add($entry.FullName) -or ($entry.ExternalAttributes -band 0x400) -or
                (($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) {
                throw 'Unexpected, duplicate, or linked ZIP entry.'
            }
            $total += $entry.Length
            if ($entry.Length -le 0 -or $entry.Length -gt 64MB -or $total -gt 128MB) { throw 'Invalid ZIP entry size.' }
        }
        foreach ($entry in $archive.Entries) {
            $inputStream = $entry.Open()
            $outputStream = $null
            try {
                $outputStream = [IO.File]::Open((Join-Path $OutputDirectory $entry.FullName), [IO.FileMode]::CreateNew)
                $buffer = [byte[]]::new(81920)
                $written = 0L
                while (($count = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $written += $count
                    if ($written -gt $entry.Length) { throw 'ZIP entry exceeded its declared size.' }
                    $outputStream.Write($buffer, 0, $count)
                }
                if ($written -ne $entry.Length) { throw 'Truncated ZIP entry.' }
            } finally {
                if ($outputStream) { $outputStream.Dispose() }
                $inputStream.Dispose()
            }
        }
    } finally {
        if ($archive) { $archive.Dispose() }
        $packageStream.Dispose()
    }
    Assert-WindowSwitcherExecutable (Join-Path $OutputDirectory 'window-switcher.exe') (($Tag.Substring(1) -split '-')[0])
    return $hash
}

$nativePlatform = switch ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    'X64' { 'windows-64' }
    'Arm64' { 'windows-arm64' }
    default { throw 'Only x64 and ARM64 Windows are supported.' }
}
if (-not $Platform) { $Platform = $nativePlatform }
if (-not $ValidateOnly -and $Platform -ne $nativePlatform) { throw 'Cannot install a package for a different OS architecture.' }
$ownedDirectories = [Collections.Generic.List[object]]::new()
$installLock = $null
$preserveStage = $false
$stage = $null
try {
    if (-not $Rollback) {
        if ($Tag -and $Tag -cnotmatch '^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') { throw 'Invalid release tag.' }
        $temporaryParent = Assert-WindowSwitcherPath ([IO.Path]::GetTempPath())
        $temporary = Join-Path $temporaryParent ('window-switcher-install-' + [guid]::NewGuid().ToString('N'))
        $null = New-Item -ItemType Directory -Path $temporary
        $ownedDirectories.Add(@{ Path = $temporary; Parent = $temporaryParent })
        if (-not $ArchivePath) {
            $api = "https://api.github.com/repos/$Repository/releases"
            if ($Tag) { $release = Invoke-WindowSwitcherDownload "$api/tags/$Tag" }
            elseif ($Channel -eq 'stable') { $release = Invoke-WindowSwitcherDownload "$api/latest" }
            else {
                $releases = @(Invoke-WindowSwitcherDownload ($api + '?per_page=100'))
                $release = $releases | Where-Object { -not $_.draft -and $_.prerelease } | Sort-Object published_at -Descending | Select-Object -First 1
            }
            if (-not $release -or $release.draft -or ($Tag -and $release.tag_name -cne $Tag)) { throw 'No matching public release was found.' }
            if (-not $Tag -and $Channel -eq 'stable' -and $release.prerelease) { throw 'The stable channel returned a prerelease.' }
            $Tag = $release.tag_name
            if ($Tag -cnotmatch '^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') { throw 'The release has an invalid tag.' }
            $archiveName = "window-switcher-$Tag-$Platform.zip"
            foreach ($name in @($archiveName, "$archiveName.sha256")) {
                $asset = @($release.assets | Where-Object name -CEQ $name)
                if ($asset.Count -ne 1 -or $asset[0].state -ne 'uploaded' -or $asset[0].size -le 0 -or $asset[0].size -gt 128MB) {
                    throw "Missing or invalid release asset: $name"
                }
                $path = Join-Path $temporary $name
                Invoke-WindowSwitcherDownload "https://github.com/$Repository/releases/download/$Tag/$name" $path
                $digest = 'sha256:' + (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($asset[0].digest -cne $digest -or (Get-Item -LiteralPath $path).Length -ne $asset[0].size) {
                    throw "GitHub asset digest or size mismatch: $name"
                }
            }
            $ArchivePath = Join-Path $temporary $archiveName
        } elseif (-not $Tag) { throw 'A local archive requires an explicit -Tag.' }
        $ArchivePath = (Resolve-Path -LiteralPath $ArchivePath).Path
        $content = Join-Path $temporary 'content'
        $null = New-Item -ItemType Directory -Path $content
        $hash = Expand-WindowSwitcherPackage $ArchivePath $content
        Write-Verbose "stage=package-validated tag=$Tag platform=$Platform sha256=$hash"
        if ($ValidateOnly) { return [pscustomobject]@{ Tag = $Tag; Platform = $Platform; SHA256 = $hash; Validated = $true } }
    }
    if (-not $Destination) {
        $localData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
        if (-not $localData) { throw 'LocalApplicationData is unavailable; specify -Destination.' }
        $Destination = Join-Path $localData 'Programs/window-switcher'
    }
    $Destination = Assert-WindowSwitcherPath $Destination
    if (-not (Test-Path -LiteralPath $Destination)) { $null = New-Item -ItemType Directory -Path $Destination }
    $executable = Assert-WindowSwitcherPath (Join-Path $Destination 'window-switcher.exe')
    $previous = Assert-WindowSwitcherPath "$executable.previous"
    $lockPath = Assert-WindowSwitcherPath (Join-Path $Destination '.window-switcher.install.lock')
    $installLock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    foreach ($process in Get-Process -Name 'window-switcher' -ErrorAction SilentlyContinue) {
        if ($process.Path -and [string]::Equals($process.Path, $executable, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Window Switcher is running in the destination. Exit it from the tray, then retry.'
        }
    }
    foreach ($path in @($executable, $previous)) {
        if (Test-Path -LiteralPath $path) { Assert-WindowSwitcherExecutable $path '' }
    }
    if ($Rollback) {
        if (-not (Test-Path -LiteralPath $previous) -or -not (Test-Path -LiteralPath $executable)) { throw 'No complete previous installation is available for rollback.' }
        Write-Verbose 'stage=rollback action=replace-previous'
        [IO.File]::Replace($previous, $executable, [System.Management.Automation.Language.NullString]::Value)
        return [pscustomobject]@{ Status = 'RolledBack'; Executable = $executable; Configuration = 'Preserved' }
    }
    $stage = Join-Path $Destination ('.window-switcher-stage-' + [guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $stage
    $ownedDirectories.Add(@{ Path = $stage; Parent = $Destination })
    $candidate = Join-Path $stage 'window-switcher.exe'
    Copy-Item -LiteralPath (Join-Path $content 'window-switcher.exe') -Destination $candidate
    # Create missing support files only. Never open an existing INI for writing.
    foreach ($name in @('window-switcher.ini', 'LICENSE', 'USAGE.txt')) {
        $source = Join-Path $content $name
        $target = Assert-WindowSwitcherPath (Join-Path $Destination $name)
        if ((Test-Path -LiteralPath $source) -and -not (Test-Path -LiteralPath $target)) { [IO.File]::Copy($source, $target, $false) }
    }
    $oldHash = if (Test-Path -LiteralPath $executable) { (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash } else { $null }
    try {
        if ($oldHash) { [IO.File]::Replace($candidate, $executable, $previous) }
        else { [IO.File]::Move($candidate, $executable) }
    } catch {
        $preserveStage = $true
        # ReplaceFile can report partial failures. Restore only a verified old image.
        if ($oldHash -and (Test-Path -LiteralPath $previous) -and
            (Get-FileHash -LiteralPath $previous -Algorithm SHA256).Hash -ceq $oldHash -and -not (Test-Path -LiteralPath $executable)) {
            [IO.File]::Move($previous, $executable)
        }
        throw "Executable replacement failed. Recovery files are retained in $Destination. $($_.Exception.Message)"
    }
    [pscustomobject]@{ Status = 'Installed'; Tag = $Tag; Platform = $Platform; Executable = $executable; Configuration = 'Preserved'; SHA256 = $hash }
} finally {
    if ($installLock) { $installLock.Dispose() }
    foreach ($owned in $ownedDirectories) {
        if ($preserveStage -and $owned.Path -eq $stage) { continue }
        try {
            $full = Assert-WindowSwitcherPath $owned.Path
            $boundary = [IO.Path]::GetFullPath($owned.Parent).TrimEnd('\') + '\'
            if (-not $full.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase)) { throw 'Cleanup path escaped its owned parent.' }
            if (Get-ChildItem -LiteralPath $full -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
                throw 'Cleanup refused a reparse point.'
            }
            Remove-Item -LiteralPath $full -Recurse -ErrorAction Stop
        } catch { Write-Warning "Cleanup failed; retained $($owned.Path): $($_.Exception.Message)" }
    }
}
