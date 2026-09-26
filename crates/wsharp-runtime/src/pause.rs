//! Per-pause timing: a histogram that is always on, and an exact sample log
//! that is not.
//!
//! [`crate::gc::record_pause`] used to keep three numbers per worker -- a
//! count, a total and a maximum. Those support a mean and a maximum and
//! nothing else: a median or a 99th percentile needs the individual samples,
//! and they were gone by the time anything could read them. That is the whole
//! of issue #18, and it is why no percentile about this collector was
//! publishable.
//!
//! Two instruments, because they answer different questions and cost different
//! amounts.
//!
//! * A **log-spaced histogram**, always on. One `leading_zeros` and one relaxed
//!   `fetch_add` per pause, into a fixed array that is part of the worker's
//!   `Stats`. It allocates nothing, needs no opt-in, and costs nothing that can
//!   be measured against a pause of even a single microsecond. Four sub-buckets
//!   per octave, so a percentile read out of it is within 25% of the truth --
//!   which is a distribution rather than a point, and a number published from
//!   it says which bucket it came from.
//! * An **exact sample log**, opt-in with `WSHARP_GC_PAUSE_LOG=<path>`. The
//!   buffer is allocated once, when the worker is created, and the pause path
//!   only claims a slot with a `fetch_add` and stores one word into it. The
//!   file is written at exit. Nothing allocates on the pause path here either,
//!   and -- just as important -- no syscall happens on it, because the thing
//!   being timed is the pause itself.
//!
//! Neither instrument is behind a build flag: the histogram is in every build,
//! release included, and the log is one environment variable away. An
//! instrument nobody can switch on in a shipped binary cannot measure a shipped
//! binary.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Which of the three pauses a sample came from.
///
/// The trace has exactly three: the one that takes the snapshot, the one that
/// finishes marking and moves the roots, and the one that fixes references
/// after the copying. They cost very different amounts, so a distribution that
/// mixed them without saying so would describe no single thing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Pause {
    /// Snapshot the roots and flip the mark parity.
    Initial = 0,
    /// Finish marking, then evacuate everything the roots point at.
    MarkDone = 1,
    /// Fix the references the marker recorded, and release the emptied blocks.
    EvacDone = 2,
}

impl Pause {
    pub fn name(self) -> &'static str {
        match self {
            Pause::Initial => "initial",
            Pause::MarkDone => "mark-done",
            Pause::EvacDone => "evac-done",
        }
    }

    fn from_bits(bits: u32) -> Pause {
        match bits {
            0 => Pause::Initial,
            1 => Pause::MarkDone,
            _ => Pause::EvacDone,
        }
    }
}

// ---------------------------------------------------------------------------
// The histogram
// ---------------------------------------------------------------------------

/// Sub-buckets per octave, as an exponent: 2 means four of them, so a bucket
/// is at most 25% wider than its own lower bound.
const SUB_BITS: u32 = 2;
const SUBS: usize = 1 << SUB_BITS;

/// Exactly enough buckets for every `usize` a pause could be measured in, and
/// not one more.
///
/// `usize::MAX` lands in the last of them: its octave is `usize::BITS - 1`, so
/// its shift is `usize::BITS - 1 - SUB_BITS` and its sub-bucket is the last.
/// Rounding this up to a tidier 256 was the first version and was wrong --
/// `bucket_lo` of an index past the end shifts a value off the top of the
/// word, which is an overflow panic in a debug build and a silently wrong
/// bound in a release one. The two tests below walk every index for that
/// reason.
pub const BUCKETS: usize = ((((usize::BITS - 1 - SUB_BITS) + 1) as usize) << SUB_BITS) + SUBS;

/// The bucket a pause of `us` microseconds falls in.
///
/// Values below [`SUBS`] get a bucket each, so the short pauses that dominate
/// this collector are recorded exactly; above that each octave is cut into
/// [`SUBS`] equal parts.
pub fn bucket(us: usize) -> usize {
    if us < SUBS {
        return us;
    }
    let octave = (usize::BITS - 1 - us.leading_zeros()) as usize;
    let shift = octave - SUB_BITS as usize;
    let sub = (us >> shift) - SUBS;
    ((shift + 1) << SUB_BITS) + sub
}

