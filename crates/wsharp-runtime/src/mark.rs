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

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::Instant;

use crate::evacuate;
use crate::gc::{self, with_buffers};
use crate::header::{claim_mark, flip_mark_parity, is_marked, type_id_of};
use crate::heap::{self, is_collectable};
use crate::stackwalk::walk_roots;
use crate::types;

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

static PHASE: AtomicU8 = AtomicU8::new(Phase::Idle as u8);

/// True from the initial pause until marking has finished: the window in which
/// the barrier must record what the program overwrites and counting must not
/// free.
static TRACING: AtomicBool = AtomicBool::new(false);

/// Set at exit to make the collector thread drop whatever it is doing.
static ABANDON: AtomicBool = AtomicBool::new(false);

/// What the mutator hands the collector thread, and gets back.
struct State {
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

static STATE: Mutex<State> = Mutex::new(State {
    work: Vec::new(),
    remembered: Vec::new(),
    cset: Vec::new(),
});
/// Signalled on every phase change and on abandon.
static CHANGED: Condvar = Condvar::new();
static THREAD: Once = Once::new();

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn phase() -> Phase {
    Phase::from_u8(PHASE.load(Ordering::Acquire))
}

/// Whether a trace is between its snapshot and the end of marking.
pub(crate) fn tracing() -> bool {
    TRACING.load(Ordering::Acquire)
}

/// Publish a phase change. The store happens under the state lock and the
/// waiters check under it, so a wake-up cannot be lost.
fn set_phase(phase: Phase) {
    let _guard = state();
    PHASE.store(phase as u8, Ordering::Release);
    CHANGED.notify_all();
}

fn wait_until(ready: impl Fn() -> bool) {
    let mut guard = state();
    while !ready() {
        guard = CHANGED.wait(guard).unwrap_or_else(|e| e.into_inner());
    }
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
    gc::note_trace_started();

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
    TRACING.store(true, Ordering::Release);
    ensure_thread();
    set_phase(Phase::Marking);
    gc::record_pause(started);
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
    TRACING.store(false, Ordering::Release);

    // Give the block this thread was allocating out of back to the heap, so
    // the sweep to come can see into it. Not at the trace's *start*: the
    // object whose allocation got us here lives in that block, and a block an
    // allocator holds is the one thing evacuation is guaranteed never to
    // choose.
    heap::retire_local_buffer();

    // Settle the counts and free what counting found dead during the mark.
    // This empties every buffer but the nursery.
    unsafe { gc::collect() };

    if cset.is_empty() {
        finish_without_evacuation(remembered);
        gc::record_pause(started);
        return;
    }

    // Move everything the roots point at, before letting the program run
    // again. From here until the blocks are released, every reference the
    // program loads out of the heap goes through the load barrier, so this is
    // what establishes the invariant that barrier maintains: nothing the
    // program holds is in a block that is being emptied.
    gc::set_evacuating(true);
    unsafe {
        walk_roots(|slot| {
            let value = slot.read();
            if heap::is_evacuating(value) {
                slot.write(evacuate::evacuate_one(value));
            }
        })
    };

    {
        let mut s = state();
        s.remembered = remembered;
    }
    gc::clear_poll();
    set_phase(Phase::Evacuating);
    gc::record_pause(started);
}

/// A trace with nothing to evacuate skips straight to sweeping.
fn finish_without_evacuation(remembered: Vec<*mut *mut u8>) {
    debug_assert!(remembered.is_empty(), "no blocks were chosen to empty");
    with_buffers(|b| {
        b.nursery.retain(|&p| unsafe { is_marked(p) });
        b.to_scan.clear();
    });
    gc::clear_poll();
    set_phase(Phase::Sweeping);
}

/// The third pause: repoint every reference that still points into a block
/// being emptied, and release the blocks.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact and the phase `EvacDone`.
unsafe fn finish_evacuation() {
    debug_assert_eq!(phase(), Phase::EvacDone);
    let started = Instant::now();
    let remembered = std::mem::take(&mut state().remembered);
    let to_scan = with_buffers(|b| std::mem::take(&mut b.to_scan));

    unsafe { evacuate::fix_references(&remembered, &to_scan) };
    if gc::stress() {
        unsafe { evacuate::verify_no_stale_references() };
    }

    heap::note_evacuated_blocks();
    let released = heap::release_evacuated();
    gc::set_evacuating(false);
    state().cset.clear();

    // Counting from here must never name an unmarked object, because the sweep
    // about to run frees exactly those.
    with_buffers(|b| {
        b.nursery.retain(|&p| unsafe { is_marked(p) });
    });
    debug_assert!(released > 0);

    gc::clear_poll();
    set_phase(Phase::Sweeping);
    gc::record_pause(started);
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
pub fn quiesce() {
    loop {
        match phase() {
            Phase::Idle => return,
            Phase::Sweeping => wait_until(|| phase() == Phase::Idle),
            Phase::Marking | Phase::MarkDone => {
                {
                    let _guard = state();
                    ABANDON.store(true, Ordering::Release);
                    CHANGED.notify_all();
                }
                wait_until(|| phase() == Phase::Idle);
                return;
            }
            // Safe on this thread for the same reason the pauses are: `main`
            // has returned, so the stack holds no generated frames and the
            // root walk finds nothing.
            Phase::Evacuating => wait_until(|| phase() != Phase::Evacuating),
            Phase::EvacDone => unsafe { finish_evacuation() },
        }
    }
}

// ---------------------------------------------------------------------------
// The collector thread
// ---------------------------------------------------------------------------

fn ensure_thread() {
    THREAD.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("wsharp-gc".into())
            .spawn(|| {
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
    ABANDON.load(Ordering::Acquire)
}

fn collector_main() {
    loop {
        wait_until(|| phase() == Phase::Marking || abandoning());
        if !abandoning() {
            mark_concurrently();
        }

        wait_until(|| matches!(phase(), Phase::Evacuating | Phase::Sweeping) || abandoning());
        if abandoning() {
            abandon();
            continue;
        }
        if phase() == Phase::Evacuating {
            evacuate_concurrently();
            wait_until(|| phase() == Phase::Sweeping || abandoning());
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
        gc::request_safepoint();
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
    gc::note_moved(moved);
    set_phase(Phase::EvacDone);
    gc::request_safepoint();
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
    gc::note_freed(freed);
    gc::set_trace_baseline(heap::heap_stats().live_bytes);
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
    TRACING.store(false, Ordering::Release);
    gc::clear_poll();
    ABANDON.store(false, Ordering::Release);
    set_phase(Phase::Idle);
}
