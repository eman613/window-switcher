using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;

namespace WindowSwitcher.Performance
{
    public sealed class WindowProbe : ICycleTarget
    {
        private const uint OpenMessage = 6010;
        private const uint CancelMessage = 6012;
        private const uint CommandMessage = 0x0111;
        private const uint ExitCommand = 1;
        private const uint SendFlags = 0x0001 | 0x0002 | 0x0020;
        private const int UserDataIndex = -21;
        private readonly Process process;
        private readonly IntPtr window;
        private readonly object processLock = new object();

        public WindowProbe(Process process, IntPtr window)
        {
            this.process = process ?? throw new ArgumentNullException(nameof(process));
            this.window = window;
            CheckWindow();
        }

        public bool IsVisible
        {
            get
            {
                CheckWindow();
                bool visible = Native.IsWindowVisible(window);
                if (visible)
                {
                    Rect bounds;
                    if (!Native.GetWindowRect(window, out bounds))
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "GetWindowRect failed.");
                    if (bounds.Right <= bounds.Left || bounds.Bottom <= bounds.Top)
                        throw new InvalidOperationException("Visible switcher window has empty bounds.");
                }
                return visible;
            }
        }

        public uint Dpi { get { return Native.GetDpiForWindow(window); } }

        public void Open(int timeoutMilliseconds)
        {
            CheckWindow();
            Send(window, OpenMessage, UIntPtr.Zero, IntPtr.Zero, timeoutMilliseconds);
        }

        public void Close(int timeoutMilliseconds)
        {
            CheckWindow();
            Send(window, CancelMessage, UIntPtr.Zero, IntPtr.Zero, timeoutMilliseconds);
        }

        public ResourceSample Capture()
        {
            lock (processLock)
            {
                CheckWindow();
                process.Refresh();
                return new ResourceSample
                {
                    TimestampUtc = DateTime.UtcNow.ToString("o"),
                    WorkingSetBytes = process.WorkingSet64,
                    PrivateMemoryBytes = process.PrivateMemorySize64,
                    CpuSeconds = process.TotalProcessorTime.TotalSeconds,
                    HandleCount = process.HandleCount,
                    GdiObjects = ReadGuiResources(process.Handle, 0),
                    UserObjects = ReadGuiResources(process.Handle, 1),
                    ThreadCount = process.Threads.Count,
                    WindowVisible = IsVisible
                };
            }
        }

        private void CheckWindow()
        {
            lock (processLock)
            {
                if (process.HasExited || !Native.IsWindow(window))
                    throw new InvalidOperationException("The tested process or window has exited.");
                uint owner;
                if (Native.GetWindowThreadProcessId(window, out owner) == 0 || owner != process.Id)
                    throw new InvalidOperationException("Window ownership changed during the test.");
            }
        }

        public static IntPtr FindReadyWindow(int processId)
        {
            IntPtr found = IntPtr.Zero;
            Native.EnumWindowsCallback callback = delegate(IntPtr candidate, IntPtr unused)
            {
                uint owner;
                Native.GetWindowThreadProcessId(candidate, out owner);
                if (owner != processId) return true;
                var name = new StringBuilder(256);
                if (Native.GetClassNameW(candidate, name, name.Capacity) == 0 ||
                    name.ToString() != "Window Switcher") return true;
                IntPtr data = IntPtr.Size == 8
                    ? Native.GetWindowLongPtrW(candidate, UserDataIndex)
                    : new IntPtr(Native.GetWindowLongW(candidate, UserDataIndex));
                if (data == IntPtr.Zero) return true;
                found = candidate;
                return false;
            };
            Native.EnumWindows(callback, IntPtr.Zero);
            GC.KeepAlive(callback);
            return found;
        }

        public static void RequestExit(int processId, int timeoutMilliseconds)
        {
            IntPtr window = FindReadyWindow(processId);
            if (window == IntPtr.Zero)
                throw new InvalidOperationException("No initialized switcher window was found for exit.");
            Send(window, CommandMessage, new UIntPtr(ExitCommand), IntPtr.Zero, timeoutMilliseconds);
        }

        public static string GetProcessImagePath(int processId)
        {
            IntPtr handle = Native.OpenProcess(0x1000, false, processId);
            if (handle == IntPtr.Zero)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot query the existing process path.");
            try
            {
                const int capacity = 32768;
                var path = new StringBuilder(capacity);
                uint length = capacity;
                if (!Native.QueryFullProcessImageNameW(handle, 0, path, ref length))
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "QueryFullProcessImageNameW failed.");
                if (length == 0 || length >= capacity)
                    throw new InvalidOperationException("The existing process path is empty or truncated.");
                return path.ToString();
            }
            finally { Native.CloseHandle(handle); }
        }

        private static int ReadGuiResources(IntPtr processHandle, uint kind)
        {
            Native.SetLastError(0);
            uint count = Native.GetGuiResources(processHandle, kind);
            int error = Marshal.GetLastWin32Error();
            if (count == 0 && error != 0)
                throw new Win32Exception(error, "GetGuiResources failed.");
            return checked((int)count);
        }

        private static void Send(IntPtr window, uint message, UIntPtr wparam, IntPtr lparam, int timeout)
        {
            if (timeout < 1) throw new ArgumentOutOfRangeException(nameof(timeout));
            Native.SetLastError(0);
            UIntPtr response;
            IntPtr result = Native.SendMessageTimeoutW(window, message, wparam, lparam,
                SendFlags, checked((uint)timeout), out response);
            if (result == IntPtr.Zero)
            {
                int error = Marshal.GetLastWin32Error();
                throw new Win32Exception(error,
                    "Window message " + message + " failed or timed out; win32_error=" + error);
            }
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct Rect { public int Left, Top, Right, Bottom; }

        private static class Native
        {
            internal delegate bool EnumWindowsCallback(IntPtr window, IntPtr parameter);
            [DllImport("user32.dll")]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static extern bool EnumWindows(EnumWindowsCallback callback, IntPtr parameter);
            [DllImport("user32.dll")]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static extern bool IsWindow(IntPtr window);
            [DllImport("user32.dll")]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static extern bool IsWindowVisible(IntPtr window);
            [DllImport("user32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static extern bool GetWindowRect(IntPtr window, out Rect bounds);
            [DllImport("user32.dll", SetLastError = true)]
            internal static extern uint GetWindowThreadProcessId(IntPtr window, out uint processId);
            [DllImport("user32.dll", CharSet = CharSet.Unicode)]
            internal static extern int GetClassNameW(IntPtr window, StringBuilder name, int capacity);
            [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
            internal static extern IntPtr GetWindowLongPtrW(IntPtr window, int index);
            [DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
            internal static extern int GetWindowLongW(IntPtr window, int index);
            [DllImport("user32.dll")]
            internal static extern uint GetDpiForWindow(IntPtr window);
            [DllImport("user32.dll", SetLastError = true)]
            internal static extern uint GetGuiResources(IntPtr process, uint flags);
            [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
            internal static extern IntPtr SendMessageTimeoutW(IntPtr window, uint message, UIntPtr wparam,
                IntPtr lparam, uint flags, uint timeout, out UIntPtr response);
            [DllImport("kernel32.dll")]
            internal static extern void SetLastError(uint error);
            [DllImport("kernel32.dll", SetLastError = true)]
            internal static extern IntPtr OpenProcess(uint access, bool inherit, int processId);
            [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags,
                StringBuilder path, ref uint length);
            [DllImport("kernel32.dll")]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static extern bool CloseHandle(IntPtr handle);
        }
    }
}
