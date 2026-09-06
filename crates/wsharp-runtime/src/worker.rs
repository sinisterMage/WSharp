//! A worker: one mutator thread, its heap, and the collector thread that
//! serves it.
//!
//! Everything the collector owns used to be a process-wide `static`, which was
//! right while there was one mutator. It is the wrong shape for more than one,
//! and the reason is the pause protocol rather than the heap: all three pauses
//! run *on the mutator*, because only a mutator can walk its own stack. One
//! shared heap would need every mutator to stop before any pause, which turns
//! a 40-microsecond pause into one that waits for the slowest thread to reach
//! a safepoint. Per-worker heaps keep every worker's pauses to itself and need
//! no rendezvous at all -- which is why no object is ever reachable from two
//! workers, and why nothing is sent between them by pointer.
//!
//! Three things stay process-wide on purpose:
//!
//! - **The type registry and the stack maps.** Frozen before any code runs and
//!   read-only afterwards, so sharing them costs nothing.
//! - **The space directory** (`heap::PUBLISHED`). It answers "which space is
//!   this address in?", and the load barrier asks it about whatever reference
//!   it was handed. One worker's spaces have to be findable from the answer
//!   even though only its owner will ever be allocating in them.
//! - **The two flag words generated code reads**, for the poll and for
//!   evacuation. Their addresses are compiled into the code as constants, so
//!   they cannot be per-worker without teaching the barriers thread-local
//!   access. They mean "*some* worker wants a pause" and "*some* worker is
//!   moving" instead, and the slow path asks the current worker whether the
//!   request is its own. The cost is a false slow path on an uninvolved
//!   worker: correct, because both slow paths are idempotent, and rare,
//!   because both flags are set only around a pause.

use std::cell::{Cell, RefCell};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, Once};

use crate::gc::{Buffers, TRACE_EVERY_ALLOCATIONS};
use crate::heap::{Counters, Heap};
use crate::mark::{Phase, State};

/// What one worker's collector needs to talk to it.
pub(crate) struct MarkState {
    pub(crate) phase: AtomicU8,
    /// True from the initial pause until marking has finished: the window in
    /// which the barrier must record what the program overwrites and counting
    /// must not free.
    pub(crate) tracing: AtomicBool,
    /// Set at exit to make the collector thread drop whatever it is doing.
    pub(crate) abandon: AtomicBool,
    /// Whether *this* worker is the one that wants a pause, and whether it is
    /// the one moving objects. The words generated code reads are shared, so
    /// these are what tell a woken worker the request was not its own.
    pub(crate) wanted: AtomicBool,
    pub(crate) evacuating: AtomicBool,
    pub(crate) state: Mutex<State>,
    /// Signalled on every phase change and on abandon.
    pub(crate) changed: Condvar,
    pub(crate) thread: Once,
}

/// Counters the collector reports, and the schedule it collects on.
pub(crate) struct Stats {
    pub(crate) collections: AtomicUsize,
    pub(crate) allocations: AtomicUsize,
    /// The allocation count at which the next scheduled trace is due.
    pub(crate) trace_at: AtomicUsize,
    /// Roots the stack walk has enumerated, in total. Worth having because a
    /// walk that silently found *nothing* would let every root check pass
    /// vacuously.
    pub(crate) roots_seen: AtomicUsize,
    pub(crate) freed: AtomicUsize,
    pub(crate) traces: AtomicUsize,
    pub(crate) moved: AtomicUsize,
    pub(crate) pauses: AtomicUsize,
    pub(crate) max_pause_us: AtomicUsize,
    pub(crate) total_pause_us: AtomicUsize,
    /// Live bytes when the last trace finished sweeping: the growth trigger's
    /// point of comparison.
    pub(crate) trace_baseline_bytes: AtomicUsize,
}

pub struct Worker {
    pub(crate) id: usize,
    pub(crate) heap: Mutex<Heap>,
    pub(crate) buffers: Mutex<Buffers>,
    pub(crate) mark: MarkState,
    /// "Marked" means the bit equals this. It flips in a trace's initial pause,
    /// which unmarks this worker's whole heap at once -- and only this
    /// worker's, which is the reason it cannot be shared.
    pub(crate) parity: AtomicU64,
    pub(crate) stats: Stats,
}

// The worker owns raw pointers into a heap that lives as long as the process.
unsafe impl Send for Worker {}
unsafe impl Sync for Worker {}

thread_local! {
    /// The worker this thread belongs to: the mutator that owns it, and the
    /// collector thread that serves it. A collector thread installs its
    /// worker's pointer on entry, so a copy it makes while evacuating lands in
    /// the heap the original came from.
    static CURRENT: Cell<*const Worker> = const { Cell::new(ptr::null()) };
}

