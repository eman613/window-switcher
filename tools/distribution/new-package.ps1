#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$')][string]$Tag,
    [Parameter(Mandatory)][ValidateSet('windows-64', 'windows-arm64')][string]$Platform,
    [Parameter(Mandatory)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
if (-not (Test-Path -LiteralPath $output)) { $null = New-Item -ItemType Directory -Path $output }
if ((Get-Item -LiteralPath $output).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Package output cannot be a reparse point.' }
$archive = Join-Path $output "window-switcher-$Tag-$Platform.zip"
if ((Test-Path -LiteralPath $archive) -or (Test-Path -LiteralPath "$archive.sha256")) { throw 'Package output already exists.' }
$stage = Join-Path $output ('.package-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $stage
try {
    $inputs = [ordered]@{
        'window-switcher.exe' = $Executable
        'window-switcher.ini' = Join-Path $repository 'window-switcher.ini'
        'LICENSE' = Join-Path $repository 'LICENSE'
        'USAGE.txt' = Join-Path $repository 'assets/USAGE.txt'
    }
    foreach ($entry in $inputs.GetEnumerator()) {
        if ((Get-Item -LiteralPath $entry.Value).Length -eq 0) { throw "Empty package input: $($entry.Key)" }
        Copy-Item -LiteralPath $entry.Value -Destination (Join-Path $stage $entry.Key)
    }
    $files = @($inputs.Keys | ForEach-Object { Join-Path $stage $_ })
    Compress-Archive -LiteralPath $files -DestinationPath $archive -CompressionLevel Optimal
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($archive))" | Set-Content -LiteralPath "$archive.sha256" -Encoding UTF8
    $validation = & (Join-Path $repository 'install.ps1') -ArchivePath $archive -Tag $Tag -Platform $Platform -ValidateOnly -RequireUsage
    if (-not $validation.Validated) { throw 'Package validation did not complete.' }
    [pscustomobject]@{ Archive = $archive; SHA256 = $hash; Platform = $Platform; Tag = $Tag }
} finally {
    $stageFull = [IO.Path]::GetFullPath($stage)
    $boundary = [IO.Path]::GetFullPath($output).TrimEnd('\') + '\'
    if (-not $stageFull.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase) -or
        ((Get-Item -LiteralPath $stageFull).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Refusing unsafe package staging cleanup.'
    }
    Remove-Item -LiteralPath $stageFull -Recurse -ErrorAction Stop
}
