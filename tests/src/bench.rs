//! Benchmark measurement primitives.
//!
//! Timing, statistics, and workload calibration for benchmark binaries.
//! All timing functions require clock_access=true (root Observer only).

use core::arch::asm;

// ── Counter read ────────────────────────────────────────────────

/// Read the virtual counter (CNTVCT_EL0) directly.
///
/// Requires EL0 access enabled (CNTKCTL_EL1.EL0VCTEN=1), which the
/// kernel sets for root Observer via clock_access=true (D66).
/// Will fault at EL0 for child Observers without clock_access.
#[inline(always)]
pub fn cycles() -> u64 {
    let val: u64;

    // SAFETY: MRS CNTVCT_EL0 reads the virtual count register.
    // Requires EL0 access enabled (CNTKCTL_EL1.EL0VCTEN=1, which D66
    // sets for root Observer via clock_access=true). This will fault
    // at EL0 for child Observers without clock_access.
    // nomem is safe: CNTVCT_EL0 is a read-only counter register (ARM ARM
    // D13.11.5), the MRS does not access memory and has no side effects
    // beyond reading the counter value.
    unsafe {
        asm!(
            "mrs {val}, CNTVCT_EL0",
            val = out(reg) val,
            options(nomem, nostack, preserves_flags),
        );
    }

    val
}

// ── Benchmark emission ──────────────────────────────────────────

/// Emit a benchmark data point via BRK #0x48.
///
/// The kernel reads x0-x3, prints a structured BENCH line to serial,
/// advances PC past the BRK, and resumes execution.
#[inline(never)]
pub fn bench_emit(tag: u64, value: u64, meta0: u64, meta1: u64) {
    // SAFETY: BRK #0x48 is the kernel's benchmark data point signal.
    // The kernel reads x0-x3, prints a BENCH line, advances PC, and
    // resumes execution. This is a non-terminal debug exception.
    unsafe {
        asm!(
            "brk #0x48",
            in("x0") tag,
            in("x1") value,
            in("x2") meta0,
            in("x3") meta1,
        );
    }
}

// ── Statistics ──────────────────────────────────────────────────

pub const MAX_SAMPLES: usize = 1024;

pub struct Stats {
    pub min: u64,
    pub max: u64,
    pub sum: u64,
    pub count: u32,
    samples: [u64; MAX_SAMPLES],
    sorted: bool,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            min: u64::MAX,
            max: 0,
            sum: 0,
            count: 0,
            samples: [0; MAX_SAMPLES],
            sorted: false,
        }
    }

    /// Record a sample value. Stores up to MAX_SAMPLES values; additional
    /// samples wrap around (modulo). Min/max/sum/count are always accurate.
    pub fn record(&mut self, value: u64) {
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }

        self.sum += value;
        self.samples[self.count as usize % MAX_SAMPLES] = value;
        self.count += 1;
        self.sorted = false;
    }

    /// Integer mean of all recorded values.
    pub fn mean(&self) -> u64 {
        if self.count == 0 {
            return 0;
        }

        self.sum / self.count as u64
    }

    /// Median of stored samples (sorts in-place on first call).
    pub fn median(&mut self) -> u64 {
        let n = self.sample_count();

        if n == 0 {
            return 0;
        }

        self.sort();
        self.samples[n / 2]
    }

    /// 99th percentile of stored samples (sorts in-place on first call).
    pub fn p99(&mut self) -> u64 {
        let n = self.sample_count();

        if n == 0 {
            return 0;
        }

        self.sort();
        self.samples[n * 99 / 100]
    }

    /// Emit 5 BENCH lines: tag+0=min, tag+1=median, tag+2=p99, tag+3=mean, tag+4=count.
    pub fn emit(&mut self, tag: u64) {
        self.sort();
        let n = self.sample_count();
        let median = if n > 0 { self.samples[n / 2] } else { 0 };
        let p99 = if n > 0 { self.samples[n * 99 / 100] } else { 0 };

        bench_emit(tag, self.min, 0, 0);
        bench_emit(tag + 1, median, 0, 0);
        bench_emit(tag + 2, p99, 0, 0);
        bench_emit(tag + 3, self.mean(), 0, 0);
        bench_emit(tag + 4, self.count as u64, 0, 0);
    }

    fn sample_count(&self) -> usize {
        (self.count as usize).min(MAX_SAMPLES)
    }

    /// Insertion sort on the stored samples. O(n^2) but n <= 1024
    /// and called at most once after measurement completes.
    fn sort(&mut self) {
        if self.sorted {
            return;
        }

        let n = self.sample_count();

        for i in 1..n {
            let key = self.samples[i];
            let mut j = i;

            while j > 0 && self.samples[j - 1] > key {
                self.samples[j] = self.samples[j - 1];
                j -= 1;
            }

            self.samples[j] = key;
        }

        self.sorted = true;
    }
}

