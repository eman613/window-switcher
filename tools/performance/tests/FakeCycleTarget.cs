using System;
using System.Threading;

namespace WindowSwitcher.Performance.Tests
{
    public sealed class FakeCycleTarget : ICycleTarget
    {
        private bool visible;
        private int openCalls;
        public bool IgnoreOpen { get; set; }
        public bool IgnoreClose { get; set; }
        public int FailOnOpen { get; set; }
        public long PrivateBytes { get; set; } = 10 * 1024 * 1024;
        public bool IsVisible { get { return Volatile.Read(ref visible); } }
        public void SetVisible(bool value) { Volatile.Write(ref visible, value); }

        public void Open(int timeoutMilliseconds)
        {
            int count = Interlocked.Increment(ref openCalls);
            if (count == FailOnOpen) throw new InvalidOperationException("Injected open failure.");
            if (!IgnoreOpen) SetVisible(true);
        }

        public void Close(int timeoutMilliseconds)
        {
            if (!IgnoreClose) SetVisible(false);
        }

        public ResourceSample Capture()
        {
            return new ResourceSample
            {
                TimestampUtc = DateTime.UtcNow.ToString("o"),
                WorkingSetBytes = 12 * 1024 * 1024,
                PrivateMemoryBytes = PrivateBytes,
                HandleCount = 40, GdiObjects = 20, UserObjects = 10, ThreadCount = 5,
                WindowVisible = IsVisible
            };
        }
    }
}
