//! The collector.
//!
//! LXR: reference counting for the common case, a concurrent mark trace to
//! reclaim the cycles counting cannot, and evacuation of sparsely occupied
//! blocks to keep fragmentation down.
//!
//! This module owns the policy; [`crate::heap`] owns the memory, and
//! [`crate::stackwalk`] owns finding roots.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use crate::header::{
    ALIGN, FLAG_LOGGED, FLAG_MARKED, clear_flag, rc_dec, rc_inc, rc_of, set_flag, test_flag,
    type_id_of,
};
use crate::heap::in_heap;
use crate::stackwalk::walk_roots;
use crate::types;

/// What the write barrier has recorded since the last collection.
///
/// Coalescing reference counting works by *difference*: the barrier snapshots
/// an object's outgoing references the first time it is modified, and the
/// collector compares that snapshot against the object's current state.
/// Everything the barrier needs to say fits in two flat lists.
#[derive(Default)]
struct Buffers {
    /// Objects whose references were snapshotted. Their current references
    /// become increments, and their `FLAG_LOGGED` bit is cleared afterwards.
    logged: Vec<*mut u8>,
    /// The snapshotted references themselves, flattened. Each becomes one
    /// decrement.
    decrements: Vec<*mut u8>,
    /// Objects nothing on the heap points at yet.
    ///
    /// Reference counting counts *heap* references, so an object reachable only
    /// from the stack has a count of zero and no decrement will ever be
    /// recorded for it. Keeping such objects on a list is what lets a
    /// collection notice the moment they leave the root set. They graduate off
    /// the list as soon as something on the heap refers to them, after which
    /// the ordinary count tracks them.
    nursery: Vec<*mut u8>,
    /// Objects allocated since the last collection, exempt from it.
    ///
    /// A collection triggered *by* an allocation runs before the allocator has
    /// returned, so the new object is not in a register, not on the stack, and
    /// not in any stack map -- it is unreachable by every test the collector
    /// has, while being about to be used. One cycle of grace is what makes the
    /// object visible before it can be judged.
    fresh: Vec<*mut u8>,
}

// The buffers hold raw pointers into the heap, which the collector owns for the
// lifetime of the process; the mutex makes concurrent access safe.
unsafe impl Send for Buffers {}

static BUFFERS: Mutex<Buffers> = Mutex::new(Buffers {
    logged: Vec::new(),
    decrements: Vec::new(),
    nursery: Vec::new(),
    fresh: Vec::new(),
});

fn with_buffers<R>(f: impl FnOnce(&mut Buffers) -> R) -> R {
    f(&mut BUFFERS.lock().unwrap_or_else(|e| e.into_inner()))
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
    if info.ptr_offsets.is_empty() {
        return;
    }
    with_buffers(|buffers| {
        for &offset in info.ptr_offsets {
            let field = unsafe { (obj.add(offset as usize) as *const *mut u8).read() };
            if !field.is_null() {
                buffers.decrements.push(field);
            }
        }
        buffers.logged.push(obj);
    });
}

/// Set when the collector wants the program to stop at its next safepoint.
///
/// Generated code tests this byte at every loop back edge, which is what makes
/// a computational loop interruptible at all -- Cranelift's only safepoints are
/// calls, and a loop need not contain one.
static POLL: AtomicU8 = AtomicU8::new(0);

/// The address generated code reads to see whether a collection is wanted.
pub fn poll_flag_address() -> usize {
    &POLL as *const AtomicU8 as usize
}

pub fn request_collection() {
    POLL.store(1, Ordering::Release);
}

/// The slow path of the loop safepoint check.
///
/// # Safety
/// Called from generated code with the frame chain intact.
pub extern "C" fn ws_gc_poll() {
    if POLL.swap(0, Ordering::AcqRel) == 0 {
        return;
    }
    unsafe { trace() };
}

/// Collect at every allocation and check every root found.
static STRESS: AtomicBool = AtomicBool::new(false);
static COLLECTIONS: AtomicUsize = AtomicUsize::new(0);
/// Roots the stack walk has enumerated, in total. Worth having because a walk
/// that silently finds *nothing* would let every root check pass vacuously.
static ROOTS_SEEN: AtomicUsize = AtomicUsize::new(0);
static FREED: AtomicUsize = AtomicUsize::new(0);
static TRACES: AtomicUsize = AtomicUsize::new(0);
static MOVED: AtomicUsize = AtomicUsize::new(0);

