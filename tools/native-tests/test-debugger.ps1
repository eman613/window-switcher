#Requires -Version 7.0
param([Parameter(Mandatory)][string]$OutputDirectory)
$ErrorActionPreference = 'Stop'
$output = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $output) { throw 'Probe directory must be new.' }
$null = New-Item -ItemType Directory -Path $output
Add-Type -LiteralPath (Join-Path $PSScriptRoot 'NativeTestDebugger.cs')
$fixture = @'
#[link(name = "kernel32")]
unsafe extern "system" { fn RaiseException(code: u32, flags: u32, count: u32, args: *const usize); }
fn main() {
    println!("native-debugger-fixture");
    match std::env::args().nth(1).as_deref() {
        Some("crash") => unsafe { RaiseException(0xc0000005, 1, 0, std::ptr::null()) },
        Some("timeout") => std::thread::sleep(std::time::Duration::from_secs(30)),
        Some("quoted") => assert_eq!(std::env::args().nth(2).unwrap(), "a b\\\"c\\"),
        _ => (),
    }
}
'@
$source = Join-Path $output 'fixture.rs'
$executable = Join-Path $output 'fixture.exe'
Set-Content -LiteralPath $source -Value $fixture -Encoding UTF8
& rustc --edition=2021 -g $source -o $executable
if ($LASTEXITCODE -ne 0) { throw 'Native debugger fixture compilation failed.' }
foreach ($mode in @('success','quoted','crash','timeout')) {
    $dump = Join-Path $output "$mode.dmp"
    $arguments = if ($mode -eq 'quoted') { @($mode, 'a b\"c\') } else { @($mode) }
    $timeout = if ($mode -eq 'timeout') { 1 } else { 15 }
    $result = [NativeTestDebugger]::Run($executable, $arguments, $output, $dump, $timeout)
    $result | ConvertTo-Json | Set-Content (Join-Path $output "$mode.json") -Encoding UTF8
    if ((Get-Content -LiteralPath ([IO.Path]::ChangeExtension($dump, '.stdout.log')) -Raw) -notmatch 'native-debugger-fixture') {
        throw 'Test standard output was not captured.'
    }
    switch ($mode) {
        'crash' {
            if ($result.ExitCode -ne 0xc0000005u -or $result.ExceptionCode -ne 0xc0000005u -or -not $result.DumpWritten) {
                throw 'First unhandled exception was not preserved.'
            }
            $bytes = [IO.File]::ReadAllBytes($dump)
            if ([Text.Encoding]::ASCII.GetString($bytes,0,4) -ne 'MDMP') { throw 'Invalid minidump header.' }
        }
        'timeout' { if (-not $result.TimedOut -or $result.ExitCode -eq 0) { throw 'Timeout did not terminate owned fixture.' } }
        default { if ($result.ExitCode -ne 0 -or (Test-Path -LiteralPath $dump)) { throw 'Normal test/argument quoting failed.' } }
    }
}
Write-Output 'Debugger probes passed: normal exit, quoted arguments, native exception dump, bounded timeout.'
