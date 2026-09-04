[CmdletBinding()]
param(
    [Parameter()]
    [ValidatePattern('^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$')]
    [string] $Repository = 'sigoden/window-switcher',

    [Parameter()]
    [string] $Tag,

    [Parameter()]
    [string] $InstallDirectory = (Join-Path (
            [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
        ) 'Programs\window-switcher'),

    [Parameter()]
    [switch] $Launch
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$commandName = 'window-switcher'
$githubApiBaseUri = 'https://api.github.com'
$githubWebBaseUri = 'https://github.com'
$releaseTagPattern = '^v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$'
$temporaryDirectory = $null
$stagedExecutable = $null
$backupExecutable = $null

function Get-WindowSwitcherRequestHeaders {
    $headers = @{
        Accept = 'application/vnd.github+json'
        'User-Agent' = 'window-switcher-installer'
        'X-GitHub-Api-Version' = '2022-11-28'
    }
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_TOKEN)) {
        $headers.Authorization = "Bearer $env:GITHUB_TOKEN"
    }
    $headers
}

function Get-WindowSwitcherRelease {
    param(
        [Parameter(Mandatory)]
        [string] $RepositoryName,

        [Parameter()]
        [string] $RequestedTag,

        [Parameter(Mandatory)]
        [hashtable] $Headers
    )

    $releaseUri = if ([string]::IsNullOrWhiteSpace($RequestedTag)) {
        "$githubApiBaseUri/repos/$RepositoryName/releases/latest"
    } else {
        $encodedTag = [Uri]::EscapeDataString($RequestedTag)
        "$githubApiBaseUri/repos/$RepositoryName/releases/tags/$encodedTag"
    }

    try {
        Invoke-RestMethod -Method Get -Uri $releaseUri -Headers $Headers
    } catch {
        throw "Failed to resolve release metadata from '$releaseUri': $($_.Exception.Message)"
    }
}

function Get-WindowSwitcherReleaseAsset {
    param(
        [Parameter(Mandatory)]
        [object] $Release,

        [Parameter(Mandatory)]
        [string] $AssetName,

        [Parameter()]
        [switch] $Optional
    )

    $matches = @($Release.assets | Where-Object { $_.name -ceq $AssetName })
    if ($matches.Count -eq 1) {
        return $matches[0]
    }
    if ($matches.Count -gt 1) {
        throw "Release contains duplicate assets named '$AssetName'."
    }
    if ($Optional) {
        return $null
    }
    throw "Release asset not found: $AssetName"
}

function Read-WindowSwitcherChecksum {
    param(
        [Parameter(Mandatory)]
        [string] $ChecksumPath,

        [Parameter(Mandatory)]
        [string] $ArchiveName
    )

    $checksumText = [System.IO.File]::ReadAllText($ChecksumPath).Trim().TrimStart([char] 0xfeff)
    $checksumMatch = [regex]::Match(
        $checksumText,
        '^(?<hash>[0-9a-fA-F]{64})\s+\*?(?<name>.+)$'
    )
    if (-not $checksumMatch.Success -or $checksumMatch.Groups['name'].Value -cne $ArchiveName) {
        throw "Invalid checksum file for '$ArchiveName'."
    }
    $checksumMatch.Groups['hash'].Value.ToLowerInvariant()
}

function ConvertTo-WindowSwitcherDownloadUri {
    param(
        [Parameter(Mandatory)]
        [string] $Value
    )

    $downloadUri = [Uri] $Value
    if (-not $downloadUri.IsAbsoluteUri -or
        $downloadUri.Scheme -cne [Uri]::UriSchemeHttps -or
        $downloadUri.Host -ine 'github.com') {
        throw "Refusing unexpected release asset URI: '$Value'."
    }
    $downloadUri
}

