//! Reference counting, the write barrier's buffers, and when collections
//! happen.
//!
//! LXR: reference counting for the common case, a concurrent mark trace to
//! reclaim the cycles counting cannot, and evacuation of sparsely occupied
//! blocks to keep fragmentation down.
//!
//! This module owns counting and policy. [`crate::mark`] owns the trace and
//! the thread that runs it, [`crate::evacuate`] owns moving objects,
//! [`crate::heap`] owns the memory, and [`crate::stackwalk`] owns finding
//! roots.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::header::{
    ALIGN, FLAG_DEAD, FLAG_LOGGED, clear_flag, rc_dec, rc_inc, rc_of, set_flag, test_flag,
    type_id_of,
};
use crate::heap::{in_heap, is_collectable};
use crate::worker::Worker;
use crate::mark;
use crate::stackwalk::walk_roots;
use crate::types;

/// What the write barrier has recorded since the last collection.
///
/// Coalescing reference counting works by *difference*: the barrier snapshots
/// an object's outgoing references the first time it is modified, and the
/// collector compares that snapshot against the object's current state.
/// Everything the barrier needs to say fits in a few flat lists.
#[derive(Default)]
pub(crate) struct Buffers {
    /// Objects whose references were snapshotted. Their current references
    /// become increments, and their `FLAG_LOGGED` bit is cleared afterwards.
    pub logged: Vec<*mut u8>,
    /// The snapshotted references themselves, flattened. Each becomes one
    /// decrement.
    pub decrements: Vec<*mut u8>,
    /// Objects nothing on the heap points at yet.
    ///
    /// Reference counting counts *heap* references, so an object reachable only
    /// from the stack has a count of zero and no decrement will ever be
    /// recorded for it. Keeping such objects on a list is what lets a
    /// collection notice the moment they leave the root set. They graduate off
    /// the list as soon as something on the heap refers to them, after which
    /// the ordinary count tracks them.
    pub nursery: Vec<*mut u8>,
    /// Objects allocated since the last collection, exempt from it.
    ///
    /// A collection triggered *by* an allocation runs before the allocator has
    /// returned, so the new object is not in a register, not on the stack, and
    /// not in any stack map -- it is unreachable by every test the collector
    /// has, while being about to be used. One cycle of grace is what makes the
    /// object visible before it can be judged.
    pub fresh: Vec<*mut u8>,
    /// The references the barrier has recorded since the trace in progress
    /// took its snapshot: the same values as go into `decrements`, kept apart
    /// because a different consumer takes them. A reference that existed at
    /// the snapshot and was overwritten since is exactly what a
    /// snapshot-at-the-beginning marker must still follow, and the barrier's
    /// snapshot is exactly that set.
    pub satb: Vec<*mut u8>,
    /// Objects whose every pointer field the evacuation pause must re-examine:
    /// everything the write barrier logged while the trace ran, everything
    /// allocated while it ran (a new object is born marked, so the marker
    /// never scans it), and every copy evacuation made. See
    /// `evacuate::fix_references`.
    pub to_scan: Vec<*mut u8>,
    /// Objects counting found dead while a trace was marking.
    ///
    /// The marker reads any object it is handed, so nothing may be freed --
    /// tombstoned, recycled, deallocated -- while it runs. Counting still runs
    /// during a trace and still settles the counts; it just leaves the
    /// freeing to the trace's final pause. An object judged dead stays dead:
    /// nothing on the heap or the stack referred to it, so nothing can again.
    pub deferred_dead: Vec<*mut u8>,
}

// The buffers hold raw pointers into the heap, which the collector owns for the
// lifetime of the process; the mutex makes concurrent access safe.
unsafe impl Send for Buffers {}

impl Buffers {
    pub(crate) const fn new() -> Buffers {
        Buffers {
            logged: Vec::new(),
            decrements: Vec::new(),
            nursery: Vec::new(),
            fresh: Vec::new(),
            satb: Vec::new(),
            to_scan: Vec::new(),
            deferred_dead: Vec::new(),
        }
    }
}

/// Run `f` on the buffers of the worker this thread belongs to.
pub(crate) fn with_buffers<R>(f: impl FnOnce(&mut Buffers) -> R) -> R {
    with_buffers_of(Worker::current(), f)
}

pub(crate) fn with_buffers_of<R>(worker: &Worker, f: impl FnOnce(&mut Buffers) -> R) -> R {
    f(&mut worker.buffers.lock().unwrap_or_else(|e| e.into_inner()))
}

