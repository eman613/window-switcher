using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Threading;
using System.Threading.Tasks;

namespace WindowSwitcher.Performance
{
    public sealed class CycleRunner : IDisposable
    {
        private readonly ICycleTarget target;
        private readonly CycleOptions options;
        private readonly CancellationTokenSource cancellation = new CancellationTokenSource();
        private readonly ConcurrentQueue<ResourceSample> checkpoints = new ConcurrentQueue<ResourceSample>();
        private readonly Stopwatch elapsed = new Stopwatch();
        private int started, warmupCompleted, completed, openAcknowledged, closeAcknowledged;
        private int openedVerified, closedVerified;
        private int openRequested, closeRequested;
        private string phase = "Ready";

        public CycleRunner(ICycleTarget target, CycleOptions options)
        {
            this.target = target ?? throw new ArgumentNullException(nameof(target));
            this.options = options ?? throw new ArgumentNullException(nameof(options));
            options.Validate();
        }

        public Task Completion { get; private set; } = Task.CompletedTask;
        public string Phase { get { return Volatile.Read(ref phase); } }
        public int CompletedCycles { get { return Volatile.Read(ref completed); } }
        public int WarmupCompleted { get { return Volatile.Read(ref warmupCompleted); } }
        public int OpenRequested { get { return Volatile.Read(ref openRequested); } }
        public int CloseRequested { get { return Volatile.Read(ref closeRequested); } }
        public int OpenAcknowledged { get { return Volatile.Read(ref openAcknowledged); } }
        public int CloseAcknowledged { get { return Volatile.Read(ref closeAcknowledged); } }
        public int OpenedVerified { get { return Volatile.Read(ref openedVerified); } }
        public int ClosedVerified { get { return Volatile.Read(ref closedVerified); } }
        public string Error { get; private set; }
        public string CleanupError { get; private set; }
        public double ElapsedMilliseconds { get { return elapsed.Elapsed.TotalMilliseconds; } }
        public ResourceSample[] Checkpoints { get { return checkpoints.ToArray(); } }

        public void Start()
        {
            if (Interlocked.Exchange(ref started, 1) != 0)
                throw new InvalidOperationException("The cycle runner has already started.");
            Completion = Task.Run((Action)Run);
        }

        public void Cancel() { cancellation.Cancel(); }

        public void ValidateResources(ResourceSample sample)
        {
            if (sample.PrivateMemoryBytes > options.MaxPrivateMemoryBytes ||
                sample.GdiObjects >= options.MaxGdiObjects || sample.UserObjects >= options.MaxUserObjects)
                throw new InvalidOperationException("The tested process exceeded the resource safety limit.");
        }

        private void Run()
        {
            elapsed.Start();
            try
            {
                SetPhase("Warmup");
                for (int i = 0; i < options.WarmupCycles; i++)
                {
                    RunOneCycle();
                    Interlocked.Increment(ref warmupCompleted);
                }
                CaptureCheckpoint("Baseline");
                SetPhase("Measure");
                for (int i = 0; i < options.RequestedCycles; i++)
                {
                    RunOneCycle();
                    int count = Interlocked.Increment(ref completed);
                    if (count % options.CheckpointEvery == 0 || count == options.RequestedCycles)
                        CaptureCheckpoint("Closed");
                }
                SetPhase("Cooldown");
                Delay(options.CooldownMilliseconds);
                CaptureCheckpoint("Settled");
                SetPhase("Completed");
            }
            catch (OperationCanceledException) { SetPhase("Cancelled"); }
            catch (Exception error)
            {
                Error = error.ToString();
                SetPhase("Failed");
            }
            finally
            {
                try
                {
                    if (target.IsVisible)
                    {
                        target.Close(options.StepTimeoutMilliseconds);
                        if (target.IsVisible)
                            CleanupError = "The panel remained visible after cleanup cancellation.";
                    }
                }
                catch (Exception error) { CleanupError = error.Message; }
                elapsed.Stop();
            }
        }

        private void RunOneCycle()
        {
            CheckDeadline();
            if (target.IsVisible)
                throw new InvalidOperationException("A cycle must begin with the panel hidden.");
            Interlocked.Increment(ref openRequested);
            target.Open(options.StepTimeoutMilliseconds);
            Interlocked.Increment(ref openAcknowledged);
            WaitForVisibility(true);
            Interlocked.Increment(ref openedVerified);
            Delay(options.VisibleMilliseconds);
            if (!target.IsVisible)
                throw new InvalidOperationException("The panel disappeared before cancellation.");
            Interlocked.Increment(ref closeRequested);
            target.Close(options.StepTimeoutMilliseconds);
            Interlocked.Increment(ref closeAcknowledged);
            WaitForVisibility(false);
            Interlocked.Increment(ref closedVerified);
            Delay(options.HiddenMilliseconds);
            if (target.IsVisible)
                throw new InvalidOperationException("The panel became visible after cancellation.");
        }

        private void WaitForVisibility(bool expected)
        {
            var wait = Stopwatch.StartNew();
            while (target.IsVisible != expected)
            {
                if (wait.ElapsedMilliseconds >= options.StepTimeoutMilliseconds)
                    throw new TimeoutException("Window visibility did not become " + expected + " after message acknowledgement.");
                Delay(5);
            }
            CheckDeadline();
        }

        private void CaptureCheckpoint(string checkpointPhase)
        {
            CheckDeadline();
            ResourceSample sample = target.Capture();
            if (sample.WindowVisible)
                throw new InvalidOperationException("Resource checkpoint was taken while the panel was visible.");
            ValidateResources(sample);
            sample.Phase = checkpointPhase;
            sample.CompletedCycles = CompletedCycles;
            sample.ElapsedMilliseconds = elapsed.Elapsed.TotalMilliseconds;
            checkpoints.Enqueue(sample);
        }

        private void CheckDeadline()
        {
            cancellation.Token.ThrowIfCancellationRequested();
            if (elapsed.ElapsedMilliseconds >= options.MaxRunMilliseconds)
                throw new TimeoutException("The cycle run exceeded its total time limit.");
        }

        private void Delay(int milliseconds)
        {
            CheckDeadline();
            int remaining = Math.Max(0, options.MaxRunMilliseconds - (int)elapsed.ElapsedMilliseconds);
            if (cancellation.Token.WaitHandle.WaitOne(Math.Min(milliseconds, remaining)))
                cancellation.Token.ThrowIfCancellationRequested();
            CheckDeadline();
        }

        private void SetPhase(string value) { Volatile.Write(ref phase, value); }

        public void Dispose()
        {
            Cancel();
            if (!Completion.Wait(options.StepTimeoutMilliseconds + 1000))
                throw new TimeoutException("The cycle worker did not stop within the cleanup deadline.");
            cancellation.Dispose();
        }
    }
}
