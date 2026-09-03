//! The mark trace, and the thread that runs it.
//!
//! Counting reclaims most garbage as it appears, but it cannot reclaim a cycle
//! -- two objects pointing at each other keep each other's counts above zero
//! for ever -- and it gives up on an object whose count saturated. Only
//! reachability settles those, and a reachability trace is a walk over the
//! whole live heap. Doing that walk with the program stopped is the pause LXR
//! exists to avoid, so it runs on its own thread while the program continues,
//! bracketed by two short pauses:
//!
//! ```text
//!   initial pause    counting collection; flip the mark parity; choose the
//!   (mutator)        blocks to evacuate; snapshot the roots
//!   marking          the collector thread marks from the snapshot while the
//!   (this thread)    program runs; the write barrier records what it
//!                    overwrites, and the marker follows that too
//!   final pause      finish marking; counting collection; copy survivors out
//!   (mutator)        of the chosen blocks; repoint every reference; release
//!                    the blocks
//!   sweeping         the collector thread frees what was not marked, a block
//!   (this thread)    at a time under the heap lock
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
//! **Why nothing needs a load barrier**: objects only move in the final pause,
//! with the program stopped, and that pause then visits every place a
//! reference can live -- the stack, every live object, the collector's own
//! lists -- and repoints it. Concurrent *copying* would need every load to
//! check for forwarding; concurrent *marking* does not.
//!
//! **Why nothing is freed while marking**: the marker reads any object it is
//! handed, including one that counting has since found dead, because such an
//! object may be the only path the snapshot had to something still live. So
//! counting keeps running during a trace but defers its freeing to the final
//! pause; no memory the marker can reach is recycled underneath it.
//!
//! **Where the pauses happen.** The roots come from walking the mutator's own
//! stack, which only the mutator can do, so both pauses run *on the mutator*,
//! inside a runtime call at a safepoint: the allocator, the loop poll, or the
//! `gc_trace*` builtins. Never a builtin such as `print`, which may be holding
//! its argument in a Rust local no stack map describes. The collector thread
//! asks for the final pause by setting the poll byte; the program gets there at
//! its next back edge or allocation.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::Instant;

use crate::evacuate;
use crate::gc::{self, with_buffers};
use crate::header::{claim_mark, flip_mark_parity, is_marked, type_id_of};
use crate::heap::{self, is_collectable};
use crate::types;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Idle = 0,
    /// The collector thread is marking; the program runs.
    Marking = 1,
    /// Marking is done and the final pause is wanted at the next safepoint.
    MarkDone = 2,
    /// The final pause has happened; the collector thread is sweeping.
    Sweeping = 3,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            0 => Phase::Idle,
            1 => Phase::Marking,
            2 => Phase::MarkDone,
            _ => Phase::Sweeping,
        }
    }
}

static PHASE: AtomicU8 = AtomicU8::new(Phase::Idle as u8);

/// True from the initial pause until the final pause has finished marking:
/// the window in which the barrier must record what it overwrites and
/// counting must not free.
static TRACING: AtomicBool = AtomicBool::new(false);

/// Set at exit to make the collector thread drop whatever it is doing.
static ABANDON: AtomicBool = AtomicBool::new(false);

/// What the mutator hands the collector thread, and gets back.
struct State {
    work: Vec<*mut u8>,
    candidates: usize,
}

// Raw pointers into a heap that outlives the process.
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State {
    work: Vec::new(),
    candidates: 0,
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
// The mutator's side: the two pauses
// ---------------------------------------------------------------------------

/// The initial pause, given the roots a counting collection just found.
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
    // Chosen before marking begins, so that nothing is ever allocated into a
    // block that is about to be emptied.
    let candidates = heap::select_evacuation(evacuate::EVACUATE_BELOW_LINES);

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
        s.candidates = candidates;
    }
    TRACING.store(true, Ordering::Release);
    ensure_thread();
    set_phase(Phase::Marking);
    gc::record_pause(started);
}

/// The final pause.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact and the phase `MarkDone`.
pub(crate) unsafe fn finish_trace() {
    debug_assert_eq!(phase(), Phase::MarkDone);
    let started = Instant::now();
    let (mut work, candidates) = {
        let mut s = state();
        (std::mem::take(&mut s.work), s.candidates)
    };

    // Whatever the barrier recorded after the collector thread declared
    // itself done, and whatever that leads to. The program is stopped, so this
    // converges.
    loop {
        unsafe { mark_all(&mut work) };
        let more = with_buffers(|b| std::mem::take(&mut b.satb));
        if more.is_empty() {
            break;
        }
        work = more;
    }
    TRACING.store(false, Ordering::Release);

    // Settle the counts and free what counting found dead during the mark.
    // This empties every buffer but the nursery, which is what lets the
    // fix-up below know where every reference is.
    unsafe { gc::collect() };

    let moved = if candidates > 0 {
        let moved = heap::evacuate(&|p| unsafe { is_marked(p) });
        unsafe { evacuate::fix_references() };
        if gc::stress() {
            unsafe { evacuate::verify_no_stale_references() };
        }
        moved
    } else {
        0
    };
    let released = heap::release_evacuated();
    debug_assert_eq!(released, candidates);
    gc::note_moved(moved);

    // Counting after this point must never name an unmarked object, because
    // the sweep is about to free every one of them.
    with_buffers(|b| {
        b.nursery.retain(|&p| unsafe { is_marked(p) });
        debug_assert!(b.logged.is_empty() && b.decrements.is_empty());
        debug_assert!(b.fresh.is_empty() && b.deferred_dead.is_empty());
    });

    gc::clear_poll();
    set_phase(Phase::Sweeping);
    gc::record_pause(started);
}

