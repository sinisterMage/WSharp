//! The mark trace, the evacuation that follows it, and the thread that runs
//! both.
//!
//! Counting reclaims most garbage as it appears, but it cannot reclaim a cycle
//! -- two objects pointing at each other keep each other's counts above zero
//! for ever -- and it gives up on an object whose count saturated. Only
//! reachability settles those, and a reachability trace is a walk over the
//! whole live heap. Doing that walk, and the copying that follows it, with the
//! program stopped is the pause LXR exists to avoid, so both run on their own
//! thread while the program continues:
//!
//! ```text
//!   [pause]        counting collection; flip the mark parity; choose the
//!                  blocks to empty; snapshot the roots
//!   marking        the collector thread marks from the snapshot while the
//!                  program runs. The write barrier records what the program
//!                  overwrites and the marker follows that too, and it notes
//!                  every reference it sees into a block being emptied
//!   [pause]        finish marking; counting collection; move every object the
//!                  roots point at, so that nothing the program is holding is
//!                  about to move under it
//!   evacuating     the collector thread copies the rest while the program
//!                  runs, and the program moves whatever it reaches first
//!   [pause]        repoint the references the marker noted, and the few
//!                  places it could not see; release the emptied blocks
//!   sweeping       the collector thread frees what was not marked, a block at
//!                  a time under the heap lock
//! ```
//!
//! **Why the marker can run while the program mutates the heap** is the usual
//! snapshot-at-the-beginning argument. Every object the program can reach at
//! the end was either reachable at the snapshot, or allocated since. The
//! second kind are born marked (`heap::stamp` writes the current mark
//! parity). The first kind are reached from the snapshot's roots through the
//! references that existed then -- and the only way the program can hide one
//! of those from the marker is to overwrite it, which the write barrier
//! records, because recording overwritten references is what it does for
//! counting anyway. The marker drains that record too.
//!
//! **Why the copying can too** is the load barrier: every reference the
//! program reads out of a heap object is resolved to wherever that object
//! lives now, so the program can never hold an address the collector has
//! abandoned, and a write through a reference can never land somewhere nobody
//! will look again. See [`crate::evacuate`].
//!
//! **Why nothing is freed while any of it runs**: the marker reads any object
//! it is handed, including one that counting has since found dead, because
//! such an object may be the only path the snapshot had to something still
//! live. So counting keeps running during the mark but defers its freeing, and
//! stops entirely while objects are moving, when a header may be a forwarding
//! word rather than a count.
//!
//! **Where the pauses happen.** The roots come from walking the mutator's own
//! stack, which only the mutator can do, so every pause runs *on the mutator*,
//! inside a runtime call at a safepoint: the allocator, the loop poll, or the
//! `gc_trace*` builtins. Never a builtin such as `print`, which may be holding
//! its argument in a Rust local no stack map describes. The collector thread
//! asks for a pause by setting the poll byte; the program gets there at its
//! next back edge or allocation.
//!
//! **Unless it is parked.** A mutator inside a blocking call cannot reach a
//! safepoint until the call returns, so it enters a safe region first
//! (`worker::blocking`) and records where its stack is. A stack that is not
//! running can be walked by anyone, so the collector claims it and runs the
//! pause here instead -- `wait_or_serve` below. That is the whole of what
//! makes a syscall safe to sit in.

use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Instant;

use crate::evacuate;
use crate::gc::{self, with_buffers};
use crate::header::{claim_mark, flip_mark_parity, is_marked, type_id_of};
use crate::heap::{self, is_collectable};
use crate::types;
use crate::worker::Worker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Idle = 0,
    /// The collector thread is marking; the program runs.
    Marking = 1,
    /// Marking is done and a pause is wanted at the next safepoint.
    MarkDone = 2,
    /// The collector thread is copying objects out of the chosen blocks; the
    /// program runs, and moves whatever it reaches first.
    Evacuating = 3,
    /// Everything has been copied and a pause is wanted to repoint references.
    EvacDone = 4,
    /// The collector thread is freeing what was not marked.
    Sweeping = 5,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            0 => Phase::Idle,
            1 => Phase::Marking,
            2 => Phase::MarkDone,
            3 => Phase::Evacuating,
            4 => Phase::EvacDone,
            _ => Phase::Sweeping,
        }
    }
}

/// The worker this thread belongs to.
///
/// Every function below is on one of exactly two threads -- the mutator, or
/// the collector thread serving it -- and the collector installs its worker on
/// entry, so this is the right one on both. `quiesce` is the exception and
/// takes its worker: it runs on the main thread, for every worker in turn.
fn me() -> &'static Worker {
    Worker::current()
}