/// How many objects the barrier has logged since the last collection.
pub fn logged_objects() -> usize {
    with_buffers(|b| b.logged.len())
}

/// The write barrier's slow path: snapshot an object's outgoing references.
///
/// Reached only the first time an object is modified in a cycle -- generated
/// code tests `FLAG_LOGGED` inline and calls here only when it is clear. See
/// `emit_log_barrier` in the code generator for the fast path.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `obj` must point at a
/// live heap object whose type is registered.
pub unsafe extern "C" fn ws_log_object(obj: *mut u8) {
    // Exactly one caller wins the bit and does the snapshot.
    if !unsafe { set_flag(obj, FLAG_LOGGED) } {
        return;
    }
    let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
        return;
    };
    if !info.has_references() {
        return;
    }
    let tracing = mark::tracing();
    let moving = evacuating();

    // Read the fields before taking the buffers lock, not inside it. While
    // objects are moving, a field may name one that has gone, and recording
    // where it went means resolving it -- which may move it, which takes the
    // buffers lock to note the copy. The lock does not nest.
    let mut snapshot: SmallSnapshot = SmallSnapshot::new();
    unsafe {
        types::for_each_ptr_offset(obj, info, |offset| {
            let field = (obj.add(offset as usize) as *const *mut u8).read();
            // A string literal or a singleton is a legal field value but not a
            // counted one: it lives in read-only memory, and decrementing it
            // would fault.
            if !is_collectable(field) {
                return;
            }
            // Record where the object lives now: these lists outlive the blocks
            // being emptied, and adjusting a count through a stale address would
            // be a write into released memory.
            snapshot.push(if moving {
                crate::evacuate::ws_resolve(field)
            } else {
                field
            });
        })
    };

    with_buffers(|buffers| {
        for &field in &snapshot {
            buffers.decrements.push(field);
            if tracing {
                buffers.satb.push(field);
            }
        }
        buffers.logged.push(obj);
    });
}

/// An object's outgoing references, gathered before the buffers lock is taken.
/// A `Vec` rather than anything cleverer: this is the barrier's slow path,
/// reached once per object per collection cycle.
type SmallSnapshot = Vec<*mut u8>;

/// Set when the collector wants the program to stop at its next safepoint.
///
/// Generated code tests this byte at every loop back edge, which is what makes
/// a computational loop interruptible at all -- Cranelift's only safepoints are
/// calls, and a loop need not contain one. The collector thread sets it when
/// marking is done and the trace needs the program stopped to finish.
pub use crate::worker::poll_flag_address;
pub(crate) use crate::worker::{clear_poll, request_safepoint};

/// The slow path of the loop safepoint check.
///
/// # Safety
/// Called from generated code with the frame chain intact.
pub extern "C" fn ws_gc_poll() {
    if !crate::worker::poll_wanted() {
        return;
    }
    // The word is shared, so another worker's request is what usually brings
    // us here. Idempotent either way: a worker that was not asked for a pause
    // simply goes back to what it was doing.
    let worker = Worker::current();
    if !crate::worker::take_safepoint_request(worker) {
        return;
    }
    unsafe { mark::safepoint() };
}

/// Set while a trace is moving objects, and read by the load barrier in front
/// of every reference the program loads out of a heap object.
///
/// One byte rather than a phase comparison because generated code tests it on
/// a path that is taken on every field read: while it is zero the barrier
/// costs a load, a test and a branch that falls through.
pub use crate::worker::evacuating_flag_address;
pub(crate) use crate::worker::set_evacuating;

/// Whether *this* worker is moving objects.
pub fn evacuating() -> bool {
    crate::worker::evacuating(Worker::current())
}

/// Collect at every allocation and check every root found.
/// The one collector switch that stays process-wide: it is set once, before
/// any code runs, and every worker wants the same answer.
static STRESS: AtomicBool = AtomicBool::new(false);

/// Add one worker's counter up across every worker there is.
///
/// A program's collector did what all of its workers' collectors did, so the
/// numbers a program asserts on are the totals.
fn total(of: impl Fn(&Worker) -> usize) -> usize {
    let mut sum = 0;
    crate::worker::for_each_worker(|w| sum += of(w));
    sum
}

/// Objects the collector has relocated.
pub fn moved() -> usize {
    total(|w| w.stats.moved.load(Ordering::Relaxed))
}

/// Mark traces started.
pub fn traces() -> usize {
    total(|w| w.stats.traces.load(Ordering::Relaxed))
}

