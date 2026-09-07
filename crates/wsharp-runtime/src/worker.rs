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
//!   evacuation. Generated code names them as symbols, so they cannot be
//!   per-worker without teaching the barriers thread-local access. They mean
//!   "*some* worker wants a pause" and "*some* worker is moving" instead, and
//!   the slow path asks the current worker whether the request is its own. The
//!   cost is a false slow path on an uninvolved worker: correct, because both
//!   slow paths are idempotent, and rare, because both flags are set only
//!   around a pause.
//!
//! Those two bytes are the only runtime *state* a compiled program reaches by
//! name rather than through a call, which is why they are exported and why
//! [`POLL_FLAG_SYMBOL`] and [`EVACUATING_FLAG_SYMBOL`] are written down.

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
    /// Of those, the ones a collector ran for a mutator parked in a syscall.
    /// Worth counting for the reason the root count is: a safe region that is
    /// never actually used would let its tests pass without testing anything.
    pub(crate) served_pauses: AtomicUsize,
    pub(crate) max_pause_us: AtomicUsize,
    pub(crate) total_pause_us: AtomicUsize,
    /// Live bytes when the last trace finished sweeping: the growth trigger's
    /// point of comparison.
    pub(crate) trace_baseline_bytes: AtomicUsize,
}

/// A worker running normally: it answers its own pause requests.
pub(crate) const RUNNING: u8 = 0;
/// Parked in a syscall. Its stack is frozen and `parked_fp` says where it is,
/// so its collector may walk it and run the pause on its behalf.
pub(crate) const PARKED: u8 = 1;
/// A collector has claimed a parked worker and is walking it. The mutator must
/// not resume until it is let go.
pub(crate) const SCANNING: u8 = 2;

pub struct Worker {
    pub(crate) id: usize,
    pub(crate) heap: Mutex<Heap>,
    pub(crate) buffers: Mutex<Buffers>,
    pub(crate) mark: MarkState,
    /// [`RUNNING`], [`PARKED`] or [`SCANNING`]: the safe-region handshake.
    pub(crate) parked: AtomicU8,
    /// Where this worker's stack was when it parked. Meaningful only while
    /// `parked` is not [`RUNNING`], and published before it is set.
    pub(crate) parked_fp: AtomicUsize,
    /// The length of this thread's [`PINNED`] list, where another thread can
    /// read it. A parked worker's stack is walkable; that list is not, because
    /// it is on the thread rather than in it.
    pub(crate) pinned_depth: AtomicUsize,
    /// Guards the handshake's wake-up. Its own lock rather than the mark
    /// state's, because a pause takes that one.
    pub(crate) park_lock: Mutex<()>,
    pub(crate) park_changed: Condvar,
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
            parked: AtomicU8::new(RUNNING),
            parked_fp: AtomicUsize::new(0),
            pinned_depth: AtomicUsize::new(0),
            park_lock: Mutex::new(()),
            park_changed: Condvar::new(),
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
                served_pauses: AtomicUsize::new(0),
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
    /// is: only the mutator ever holds one. That used to be the whole argument,
    /// because all three pauses ran on the mutator; a safe region lets a
    /// collector run one instead, and it can reach a parked worker's stack but
    /// not its thread. So the length is mirrored onto the worker
    /// (`pinned_depth`) and a collector declines any worker holding runtime
    /// roots -- which no blocking builtin does, by construction.
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
        let depth = PINNED.with(|p| {
            let mut list = p.borrow_mut();
            list.push(obj);
            list.len()
        });
        publish_depth(depth);
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        PINNED.with(|p| p.borrow_mut().truncate(self.depth));
        publish_depth(self.depth);
    }
}