/// Run the final pause if one is wanted. The safepoints call this.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact and no heap pointer held anywhere but the stack maps.
pub unsafe fn safepoint() {
    if phase() == Phase::MarkDone {
        unsafe { finish_trace() };
    }
}

/// Begin a trace, or join the one in flight. The `gc_trace_start` builtin.
///
/// # Safety
/// As [`safepoint`].
pub unsafe fn trace_start() {
    match phase() {
        Phase::Idle => {}
        Phase::Marking => return,
        Phase::MarkDone => {
            unsafe { finish_trace() };
            wait_until(|| phase() == Phase::Idle);
        }
        Phase::Sweeping => wait_until(|| phase() == Phase::Idle),
    }
    let roots = unsafe { gc::collect() };
    unsafe { start_with_roots(roots) };
}

/// Wait for the trace in flight, if any, to be entirely over -- marked,
/// finished and swept -- so that what follows sees its effect. The
/// `gc_trace_finish` builtin.
///
/// # Safety
/// As [`safepoint`].
pub unsafe fn trace_finish() {
    match phase() {
        Phase::Idle => return,
        Phase::Marking => {
            wait_until(|| phase() != Phase::Marking);
            unsafe { safepoint() };
        }
        Phase::MarkDone => unsafe { finish_trace() },
        Phase::Sweeping => {}
    }
    wait_until(|| phase() == Phase::Idle);
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

/// At exit: let a sweep finish, and tell a trace still marking to stand down
/// rather than finish -- copying objects for a program that has already
/// returned would be work for no observer.
pub fn quiesce() {
    match phase() {
        Phase::Idle => {}
        Phase::Sweeping => wait_until(|| phase() == Phase::Idle),
        Phase::Marking | Phase::MarkDone => {
            {
                let _guard = state();
                ABANDON.store(true, Ordering::Release);
                CHANGED.notify_all();
            }
            wait_until(|| phase() == Phase::Idle);
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

fn collector_main() {
    loop {
        wait_until(|| phase() == Phase::Marking || ABANDON.load(Ordering::Acquire));
        if !ABANDON.load(Ordering::Acquire) {
            let mut work = std::mem::take(&mut state().work);
            let completed = loop {
                if !unsafe { mark_all(&mut work) } {
                    break false;
                }
                // Marking is done when the work is gone and one look at the
                // barrier's record finds nothing new. The program can keep
                // adding to the record for as long as it runs, so this is
                // not chased to a fixpoint; the final pause takes the rest.
                let more = with_buffers(|b| std::mem::take(&mut b.satb));
                if more.is_empty() {
                    break true;
                }
                work = more;
            };
            if completed {
                // Phase first, poll second: the mutator checks the phase when
                // the poll fires, and must find the pause wanted.
                set_phase(Phase::MarkDone);
                gc::request_safepoint();
            }
        }

        wait_until(|| phase() == Phase::Sweeping || ABANDON.load(Ordering::Acquire));
        if ABANDON.load(Ordering::Acquire) {
            abandon();
            continue;
        }
        sweep();
        set_phase(Phase::Idle);
    }
}

/// Mark everything reachable from `work`. Returns false if told to stop.
///
/// Shared by the collector thread and the final pause. Fields are read
/// atomically because the program may be writing them at the same time; a
/// value seen is either what was there at the snapshot or something the
/// program stored since, and both are safe to follow.
///
/// # Safety
/// Every entry of `work` must be a collectable object.
unsafe fn mark_all(work: &mut Vec<*mut u8>) -> bool {
    let mut visited = 0usize;
    while let Some(obj) = work.pop() {
        visited += 1;
        if visited.is_multiple_of(4096) && ABANDON.load(Ordering::Relaxed) {
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
        for &offset in info.ptr_offsets {
            let slot = unsafe { obj.add(offset as usize) } as *const AtomicPtr<u8>;
            let field = unsafe { (*slot).load(Ordering::Acquire) };
            if unsafe { is_collectable(field) } {
                work.push(field);
            }
        }
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

/// Stand down from a trace that will not be finished.
fn abandon() {
    heap::revert_evacuation();
    with_buffers(|b| b.satb.clear());
    state().work.clear();
    TRACING.store(false, Ordering::Release);
    gc::clear_poll();
    ABANDON.store(false, Ordering::Release);
    set_phase(Phase::Idle);
}