/// The largest pause that lands in bucket `i`. Reporting a percentile as this
/// rather than as the bucket's lower bound keeps the published number an upper
/// bound on the truth, which is the safe direction for a latency figure.
pub fn bucket_hi(i: usize) -> usize {
    if i < SUBS {
        return i;
    }
    let shift = (i >> SUB_BITS) - 1;
    let sub = i & (SUBS - 1);
    let lo = (SUBS + sub) << shift;
    // The width added last, not the exclusive end taken first: the top
    // bucket's exclusive end is exactly 2^64, which does not fit in the word
    // its inclusive end does.
    lo + ((1usize << shift) - 1)
}

/// The smallest pause that lands in bucket `i`.
pub fn bucket_lo(i: usize) -> usize {
    if i < SUBS {
        return i;
    }
    let shift = (i >> SUB_BITS) - 1;
    let sub = i & (SUBS - 1);
    (SUBS + sub) << shift
}

/// A worker's pause histogram: one counter per bucket, and nothing else.
pub(crate) struct Histogram {
    buckets: [AtomicUsize; BUCKETS],
}

impl Histogram {
    pub(crate) const fn new() -> Histogram {
        Histogram {
            buckets: [const { AtomicUsize::new(0) }; BUCKETS],
        }
    }

    /// The whole cost of the always-on instrument: one count-leading-zeros and
    /// one relaxed increment.
    pub(crate) fn record(&self, us: usize) {
        self.buckets[bucket(us)].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn add_into(&self, out: &mut [usize; BUCKETS]) {
        for (slot, counter) in out.iter_mut().zip(self.buckets.iter()) {
            *slot += counter.load(Ordering::Relaxed);
        }
    }
}

/// How many samples a histogram holds.
pub fn samples(hist: &[usize; BUCKETS]) -> usize {
    hist.iter().sum()
}

/// The `q`th percentile, in microseconds, as an upper bound.
///
/// `q` is a percentage: 50 is the median, 100 the maximum. Returns 0 for an
/// empty histogram, which is the honest answer -- there were no pauses -- and
/// is distinguishable from a real measurement by [`samples`] being zero.
///
/// The answer is the upper bound of the bucket the rank falls in, so it
/// overstates by at most 25%. A published figure must say so; that is what
/// makes this a defensible number rather than a precise-looking one.
pub fn percentile_us(hist: &[usize; BUCKETS], q: usize) -> usize {
    let total = samples(hist);
    if total == 0 {
        return 0;
    }
    // The rank of the sample being asked for, 1-based: q=100 is the last one.
    let rank = (total * q.min(100)).div_ceil(100).max(1);
    let mut seen = 0usize;
    for (i, &count) in hist.iter().enumerate() {
        seen += count;
        if seen >= rank {
            return bucket_hi(i);
        }
    }
    bucket_hi(BUCKETS - 1)
}

/// The histogram as `lo-hi:count` triples, occupied buckets only.
///
/// This is what `WSHARP_GC_STATS=1` prints, and it is the raw data: every
/// percentile in the summary line beside it is recomputable from these pairs,
/// which is what stops the summary being the only record.
pub fn render(hist: &[usize; BUCKETS]) -> String {
    let mut out = String::new();
    for (i, &count) in hist.iter().enumerate() {
        if count == 0 {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{}-{}:{}", bucket_lo(i), bucket_hi(i), count));
    }
    if out.is_empty() {
        out.push('-');
    }
    out
}

// ---------------------------------------------------------------------------
// The exact sample log
// ---------------------------------------------------------------------------

/// Where `WSHARP_GC_PAUSE_LOG` says to write, read once.
///
/// Read once because the pause path must not touch the environment, and
/// because a worker created halfway through a run must make the same decision
/// the first one did.
fn log_path() -> Option<&'static String> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| match std::env::var("WSHARP_GC_PAUSE_LOG") {
        Ok(p) if !p.is_empty() => Some(p),
        _ => None,
    })
    .as_ref()
}

