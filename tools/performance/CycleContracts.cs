using System;

namespace WindowSwitcher.Performance
{
    public sealed class ResourceSample
    {
        public string TimestampUtc { get; set; }
        public double ElapsedMilliseconds { get; set; }
        public string Phase { get; set; }
        public int CompletedCycles { get; set; }
        public long SampleSequence { get; set; }
        public long DroppedSamples { get; set; }
        public long WorkingSetBytes { get; set; }
        public long PrivateMemoryBytes { get; set; }
        public double CpuSeconds { get; set; }
        public int HandleCount { get; set; }
        public int GdiObjects { get; set; }
        public int UserObjects { get; set; }
        public int ThreadCount { get; set; }
        public bool WindowVisible { get; set; }
    }

    public interface ICycleTarget
    {
        bool IsVisible { get; }
        void Open(int timeoutMilliseconds);
        void Close(int timeoutMilliseconds);
        ResourceSample Capture();
    }

    public sealed class CycleOptions
    {
        public int RequestedCycles { get; set; }
        public int WarmupCycles { get; set; } = 200;
        public int VisibleMilliseconds { get; set; } = 20;
        public int HiddenMilliseconds { get; set; } = 20;
        public int StepTimeoutMilliseconds { get; set; } = 2000;
        public int MaxRunMilliseconds { get; set; } = 3600000;
        public int CheckpointEvery { get; set; } = 100;
        public int CooldownMilliseconds { get; set; } = 10000;
        public long MaxPrivateMemoryBytes { get; set; } = 1024L * 1024 * 1024;
        public int MaxGdiObjects { get; set; } = 8000;
        public int MaxUserObjects { get; set; } = 8000;

        public void Validate()
        {
            if (RequestedCycles < 1 || WarmupCycles < 0 || VisibleMilliseconds < 0 ||
                HiddenMilliseconds < 0 || StepTimeoutMilliseconds < 1 ||
                MaxRunMilliseconds < 1 || CheckpointEvery < 1 || CooldownMilliseconds < 0 ||
                MaxPrivateMemoryBytes < 1 || MaxGdiObjects < 1 || MaxUserObjects < 1)
                throw new ArgumentOutOfRangeException(nameof(RequestedCycles), "Invalid cycle options.");
        }
    }
}