/// Objects the collector has relocated.
pub fn moved() -> usize {
    MOVED.load(Ordering::Relaxed)
}

/// Backup mark traces run.
pub fn traces() -> usize {
    TRACES.load(Ordering::Relaxed)
}

/// Objects the collector has reclaimed.
pub fn freed() -> usize {
    FREED.load(Ordering::Relaxed)
}

pub fn roots_seen() -> usize {
    ROOTS_SEEN.load(Ordering::Relaxed)
}

pub fn set_stress(on: bool) {
    STRESS.store(on, Ordering::Relaxed);
}

pub fn stress() -> bool {
    STRESS.load(Ordering::Relaxed)
}

pub fn collections() -> usize {
    COLLECTIONS.load(Ordering::Relaxed)
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
    // Outside the heap it can still be a static object -- a string literal or a
    // zero-field struct's singleton -- which the code generator emitted into
    // the module's data section. Those are identifiable by their type id.
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
    ROOTS_SEEN.fetch_add(checked, Ordering::Relaxed);
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
    if std::env::var_os("WSHARP_GC_STATS").is_none() {
        return;
    }
    let heap = crate::heap::heap_stats();
    let (funcs, safepoints) = crate::stackwalk::registered();
    eprintln!(
        "W# gc: {} collections, {} roots seen, {} freed, {} live of {} allocated \
         ({} live bytes of {}), {} traces, {} moved, {} blocks, {} large, \
         {funcs} functions, {safepoints} safepoints",
        collections(),
        roots_seen(),
        freed(),
        heap.live_objects,
        heap.objects_allocated,
        heap.live_bytes,
        heap.bytes_allocated,
        traces(),
        moved(),
        heap.blocks,
        heap.large_objects,
    );
}

/// How often to collect: every this many objects allocated.
///
/// A count rather than a byte threshold, because reclamation here is per object
/// and the interesting cost is the buffer processing.
const COLLECT_EVERY: usize = 4096;

/// Called from the allocator once it has handed an object back.
///
/// This is one of the program points a collection can actually happen at -- a
/// call, hence a Cranelift safepoint, hence a point where the stack maps
/// describe the roots.
///
/// # Safety
/// Must be called from `ws_alloc`, with the frame chain intact.
pub unsafe fn on_allocation(object: *mut u8) {
    // A new object is born logged: its fields are all null, so there is nothing
    // for the barrier to snapshot and its fast path can simply skip. But it
    // must still join the logged list, because that list is what the next
    // collection re-reads to turn its fields into increments -- the references
    // a struct literal is about to install would otherwise be counted by no
    // one, and everything it points at would look like garbage.
    let has_references =
        types::info(unsafe { type_id_of(object) }).is_some_and(|i| !i.ptr_offsets.is_empty());

    let due = with_buffers(|b| {
        b.fresh.push(object);
        if has_references {
            b.logged.push(object);
        }
        b.fresh.len() + b.nursery.len() >= COLLECT_EVERY
    });
    if stress() {
        unsafe { validate_roots() };
        unsafe { collect() };
        return;
    }
    if due {
        unsafe { collect() };
        // Counting cannot reclaim a cycle, so ask for a mark trace every so
        // many collections. Requesting rather than running it means the trace
        // happens at the program's next safepoint, which may be a loop back
        // edge rather than another allocation -- a program that has stopped
        // allocating and is spinning on a cycle-laden heap still gets swept.
        if COLLECTIONS
            .load(Ordering::Relaxed)
            .is_multiple_of(TRACE_EVERY)
        {
            request_collection();
        }
    }
}

/// Run a mark trace once every this many counting collections.
const TRACE_EVERY: usize = 8;