/// Objects the collector has reclaimed.
pub fn freed() -> usize {
    total(|w| w.stats.freed.load(Ordering::Relaxed))
}

pub fn roots_seen() -> usize {
    total(|w| w.stats.roots_seen.load(Ordering::Relaxed))
}

pub fn set_stress(on: bool) {
    STRESS.store(on, Ordering::Relaxed);
}

pub fn stress() -> bool {
    STRESS.load(Ordering::Relaxed)
}

pub fn collections() -> usize {
    total(|w| w.stats.collections.load(Ordering::Relaxed))
}

pub(crate) fn note_trace_started(worker: &Worker) {
    worker.stats.traces.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_moved(worker: &Worker, n: usize) {
    worker.stats.moved.fetch_add(n, Ordering::Relaxed);
}

/// A copy the program made for itself through the load barrier. Its fields
/// need the same revisit the collector's own copies get.
pub(crate) fn note_copy(copy: *mut u8) {
    let worker = Worker::current();
    worker.stats.moved.fetch_add(1, Ordering::Relaxed);
    with_buffers_of(worker, |b| b.to_scan.push(copy));
}

pub(crate) fn note_freed(worker: &Worker, n: usize) {
    worker.stats.freed.fetch_add(n, Ordering::Relaxed);
}

pub(crate) fn set_trace_baseline(worker: &Worker, live_bytes: usize) {
    worker
        .stats
        .trace_baseline_bytes
        .store(live_bytes, Ordering::Relaxed);
}

/// Account for a pause that began at `started`. The maximum is the number
/// that matters for a collector whose point is low latency.
pub(crate) fn record_pause(worker: &Worker, started: Instant) {
    let us = started.elapsed().as_micros() as usize;
    worker.stats.pauses.fetch_add(1, Ordering::Relaxed);
    worker.stats.total_pause_us.fetch_add(us, Ordering::Relaxed);
    worker.stats.max_pause_us.fetch_max(us, Ordering::Relaxed);
}

/// Whether an environment variable is set to something other than `0` or the
/// empty string. `WSHARP_GC_STATS=0` means off, as anyone would expect.
pub fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

/// What a root slot may hold.
///
/// A pointer slot is not always a pointer: the payload of a null optional is a
/// zero written by the code generator, and it is rooted because the *same* slot
/// holds a real reference when the optional is present. So zero is legal, and
/// anything else must be a real object.
fn root_is_plausible(value: *mut u8) -> bool {
    if value.is_null() {
        return true;
    }
    if !(value as usize).is_multiple_of(ALIGN) {
        return false;
    }
    if in_heap(value) {
        return true;
    }
    // Outside the block space it can still be a large object, or a static one
    // -- a string literal or a zero-field struct's singleton -- which the code
    // generator emitted into the module's data section. Both are identifiable
    // by their type id.
    //
    // Dereferencing something that turned out not to be an object would fault,
    // which under a stress run is exactly the loud failure wanted.
    types::info(unsafe { type_id_of(value) }).is_some()
}

/// Walk the stack and check that every root the maps describe is a real
/// reference.
///
/// This is the defence for the part of the collector that is hardest to get
/// right. Rooting is spread across every expression the code generator lowers,
/// and a slot it forgot shows up here as an immediate, reproducible abort
/// rather than as a corrupted object thousands of allocations later.
///
/// # Safety
/// Must be called from a runtime function that generated code called into.
pub unsafe fn validate_roots() {
    let mut checked = 0usize;
    let mut bad: Option<(*mut *mut u8, *mut u8)> = None;
    unsafe {
        walk_roots(|slot| {
            checked += 1;
            let value = slot.read();
            if bad.is_none() && !root_is_plausible(value) {
                bad = Some((slot, value));
            }
        })
    };
    Worker::current()
        .stats
        .roots_seen
        .fetch_add(checked, Ordering::Relaxed);
    if let Some((slot, value)) = bad {
        eprintln!(
            "W# collector: stack slot {slot:p} was recorded as a heap reference \
             but holds {value:p}, which is not one ({checked} roots checked)"
        );
        std::process::abort();
    }
}

/// Print what the collector has done so far, when `WSHARP_GC_STATS` is set.
///
/// The counts matter as much as the checks: a stack walk that found no roots at
/// all would make every root check pass for the wrong reason.
pub fn report_if_asked() {
    if !env_flag("WSHARP_GC_STATS") {
        return;
    }
    let heap = crate::heap::total_heap_stats();
    let (funcs, safepoints) = crate::stackwalk::registered();
    eprintln!(
        "W# gc: {} collections, {} roots seen, {} freed, {} live of {} allocated \
         ({} live bytes of {}), {} traces, {} moved, {} pauses (max {} us, total {} us), \
         {} blocks, {} large, {funcs} functions, {safepoints} safepoints",
        collections(),
        roots_seen(),
        freed(),
        heap.live_objects,
        heap.objects_allocated,
        heap.live_bytes,
        heap.bytes_allocated,
        traces(),
        moved(),
        total(|w| w.stats.pauses.load(Ordering::Relaxed)),
        // The longest of any worker's, not the sum: a pause is what one
        // program thread waited for, and workers pause independently.
        {
            let mut longest = 0;
            crate::worker::for_each_worker(|w| {
                longest = longest.max(w.stats.max_pause_us.load(Ordering::Relaxed));
            });
            longest
        },
        total(|w| w.stats.total_pause_us.load(Ordering::Relaxed)),
        heap.blocks,
        heap.large_objects,
    );
}

/// Let a trace in flight finish or stand down, so that the numbers printed at
/// exit describe a heap nothing is still working on.
pub fn quiesce() {
    crate::worker::for_each_worker(|w| mark::quiesce(w));
}

/// How often to collect: every this many objects allocated.
///
/// A count rather than a byte threshold, because reclamation here is per object
/// and the interesting cost is the buffer processing.
const COLLECT_EVERY: usize = 4096;

/// Start a mark trace every this many allocations, whatever the heap looks
/// like: counting cannot reclaim a cycle, and a program can make them steadily
/// without ever growing much. A count of allocations rather than of
/// collections so that `--gc-stress`, which collects at every allocation,
/// traces on the same schedule as an ordinary run and the tests stay
/// deterministic under both.
pub(crate) const TRACE_EVERY_ALLOCATIONS: usize = 8 * COLLECT_EVERY;

/// ...and sooner than that if the live heap has doubled since the last trace
/// -- a heap growing that fast is usually one counting is failing to keep up
/// with -- provided it is at least this big, so a small program that merely
/// went from nothing to something is not traced for it.
const TRACE_GROWTH_FLOOR_BYTES: usize = 4 << 20;

/// Called from the allocator once it has handed an object back.
///
/// This is one of the program points a collection can actually happen at -- a
/// call, hence a Cranelift safepoint, hence a point where the stack maps
/// describe the roots.
///
/// # Safety
/// Must be called from `ws_alloc`, with the frame chain intact.
pub unsafe fn on_allocation(object: *mut u8) {
    let worker = Worker::current();
    // A trace that has finished marking is waiting for the program to stop so
    // it can finish. The object just allocated is not on any list yet and not
    // in any stack map, and neither matters: it is in the open block, which is
    // never evacuated, and nothing judges an object no list names.
    unsafe { mark::safepoint() };

    // A new object is born logged: its fields are all null, so there is nothing
    // for the barrier to snapshot and its fast path can simply skip. But it
    // must still join the logged list, because that list is what the next
    // collection re-reads to turn its fields into increments -- the references
    // a struct literal is about to install would otherwise be counted by no
    // one, and everything it points at would look like garbage.
    // An array of references answers yes here with an empty `ptr_offsets`,
    // which is exactly what a fixed list cannot express.
    let has_references =
        types::info(unsafe { type_id_of(object) }).is_some_and(|i| i.has_references());

    let due = with_buffers(|b| {
        b.fresh.push(object);
        if has_references {
            b.logged.push(object);
        }
        b.fresh.len() + b.nursery.len() >= COLLECT_EVERY
    });
    let allocations = worker.stats.allocations.fetch_add(1, Ordering::Relaxed) + 1;
    if !(stress() || due) {
        return;
    }
    // While objects are moving a header may be a forwarding address rather
    // than a count, so counting stands aside until the blocks are released.
    // The window is one evacuation long.
    if evacuating() {
        return;
    }
    if stress() {
        unsafe { validate_roots() };
    }
    let roots = unsafe { collect() };
    // Counting cannot reclaim a cycle, so trace every so often. The trace
    // begins here, in the same pause: the roots just found are its snapshot,
    // and the object just allocated is on the nursery list, which is part of
    // that snapshot.
    if trace_due(worker, allocations) && mark::phase() == mark::Phase::Idle {
        worker
            .stats
            .trace_at
            .store(allocations + TRACE_EVERY_ALLOCATIONS, Ordering::Relaxed);
        unsafe { mark::start_with_roots(roots) };
    }
}

/// A threshold rather than a modulus, because this is only consulted when a
/// collection is due, and a due collection need not land on a round number.
fn trace_due(worker: &Worker, allocations: usize) -> bool {
    if allocations >= worker.stats.trace_at.load(Ordering::Relaxed) {
        return true;
    }
    let live = crate::heap::heap_stats().live_bytes;
    live >= TRACE_GROWTH_FLOOR_BYTES
        && live >= 2 * worker.stats.trace_baseline_bytes.load(Ordering::Relaxed)
}

/// One reference-counting collection. Returns the roots it found, so that a
/// trace started in the same pause need not walk the stack again.
///
/// Three sources of truth are reconciled:
///
/// * the **stack**, whose roots the maps describe exactly;
/// * the **snapshot** the write barrier took of every modified object, which
///   gives the decrements;
/// * the objects' **current** references, which give the increments.
///
/// Roots are handled as a set rather than as increments-then-decrements. The
/// obvious "increment every root, collect, decrement every root" is wrong in a
/// way that is easy to miss: the final decrements would drive stack-only
/// objects to zero and free things that are plainly still live.
///
/// While a trace is marking, the counts are settled but nothing is freed; see
/// [`Buffers::deferred_dead`].
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact.
pub unsafe fn collect() -> Vec<*mut u8> {
    let worker = Worker::current();
    worker.stats.collections.fetch_add(1, Ordering::Relaxed);
    // Publish this thread's allocations before anything is taken off the live
    // count, so that a free can never run ahead of the allocation it undoes.
    crate::heap::flush_local_counters();

    let mut roots: Vec<*mut u8> = Vec::new();
    unsafe {
        walk_roots(|slot| {
            let value = slot.read();
            if is_collectable(value) {
                roots.push(value);
            }
        })
    };
    // What the runtime is holding, which no stack map describes. See
    // `worker::PINNED`.
    crate::worker::for_each_pinned_slot(|slot| {
        let value = unsafe { slot.read() };
        if unsafe { is_collectable(value) } {
            roots.push(value);
        }
    });
    worker
        .stats
        .roots_seen
        .fetch_add(roots.len(), Ordering::Relaxed);
    roots.sort_unstable();
    roots.dedup();
    let rooted = |p: *mut u8| roots.binary_search(&p).is_ok();

    let defer_frees = mark::tracing();
    let (logged, decrements, nursery, fresh, deferred) = with_buffers(|b| {
        let logged = std::mem::take(&mut b.logged);
        // Everything the barrier logged during a trace is an object whose
        // fields the evacuation pause must look at again: the marker either
        // never saw them, or saw them before the program changed them.
        if defer_frees {
            b.to_scan.extend_from_slice(&logged);
        }
        (
            logged,
            std::mem::take(&mut b.decrements),
            std::mem::take(&mut b.nursery),
            std::mem::take(&mut b.fresh),
            if defer_frees {
                Vec::new()
            } else {
                std::mem::take(&mut b.deferred_dead)
            },
        )
    });

    // Increments first, so an object whose reference merely moved from one
    // field to another is never transiently zero.
    for &obj in &logged {
        for_each_reference(obj, |field| unsafe {
            rc_inc(field);
        });
        // Re-arm the barrier for the next cycle.
        unsafe { clear_flag(obj, FLAG_LOGGED) };
    }

    let mut dead: Vec<*mut u8> = Vec::new();
    for &old in &decrements {
        if unsafe { rc_dec(old) } == 0 && !rooted(old) {
            dead.push(old);
        }
    }

    // An object nothing on the heap points at is garbage the moment it leaves
    // the root set. One that something *does* point at now has a real count,
    // so it graduates off the list.
    let mut survivors = Vec::with_capacity(nursery.len());
    for &obj in &nursery {
        let rc = unsafe { rc_of(obj) };
        if rc > 0 {
            continue;
        }
        if rooted(obj) {
            survivors.push(obj);
        } else {
            dead.push(obj);
        }
    }

    if defer_frees {
        with_buffers(|b| {
            b.deferred_dead.extend(dead);
            b.nursery.extend(survivors);
            b.nursery.extend(fresh);
        });
        return roots;
    }
    dead.extend(deferred);

    // Freeing is iterative, never recursive: a long list is ordinary user data
    // and must not be able to overflow the collector's own stack.
    let mut freed = 0usize;
    while let Some(obj) = dead.pop() {
        if unsafe { rc_of(obj) } > 0 || rooted(obj) {
            continue;
        }
        // The same object can arrive twice -- two fields of two different
        // objects can both have pointed at it -- and the tombstone the first
        // free left behind is what says so.
        if unsafe { test_flag(obj, FLAG_DEAD) } {
            continue;
        }

        for_each_reference(obj, |child| {
            if unsafe { rc_dec(child) } == 0 && !rooted(child) {
                dead.push(child);
            }
        });
        let Some(size) = (unsafe { types::object_size(obj) }) else {
            continue;
        };
        unsafe { crate::heap::free_object(obj, size) };
        freed += 1;
    }
    worker.stats.freed.fetch_add(freed, Ordering::Relaxed);

    with_buffers(|b| {
        b.nursery.extend(survivors);
        // This cycle's newcomers become next cycle's candidates.
        b.nursery.extend(fresh);
    });
    roots
}

/// Visit each collectable reference held by `obj`.
pub(crate) fn for_each_reference(obj: *mut u8, mut visit: impl FnMut(*mut u8)) {
    let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
        return;
    };
    unsafe {
        types::for_each_ptr_offset(obj, info, |offset| {
            let field = (obj.add(offset as usize) as *const *mut u8).read();
            if is_collectable(field) {
                visit(field);
            }
        })
    };
}