/// How many samples one worker keeps when the log is on.
///
/// A hard bound rather than a growing buffer, because growing one would be an
/// allocation on the pause path. At 4 bytes a sample this is 4 MiB per worker,
/// and it is only paid when the variable is set. A run that overflows it says
/// so in the file's trailer rather than silently reporting a truncated
/// distribution as a whole one.
const LOG_CAPACITY: usize = 1 << 20;

/// A worker's exact pause samples, preallocated.
pub(crate) struct Log {
    slots: Box<[AtomicU32]>,
    /// How many pauses were offered, which can exceed `slots.len()`.
    offered: AtomicUsize,
}

/// Microseconds are held in 30 bits and the pause kind in the top two, so a
/// sample is one word and storing it is one relaxed write. 2^30 microseconds
/// is about eighteen minutes; anything at or above that saturates, and a pause
/// that long is a bug rather than a measurement.
const US_MAX: u32 = (1 << 30) - 1;

impl Log {
    /// The log for a new worker: a buffer if the variable is set, nothing if
    /// it is not. Called from `Worker::create`, never from a pause.
    pub(crate) fn for_new_worker() -> Option<Log> {
        log_path()?;
        let mut slots = Vec::with_capacity(LOG_CAPACITY);
        slots.resize_with(LOG_CAPACITY, || AtomicU32::new(0));
        Some(Log {
            slots: slots.into_boxed_slice(),
            offered: AtomicUsize::new(0),
        })
    }

    /// Claim a slot and store the sample. No allocation, no syscall, no lock.
    pub(crate) fn record(&self, us: usize, kind: Pause) {
        let i = self.offered.fetch_add(1, Ordering::Relaxed);
        if i >= self.slots.len() {
            return;
        }
        let packed = ((kind as u32) << 30) | (us.min(US_MAX as usize) as u32);
        self.slots[i].store(packed, Ordering::Relaxed);
    }

    /// `(microseconds, which pause)` for every sample actually kept.
    fn samples(&self) -> impl Iterator<Item = (u32, Pause)> + '_ {
        let kept = self.offered.load(Ordering::Relaxed).min(self.slots.len());
        self.slots[..kept].iter().map(|s| {
            let packed = s.load(Ordering::Relaxed);
            (packed & US_MAX, Pause::from_bits(packed >> 30))
        })
    }

    fn offered(&self) -> usize {
        self.offered.load(Ordering::Relaxed)
    }
}

