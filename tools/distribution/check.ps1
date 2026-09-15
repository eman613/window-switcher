#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('scripts', 'workflows', 'dependencies')][string]$Check,
    [string]$ToolPath
)
$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
if ($Check -eq 'scripts') {
    $scripts = @((Join-Path $repository 'install.ps1')) + @(Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1' -File | Select-Object -ExpandProperty FullName)
    foreach ($path in $scripts) {
        $tokens = $null
        $parseErrors = $null
        $null = [Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$parseErrors)
        if ($parseErrors.Count) { throw "PowerShell syntax failed in ${path}: $($parseErrors.Message -join '; ')" }
    }
    Write-Output "PowerShell syntax passed: $($scripts.Count) scripts."
    return
}
$verificationTools = @{
    workflows = @{
        Version = '1.7.12'
        Uri = 'https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_windows_amd64.zip'
        Hash = '6e7241b51e6817ea6a047693d8e6fed13b31819c9a0dd6c5a726e1592d22f6e9'
        Archive = 'actionlint.zip'
        Executable = 'actionlint.exe'
    }
    dependencies = @{
        Version = '0.20.2'
        Uri = 'https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-pc-windows-msvc.tar.gz'
        Hash = '975a22143262fd27476d19ee00c7af67978426e40e1dee94eed6bbade1cf87dc'
        Archive = 'cargo-deny.tar.gz'
        Executable = 'cargo-deny-0.20.2-x86_64-pc-windows-msvc/cargo-deny.exe'
    }
}
$tool = $verificationTools[$Check]
$temporary = $null
$parent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
Push-Location -LiteralPath $repository
try {
    if (-not $ToolPath) {
        $temporary = Join-Path $parent ('window-switcher-check-' + [guid]::NewGuid().ToString('N'))
        $null = New-Item -ItemType Directory -Path $temporary
        $archive = Join-Path $temporary $tool.Archive
        Invoke-WebRequest -Uri $tool.Uri -OutFile $archive -TimeoutSec 60 -MaximumRetryCount 2 -RetryIntervalSec 1
        if ((Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant() -cne $tool.Hash) { throw 'Verification tool SHA-256 mismatch.' }
        if ($Check -eq 'workflows') { Expand-Archive -LiteralPath $archive -DestinationPath $temporary }
        else {
            tar -xzf $archive -C $temporary
            if ($LASTEXITCODE -ne 0) { throw 'Verification tool extraction failed.' }
        }
        $ToolPath = Join-Path $temporary $tool.Executable
    }
    $ToolPath = (Resolve-Path -LiteralPath $ToolPath).Path
    $version = & $ToolPath --version
    if ($LASTEXITCODE -ne 0 -or "$version" -notmatch [regex]::Escape($tool.Version)) { throw 'Unexpected verification tool version.' }
    if ($Check -eq 'workflows') { & $ToolPath }
    else { & $ToolPath --locked --workspace check advisories licenses bans sources }
    if ($LASTEXITCODE -ne 0) { throw "$Check verification failed with exit code $LASTEXITCODE." }
    Write-Output "$Check verification passed using version $($tool.Version)."
} finally {
    Pop-Location
    if ($temporary) {
        $full = [IO.Path]::GetFullPath($temporary)
        if (-not $full.StartsWith($parent.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -or
            ((Get-Item -LiteralPath $full).Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Unsafe verification tool cleanup path.' }
        Remove-Item -LiteralPath $full -Recurse -ErrorAction Stop
    }
}