/// What the mutator hands the collector thread, and gets back.
pub(crate) struct State {
    work: Vec<*mut u8>,
    /// Every slot the marker saw holding a reference into a block being
    /// emptied. The evacuation pause revisits exactly these rather than the
    /// whole heap, which is what keeps that pause proportional to what the
    /// program is doing rather than to how much it is holding.
    remembered: Vec<*mut *mut u8>,
    cset: Vec<(usize, usize)>,
}

// Raw pointers into a heap that outlives the process.
unsafe impl Send for State {}

impl State {
    pub(crate) const fn new() -> State {
        State {
            work: Vec::new(),
            remembered: Vec::new(),
            cset: Vec::new(),
        }
    }
}

fn state_of(w: &'static Worker) -> std::sync::MutexGuard<'static, State> {
    w.mark.state.lock().unwrap_or_else(|e| e.into_inner())
}

fn state() -> std::sync::MutexGuard<'static, State> {
    state_of(me())
}

pub fn phase() -> Phase {
    phase_of(me())
}

fn phase_of(w: &Worker) -> Phase {
    Phase::from_u8(w.mark.phase.load(Ordering::Acquire))
}

/// Whether a trace is between its snapshot and the end of marking.
pub(crate) fn tracing() -> bool {
    me().mark.tracing.load(Ordering::Acquire)
}

/// Publish a phase change. The store happens under the state lock and the
/// waiters check under it, so a wake-up cannot be lost.
fn set_phase(phase: Phase) {
    set_phase_of(me(), phase);
}

fn set_phase_of(w: &'static Worker, phase: Phase) {
    let _guard = state_of(w);
    w.mark.phase.store(phase as u8, Ordering::Release);
    w.mark.changed.notify_all();
}

fn wait_until(ready: impl Fn() -> bool) {
    wait_until_on(me(), ready);
}

fn wait_until_on(w: &'static Worker, ready: impl Fn() -> bool) {
    let mut guard = state_of(w);
    while !ready() {
        guard = w
            .mark
            .changed
            .wait(guard)
            .unwrap_or_else(|e| e.into_inner());
    }
}

