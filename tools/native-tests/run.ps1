#Requires -Version 7.0
[CmdletBinding()]
param(
    [string]$Target,
    [string]$OutputDirectory,
    [ValidateRange(1,900)][int]$TimeoutSeconds = 300,
    [string[]]$TestArguments = @()
)
$ErrorActionPreference = 'Stop'
$repository = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../..')).Path
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repository 'target/native-test-evidence' }
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw 'Evidence directory must be new; first failure evidence is never overwritten.' }
$null = New-Item -ItemType Directory -Path $output
Add-Type -LiteralPath (Join-Path $PSScriptRoot 'NativeTestDebugger.cs')
Push-Location -LiteralPath $repository
try {
    $buildArgs = @('test','--locked','--workspace','--all-targets','--no-run','--message-format=json')
    if ($Target) { $buildArgs += @('--target', $Target) }
    & cargo @buildArgs 1> (Join-Path $output 'build.jsonl') 2> (Join-Path $output 'build.stderr.log')
    if ($LASTEXITCODE -ne 0) { throw 'Test compilation failed; see build.stderr.log.' }
    $binaries = @(Get-Content -LiteralPath (Join-Path $output 'build.jsonl') | ForEach-Object {
        $entry = $_ | ConvertFrom-Json
        if ($entry.reason -eq 'compiler-artifact' -and $entry.profile.test -and $entry.executable) { $entry.executable }
    } | Sort-Object -Unique)
    if ($binaries.Count -eq 0) { throw 'Cargo produced no test executables.' }
    $sha = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Cannot identify tested commit.' }
    $metadata = [ordered]@{
        commit=$sha; target=$Target; rustc=(& rustc -Vv) -join "`n"
        working_tree_dirty=[bool](& git status --porcelain)
        test_arguments=$TestArguments; timeout_seconds=$TimeoutSeconds
        capture='DEBUG_ONLY_THIS_PROCESS; first unhandled exception; MiniDumpNormal; no automatic retry'
    }
    $metadata | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $output 'run.json') -Encoding UTF8
    foreach ($binary in $binaries) {
        $name = [IO.Path]::GetFileNameWithoutExtension($binary)
        $directory = Join-Path $output $name
        $null = New-Item -ItemType Directory -Path $directory
        $result = [NativeTestDebugger]::Run($binary, $TestArguments, $repository,
            (Join-Path $directory 'first-failure.dmp'), $TimeoutSeconds)
        [pscustomobject]@{
            executable=$binary; sha256=(Get-FileHash -LiteralPath $binary).Hash
            exit_code=('0x{0:X8}' -f $result.ExitCode); exception_code=('0x{0:X8}' -f $result.ExceptionCode)
            exception_address=('0x{0:X}' -f $result.ExceptionAddress); thread_id=$result.ThreadId
            timed_out=$result.TimedOut; output_limit_exceeded=$result.OutputLimitExceeded
            dump_written=$result.DumpWritten; dump_error=$result.DumpError
        } | ConvertTo-Json | Set-Content (Join-Path $directory 'result.json') -Encoding UTF8
        if ($result.ExitCode -ne 0 -or $result.TimedOut) {
            # Preserve matching image/symbols only for the first failing executable.
            foreach ($path in @($binary, [IO.Path]::ChangeExtension($binary, '.pdb'))) {
                if (Test-Path -LiteralPath $path) {
                    if ((Get-Item -LiteralPath $path).Length -gt 256MB) { Write-Warning 'Symbol/image exceeds retention budget.' }
                    else { Copy-Item -LiteralPath $path -Destination $directory }
                }
            }
            throw "Native tests failed ($name); original evidence retained at $directory"
        }
        Get-Content -LiteralPath (Join-Path $directory 'first-failure.stdout.log') -Tail 3
    }
    Write-Output "Native test executables passed: $($binaries.Count)."
} finally {
    Pop-Location
}