/// Mirror the list's length onto the worker, where another thread can read it.
///
/// A parked worker's *stack* can be walked by its collector, because the stack
/// is frozen and the frame pointer says where it is. This list cannot: it
/// belongs to the thread rather than to the worker. So a collector offered a
/// parked worker checks the length first and declines if the runtime is
/// holding anything -- which no blocking builtin does, and which the assertion
/// in [`blocking`] states.
fn publish_depth(depth: usize) {
    Worker::current()
        .pinned_depth
        .store(depth, Ordering::Release);
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
// The safe region
// ---------------------------------------------------------------------------

/// Run `f` outside the safepoint protocol, so that it may block.
///
/// All three pauses run on the mutator because only a mutator can walk its own
/// stack, so a thread parked in `read(2)` cannot answer a pause request and its
/// trace waits for the syscall. With workers that is one worker's heap held up
/// by another's slow client, which is the failure a worker model exists to
/// avoid.
///
/// The way out is that a blocked thread's stack is *frozen*, and a frozen stack
/// can be walked by anyone. So this records where the stack is and says the
/// worker is parked; its collector then walks the recorded chain and runs the
/// pause itself. `mark::quiesce` already did this much for a worker whose stack
/// held no roots -- the recorded frame pointer is what generalises it to one
/// that does.
///
/// **While parked, a thread may touch nothing on the heap.** Everything `f`
/// needs must already be copied into plain bytes. That is the same bargain an
/// allocating builtin makes, and for the same reason: a Rust local is described
/// by no stack map, so a reference held in one is invisible to the collection
/// that runs while this blocks.
///
/// `#[inline(never)]` because the frame recorded is this one, and it has to
/// stay live for as long as `f` runs.
#[inline(never)]
pub(crate) fn blocking<T>(f: impl FnOnce() -> T) -> T {
    let worker = Worker::current();
    debug_assert_eq!(
        worker.pinned_depth.load(Ordering::Acquire),
        0,
        "a safe region may not be entered while the runtime holds pinned roots: \
         they are on the thread, and the thread is about to stop answering"
    );
    // Give the allocation buffer back before parking, because it is the one
    // piece of this mutator that is on the *thread* and not in the worker: a
    // collector running the pause from its own thread would retire its own
    // buffer and leave this one open, and a block an allocator holds is never
    // swept, recycled or evacuated. Publishing the counters is the same story.
    // One buffer refill on the way out is nothing beside a syscall.
    crate::heap::retire_local_buffer();
    crate::heap::flush_local_counters();

    // The frame pointer first, then the state: whoever sees `PARKED` must see
    // a frame pointer that describes this stack.
    worker
        .parked_fp
        .store(crate::stackwalk::current_frame_pointer(), Ordering::Relaxed);
    worker.parked.store(PARKED, Ordering::Release);
    // A collector that has already asked for a pause is asleep waiting for one
    // that will not come until it runs it. Tell it that it can.
    {
        let _guard = worker.mark.state.lock().unwrap_or_else(|e| e.into_inner());
        worker.mark.changed.notify_all();
    }

    let out = f();

    unpark(worker);
    out
}

/// Leave a safe region, waiting out a collector that is walking this stack.
///
/// Resuming mid-walk is the one race that matters, and it is why leaving is a
/// compare-exchange rather than a store: losing it means the collector claimed
/// this worker, and the only safe answer is to wait until it lets go.
fn unpark(worker: &'static Worker) {
    loop {
        if worker
            .parked
            .compare_exchange(PARKED, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
        let guard = worker.park_lock.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = worker
            .park_changed
            .wait_while(guard, |_| worker.parked.load(Ordering::Acquire) == SCANNING)
            .unwrap_or_else(|e| e.into_inner());
    }
}

/// Take a parked worker, so its pause can be run on this thread.
///
/// Declines a worker whose runtime roots are non-empty: that list lives on the
/// parked thread and cannot be reached from here, so the honest answer is to
/// wait for the mutator, exactly as everything did before safe regions.
pub(crate) fn claim_parked(worker: &Worker) -> bool {
    if worker.pinned_depth.load(Ordering::Acquire) != 0 {
        return false;
    }
    worker
        .parked
        .compare_exchange(PARKED, SCANNING, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Give a claimed worker back. It returns to being parked, not to running:
/// only the thread that parked may decide it has stopped being blocked.
pub(crate) fn release_parked(worker: &Worker) {
    let _guard = worker.park_lock.lock().unwrap_or_else(|e| e.into_inner());
    worker.parked.store(PARKED, Ordering::Release);
    worker.park_changed.notify_all();
}

/// Whether this worker is parked and unclaimed, so a pause could be run for it.
pub(crate) fn is_parked(worker: &Worker) -> bool {
    worker.parked.load(Ordering::Acquire) == PARKED
}

/// Walk `worker`'s stack roots: this thread's own, or a parked worker's from
/// the frame pointer it recorded.
///
/// Every root walk goes through here rather than calling the stack walker
/// directly, because "whose stack, and where does it start" is exactly the
/// question a safe region changes the answer to.
///
/// # Safety
/// As [`crate::stackwalk::walk_roots`]. When walking a parked worker it must
/// be held claimed -- [`claim_parked`] -- for the whole walk.
pub(crate) unsafe fn walk_worker_roots(worker: &Worker, visit: impl FnMut(*mut *mut u8)) {
    if worker.parked.load(Ordering::Acquire) == SCANNING {
        let fp = worker.parked_fp.load(Ordering::Acquire);
        unsafe { crate::stackwalk::walk_roots_from(fp, visit) };
    } else {
        unsafe { crate::stackwalk::walk_roots(visit) };
    }
}

// ---------------------------------------------------------------------------
// The two words generated code reads
// ---------------------------------------------------------------------------

/// How many workers want a pause, and the byte that says whether any does.
///
/// Two words rather than one because the byte is what generated code loads,
/// and a byte cannot count past 255 -- while the count is what makes one
/// worker finishing its pause not silence another's request.
///
/// The byte is exported, and that is not decoration. Generated code reads it
/// directly, so it is the one piece of runtime *state* -- as against runtime
/// *functions* -- that a compiled program names. Under the JIT the name is
/// resolved to this address in this process; in an object file it is a
/// relocation a linker fills in. Renaming it breaks the second and not the
/// first, which is why the name is a constant both sides read.
static POLL_COUNT: AtomicUsize = AtomicUsize::new(0);
#[unsafe(no_mangle)]
pub static ws_gc_poll_flag: AtomicU8 = AtomicU8::new(0);

/// The same pair for evacuation, read by the load barrier in front of every
/// reference the program loads out of a heap object.
static EVACUATING_COUNT: AtomicUsize = AtomicUsize::new(0);
#[unsafe(no_mangle)]
pub static ws_gc_evacuating_flag: AtomicU8 = AtomicU8::new(0);

/// What generated code links the two flag bytes by.
///
/// Read by the code generator, which declares them as imported data, and by
/// the JIT, which resolves that import to [`poll_flag_address`] and
/// [`evacuating_flag_address`]. One spelling, so the two backends cannot
/// disagree about it.
pub const POLL_FLAG_SYMBOL: &str = "ws_gc_poll_flag";
pub const EVACUATING_FLAG_SYMBOL: &str = "ws_gc_evacuating_flag";

pub fn poll_flag_address() -> usize {
    &ws_gc_poll_flag as *const AtomicU8 as usize
}

pub fn evacuating_flag_address() -> usize {
    &ws_gc_evacuating_flag as *const AtomicU8 as usize
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
    set_shared(&ws_gc_poll_flag, &POLL_COUNT, &worker.mark.wanted, true);
}

pub(crate) fn clear_poll(worker: &Worker) {
    set_shared(&ws_gc_poll_flag, &POLL_COUNT, &worker.mark.wanted, false);
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
    ws_gc_poll_flag.load(Ordering::Acquire) != 0
}

pub(crate) fn set_evacuating(worker: &Worker, on: bool) {
    set_shared(
        &ws_gc_evacuating_flag,
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
            (
                there.id,
                crate::heap::ws_alloc(TYPE_ID_FIRST_USER, 32, 0) as usize,
            )
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

    /// A parked worker can be claimed and walked, and it may not resume until
    /// it is let go.
    ///
    /// That last part is the one race the handshake exists for: a mutator that
    /// returned from its syscall and carried on while its collector was still
    /// reading its stack would be a collector reading a stack that is being
    /// rewritten underneath it.
    #[test]
    fn a_parked_worker_is_released_before_it_resumes() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (worker_tx, worker_rx) = mpsc::channel::<&'static Worker>();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (left_tx, left_rx) = mpsc::channel::<()>();

        let mutator = std::thread::spawn(move || {
            let w = Worker::current();
            worker_tx.send(w).expect("the test is listening");
            // Parked for as long as the other thread keeps it waiting, which
            // is what a syscall looks like from here.
            blocking(|| go_rx.recv().expect("released"));
            left_tx.send(()).expect("the test is listening");
        });

        let w = worker_rx.recv().expect("the mutator started");
        while !is_parked(w) {
            std::thread::yield_now();
        }
        assert!(claim_parked(w), "an unclaimed parked worker can be taken");
        assert!(!claim_parked(w), "and only once");

        // Its stack is walkable from this thread. Nothing generated is on it,
        // so the walk finds nothing -- what matters is that it terminates.
        let mut roots = 0;
        unsafe { walk_worker_roots(w, |_| roots += 1) };

        // Let the syscall return. The mutator must still not get past `unpark`.
        go_tx.send(()).expect("the mutator is waiting");
        assert!(
            left_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a mutator resumed while its stack was being walked"
        );

        release_parked(w);
        // Generous on purpose: this is a hang detector, not a latency check,
        // and a loaded machine must not fail it.
        left_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("released, so it resumes");
        mutator.join().expect("the mutator finished");
    }

    /// The flag generated code reads means "*some* worker", so it stays raised
    /// while any worker is asking and falls only when the last one stops.
    #[test]
    fn the_shared_poll_flag_counts_workers() {
        // The flag and its count are process-wide, and any trace anywhere in
        // the test binary raises them.
        let _serial = crate::test_support::SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