/// Called from every builtin under stress.
///
/// Allocation sites turn out to be the *least* interesting safepoints to check:
/// a fresh object's operands are scalars, and anything older is usually dead by
/// then. The calls with live references across them are the ordinary ones --
/// `print(p.name)`, `f(a, b)` -- so checking at every runtime entry point,
/// not just the allocator, is what actually exercises the maps.
///
/// This checks and does nothing else. A builtin may be holding its argument in
/// a Rust local that no stack map describes, so it is not a place a trace may
/// finish and move things.
///
/// # Safety
/// Must be called directly from a function generated code called into.
pub unsafe fn checkpoint() {
    if !stress() {
        return;
    }
    unsafe { validate_roots() };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{TYPE_ID_FIRST_USER, meta_word};
    use crate::heap::ws_alloc;
    use crate::test_support::SERIAL;

    #[test]
    fn a_null_slot_is_a_legal_root() {
        // The payload of a null optional is a zero, and that slot is rooted
        // because the same slot holds a real reference when it is present.
        assert!(root_is_plausible(std::ptr::null_mut()));
    }

    #[test]
    fn a_heap_object_is_a_legal_root() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        assert!(root_is_plausible(p));
    }

    #[test]
    fn a_misaligned_value_is_not_a_root() {
        // Every object is 16-byte aligned, so this is a cheap check that needs
        // no dereference -- which matters, because dereferencing rubbish is how
        // a bad root turns into a crash somewhere unrelated.
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32, 0);
        assert!(!root_is_plausible(unsafe { p.add(1) }));
        assert!(!root_is_plausible(0xDEAD_BEEF_usize as *mut u8));
    }

    #[test]
    fn a_static_object_outside_the_heap_is_a_legal_root() {
        types::register_type(
            TYPE_ID_FIRST_USER + 300,
            types::TypeLayout::fixed("Static", 16, Vec::new()),
        );
        types::publish();

        #[repr(align(16))]
        #[allow(dead_code)]
        struct Obj([u64; 2]);
        let obj = Obj([meta_word(TYPE_ID_FIRST_USER + 300, 0), 0]);
        let p = &obj as *const Obj as *mut u8;
        assert!(!in_heap(p), "it is in the data section, not the heap");
        assert!(root_is_plausible(p));
    }

    #[test]
    fn stress_can_be_switched_on_and_off() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let was = stress();
        set_stress(true);
        assert!(stress());
        set_stress(false);
        assert!(!stress());
        set_stress(was);
    }

    #[test]
    fn env_flags_treat_zero_and_empty_as_unset() {
        // Serialised because the environment is process-wide.
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let name = "WSHARP_TEST_FLAG";
        unsafe { std::env::remove_var(name) };
        assert!(!env_flag(name));
        for (value, expected) in [("1", true), ("yes", true), ("0", false), ("", false)] {
            unsafe { std::env::set_var(name, value) };
            assert_eq!(env_flag(name), expected, "{name}={value:?}");
        }
        unsafe { std::env::remove_var(name) };
    }
}