function Assert-WindowSwitcherArchive {
    param(
        [Parameter(Mandatory)]
        [string] $ArchivePath,

        [Parameter(Mandatory)]
        [string] $ExecutableName
    )

    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entries = @($archive.Entries)
        if ($entries.Count -ne 1 -or
            $entries[0].FullName -cne $ExecutableName -or
            $entries[0].Length -le 0) {
            throw "Archive must contain exactly one non-empty '$ExecutableName' entry."
        }
    } finally {
        $archive.Dispose()
    }
}

if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )) {
    throw 'Unsupported operating system. Only Windows is supported.'
}
if ($Repository.Split('/') | Where-Object { $_ -in '.', '..' }) {
    throw "Unsupported repository name: '$Repository'."
}
if (-not [string]::IsNullOrWhiteSpace($Tag) -and
    $Tag -notmatch $releaseTagPattern) {
    throw "Unsupported release tag: '$Tag'."
}

$platform = switch (
    [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
) {
    'X64' { 'windows-64'; break }
    'Arm64' { 'windows-arm64'; break }
    default { throw "Unsupported architecture: $_" }
}

$requestHeaders = Get-WindowSwitcherRequestHeaders
$release = Get-WindowSwitcherRelease -RepositoryName $Repository -RequestedTag $Tag -Headers $requestHeaders
$resolvedTag = [string] $release.tag_name
if ([string]::IsNullOrWhiteSpace($resolvedTag) -or
    $resolvedTag -notmatch $releaseTagPattern) {
    throw "Release metadata contains an unsupported tag name: '$resolvedTag'."
}

$archiveName = "$commandName-$resolvedTag-$platform.zip"
$checksumName = "$archiveName.sha256"
$archiveAsset = Get-WindowSwitcherReleaseAsset -Release $release -AssetName $archiveName
$checksumAsset = Get-WindowSwitcherReleaseAsset -Release $release -AssetName $checksumName -Optional
$archiveDownloadUri = ConvertTo-WindowSwitcherDownloadUri -Value $archiveAsset.browser_download_url
$checksumDownloadUri = if ($null -ne $checksumAsset) {
    ConvertTo-WindowSwitcherDownloadUri -Value $checksumAsset.browser_download_url
} else {
    $null
}
$repositoryUri = "$githubWebBaseUri/$Repository"
$resolvedInstallDirectory = [System.IO.Path]::GetFullPath($InstallDirectory)
$destinationExecutable = Join-Path $resolvedInstallDirectory "$commandName.exe"

Write-Host "Repository:  $repositoryUri"
Write-Host "Tag:         $resolvedTag"
Write-Host "Target:      $platform"
Write-Host "Destination: $resolvedInstallDirectory"

try {
    $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    $temporaryDirectory = Join-Path $temporaryRoot (
        "$commandName-install-$([Guid]::NewGuid().ToString('N'))"
    )
    $resolvedTemporaryDirectory = [System.IO.Path]::GetFullPath($temporaryDirectory)
    if ([System.IO.Path]::GetDirectoryName($resolvedTemporaryDirectory) -ine
        $temporaryRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar)) {
        throw "Refusing to use unexpected temporary directory: $resolvedTemporaryDirectory"
    }
    $null = [System.IO.Directory]::CreateDirectory($resolvedTemporaryDirectory)

    $archivePath = Join-Path $resolvedTemporaryDirectory $archiveName
    $checksumPath = Join-Path $resolvedTemporaryDirectory $checksumName
    Invoke-WebRequest -Uri $archiveDownloadUri -OutFile $archivePath

    $expectedHashes = [System.Collections.Generic.List[string]]::new()
    $digestProperty = $archiveAsset.PSObject.Properties['digest']
    if ($null -ne $digestProperty -and
        -not [string]::IsNullOrWhiteSpace([string] $digestProperty.Value)) {
        $digestMatch = [regex]::Match(
            [string] $digestProperty.Value,
            '^sha256:(?<hash>[0-9a-fA-F]{64})$'
        )
        if (-not $digestMatch.Success) {
            throw "Unsupported release digest: '$($digestProperty.Value)'."
        }
        $expectedHashes.Add($digestMatch.Groups['hash'].Value.ToLowerInvariant())
    }

    if ($null -ne $checksumAsset) {
        Invoke-WebRequest -Uri $checksumDownloadUri -OutFile $checksumPath
        $checksumReadParameters = @{
            ChecksumPath = $checksumPath
            ArchiveName = $archiveName
        }
        $expectedHashes.Add((Read-WindowSwitcherChecksum @checksumReadParameters))
    }
    $distinctExpectedHashes = @($expectedHashes | Select-Object -Unique)
    if ($distinctExpectedHashes.Count -eq 0) {
        throw "Release '$resolvedTag' does not provide a SHA-256 digest or checksum file."
    }
    if ($distinctExpectedHashes.Count -ne 1) {
        throw "Release digest and checksum file disagree for '$archiveName'."
    }

    $actualArchiveHash = (
        Get-FileHash -LiteralPath $archivePath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    if ($actualArchiveHash -cne $distinctExpectedHashes[0]) {
        throw "SHA-256 verification failed for '$archiveName'."
    }

    Assert-WindowSwitcherArchive -ArchivePath $archivePath -ExecutableName "$commandName.exe"
    $extractionDirectory = Join-Path $resolvedTemporaryDirectory 'extracted'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractionDirectory
    $sourceExecutable = Join-Path $extractionDirectory "$commandName.exe"
    if (-not (Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) {
        throw "The verified archive does not contain '$commandName.exe'."
    }

    $null = [System.IO.Directory]::CreateDirectory($resolvedInstallDirectory)
    $installationNonce = [Guid]::NewGuid().ToString('N')
    $stagedExecutable = Join-Path $resolvedInstallDirectory ".$commandName.$installationNonce.tmp"
    $backupExecutable = Join-Path $resolvedInstallDirectory ".$commandName.$installationNonce.bak"
    Copy-Item -LiteralPath $sourceExecutable -Destination $stagedExecutable

    if ([System.IO.File]::Exists($destinationExecutable)) {
        try {
            [System.IO.File]::Replace(
                $stagedExecutable,
                $destinationExecutable,
                $backupExecutable,
                $true
            )
        } catch {
            throw "Failed to replace '$destinationExecutable'. Close the running application and retry: $($_.Exception.Message)"
        }
    } else {
        [System.IO.File]::Move($stagedExecutable, $destinationExecutable)
    }
    $stagedExecutable = $null

    Write-Host "SHA-256:    $actualArchiveHash"
    Write-Host 'Installation successful.'
    if ($Launch) {
        Start-Process -FilePath $destinationExecutable
    } else {
        Write-Host "Run '$destinationExecutable' to start Window Switcher."
    }
} finally {
    if ($null -ne $stagedExecutable -and [System.IO.File]::Exists($stagedExecutable)) {
        try {
            [System.IO.File]::Delete($stagedExecutable)
        } catch {
            Write-Warning "Failed to remove staged executable '$stagedExecutable': $($_.Exception.Message)"
        }
    }
    if ($null -ne $backupExecutable -and [System.IO.File]::Exists($backupExecutable)) {
        try {
            [System.IO.File]::Delete($backupExecutable)
        } catch {
            Write-Warning "Failed to remove backup executable '$backupExecutable': $($_.Exception.Message)"
        }
    }
    if ($null -ne $temporaryDirectory -and
        [System.IO.Directory]::Exists($temporaryDirectory)) {
        $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
        $resolvedTemporaryDirectory = [System.IO.Path]::GetFullPath($temporaryDirectory)
        if ([System.IO.Path]::GetDirectoryName($resolvedTemporaryDirectory) -ieq
            $temporaryRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar)) {
            try {
                [System.IO.Directory]::Delete($resolvedTemporaryDirectory, $true)
            } catch {
                Write-Warning "Failed to remove temporary directory '$resolvedTemporaryDirectory': $($_.Exception.Message)"
            }
        } else {
            Write-Warning "Temporary directory was not removed because its path was unexpected: $resolvedTemporaryDirectory"
        }
    }
}