// ── Stopwatch ───────────────────────────────────────────────────

pub struct Stopwatch {
    start: u64,
}

impl Stopwatch {
    pub fn start() -> Self {
        Self { start: cycles() }
    }

    pub fn elapsed(&self) -> u64 {
        cycles() - self.start
    }

    pub fn lap(&mut self) -> u64 {
        let now = cycles();
        let elapsed = now - self.start;
        self.start = now;
        elapsed
    }
}

// ── Benchmark harness ───────────────────────────────────────────

/// Run a benchmark: discard `warmup` iterations, then measure `measure`
/// iterations, returning collected Stats.
pub fn benchmark<F: FnMut()>(warmup: u32, measure: u32, mut body: F) -> Stats {
    for _ in 0..warmup {
        body();
    }

    let mut stats = Stats::new();

    for _ in 0..measure {
        let sw = Stopwatch::start();

        body();

        stats.record(sw.elapsed());
    }

    stats
}

// ── Calibrated busy loop ────────────────────────────────────────

/// Burn approximately `target_ticks` worth of CPU cycles.
///
/// Calibrates by measuring a known number of loop iterations first,
/// then runs proportionally more. Requires clock_access=true.
///
/// Uses ratio math (target * calibration_iters / calibration_ticks)
/// instead of per-iteration cost to avoid integer division truncating
/// to zero when each iteration costs less than one counter tick.
pub fn burn_cycles(target_ticks: u64) {
    let calibration_iters: u64 = 10_000;
    let start = cycles();

    for _ in 0..calibration_iters {
        core::hint::black_box(0u64);
    }

    let calibration_ticks = cycles() - start;

    if calibration_ticks == 0 {
        return;
    }

    let needed_iters = target_ticks * calibration_iters / calibration_ticks;

    for _ in 0..needed_iters {
        core::hint::black_box(0u64);
    }
}

// ── Host unit tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_basic() {
        let mut s = Stats::new();

        for v in [10, 20, 30, 40, 50] {
            s.record(v);
        }

        assert_eq!(s.min, 10);
        assert_eq!(s.max, 50);
        assert_eq!(s.mean(), 30);
        assert_eq!(s.median(), 30);
    }

    #[test]
    fn stats_p99_with_outlier() {
        let mut s = Stats::new();

        for i in 0..100 {
            s.record(i);
        }

        s.record(1000); // outlier

        assert!(s.p99() >= 99); // p99 should be near 99, not 1000
    }

    #[test]
    fn stats_empty() {
        let mut s = Stats::new();

        assert_eq!(s.mean(), 0);
        assert_eq!(s.median(), 0);
        assert_eq!(s.p99(), 0);
    }

    #[test]
    fn stats_single() {
        let mut s = Stats::new();

        s.record(42);

        assert_eq!(s.min, 42);
        assert_eq!(s.max, 42);
        assert_eq!(s.mean(), 42);
        assert_eq!(s.median(), 42);
        assert_eq!(s.p99(), 42);
    }

    #[test]
    fn stats_count_tracking() {
        let mut s = Stats::new();

        for i in 0..200 {
            s.record(i);
        }

        assert_eq!(s.count, 200);
        assert_eq!(s.min, 0);
        assert_eq!(s.max, 199);
    }

    #[test]
    fn benchmark_harness() {
        let mut call_count = 0u32;
        let stats = benchmark(5, 10, || {
            call_count += 1;
        });

        // 5 warmup + 10 measure = 15 total calls
        assert_eq!(call_count, 15);
        assert_eq!(stats.count, 10);
    }
}
