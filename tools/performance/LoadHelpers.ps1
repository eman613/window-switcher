$script:WindowSwitcherPerformanceHelperRoot = $PSScriptRoot

function Get-WindowSwitcherPerformanceHelperManifest {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $SourcePaths
    )

    $entries = @(
        foreach ($sourcePath in $SourcePaths) {
            $resolved = (Resolve-Path -LiteralPath $sourcePath -ErrorAction Stop).ProviderPath
            if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
                throw "Performance helper source is not a file: $resolved"
            }
            $relative = [IO.Path]::GetRelativePath(
                $script:WindowSwitcherPerformanceHelperRoot,
                $resolved
            ).Replace('\', '/')
            [pscustomobject]@{
                Path = $resolved
                RelativePath = $relative
                Sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
    ) | Sort-Object RelativePath

    if ($entries.Count -eq 0) {
        throw 'No performance helper sources were supplied.'
    }

    $manifestText = (($entries | ForEach-Object {
        "$($_.RelativePath)=$($_.Sha256)"
    }) -join "`n") + "`n"
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $manifestHash = [Convert]::ToHexString(
            $sha.ComputeHash([Text.UTF8Encoding]::new($false).GetBytes($manifestText))
        ).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }

    [pscustomobject]@{
        Entries = $entries
        Text = $manifestText
        Sha256 = $manifestHash
    }
}

function Import-WindowSwitcherPerformanceHelpers {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]] $SourcePaths
    )

    $manifest = Get-WindowSwitcherPerformanceHelperManifest -SourcePaths $SourcePaths
    $runnerType = 'WindowSwitcher.Performance.CycleRunner' -as [type]
    if ($null -ne $runnerType) {
        $assembly = $runnerType.Assembly
        $metadata = @(
            $assembly.GetCustomAttributes([Reflection.AssemblyMetadataAttribute], $false) |
                Where-Object Key -CEq 'WindowSwitcher.Performance.HelperSourceSha256'
        )
        if ($metadata.Count -ne 1 -or $metadata[0].Value -cne $manifest.Sha256) {
            throw 'The loaded performance helper assembly does not match the current helper sources.'
        }
        return [pscustomobject]@{
            SourceSha256 = $manifest.Sha256
            SourceEntries = $manifest.Entries
            AssemblySha256 = $null
            AssemblyFullName = $assembly.FullName
            ModuleVersionId = $assembly.ManifestModule.ModuleVersionId.ToString()
            AssemblyPath = $assembly.Location
        }
    }

    $assemblyDirectory = Join-Path ([IO.Path]::GetTempPath()) (
        'window-switcher-performance-helpers-' + [Guid]::NewGuid().ToString('N')
    )
    $assemblyPath = Join-Path $assemblyDirectory 'helpers.dll'
    $metadataPath = Join-Path $assemblyDirectory 'source-metadata.cs'
    $null = New-Item -ItemType Directory -Path $assemblyDirectory -Force
    $metadataSource = @"
using System.Reflection;
[assembly: AssemblyMetadata("WindowSwitcher.Performance.HelperSourceSha256", "$($manifest.Sha256)")]
"@
    [IO.File]::WriteAllText($metadataPath, $metadataSource, [Text.UTF8Encoding]::new($false))
    try {
        Add-Type -Path (@($manifest.Entries.Path) + $metadataPath) -OutputAssembly $assemblyPath
        $assemblyBytes = [IO.File]::ReadAllBytes($assemblyPath)
        $assemblySha256 = [Convert]::ToHexString(
            [Security.Cryptography.SHA256]::HashData($assemblyBytes)
        ).ToLowerInvariant()
        $assembly = [Reflection.Assembly]::Load($assemblyBytes)
        $metadata = @(
            $assembly.GetCustomAttributes([Reflection.AssemblyMetadataAttribute], $false) |
                Where-Object Key -CEq 'WindowSwitcher.Performance.HelperSourceSha256'
        )
        if ($metadata.Count -ne 1 -or $metadata[0].Value -cne $manifest.Sha256) {
            throw 'The compiled performance helper assembly metadata does not match its sources.'
        }
        if ($null -eq $assembly.GetType('WindowSwitcher.Performance.CycleRunner', $false)) {
            throw 'The performance helper assembly is missing CycleRunner.'
        }
        [pscustomobject]@{
            SourceSha256 = $manifest.Sha256
            SourceEntries = $manifest.Entries
            AssemblySha256 = $assemblySha256
            AssemblyFullName = $assembly.FullName
            ModuleVersionId = $assembly.ManifestModule.ModuleVersionId.ToString()
            AssemblyPath = $assemblyPath
        }
    } finally {
        foreach ($path in @($metadataPath, $assemblyPath)) {
            if (Test-Path -LiteralPath $path -PathType Leaf) {
                [IO.File]::Delete($path)
            }
        }
        if (Test-Path -LiteralPath $assemblyDirectory -PathType Container) {
            [IO.Directory]::Delete($assemblyDirectory)
        }
    }
}
