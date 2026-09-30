// x64/ARM64 Windows SDK layouts: DEBUG_EVENT, STARTUPINFOW, PROCESS_INFORMATION.
// A separate debugger owns only the test process it creates. No registry/WER changes.
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class NativeTestDebugger
{
    [StructLayout(LayoutKind.Explicit, Size = 176)]
    struct DebugEvent
    {
        [FieldOffset(0)] public uint Kind;
        [FieldOffset(4)] public uint ProcessId;
        [FieldOffset(8)] public uint ThreadId;
        [FieldOffset(16)] public uint Code;
        [FieldOffset(16)] public IntPtr File;
        [FieldOffset(32)] public IntPtr ExceptionAddress;
        [FieldOffset(168)] public uint FirstChance;
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct StartupInfo
    {
        public uint Size;
        public string Reserved, Desktop, Title;
        public uint X, Y, Width, Height, XChars, YChars, Fill, Flags;
        public ushort Show, ReservedSize;
        public IntPtr ReservedBytes, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInfo
    {
        public IntPtr Process, Thread;
        public uint ProcessId, ThreadId;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string image, StringBuilder command, IntPtr processAttributes,
        IntPtr threadAttributes, bool inheritHandles, uint flags, IntPtr environment,
        string directory, ref StartupInfo startup, out ProcessInfo process);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WaitForDebugEvent(out DebugEvent debugEvent, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool ContinueDebugEvent(uint process, uint thread, uint status);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr process, uint status);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll")]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);
    [DllImport("dbghelp.dll", SetLastError = true)]
    static extern bool MiniDumpWriteDump(IntPtr process, uint processId, IntPtr file, uint type,
        IntPtr exception, IntPtr streams, IntPtr callback);

    public sealed class Result
    {
        public uint ExitCode, ExceptionCode, ThreadId;
        public long ExceptionAddress;
        public bool TimedOut, OutputLimitExceeded, DumpWritten;
        public string DumpError;
    }

    // Windows command-line quoting, without any shell expansion.
    static string Quote(string value)
    {
        var result = new StringBuilder("\"");
        int slashes = 0;
        foreach (char ch in value)
        {
            if (ch == '\\') { slashes++; continue; }
            result.Append('\\', ch == '"' ? slashes * 2 + 1 : slashes);
            result.Append(ch);
            slashes = 0;
        }
        return result.Append('\\', slashes * 2).Append('"').ToString();
    }

    public static Result Run(string executable, string[] arguments, string workingDirectory, string dumpPath, int timeoutSeconds)
    {
        if (IntPtr.Size != 8 || Marshal.SizeOf<StartupInfo>() != 104)
            throw new PlatformNotSupportedException("Requires native 64-bit Windows PowerShell 7.");
        if (timeoutSeconds < 1 || timeoutSeconds > 900)
            throw new ArgumentOutOfRangeException(nameof(timeoutSeconds));
        var command = new StringBuilder(Quote(executable));
        foreach (string arg in arguments) command.Append(' ').Append(Quote(arg));
        if (command.Length >= 32767) throw new ArgumentException("Command line too long.");
        using var output = new FileStream(Path.ChangeExtension(dumpPath, ".stdout.log"),
            FileMode.CreateNew, FileAccess.Write, FileShare.ReadWrite);
        IntPtr outputHandle = output.SafeFileHandle.DangerousGetHandle();
        if (!SetHandleInformation(outputHandle, 1, 1))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Set test output inheritance");
        var startup = new StartupInfo { Size = 104, Flags = 0x100, Output = outputHandle, Error = outputHandle };
        ProcessInfo process;
        const uint DebugOnlyThisProcess = 2;
        if (!CreateProcessW(executable, command, IntPtr.Zero, IntPtr.Zero, true,
            DebugOnlyThisProcess, IntPtr.Zero, workingDirectory, ref startup, out process))
            throw new Win32Exception(Marshal.GetLastWin32Error(), "Create test process");
        var result = new Result();
        var timer = Stopwatch.StartNew();
        bool exited = false, initialBreakpoint = true;
        try
        {
            while (!exited)
            {
                if (!result.TimedOut && (timer.Elapsed.TotalSeconds > timeoutSeconds || output.Length > 32L * 1024 * 1024))
                {
                    result.OutputLimitExceeded = output.Length > 32L * 1024 * 1024;
                    result.TimedOut = true;
                    if (!TerminateProcess(process.Process, 1460))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "Terminate timed-out test");
                }
                if (result.TimedOut && timer.Elapsed.TotalSeconds > timeoutSeconds + 10)
                    throw new TimeoutException("Timed-out test did not report exit.");
                DebugEvent ev;
                if (!WaitForDebugEvent(out ev, 250))
                {
                    int error = Marshal.GetLastWin32Error();
                    if (error == 121) continue; // ERROR_SEM_TIMEOUT
                    throw new Win32Exception(error, "Wait for test debug event");
                }
                uint status = 0x00010002; // DBG_CONTINUE
                if ((ev.Kind == 3 || ev.Kind == 6) && ev.File != IntPtr.Zero && ev.File != new IntPtr(-1))
                    CloseHandle(ev.File); // Image/DLL file; event process/thread handles are system-owned.
                if (ev.Kind == 1)
                {
                    status = 0x80010001; // DBG_EXCEPTION_NOT_HANDLED
                    if (initialBreakpoint && ev.Code == 0x80000003 && ev.FirstChance != 0)
                    {
                        initialBreakpoint = false;
                        status = 0x00010002;
                    }
                    else if (ev.FirstChance == 0 && result.ExceptionCode == 0)
                    {
                        result.ExceptionCode = ev.Code;
                        result.ExceptionAddress = ev.ExceptionAddress.ToInt64();
                        result.ThreadId = ev.ThreadId;
                        try
                        {
                            using (var dump = new FileStream(dumpPath, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                            {
                                // MiniDumpNormal captures stopped thread contexts/stacks, not full process memory.
                                if (!MiniDumpWriteDump(process.Process, process.ProcessId,
                                    dump.SafeFileHandle.DangerousGetHandle(), 0, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero))
                                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Write test minidump");
                                if (dump.Length > 64L * 1024 * 1024)
                                {
                                    dump.SetLength(0);
                                    throw new IOException("Minidump exceeded 64 MiB retention limit.");
                                }
                                result.DumpWritten = true;
                            }
                        }
                        catch (Exception error) { result.DumpError = error.Message; }
                    }
                }
                if (ev.Kind == 5) { result.ExitCode = ev.Code; exited = true; }
                if (!ContinueDebugEvent(ev.ProcessId, ev.ThreadId, status))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Continue test debug event");
            }
            return result;
        }
        finally
        {
            if (!exited)
            {
                TerminateProcess(process.Process, 1);
                // Drain exit events so a cleanup error cannot leave a suspended test process.
                var cleanup = Stopwatch.StartNew();
                while (cleanup.Elapsed.TotalSeconds < 5)
                {
                    DebugEvent ev;
                    if (!WaitForDebugEvent(out ev, 100)) continue;
                    if ((ev.Kind == 3 || ev.Kind == 6) && ev.File != IntPtr.Zero && ev.File != new IntPtr(-1))
                        CloseHandle(ev.File);
                    ContinueDebugEvent(ev.ProcessId, ev.ThreadId, 0x00010002);
                    if (ev.Kind == 5) break;
                }
                WaitForSingleObject(process.Process, 1000);
            }
            CloseHandle(process.Thread);
            CloseHandle(process.Process);
        }
    }
}