/// Wait for `ready`, running the pause here if the mutator is parked in a
/// syscall and so cannot run it itself.
///
/// This is the collector's half of the safe region, and without it the two
/// threads wait for each other: the mutator is blocked and will not reach a
/// safepoint until its syscall returns, and the collector is asleep until it
/// does. The mutator notifies this condvar when it parks, which is what makes
/// the sleep below wake at the right moment rather than on a timer.
fn wait_or_serve(ready: impl Fn() -> bool) {
    let w = me();
    loop {
        if ready() {
            return;
        }
        if serve_parked(w) {
            continue;
        }
        // The lock must be dropped before serving: a pause takes it.
        let mut guard = state_of(w);
        while !ready() && !servable(w) {
            guard = w
                .mark
                .changed
                .wait(guard)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// Whether a pause is outstanding and its mutator is parked.
fn servable(w: &'static Worker) -> bool {
    matches!(phase_of(w), Phase::MarkDone | Phase::EvacDone) && crate::worker::is_parked(w)
}

/// Run the pause this worker is waiting for, on its behalf.
///
/// Everything a pause touches is either the worker's heap and the collector's
/// own lists -- which this thread may touch, because it *is* the collector --
/// or the mutator's stack, which is frozen while it is parked and which
/// `walk_worker_roots` reads from the frame pointer it recorded.
fn serve_parked(w: &'static Worker) -> bool {
    if !servable(w) || !crate::worker::claim_parked(w) {
        return false;
    }
    unsafe { safepoint() };
    w.stats.served_pauses.fetch_add(1, Ordering::Relaxed);
    crate::worker::release_parked(w);
    true
}

// ---------------------------------------------------------------------------
// The mutator's side: the pauses
// ---------------------------------------------------------------------------

/// The first pause, given the roots a counting collection just found.
///
/// Must be called from a runtime function generated code called into, with a
/// counting collection just done: that is what clears `FLAG_LOGGED` on every
/// object, which is what guarantees the barrier will record the first overwrite
/// of each one during marking.
///
/// # Safety
/// The collector must be idle, and `roots` must be every collectable reference
/// on the stack right now.
pub(crate) unsafe fn start_with_roots(mut roots: Vec<*mut u8>) {
    debug_assert_eq!(phase(), Phase::Idle);
    let started = Instant::now();
    gc::note_trace_started(me());

    // Everything is now unmarked. Objects allocated from here on are stamped
    // with the new parity, so they are marked from birth.
    flip_mark_parity();
    // Chosen before marking begins, so that the marker knows which references
    // to note, and so that nothing is allocated into a block about to be
    // emptied.
    heap::select_evacuation(evacuate::EVACUATE_BELOW_LINES);
    let cset = heap::evacuating_blocks();

    // The nursery holds objects counting knows are reachable only from the
    // stack, plus the object whose allocation triggered this pause, which is
    // not in any stack map yet.
    with_buffers(|b| {
        roots.extend_from_slice(&b.nursery);
        roots.extend_from_slice(&b.fresh);
        debug_assert!(
            b.satb.is_empty(),
            "the previous trace left its record behind"
        );
    });
    {
        let mut s = state();
        s.work = roots;
        s.remembered.clear();
        s.cset = cset;
    }
    me().mark.tracing.store(true, Ordering::Release);
    ensure_thread();
    set_phase(Phase::Marking);
    gc::record_pause(me(), started);
}

/// The second pause: finish marking, then move everything the program is
/// holding, so that the copying which follows cannot move it underneath.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact and the phase `MarkDone`.
unsafe fn finish_marking() {
    debug_assert_eq!(phase(), Phase::MarkDone);
    let started = Instant::now();
    let (mut work, mut remembered, cset) = {
        let mut s = state();
        (
            std::mem::take(&mut s.work),
            std::mem::take(&mut s.remembered),
            s.cset.clone(),
        )
    };

    // Whatever the barrier recorded after the collector thread declared
    // itself done, and whatever that leads to. The program is stopped, so this
    // converges.
    loop {
        unsafe { mark_all(&mut work, &mut remembered) };
        let more = with_buffers(|b| std::mem::take(&mut b.satb));
        if more.is_empty() {
            break;
        }
        work = more;
    }
    me().mark.tracing.store(false, Ordering::Release);

    // Give the block this thread was allocating out of back to the heap, so
    // the sweep to come can see into it. Not at the trace's *start*: the
    // object whose allocation got us here lives in that block, and a block an
    // allocator holds is the one thing evacuation is guaranteed never to
    // choose.
    heap::retire_local_buffer();

    // Arm the load barrier *before* the counting below rather than after, and
    // for a reason that has nothing to do with the barrier: `evacuating` is
    // also what keeps counting's frees deferred, and the evacuation pause has
    // still to read `to_scan` and `remembered`. Both of those name objects and
    // slots recorded during the mark; freeing one here would let its space be
    // reused while objects move, and the pause would then walk a stranger's
    // bytes with a dead object's layout. The deferral that began at the
    // trace's start therefore runs one pause longer -- which is what
    // `fix_references` visiting `deferred_dead` was always written for.
    //
    // From here until the blocks are released, every reference the program
    // loads out of the heap goes through the load barrier, so this is also
    // what establishes the invariant that barrier maintains: nothing the
    // program holds is in a block that is being emptied.
    if !cset.is_empty() {
        gc::set_evacuating(me(), true);
    }

    // Settle the counts, and free what counting found dead during the mark
    // unless there is an evacuation still to come. This empties every buffer
    // but the nursery.
    unsafe { gc::collect() };

    if cset.is_empty() {
        finish_without_evacuation(remembered);
        gc::record_pause(me(), started);
        return;
    }

    // Move everything the roots point at, before letting the program run
    // again.
    let mut move_root = |slot: *mut *mut u8| {
        let value = unsafe { slot.read() };
        if heap::is_evacuating(value) {
            unsafe { slot.write(evacuate::evacuate_one(value)) };
        }
    };
    unsafe { crate::worker::walk_worker_roots(me(), &mut move_root) };
    // The runtime's own roots go with the program's: a pinned object left in a
    // block being emptied would be written through after the block was gone.
    crate::worker::for_each_pinned_slot(move_root);

    {
        let mut s = state();
        s.remembered = remembered;
    }
    gc::clear_poll(me());
    set_phase(Phase::Evacuating);
    gc::record_pause(me(), started);
}

/// A trace with nothing to evacuate skips straight to sweeping.
fn finish_without_evacuation(remembered: Vec<*mut *mut u8>) {
    debug_assert!(remembered.is_empty(), "no blocks were chosen to empty");
    with_buffers(|b| {
        b.nursery.retain(|&p| unsafe { is_marked(p) });
        b.to_scan.clear();
    });
    gc::clear_poll(me());
    set_phase(Phase::Sweeping);
}

/// The third pause: repoint every reference that still points into a block
/// being emptied, and release the blocks.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact and the phase `EvacDone`.
unsafe fn finish_evacuation() {
    unsafe { finish_evacuation_on(me()) }
}

/// # Safety
/// As [`finish_evacuation`], and `w` must be the worker whose heap this
/// thread's stack refers to.
unsafe fn finish_evacuation_on(w: &'static Worker) {
    debug_assert_eq!(phase_of(w), Phase::EvacDone);
    let started = Instant::now();
    let remembered = std::mem::take(&mut state().remembered);
    let to_scan = with_buffers(|b| std::mem::take(&mut b.to_scan));

    if gc::stress() {
        unsafe { evacuate::verify_nothing_scanned_is_dead(&to_scan) };
    }
    unsafe { evacuate::fix_references(&remembered, &to_scan) };
    if gc::stress() {
        unsafe { evacuate::verify_no_stale_references() };
    }

    heap::note_evacuated_blocks();
    let released = heap::release_evacuated();
    gc::set_evacuating(me(), false);
    state().cset.clear();

    // Counting from here must never name an unmarked object, because the sweep
    // about to run frees exactly those.
    with_buffers(|b| {
        b.nursery.retain(|&p| unsafe { is_marked(p) });
    });
    debug_assert!(released > 0);

    gc::clear_poll(me());
    set_phase(Phase::Sweeping);
    gc::record_pause(me(), started);
}

/// Run whichever pause is wanted. The safepoints call this.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact and no heap pointer held anywhere but the stack maps.
pub unsafe fn safepoint() {
    match phase() {
        Phase::MarkDone => unsafe { finish_marking() },
        Phase::EvacDone => unsafe { finish_evacuation() },
        _ => {}
    }
}

/// Begin a trace, or join the one in flight. The `gc_trace_start` builtin.
///
/// # Safety
/// As [`safepoint`].
pub unsafe fn trace_start() {
    loop {
        match phase() {
            Phase::Idle => break,
            Phase::Marking | Phase::Evacuating => return,
            Phase::MarkDone | Phase::EvacDone => unsafe { safepoint() },
            Phase::Sweeping => wait_until(|| phase() == Phase::Idle),
        }
    }
    let roots = unsafe { gc::collect() };
    unsafe { start_with_roots(roots) };
}

/// Wait for the trace in flight, if any, to be entirely over -- marked,
/// evacuated and swept -- so that what follows sees its effect. The
/// `gc_trace_finish` builtin.
///
/// # Safety
/// As [`safepoint`].
pub unsafe fn trace_finish() {
    loop {
        match phase() {
            Phase::Idle => return,
            Phase::Marking => wait_until(|| phase() != Phase::Marking),
            Phase::Evacuating => wait_until(|| phase() != Phase::Evacuating),
            Phase::MarkDone | Phase::EvacDone => unsafe { safepoint() },
            Phase::Sweeping => wait_until(|| phase() == Phase::Idle),
        }
    }
}

/// A whole trace, synchronously. The `gc_trace` builtin: what a program calls
/// to assert on the collector's behaviour.
///
/// # Safety
/// As [`safepoint`].
pub unsafe fn run_full_trace() {
    unsafe {
        trace_start();
        trace_finish();
    }
}

/// At exit: finish what cannot be dropped, and drop what can.
///
/// A trace that has not started moving anything is abandoned -- copying
/// objects for a program that has already returned is work for no observer.
/// One that has is driven to the end instead, because half-moved is not a
/// state the heap can be left in.
pub fn quiesce(w: &'static Worker) {
    loop {
        match phase_of(w) {
            Phase::Idle => return,
            Phase::Sweeping => wait_until_on(w, || phase_of(w) == Phase::Idle),
            Phase::Marking | Phase::MarkDone => {
                {
                    let _guard = state_of(w);
                    w.mark.abandon.store(true, Ordering::Release);
                    w.mark.changed.notify_all();
                }
                wait_until_on(w, || phase_of(w) == Phase::Idle);
                return;
            }
            // Safe on this thread for the same reason the pauses are: `main`
            // has returned, so the stack holds no generated frames and the
            // root walk finds nothing.
            Phase::Evacuating => wait_until_on(w, || phase_of(w) != Phase::Evacuating),
            // Its own worker's, driven from here: `main` is over, so this
            // thread's stack is the only one that could hold a root and it
            // holds none.
            Phase::EvacDone => unsafe { finish_evacuation_on(w) },
        }
    }
}

// ---------------------------------------------------------------------------
// The collector thread
// ---------------------------------------------------------------------------

fn ensure_thread() {
    let worker = me();
    worker.mark.thread.call_once(|| {
        let name = format!("wsharp-gc-{}", worker.id);
        let spawned = std::thread::Builder::new().name(name).spawn(move || {
            // The collector allocates -- every evacuated object is a copy --
            // and those copies belong in the heap the originals came from, so
            // this thread joins the worker it serves rather than becoming one.
            crate::worker::install(worker);
            // A panic here would leave the program waiting for a phase
            // that never comes. Failing loudly is the only honest option.
            if std::panic::catch_unwind(collector_main).is_err() {
                eprintln!("W# collector: the collector thread panicked");
                std::process::abort();
            }
        });
        if let Err(e) = spawned {
            eprintln!("W# collector: could not start the collector thread: {e}");
            std::process::abort();
        }
    });
}

fn abandoning() -> bool {
    abandoning_on(me())
}

fn abandoning_on(w: &Worker) -> bool {
    w.mark.abandon.load(Ordering::Acquire)
}

fn collector_main() {
    loop {
        wait_until(|| phase() == Phase::Marking || abandoning());
        if !abandoning() {
            mark_concurrently();
        }

        // From here the collector is waiting on a pause that only the mutator
        // can run -- unless it is parked, in which case this thread runs it.
        wait_or_serve(|| matches!(phase(), Phase::Evacuating | Phase::Sweeping) || abandoning());
        if abandoning() {
            abandon();
            continue;
        }
        if phase() == Phase::Evacuating {
            evacuate_concurrently();
            wait_or_serve(|| phase() == Phase::Sweeping || abandoning());
            if abandoning() {
                abandon();
                continue;
            }
        }

        sweep();
        set_phase(Phase::Idle);
    }
}

/// Mark from the snapshot while the program runs, then ask for a pause.
fn mark_concurrently() {
    let (mut work, mut remembered) = {
        let mut s = state();
        (
            std::mem::take(&mut s.work),
            std::mem::take(&mut s.remembered),
        )
    };
    let completed = loop {
        if !unsafe { mark_all(&mut work, &mut remembered) } {
            break false;
        }
        // Marking is done when the work is gone and one look at the barrier's
        // record finds nothing new. The program can keep adding to that record
        // for as long as it runs, so this is not chased to a fixpoint; the
        // pause takes the rest.
        let more = with_buffers(|b| std::mem::take(&mut b.satb));
        if more.is_empty() {
            break true;
        }
        work = more;
    };
    {
        let mut s = state();
        s.work = work;
        s.remembered = remembered;
    }
    if completed {
        // Phase first, poll second: the mutator checks the phase when the poll
        // fires, and must find the pause wanted.
        set_phase(Phase::MarkDone);
        gc::request_safepoint(me());
    }
}

/// Copy the survivors out of the chosen blocks while the program runs, a block
/// at a time, then ask for the pause that repoints the references.
fn evacuate_concurrently() {
    let cset = state().cset.clone();
    let live = |p: *mut u8| unsafe { is_marked(p) };
    let mut moved = 0;
    let mut copies = Vec::new();
    for (s, b) in cset {
        if abandoning() {
            break;
        }
        moved += heap::evacuate_block(s, b, &live, &mut copies);
        // A copy's fields are a snapshot of the original's, so they may still
        // name objects that had not moved when it was taken.
        with_buffers(|buffers| buffers.to_scan.append(&mut copies));
    }
    gc::note_moved(me(), moved);
    set_phase(Phase::EvacDone);
    gc::request_safepoint(me());
}

/// Mark everything reachable from `work`, noting every reference into a block
/// being emptied. Returns false if told to stop.
///
/// Shared by the collector thread and the pause that finishes marking. Fields
/// are read atomically because the program may be writing them at the same
/// time; a value seen is either what was there at the snapshot or something
/// the program stored since, and both are safe to follow.
///
/// # Safety
/// Every entry of `work` must be a collectable object.
unsafe fn mark_all(work: &mut Vec<*mut u8>, remembered: &mut Vec<*mut *mut u8>) -> bool {
    let mut visited = 0usize;
    while let Some(obj) = work.pop() {
        visited += 1;
        if visited.is_multiple_of(4096) && abandoning() {
            return false;
        }
        // `claim_mark` reports whether this call is the one that marked it,
        // which doubles as the "have I been here already?" test.
        if !unsafe { claim_mark(obj) } {
            continue;
        }
        let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
            continue;
        };
        unsafe {
            types::for_each_ptr_offset(obj, info, |offset| {
                let slot = obj.add(offset as usize) as *mut *mut u8;
                let field = (*(slot as *const AtomicPtr<u8>)).load(Ordering::Acquire);
                if !is_collectable(field) {
                    return;
                }
                if heap::is_evacuating(field) {
                    remembered.push(slot);
                }
                work.push(field);
            })
        };
    }
    true
}

/// Free everything the trace did not mark, one block at a time.
fn sweep() {
    let is_garbage = |p: *mut u8| !unsafe { is_marked(p) };
    let mut freed = 0;
    // Spaces added after this are full of objects born marked.
    for s in 0..heap::space_count() {
        for b in 0..heap::block_count(s) {
            freed += heap::sweep_block(s, b, &is_garbage);
        }
    }
    freed += heap::sweep_large(&is_garbage);
    gc::note_freed(me(), freed);
    gc::set_trace_baseline(me(), heap::heap_stats().live_bytes);
}

/// Stand down from a trace that will not be finished. Only reachable before
/// anything has been copied.
fn abandon() {
    heap::revert_evacuation();
    with_buffers(|b| {
        b.satb.clear();
        b.to_scan.clear();
    });
    {
        let mut s = state();
        s.work.clear();
        s.remembered.clear();
        s.cset.clear();
    }
    me().mark.tracing.store(false, Ordering::Release);
    gc::clear_poll(me());
    me().mark.abandon.store(false, Ordering::Release);
    set_phase(Phase::Idle);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::TYPE_ID_FIRST_USER;
    use crate::heap::ws_alloc;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Start a trace, park the mutator in a "syscall", and wait for the trace
    /// to reach `Idle`. Returns how many of its pauses the collector ran on the
    /// mutator's behalf.
    fn trace_with_a_parked_mutator() -> usize {
        let (worker_tx, worker_rx) = mpsc::channel::<&'static Worker>();
        let (parked_tx, parked_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let mutator = std::thread::spawn(move || {
            let worker = Worker::current();
            for _ in 0..64 {
                ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
            }
            // Start a trace and then stop answering, which is what a thread
            // sitting in `read(2)` does.
            unsafe { trace_start() };
            worker_tx.send(worker).expect("the test is listening");
            crate::worker::blocking(|| {
                parked_tx.send(()).expect("the test is listening");
                release_rx.recv().expect("the test releases us")
            });
        });

        let worker = worker_rx.recv().expect("the mutator started a trace");
        parked_rx.recv().expect("the mutator parked");

        let deadline = Instant::now() + Duration::from_secs(20);
        while phase_of(worker) != Phase::Idle {
            assert!(
                Instant::now() < deadline,
                "the trace stalled in {:?} with its mutator parked",
                phase_of(worker)
            );
            std::thread::yield_now();
        }

        release_tx.send(()).expect("the mutator is parked");
        mutator.join().expect("the mutator finished");
        worker.stats.served_pauses.load(Ordering::Relaxed)
    }

    /// A trace runs to completion while its mutator is parked in a syscall.
    ///
    /// This is the whole of what the safe region buys. Without it the collector
    /// finishes marking, sets `MarkDone`, asks for a pause -- and then waits for
    /// a thread that will not reach a safepoint until its syscall returns.
    ///
    /// The count is checked, not just the completion, and for the reason the
    /// root count is: a mutator that happened to park *after* its trace had
    /// already finished would reach `Idle` without the parked path ever having
    /// run, and the test would pass having tested nothing. That ordering is
    /// possible -- parking is a few atomics and the collector has a thread to
    /// start -- so this tries again rather than asserting on one race.
    ///
    /// Each attempt is a fresh worker tracing a heap of its own -- the mark
    /// parity is per worker -- but asking for a pause raises the process-wide
    /// poll flag, so this takes `SERIAL` like everything else that does.
    #[test]
    fn a_trace_finishes_while_its_mutator_is_parked() {
        // The mark parity is per worker, but asking for a pause raises the
        // process-wide poll flag, which another test asserts on.
        let _serial = crate::test_support::SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for _ in 0..8 {
            if trace_with_a_parked_mutator() > 0 {
                return;
            }
        }
        panic!("no pause was ever run for a parked mutator: the safe region is untested");
    }
}
