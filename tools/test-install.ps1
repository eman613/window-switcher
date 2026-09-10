[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryDirectory = Split-Path -Parent $PSScriptRoot
$installerPath = Join-Path $repositoryDirectory 'install.ps1'
$temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testDirectory = Join-Path $temporaryRoot "window-switcher-installer-tests-$([Guid]::NewGuid().ToString('N'))"
$null = New-Item -ItemType Directory -Path $testDirectory
$script:installerTestFixture = $null
$script:installerTestCount = 0
$testRepository = 'window-switcher-test/fixtures'
$testTag = 'v1.20.0'
$testPlatform = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()) {
    'X64' { 'windows-64' }
    'Arm64' { 'windows-arm64' }
    default { throw 'Installer regression tests require Windows x64 or ARM64.' }
}
$testArchiveName = "window-switcher-$testTag-$testPlatform.zip"
$testAssetUri = "https://github.com/$testRepository/releases/download/$testTag/$testArchiveName"

# Shadow the network calls only within this test script. Unexpected URLs fail.
function Invoke-RestMethod {
    param([string] $Method, [Uri] $Uri, [hashtable] $Headers)

    if ($Method -cne 'Get' -or
        $Uri.AbsoluteUri -cne "https://api.github.com/repos/$testRepository/releases/tags/$testTag") {
        throw "Unexpected metadata request: $Method $Uri"
    }
    $installerTestFixture.Release
}

function Invoke-WebRequest {
    param([Uri] $Uri, [string] $OutFile)

    $sourcePath = if ($Uri.AbsoluteUri -ceq $testAssetUri) {
        $installerTestFixture.ArchivePath
    } elseif ($Uri.AbsoluteUri -ceq "$testAssetUri.sha256") {
        $installerTestFixture.ChecksumPath
    } else {
        throw "Unexpected download request: $Uri"
    }
    $installerTestFixture.DownloadDirectory = Split-Path -Parent $OutFile
    Copy-Item -LiteralPath $sourcePath -Destination $OutFile
}

function New-InstallerTestFixture {
    param(
        [string] $Name,
        [AllowEmptyCollection()]
        [object[]] $Entries,
        [switch] $DigestOnly,
        [switch] $WrongHash,
        [switch] $ConflictingDigest
    )

    $archivePath = Join-Path $testDirectory "$Name.zip"
    $archive = [System.IO.Compression.ZipFile]::Open(
        $archivePath, [System.IO.Compression.ZipArchiveMode]::Create
    )
    try {
        foreach ($entry in $Entries) {
            $stream = $archive.CreateEntry($entry.Name).Open()
            try {
                $content = [System.Text.Encoding]::UTF8.GetBytes($entry.Content)
                $stream.Write($content, 0, $content.Length)
            } finally {
                $stream.Dispose()
            }
        }
    } finally {
        $archive.Dispose()
    }

    $archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($WrongHash) {
        $archiveHash = '0' * 64
    }
    $digestHash = if ($ConflictingDigest) { 'f' * 64 } else { $archiveHash }
    $checksumPath = "$archivePath.sha256"
    [System.IO.File]::WriteAllText(
        $checksumPath, "$archiveHash  $testArchiveName`n", [System.Text.UTF8Encoding]::new($false)
    )
    $releaseAssets = @([pscustomobject]@{
            name = $testArchiveName
            browser_download_url = $testAssetUri
            digest = "sha256:$digestHash"
        })
    if (-not $DigestOnly) {
        $releaseAssets += [pscustomobject]@{
            name = "$testArchiveName.sha256"
            browser_download_url = "$testAssetUri.sha256"
        }
    }
    $script:installerTestFixture = [pscustomobject]@{
        ArchivePath = $archivePath
        ChecksumPath = $checksumPath
        DownloadDirectory = $null
        Release = [pscustomobject]@{ tag_name = $testTag; assets = $releaseAssets }
    }
}

function Invoke-InstallerTestCase {
    param(
        [string] $Name,
        [string] $Destination,
        [string] $ExpectedContent,
        [string] $ExpectedError
    )

    $beforeFiles = @{}
    if (Test-Path -LiteralPath $Destination) {
        Get-ChildItem -LiteralPath $Destination -File | ForEach-Object {
            $beforeFiles[$_.Name] = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        }
    }
    $observedError = $null
    try {
        & $installerPath -Repository $testRepository -Tag $testTag -InstallDirectory $Destination 6>$null
    } catch {
        $observedError = $_.Exception.Message
    }
    if ($ExpectedError) {
        if (-not $observedError -or -not $observedError.Contains($ExpectedError)) {
            throw "${Name}: expected '$ExpectedError', got '$observedError'."
        }
    } elseif ($observedError) {
        throw "${Name}: $observedError"
    }

    $executablePath = Join-Path $Destination 'window-switcher.exe'
    if (-not $ExpectedError -and [System.IO.File]::ReadAllText($executablePath) -cne $ExpectedContent) {
        throw "${Name}: installed executable content differs."
    }
    foreach ($fileName in $beforeFiles.Keys) {
        if ($ExpectedError -or $fileName -cne 'window-switcher.exe') {
            $filePath = Join-Path $Destination $fileName
            if ((Get-FileHash -LiteralPath $filePath -Algorithm SHA256).Hash -cne $beforeFiles[$fileName]) {
                throw "${Name}: existing '$fileName' was changed."
            }
        }
    }
    $expectedFileNames = @($beforeFiles.Keys)
    if (-not $ExpectedError -and 'window-switcher.exe' -cnotin $expectedFileNames) {
        $expectedFileNames += 'window-switcher.exe'
    }
    $actualFileNames = @(Get-ChildItem -LiteralPath $Destination -Force | Select-Object -ExpandProperty Name)
    if (@(Compare-Object ($expectedFileNames | Sort-Object) ($actualFileNames | Sort-Object)).Count -ne 0) {
        throw "${Name}: installation added unexpected files or left temporary files."
    }
    if ($script:installerTestFixture.DownloadDirectory -and
        (Test-Path -LiteralPath $script:installerTestFixture.DownloadDirectory)) {
        throw "${Name}: installer download directory was not cleaned up."
    }
    $script:installerTestCount++
    Write-Host "PASS $Name"
}