/// Write `WSHARP_GC_PAUSE_LOG`'s file, if it was asked for.
///
/// One header line naming the columns, then one line per pause, then a trailer
/// saying how many pauses each worker offered -- so a reader can tell a
/// complete record from a truncated one instead of guessing. Called at exit,
/// where a syscall costs nothing that is being measured.
pub fn write_log_if_asked() {
    let Some(path) = log_path() else {
        return;
    };
    // Written once per process. Two exits racing here would interleave lines
    // into a file nobody could parse.
    static WRITTEN: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if *written {
        return;
    }
    *written = true;

    use std::io::Write;
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("W# gc: cannot write {path}: {e}");
            return;
        }
    };
    let mut out = std::io::BufWriter::new(file);
    let fail = |e: std::io::Error| eprintln!("W# gc: cannot write {path}: {e}");

    if let Err(e) = writeln!(out, "# us\tpause\tworker") {
        return fail(e);
    }
    let mut trailer: Vec<(usize, usize, usize)> = Vec::new();
    crate::worker::for_each_worker(|w| {
        let Some(log) = w.pause_log.as_ref() else {
            return;
        };
        let mut kept = 0usize;
        for (us, kind) in log.samples() {
            kept += 1;
            if let Err(e) = writeln!(out, "{us}\t{}\t{}", kind.name(), w.id) {
                fail(e);
                return;
            }
        }
        trailer.push((w.id, kept, log.offered()));
    });
    for (id, kept, offered) in trailer {
        let note = if offered > kept { " TRUNCATED" } else { "" };
        if let Err(e) = writeln!(out, "# worker {id}: {kept} kept of {offered}{note}") {
            return fail(e);
        }
    }
    if let Err(e) = out.flush() {
        fail(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_bucket_is_the_last_one_there_is() {
        // Every `usize` has a bucket, and nothing past the end of the array
        // does -- which is what keeps `bucket_lo` from shifting a value off
        // the top of the word.
        assert_eq!(bucket(usize::MAX), BUCKETS - 1);
        assert_eq!(bucket_hi(BUCKETS - 1), usize::MAX);
    }

    #[test]
    fn every_bucket_round_trips_through_its_own_bounds() {
        for i in 0..BUCKETS {
            let lo = bucket_lo(i);
            let hi = bucket_hi(i);
            assert!(lo <= hi, "bucket {i}: {lo} > {hi}");
            assert_eq!(bucket(lo), i, "bucket {i} low bound {lo}");
            assert_eq!(bucket(hi), i, "bucket {i} high bound {hi}");
            // Contiguous: no microsecond value falls between two buckets.
            if i > 0 {
                assert_eq!(bucket_hi(i - 1) + 1, lo, "gap before bucket {i}");
            }
        }
    }

    #[test]
    fn a_bucket_is_never_more_than_a_quarter_wider_than_its_floor() {
        for i in SUBS..BUCKETS {
            let lo = bucket_lo(i);
            let hi = bucket_hi(i);
            assert!(
                (hi - lo) * 4 <= lo,
                "bucket {i} spans {lo}..={hi}, wider than 25%"
            );
        }
    }

    /// The claim criterion 8 turns on: N recorded pauses produce N samples.
    #[test]
    fn n_recorded_pauses_produce_n_samples() {
        let hist = Histogram::new();
        let mut expected = [0usize; BUCKETS];
        let mut n = 0usize;
        // A spread that covers the exact buckets, several octaves and the
        // saturating end, so this is not a test of the zero bucket alone.
        for us in [0, 1, 2, 3, 7, 13, 64, 999, 100_000, usize::MAX] {
            for _ in 0..37 {
                hist.record(us);
                expected[bucket(us)] += 1;
                n += 1;
            }
        }
        let mut got = [0usize; BUCKETS];
        hist.add_into(&mut got);
        assert_eq!(samples(&got), n, "{n} pauses recorded");
        assert_eq!(got, expected);
    }

    #[test]
    fn percentiles_come_out_in_order_and_bound_the_samples() {
        let hist = Histogram::new();
        // 1,000 samples: 990 of one microsecond and 10 of a thousand, so the
        // median and the 99th percentile are on opposite sides of the tail.
        for _ in 0..990 {
            hist.record(1);
        }
        for _ in 0..10 {
            hist.record(1_000);
        }
        let mut h = [0usize; BUCKETS];
        hist.add_into(&mut h);
        assert_eq!(samples(&h), 1_000);
        assert_eq!(percentile_us(&h, 50), 1);
        assert_eq!(percentile_us(&h, 90), 1);
        // The tail is the last 1%, so the 99th percentile is still the short
        // pause and the maximum is not -- which is the whole point of keeping
        // the samples.
        assert_eq!(percentile_us(&h, 99), 1);
        let max = percentile_us(&h, 100);
        assert!(
            (1_000..=1_250).contains(&max),
            "maximum {max} should bound 1000 within 25%"
        );
    }

    #[test]
    fn an_empty_histogram_reports_nothing_rather_than_zero_microseconds() {
        let h = [0usize; BUCKETS];
        assert_eq!(samples(&h), 0);
        assert_eq!(percentile_us(&h, 99), 0);
        assert_eq!(render(&h), "-");
    }

    #[test]
    fn the_rendered_histogram_recomputes_the_percentiles() {
        let hist = Histogram::new();
        for us in [0, 5, 5, 5, 80, 4_000] {
            hist.record(us);
        }
        let mut h = [0usize; BUCKETS];
        hist.add_into(&mut h);
        let text = render(&h);
        // Every occupied bucket is present with its bounds and its count, so
        // the line is the raw data rather than a summary of it.
        let total: usize = text
            .split(' ')
            .map(|f| f.rsplit(':').next().unwrap().parse::<usize>().unwrap())
            .sum();
        assert_eq!(total, 6, "rendered as {text}");
        assert!(text.starts_with("0-0:1 "), "rendered as {text}");
    }
}