/// Every worker ever created, so that exit can settle all of them and the
/// statistics can be added up.
static WORKERS: Mutex<Vec<&'static Worker>> = Mutex::new(Vec::new());

fn workers() -> std::sync::MutexGuard<'static, Vec<&'static Worker>> {
    WORKERS.lock().unwrap_or_else(|e| e.into_inner())
}

impl Worker {
    /// The worker this thread belongs to, creating one if it has none.
    ///
    /// Created on demand rather than at a registration point, so that a thread
    /// which allocates is a worker by that fact alone -- which is what a unit
    /// test driving `ws_alloc` directly needs, and what the process's first
    /// thread needs before anything has run.
    pub fn current() -> &'static Worker {
        let existing = CURRENT.with(|c| c.get());
        if !existing.is_null() {
            // Sound: workers are leaked and never moved.
            return unsafe { &*existing };
        }
        let worker = Worker::create();
        install(worker);
        worker
    }

    fn create() -> &'static Worker {
        let counters: &'static Counters = Box::leak(Box::new(Counters::new()));
        let mut list = workers();
        let worker: &'static Worker = Box::leak(Box::new(Worker {
            id: list.len(),
            heap: Mutex::new(Heap::new_worker(counters)),
            buffers: Mutex::new(Buffers::new()),
            mark: MarkState {
                phase: AtomicU8::new(Phase::Idle as u8),
                tracing: AtomicBool::new(false),
                abandon: AtomicBool::new(false),
                wanted: AtomicBool::new(false),
                evacuating: AtomicBool::new(false),
                state: Mutex::new(State::new()),
                changed: Condvar::new(),
                thread: Once::new(),
            },
            parity: AtomicU64::new(0),
            stats: Stats {
                collections: AtomicUsize::new(0),
                allocations: AtomicUsize::new(0),
                trace_at: AtomicUsize::new(TRACE_EVERY_ALLOCATIONS),
                roots_seen: AtomicUsize::new(0),
                freed: AtomicUsize::new(0),
                traces: AtomicUsize::new(0),
                moved: AtomicUsize::new(0),
                pauses: AtomicUsize::new(0),
                max_pause_us: AtomicUsize::new(0),
                total_pause_us: AtomicUsize::new(0),
                trace_baseline_bytes: AtomicUsize::new(0),
            },
        }));
        list.push(worker);
        worker
    }
}

/// Make `worker` the one this thread belongs to.
///
/// Called by a collector thread on entry: it must allocate copies into the
/// heap it is emptying, not into one of its own.
pub(crate) fn install(worker: &'static Worker) {
    CURRENT.with(|c| c.set(worker as *const Worker));
}