/// One reference-counting collection.
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
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact.
pub unsafe fn collect() {
    COLLECTIONS.fetch_add(1, Ordering::Relaxed);

    let mut roots: Vec<*mut u8> = Vec::new();
    unsafe {
        walk_roots(|slot| {
            let value = slot.read();
            if !value.is_null() && in_heap(value) {
                roots.push(value);
            }
        })
    };
    ROOTS_SEEN.fetch_add(roots.len(), Ordering::Relaxed);
    roots.sort_unstable();
    roots.dedup();
    let rooted = |p: *mut u8| roots.binary_search(&p).is_ok();

    let (logged, decrements, nursery, fresh) = with_buffers(|b| {
        (
            std::mem::take(&mut b.logged),
            std::mem::take(&mut b.decrements),
            std::mem::take(&mut b.nursery),
            std::mem::take(&mut b.fresh),
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

    // Freeing is iterative, never recursive: a long list is ordinary user data
    // and must not be able to overflow the collector's own stack.
    let mut freed = 0usize;
    let mut seen: Vec<*mut u8> = Vec::new();
    while let Some(obj) = dead.pop() {
        if unsafe { rc_of(obj) } > 0 || rooted(obj) {
            continue;
        }
        // Guard against the same object arriving twice -- two fields of two
        // different objects can both have pointed at it.
        if seen.contains(&obj) {
            continue;
        }
        seen.push(obj);

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
    FREED.fetch_add(freed, Ordering::Relaxed);

    with_buffers(|b| {
        b.nursery.extend(survivors);
        // This cycle's newcomers become next cycle's candidates.
        b.nursery.extend(fresh);
    });
}

/// The backup mark trace: reclaim what reference counting cannot.
///
/// A cycle keeps its own counts above zero for ever -- two objects pointing at
/// each other are each referenced once, so neither ever reaches zero, and
/// neither is ever freed. No amount of counting fixes that; only reachability
/// does. LXR runs this trace occasionally alongside counting, and it is also
/// what recovers objects whose counts saturated.
///
/// Marking starts from three places, and missing any of them frees live data:
/// the stack, via the maps; the objects allocated since the last collection,
/// which nothing may point at yet; and the objects the counting collector is
/// still holding as heap-unreferenced but rooted.
///
/// # Safety
/// Must be called from a runtime function generated code called into, with the
/// frame chain intact.
pub unsafe fn trace() {
    TRACES.fetch_add(1, Ordering::Relaxed);

    // Sizes are taken now because forwarding overwrites the type id: once an
    // object has been moved, nothing can ask how big it was.
    let all = crate::heap::live_objects();
    for &(obj, _) in &all {
        unsafe { clear_flag(obj, FLAG_MARKED) };
    }

    // Choosing candidates before marking is what makes evacuation safe without
    // a load barrier: the trace visits every reference in the heap, so by the
    // time it finishes, nothing points into an evacuated block any more.
    let evacuating = crate::heap::select_evacuation(EVACUATE_BELOW_LINES);

    let mut work: Vec<*mut u8> = Vec::new();
    let mut moved = 0usize;

    // Roots are updated in place. `walk_roots` hands over the address of each
    // slot rather than its value precisely so that a moving collector can.
    unsafe {
        walk_roots(|slot| {
            if let Some(target) = forward(slot.read(), &mut moved) {
                slot.write(target);
                work.push(target);
            }
        })
    };
    // The collector's own lists name objects too, and they move as well.
    with_buffers(|b| {
        for list in [&mut b.fresh, &mut b.nursery] {
            for entry in list.iter_mut() {
                if let Some(target) = forward(*entry, &mut moved) {
                    *entry = target;
                    work.push(target);
                }
            }
        }
    });

    while let Some(obj) = work.pop() {
        // `set_flag` reports whether this call is the one that set it, which
        // doubles as the "have I been here already?" test.
        if !unsafe { set_flag(obj, FLAG_MARKED) } {
            continue;
        }
        let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
            continue;
        };
        for &offset in info.ptr_offsets {
            let slot = unsafe { obj.add(offset as usize) as *mut *mut u8 };
            if let Some(target) = forward(unsafe { slot.read() }, &mut moved) {
                unsafe { slot.write(target) };
                work.push(target);
            }
        }
    }

    let mut freed = 0usize;
    for &(obj, size) in &all {
        if crate::heap::is_evacuating(obj) {
            // Gone with its block, whether it was copied out or was garbage.
            crate::heap::note_evacuated(size);
            if !unsafe { test_flag(obj, FLAG_MARKED) } {
                freed += 1;
            }
            continue;
        }
        if unsafe { test_flag(obj, FLAG_MARKED) } {
            continue;
        }
        unsafe { crate::heap::free_object(obj, size) };
        freed += 1;
    }
    let released = crate::heap::release_evacuated();
    debug_assert_eq!(released, evacuating);
    FREED.fetch_add(freed, Ordering::Relaxed);
    MOVED.fetch_add(moved, Ordering::Relaxed);

    // Reachability has just answered the question the buffers existed to
    // approximate, and their contents may name objects that have moved or
    // gone. They start again from empty.
    with_buffers(|b| {
        for &obj in &b.logged {
            if unsafe { test_flag(obj, FLAG_MARKED) } {
                unsafe { clear_flag(obj, FLAG_LOGGED) };
            }
        }
        b.logged.clear();
        b.decrements.clear();
        b.nursery.retain(|&p| unsafe { test_flag(p, FLAG_MARKED) });
        b.fresh.retain(|&p| unsafe { test_flag(p, FLAG_MARKED) });
    });
}

/// Blocks with at most this many occupied lines are worth evacuating: mostly
/// empty, but pinned by a handful of survivors.
const EVACUATE_BELOW_LINES: u16 = (crate::heap::LINES_PER_BLOCK / 4) as u16;

/// Resolve a reference for the trace, copying the object out of an evacuating
/// block if it is in one.
///
/// Returns the address to use, or `None` if there is nothing to follow. The
/// caller writes the result back into whatever slot it came from, which is what
/// makes the move invisible to the program.
fn forward(value: *mut u8, moved: &mut usize) -> Option<*mut u8> {
    if value.is_null() || !in_heap(value) {
        return None;
    }
    // Already moved by an earlier visit: every other reference to it is being
    // repointed at the same copy.
    let meta = unsafe { crate::header::load_meta(value) };
    if crate::header::is_forwarded_meta(meta) {
        return Some(crate::header::forwarding_target(meta));
    }
    if !crate::heap::is_evacuating(value) {
        return Some(value);
    }

    let Some(size) = (unsafe { types::object_size(value) }) else {
        return Some(value);
    };
    let copy = unsafe { crate::heap::alloc_copy(size) };
    if copy.is_null() {
        return Some(value);
    }
    unsafe { std::ptr::copy_nonoverlapping(value, copy, size as usize) };
    match unsafe { crate::header::try_forward(value, copy) } {
        Ok(()) => {
            *moved += 1;
            Some(copy)
        }
        // Someone else got there first; use theirs and let this copy be
        // reclaimed with its block.
        Err(existing) => Some(existing),
    }
}

/// Visit each non-null heap reference held by `obj`.
fn for_each_reference(obj: *mut u8, mut visit: impl FnMut(*mut u8)) {
    let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
        return;
    };
    for &offset in info.ptr_offsets {
        let field = unsafe { (obj.add(offset as usize) as *const *mut u8).read() };
        if !field.is_null() && in_heap(field) {
            visit(field);
        }
    }
}

/// Called from every builtin under stress.
///
/// Allocation sites turn out to be the *least* interesting safepoints to check:
/// a fresh object's operands are scalars, and anything older is usually dead by
/// then. The calls with live references across them are the ordinary ones --
/// `print(p.name)`, `f(a, b)` -- so checking at every runtime entry point,
/// not just the allocator, is what actually exercises the maps.
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

    #[test]
    fn a_null_slot_is_a_legal_root() {
        // The payload of a null optional is a zero, and that slot is rooted
        // because the same slot holds a real reference when it is present.
        assert!(root_is_plausible(std::ptr::null_mut()));
    }

    #[test]
    fn a_heap_object_is_a_legal_root() {
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32);
        assert!(root_is_plausible(p));
    }

    #[test]
    fn a_misaligned_value_is_not_a_root() {
        // Every object is 16-byte aligned, so this is a cheap check that needs
        // no dereference -- which matters, because dereferencing rubbish is how
        // a bad root turns into a crash somewhere unrelated.
        let p = ws_alloc(TYPE_ID_FIRST_USER, 32);
        assert!(!root_is_plausible(unsafe { p.add(1) }));
        assert!(!root_is_plausible(0xDEAD_BEEF_usize as *mut u8));
    }

    #[test]
    fn a_static_object_outside_the_heap_is_a_legal_root() {
        types::register_type(
            TYPE_ID_FIRST_USER + 300,
            types::TypeLayout {
                name: "Static".into(),
                size: 16,
                ptr_offsets: Vec::new(),
            },
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
        let was = stress();
        set_stress(true);
        assert!(stress());
        set_stress(false);
        assert!(!stress());
        set_stress(was);
    }
}