try {
    $oldExecutable = @{ Name = 'window-switcher.exe'; Content = 'old executable fixture' }
    $newExecutable = @{ Name = 'window-switcher.exe'; Content = 'new executable fixture' }
    $template = @{
        Name = 'window-switcher.ini'
        Content = Get-Content -LiteralPath (Join-Path $repositoryDirectory 'window-switcher.ini') -Raw
    }
    $upgradeDirectory = Join-Path $testDirectory 'upgrade'
    New-InstallerTestFixture -Name 'legacy' -Entries @($oldExecutable) -DigestOnly
    Invoke-InstallerTestCase -Name 'legacy package with API digest only' -Destination $upgradeDirectory -ExpectedContent $oldExecutable.Content

    $configurationBytes = [System.Text.Encoding]::UTF8.GetBytes("[appearance]`nitem_gap = 4`n")
    foreach ($fileName in @('window-switcher.ini', 'windows-switcher.ini')) {
        [System.IO.File]::WriteAllBytes((Join-Path $upgradeDirectory $fileName), $configurationBytes)
    }
    New-InstallerTestFixture -Name 'bundle' -Entries @($newExecutable, $template)
    Invoke-InstallerTestCase -Name 'bundle upgrade preserves current and legacy INI' -Destination $upgradeDirectory -ExpectedContent $newExecutable.Content
    Invoke-InstallerTestCase -Name 'bundle fresh install does not force portable configuration' -Destination (Join-Path $testDirectory 'fresh') -ExpectedContent $newExecutable.Content
    Invoke-InstallerTestCase -Name 'repeated bundle upgrade is atomic' -Destination $upgradeDirectory -ExpectedContent $newExecutable.Content

    $invalidCases = @(
        @{ Name = 'empty archive'; Entries = @() }
        @{ Name = 'missing executable'; Entries = @($template) }
        @{ Name = 'empty executable'; Entries = @(@{ Name = 'window-switcher.exe'; Content = '' }) }
        @{ Name = 'empty configuration'; Entries = @($newExecutable, @{ Name = 'window-switcher.ini'; Content = '' }) }
        @{ Name = 'unexpected second file'; Entries = @($newExecutable, @{ Name = 'extra.dll'; Content = 'unexpected' }) }
        @{ Name = 'extra file'; Entries = @($newExecutable, $template, @{ Name = 'extra.dll'; Content = 'unexpected' }) }
        @{ Name = 'nested executable'; Entries = @(@{ Name = 'nested/window-switcher.exe'; Content = 'unexpected' }) }
        @{ Name = 'parent traversal'; Entries = @($newExecutable, @{ Name = '../escaped.ini'; Content = 'unexpected' }) }
        @{ Name = 'backslash traversal'; Entries = @($newExecutable, @{ Name = '..\escaped.ini'; Content = 'unexpected' }) }
        @{ Name = 'duplicate executable'; Entries = @($newExecutable, $newExecutable) }
        @{ Name = 'duplicate configuration'; Entries = @($newExecutable, $template, $template) }
        @{ Name = 'configuration name casing'; Entries = @($newExecutable, @{ Name = 'WINDOW-SWITCHER.INI'; Content = 'unexpected' }) }
        @{ Name = 'directory entry'; Entries = @($newExecutable, @{ Name = 'nested/'; Content = '' }) }
    )
    foreach ($case in $invalidCases) {
        New-InstallerTestFixture -Name $case.Name -Entries $case.Entries
        Invoke-InstallerTestCase -Name $case.Name -Destination $upgradeDirectory -ExpectedError 'Archive must contain'
    }
    New-InstallerTestFixture -Name 'bad-hash' -Entries @($newExecutable, $template) -WrongHash
    Invoke-InstallerTestCase -Name 'incorrect SHA-256 leaves installation unchanged' -Destination $upgradeDirectory -ExpectedError 'SHA-256 verification failed'
    New-InstallerTestFixture -Name 'conflicting-digest' -Entries @($newExecutable, $template) -ConflictingDigest
    Invoke-InstallerTestCase -Name 'conflicting digest leaves installation unchanged' -Destination $upgradeDirectory -ExpectedError 'Release digest and checksum file disagree'
    Write-Host "Installer regression tests passed: $script:installerTestCount"
} finally {
    $resolvedTestDirectory = [System.IO.Path]::GetFullPath($testDirectory)
    if ([System.IO.Path]::GetDirectoryName($resolvedTestDirectory) -ine
        $temporaryRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar)) {
        throw "Refusing to clean unexpected test directory: $resolvedTestDirectory"
    }
    Remove-Item -LiteralPath $resolvedTestDirectory -Recurse -Force
}
