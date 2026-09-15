#Requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Executable, [string]$Tag)
$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$installer = Join-Path $repository 'install.ps1'
$Executable = (Resolve-Path -LiteralPath $Executable).Path
if (-not $Tag) { $Tag = 'v' + [Diagnostics.FileVersionInfo]::GetVersionInfo($Executable).ProductVersion }
$platform = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64') { 'windows-arm64' } else { 'windows-64' }
$temporaryParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$testRoot = Join-Path $temporaryParent ('window-switcher-distribution-test-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $testRoot
$results = [Collections.Generic.List[object]]::new()

function Assert-Distribution([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Test-DistributionCase([string]$Name, [scriptblock]$Body) {
    try {
        & $Body
        $results.Add([pscustomobject]@{ Case = $Name; Passed = $true })
        Write-Host "PASS $Name"
    } catch {
        $results.Add([pscustomobject]@{ Case = $Name; Passed = $false; Error = $_.Exception.Message })
        Write-Warning "FAIL ${Name}: $($_.Exception.Message)"
    }
}
function Assert-DistributionFailure([scriptblock]$Body, [string]$Pattern = '.') {
    $failure = $null
    try { $null = & $Body } catch { $failure = $_.Exception.Message }
    Assert-Distribution ($failure -and $failure -match $Pattern) "Expected failure /$Pattern/, received: $failure"
}
function Write-DistributionChecksum([string]$Path) {
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($Path))" | Set-Content -LiteralPath "$Path.sha256" -Encoding UTF8
}
function New-DistributionFixture([scriptblock]$Mutate) {
    $directory = Join-Path $testRoot ([guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $directory
    $path = Join-Path $directory ([IO.Path]::GetFileName($package.Archive))
    Copy-Item -LiteralPath $package.Archive -Destination $path
    if ($Mutate) {
        $zip = [IO.Compression.ZipFile]::Open($path, [IO.Compression.ZipArchiveMode]::Update)
        try { & $Mutate $zip } finally { $zip.Dispose() }
    }
    Write-DistributionChecksum $path
    return $path
}
function Add-DistributionEntry($Zip, [string]$Name, [byte[]]$Bytes) {
    $stream = $Zip.CreateEntry($Name).Open()
    try { $stream.Write($Bytes, 0, $Bytes.Length) } finally { $stream.Dispose() }
}
try {
    $package = & (Join-Path $PSScriptRoot 'new-package.ps1') -Executable $Executable -Tag $Tag -Platform $platform -OutputDirectory (Join-Path $testRoot 'package')
    $local = @{ ArchivePath = $package.Archive; Tag = $Tag; Platform = $platform }
    Test-DistributionCase 'package validates without creating an installation' {
        $destination = Join-Path $testRoot 'validation-only'
        $result = & $installer @local -ValidateOnly -RequireUsage -Destination $destination
        Assert-Distribution ($result.Validated -and -not (Test-Path -LiteralPath $destination)) 'Validation created an installation.'
    }
    Test-DistributionCase 'fresh install, byte-preserving update, and rollback' {
        $destination = Join-Path $testRoot 'installed'
        $null = & $installer @local -Destination $destination
        $ini = Join-Path $destination 'window-switcher.ini'
        [IO.File]::WriteAllBytes($ini, [Text.Encoding]::Unicode.GetBytes("; 个性化配置`r`nunknown_key=kept`r`n"))
        $iniHash = (Get-FileHash -LiteralPath $ini).Hash
        $installed = Join-Path $destination 'window-switcher.exe'
        $stream = [IO.File]::Open($installed, [IO.FileMode]::Append)
        try { $stream.WriteByte(0x42) } finally { $stream.Dispose() }
        $oldHash = (Get-FileHash -LiteralPath $installed).Hash
        (Get-Item -LiteralPath $ini).IsReadOnly = $true
        try {
            $null = & $installer @local -Destination $destination
            Assert-Distribution ((Get-FileHash -LiteralPath $installed).Hash -eq (Get-FileHash -LiteralPath $Executable).Hash) 'Update did not publish the new executable.'
            Assert-Distribution ((Get-FileHash -LiteralPath "$installed.previous").Hash -eq $oldHash) 'Previous executable was not retained.'
            $null = & $installer -Rollback -Destination $destination
            Assert-Distribution ((Get-FileHash -LiteralPath $installed).Hash -eq $oldHash) 'Rollback did not restore the old executable.'
            Assert-Distribution ((Get-FileHash -LiteralPath $ini).Hash -eq $iniHash) 'Update or rollback changed the INI bytes.'
            Assert-Distribution (Get-Item -LiteralPath $ini).IsReadOnly 'Update or rollback changed INI attributes.'
        } finally { (Get-Item -LiteralPath $ini).IsReadOnly = $false }
    }
    Test-DistributionCase 'checksum corruption' {
        $archive = New-DistributionFixture
        'incorrect checksum' | Set-Content -LiteralPath "$archive.sha256" -Encoding UTF8
        Assert-DistributionFailure { & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly } 'SHA-256 mismatch'
    }
    Test-DistributionCase 'truncated ZIP with matching checksum' {
        $archive = New-DistributionFixture
        [IO.File]::WriteAllBytes($archive, [byte[]](1, 2, 3, 4))
        Write-DistributionChecksum $archive
        Assert-DistributionFailure { & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly }
    }
    Test-DistributionCase 'missing required license' {
        $archive = New-DistributionFixture { param($zip) $zip.GetEntry('LICENSE').Delete() }
        Assert-DistributionFailure { & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly } 'Missing package file'
    }
    Test-DistributionCase 'new packages require usage documentation' {
        $archive = New-DistributionFixture { param($zip) $zip.GetEntry('USAGE.txt').Delete() }
        Assert-DistributionFailure { & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly -RequireUsage } 'Missing package file'
        $null = & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly
    }
    foreach ($entryName in @('../escape.txt', 'LICENSE', 'license', 'directory/USAGE.txt')) {
        Test-DistributionCase "reject ZIP entry $entryName" {
            $archive = New-DistributionFixture {
                param($zip)
                $zip.GetEntry('USAGE.txt').Delete()
                Add-DistributionEntry $zip $entryName ([byte[]](65, 66))
            }
            Assert-DistributionFailure { & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly } 'ZIP entry'
            Assert-Distribution (-not (Test-Path -LiteralPath (Join-Path $testRoot 'escape.txt'))) 'Archive escaped extraction.'
        }
    }
    Test-DistributionCase 'empty and oversized entries' {
        $empty = New-DistributionFixture {
            param($zip)
            $zip.GetEntry('USAGE.txt').Delete()
            Add-DistributionEntry $zip 'USAGE.txt' ([byte[]]::new(0))
        }
        Assert-DistributionFailure { & $installer -ArchivePath $empty -Tag $Tag -ValidateOnly } 'entry size'
        $large = New-DistributionFixture {
            param($zip)
            $zip.GetEntry('USAGE.txt').Delete()
            $stream = $zip.CreateEntry('USAGE.txt').Open()
            try {
                $zeroes = [byte[]]::new(1MB)
                for ($index = 0; $index -lt 65; $index++) { $stream.Write($zeroes, 0, $zeroes.Length) }
            } finally { $stream.Dispose() }
        }
        Assert-DistributionFailure { & $installer -ArchivePath $large -Tag $Tag -ValidateOnly } 'entry size'
    }
    Test-DistributionCase 'incorrect architecture and version' {
        $other = if ($platform -eq 'windows-64') { 'windows-arm64' } else { 'windows-64' }
        Assert-DistributionFailure { & $installer -ArchivePath $package.Archive -Tag $Tag -Platform $other -ValidateOnly } 'architecture'
        Assert-DistributionFailure { & $installer -ArchivePath $package.Archive -Tag 'v999.999.999' -ValidateOnly } 'version'
        Assert-DistributionFailure { & $installer -ArchivePath $package.Archive -Tag '../invalid' -ValidateOnly } 'tag'
    }
    Test-DistributionCase 'exclusive file lock preserves the old executable and INI' {
        $destination = Join-Path $testRoot 'locked'
        $null = & $installer @local -Destination $destination
        $installed = Join-Path $destination 'window-switcher.exe'
        $before = (Get-FileHash -LiteralPath $installed).Hash
        $handle = [IO.File]::Open($installed, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
        try { Assert-DistributionFailure { & $installer @local -Destination $destination } }
        finally { $handle.Dispose() }
        Assert-Distribution ((Get-FileHash -LiteralPath $installed).Hash -eq $before) 'Locked executable changed.'
    }
    Test-DistributionCase 'concurrent installer lock' {
        $destination = Join-Path $testRoot 'concurrent'
        $null = & $installer @local -Destination $destination
        $handle = [IO.File]::Open((Join-Path $destination '.window-switcher.install.lock'), [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        try { Assert-DistributionFailure { & $installer @local -Destination $destination } }
        finally { $handle.Dispose() }
    }
    Test-DistributionCase 'write denied without elevation' {
        $destination = Join-Path $testRoot 'denied'
        $null = New-Item -ItemType Directory -Path $destination
        $acl = Get-Acl -LiteralPath $destination
        $original = $acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
        $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $rule = [Security.AccessControl.FileSystemAccessRule]::new($sid, 'Write', 'ContainerInherit, ObjectInherit', 'None', 'Deny')
        $acl.AddAccessRule($rule)
        Set-Acl -LiteralPath $destination -AclObject $acl
        try {
            Assert-DistributionFailure { & $installer @local -Destination $destination }
            Assert-Distribution (-not (Test-Path -LiteralPath (Join-Path $destination 'window-switcher.exe'))) 'Write denial was bypassed.'
        } finally {
            $acl.SetSecurityDescriptorSddlForm($original, [Security.AccessControl.AccessControlSections]::Access)
            Set-Acl -LiteralPath $destination -AclObject $acl
        }
    }
    Test-DistributionCase 'network failure has a bounded retry count' {
        $networkObservation = @{ Calls = 0 }
        Set-Item -Path Function:Invoke-RestMethod -Value {
            $networkObservation.Calls++
            throw 'Simulated connection interruption.'
        }.GetNewClosure()
        Assert-DistributionFailure { & $installer -Attempts 3 -TimeoutSec 1 -Destination (Join-Path $testRoot 'offline') -WarningAction SilentlyContinue } 'after 3 attempt'
        Assert-Distribution ($networkObservation.Calls -eq 3) 'Network retries were not bounded.'
    }
    Test-DistributionCase 'fork channels, asset digests, and interrupted downloads' {
        $downloadObservation = @{ Uris = [Collections.Generic.List[string]]::new(); Archive = $package.Archive; Tag = $Tag; Mode = 'valid'; Prerelease = $true; Outputs = [Collections.Generic.List[string]]::new() }
        Set-Item -Path Function:Invoke-RestMethod -Value {
            param([string]$Uri)
            $downloadObservation.Uris.Add($Uri)
            $assets = foreach ($path in @($downloadObservation.Archive, "$($downloadObservation.Archive).sha256")) {
                $digest = if ($downloadObservation.Mode -eq 'digest') { 'sha256:' + ('0' * 64) } else { 'sha256:' + (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() }
                [pscustomobject]@{ name = [IO.Path]::GetFileName($path); state = 'uploaded'; size = (Get-Item -LiteralPath $path).Length; digest = $digest }
            }
            [pscustomobject]@{ tag_name = $downloadObservation.Tag; draft = $false; prerelease = $downloadObservation.Prerelease; published_at = '2026-01-01T00:00:00Z'; assets = $assets }
        }.GetNewClosure()
        Set-Item -Path Function:Invoke-WebRequest -Value {
            param([string]$Uri, [string]$OutFile)
            $downloadObservation.Uris.Add($Uri)
            $downloadObservation.Outputs.Add($OutFile)
            if ($downloadObservation.Mode -eq 'interrupted') {
                [IO.File]::WriteAllBytes($OutFile, [byte[]](1, 2))
                throw 'Simulated interrupted response.'
            }
            Copy-Item -LiteralPath (Join-Path ([IO.Path]::GetDirectoryName($downloadObservation.Archive)) ([IO.Path]::GetFileName($OutFile))) -Destination $OutFile
        }.GetNewClosure()
        $null = & $installer -Destination (Join-Path $testRoot 'downloaded')
        Assert-Distribution ($downloadObservation.Uris[0] -ceq 'https://api.github.com/repos/eman613/window-switcher/releases?per_page=100') 'Default source or prerelease selection changed.'
        Assert-Distribution ($downloadObservation.Uris.Count -eq 3) 'Unexpected download requests.'
        Assert-DistributionFailure { & $installer -Channel stable -Destination (Join-Path $testRoot 'wrong-channel') } 'stable channel'
        $downloadObservation.Prerelease = $false
        $null = & $installer -Channel stable -Destination (Join-Path $testRoot 'stable')
        $downloadObservation.Mode = 'digest'
        Assert-DistributionFailure { & $installer -Tag $Tag -Destination (Join-Path $testRoot 'bad-digest') } 'digest'
        $downloadObservation.Mode = 'interrupted'
        $before = $downloadObservation.Outputs.Count
        Assert-DistributionFailure { & $installer -Tag $Tag -Attempts 2 -Destination (Join-Path $testRoot 'partial') -WarningAction SilentlyContinue } 'after 2 attempt'
        Assert-Distribution ($downloadObservation.Outputs.Count - $before -eq 2) 'Interrupted transfers exceeded the retry budget.'
        foreach ($path in $downloadObservation.Outputs) { Assert-Distribution (-not (Test-Path -LiteralPath $path)) 'A partial download was not cleaned up.' }
    }
    Test-DistributionCase 'read-only executable keeps the previous installation recoverable' {
        $destination = Join-Path $testRoot 'readonly-exe'
        $null = & $installer @local -Destination $destination
        $installed = Join-Path $destination 'window-switcher.exe'
        $before = (Get-FileHash -LiteralPath $installed).Hash
        (Get-Item -LiteralPath $installed).IsReadOnly = $true
        try {
            Assert-DistributionFailure { & $installer @local -Destination $destination } 'replacement failed'
            Assert-Distribution ((Get-FileHash -LiteralPath $installed).Hash -eq $before) 'Failed replacement changed the installed image.'
        } finally { (Get-Item -LiteralPath $installed).IsReadOnly = $false }
    }
    Test-DistributionCase 'linked archive entries and installation junctions are rejected' {
        $archive = New-DistributionFixture { param($zip) $zip.GetEntry('USAGE.txt').ExternalAttributes = 0x400 }
        Assert-DistributionFailure { & $installer -ArchivePath $archive -Tag $Tag -ValidateOnly } 'ZIP entry'
        $target = Join-Path $testRoot 'junction-target'
        $link = Join-Path $testRoot 'junction'
        $null = New-Item -ItemType Directory -Path $target
        $null = New-Item -ItemType Junction -Path $link -Target $target
        try {
            Assert-DistributionFailure { & $installer @local -Destination $link } 'Reparse points'
            Assert-Distribution (-not (Get-ChildItem -LiteralPath $target)) 'Junction target was modified.'
        } finally { Remove-Item -LiteralPath $link }
    }
    Test-DistributionCase 'cleanup failure reports retained files without undoing a valid install' {
        $cleanupObservation = @{ Paths = [Collections.Generic.List[string]]::new() }
        Set-Item -Path Function:Remove-Item -Value {
            param([string]$LiteralPath)
            $cleanupObservation.Paths.Add($LiteralPath)
            throw 'Simulated cleanup denial.'
        }.GetNewClosure()
        try {
            $destination = Join-Path $testRoot 'cleanup-denied'
            $warnings = @()
            $result = & $installer @local -Destination $destination -WarningVariable warnings -WarningAction SilentlyContinue
            Assert-Distribution ($result.Status -eq 'Installed' -and $warnings.Count -ge 1) 'Cleanup failure was silently ignored or invalidated a completed install.'
        } finally {
            foreach ($path in $cleanupObservation.Paths) {
                $full = [IO.Path]::GetFullPath($path)
                Assert-Distribution ($full.StartsWith($temporaryParent.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) 'Cleanup fixture escaped the temporary directory.'
                Microsoft.PowerShell.Management\Remove-Item -LiteralPath $full -Recurse
            }
        }
    }
    $results | ConvertTo-Json -Depth 4
    if (@($results | Where-Object { -not $_.Passed }).Count) { throw 'Distribution regression tests failed.' }
} finally {
    $full = [IO.Path]::GetFullPath($testRoot)
    if (-not $full.StartsWith($temporaryParent.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -or
        ((Get-Item -LiteralPath $full).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Unsafe distribution test cleanup path.' }
    Remove-Item -LiteralPath $full -Recurse -ErrorAction Stop
}