/// Run `f` on every worker that exists. For exit and for the statistics: a
/// program's collector did what all of its workers' collectors did.
pub(crate) fn for_each_worker(mut f: impl FnMut(&'static Worker)) {
    let list = workers().clone();
    for worker in list {
        f(worker);
    }
}

// ---------------------------------------------------------------------------
// Runtime roots
// ---------------------------------------------------------------------------

thread_local! {
    /// Heap references the *runtime* is holding, which no stack map describes.
    ///
    /// Generated code's roots are its stack slots, and the maps say where they
    /// are. A runtime function that builds an object graph -- `transfer::decode`
    /// is the only one -- holds its half-built pieces in Rust locals instead,
    /// which the collector cannot see: it would free them between one
    /// allocation and the next, and move them out from under the pointers
    /// still to be written.
    ///
    /// So they go on a list beside the stack, and that list is a fourth place a
    /// heap pointer can live. Anything that adds such a place has to be added
    /// to all four of `gc::collect`'s root set, the evacuation pause's root
    /// pass, `evacuate::fix_references` and `--gc-stress`'s verifier -- which
    /// is the standing rule this list is the first user of.
    ///
    /// Thread-local rather than on the worker, for the same reason the stack
    /// is: only the mutator ever holds one, and all three pauses run on the
    /// mutator.
    static PINNED: RefCell<Vec<*mut u8>> = const { RefCell::new(Vec::new()) };
}

/// Keeps everything pinned above a mark alive, and lets it go again.
pub(crate) struct Pinned {
    depth: usize,
}

impl Pinned {
    /// Start a region. Every [`Pinned::add`] until this is dropped is a root.
    pub(crate) fn new() -> Pinned {
        Pinned {
            depth: PINNED.with(|p| p.borrow().len()),
        }
    }

    pub(crate) fn add(&self, obj: *mut u8) {
        PINNED.with(|p| p.borrow_mut().push(obj));
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        PINNED.with(|p| p.borrow_mut().truncate(self.depth));
    }
}

/// Visit the *address* of every pinned slot, so a moving collector can rewrite
/// it in place, exactly as it does a stack slot.
pub(crate) fn for_each_pinned_slot(mut f: impl FnMut(*mut *mut u8)) {
    PINNED.with(|p| {
        for slot in p.borrow_mut().iter_mut() {
            f(slot as *mut *mut u8);
        }
    });
}

// ---------------------------------------------------------------------------
// The two words generated code reads
// ---------------------------------------------------------------------------

/// How many workers want a pause, and the byte that says whether any does.
///
/// Two words rather than one because the byte is what generated code loads,
/// and a byte cannot count past 255 -- while the count is what makes one
/// worker finishing its pause not silence another's request.
static POLL_COUNT: AtomicUsize = AtomicUsize::new(0);
static POLL_FLAG: AtomicU8 = AtomicU8::new(0);

/// The same pair for evacuation, read by the load barrier in front of every
/// reference the program loads out of a heap object.
static EVACUATING_COUNT: AtomicUsize = AtomicUsize::new(0);
static EVACUATING_FLAG: AtomicU8 = AtomicU8::new(0);

pub fn poll_flag_address() -> usize {
    &POLL_FLAG as *const AtomicU8 as usize
}

pub fn evacuating_flag_address() -> usize {
    &EVACUATING_FLAG as *const AtomicU8 as usize
}

/// Raise or lower one worker's contribution to a shared flag.
fn set_shared(flag: &AtomicU8, count: &AtomicUsize, own: &AtomicBool, on: bool) {
    if own.swap(on, Ordering::AcqRel) == on {
        return;
    }
    let now = if on {
        count.fetch_add(1, Ordering::AcqRel) + 1
    } else {
        count.fetch_sub(1, Ordering::AcqRel) - 1
    };
    flag.store(u8::from(now != 0), Ordering::Release);
}

pub(crate) fn request_safepoint(worker: &Worker) {
    set_shared(&POLL_FLAG, &POLL_COUNT, &worker.mark.wanted, true);
}

pub(crate) fn clear_poll(worker: &Worker) {
    set_shared(&POLL_FLAG, &POLL_COUNT, &worker.mark.wanted, false);
}

/// Whether *this* worker was the one asking, clearing its request if so.
pub(crate) fn take_safepoint_request(worker: &Worker) -> bool {
    if !worker.mark.wanted.load(Ordering::Acquire) {
        return false;
    }
    clear_poll(worker);
    true
}

pub(crate) fn poll_wanted() -> bool {
    POLL_FLAG.load(Ordering::Acquire) != 0
}

pub(crate) fn set_evacuating(worker: &Worker, on: bool) {
    set_shared(
        &EVACUATING_FLAG,
        &EVACUATING_COUNT,
        &worker.mark.evacuating,
        on,
    );
}

/// Whether this worker is moving objects. Asked by the write barrier, which
/// has to record where a field's object lives *now*, and by counting, which
/// stands aside while a header may be a forwarding word rather than a count.
pub(crate) fn evacuating(worker: &Worker) -> bool {
    worker.mark.evacuating.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::TYPE_ID_FIRST_USER;

    /// Two threads are two workers, and a worker's heap is its own.
    ///
    /// The point of the whole arrangement: a trace on one heap unmarks only
    /// that heap, and a pause on one worker stops only that worker. Both
    /// follow from the objects being in different places, which is what this
    /// checks.
    #[test]
    fn each_thread_gets_a_heap_of_its_own() {
        let here = Worker::current();
        let mine = crate::heap::ws_alloc(TYPE_ID_FIRST_USER, 32, 0);

        let theirs = std::thread::spawn(|| {
            let there = Worker::current();
            (there.id, crate::heap::ws_alloc(TYPE_ID_FIRST_USER, 32, 0) as usize)
        })
        .join()
        .expect("the other thread finished");

        assert_ne!(here.id, theirs.0, "two threads, two workers");
        assert_ne!(mine as usize, theirs.1);

        // And both objects are findable: the space directory is shared, which
        // is what lets the load barrier answer for either of them.
        assert!(crate::heap::in_heap(mine));
        assert!(crate::heap::in_heap(theirs.1 as *mut u8));
    }

    /// The flag generated code reads means "*some* worker", so it stays raised
    /// while any worker is asking and falls only when the last one stops.
    #[test]
    fn the_shared_poll_flag_counts_workers() {
        let a = Worker::current();
        let b: &'static Worker = std::thread::spawn(Worker::current)
            .join()
            .expect("the other thread finished");

        assert!(!poll_wanted());
        request_safepoint(a);
        request_safepoint(b);
        assert!(poll_wanted());
        clear_poll(a);
        assert!(poll_wanted(), "the other worker is still asking");
        clear_poll(b);
        assert!(!poll_wanted());
    }
}
