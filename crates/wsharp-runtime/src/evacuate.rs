//! Evacuation: moving survivors out of sparse blocks, and repointing every
//! reference at the copies.
//!
//! Refilling holes reclaims the space around a survivor, but not the
//! survivor's own line, and a block pinned by a handful of objects scattered
//! through it stays mostly unusable. Moving those few out is what recovers it.
//!
//! **The copying runs while the program does.** That is what the load barrier
//! ([`ws_resolve`]) is for: a reference read out of a heap object is resolved
//! to wherever the object lives now, so the program can never be holding an
//! address the collector has abandoned. Whoever reaches an object first --
//! the collector, or a program thread through the barrier -- copies it, and a
//! single compare-and-swap on the header decides which copy everyone else
//! will use.
//!
//! **The repointing does not**, because a reference can be sitting in a field
//! nobody is about to read, and those have to be found rather than waited
//! for. It runs in a short pause, and it visits a *list* of places rather than
//! the whole heap: the slots the marker saw pointing into the blocks being
//! emptied, the objects the trace touched afterwards, and the collector's own
//! lists. [`fix_references`] sets out why that list is complete, and under
//! `--gc-stress` the whole heap is walked afterwards to check that it was.

use crate::gc::with_buffers;
use crate::header::{
    FLAG_DEAD, FLAG_IMMORTAL, flags_of_meta, forwarding_target, is_forwarded_meta, is_marked,
    load_meta, test_flag, type_id_of,
};

use crate::heap;
use crate::types;

/// Blocks with at most this many occupied lines are worth evacuating: mostly
/// empty, but pinned by a handful of survivors.
pub(crate) const EVACUATE_BELOW_LINES: u16 = (heap::LINES_PER_BLOCK / 4) as u16;

/// The load barrier's slow path: where this reference lives now.
///
/// Reached only while a trace is moving objects. Three cases: the object has
/// already been moved and the header says where; it is not in a block being
/// emptied, so it is staying put; or it is in one and nobody has moved it yet
/// -- in which case this thread moves it, rather than waiting for the
/// collector to get there. Moving it here is what lets the program *write* to
/// it: a write to a copy the collector is about to abandon would be lost.
///
/// # Safety
/// Called from JIT-compiled code across an FFI boundary; `p` must be null or a
/// reference the program holds.
pub unsafe extern "C" fn ws_resolve(p: *mut u8) -> *mut u8 {
    if p.is_null() {
        return p;
    }
    let meta = unsafe { load_meta(p) };
    if is_forwarded_meta(meta) {
        return forwarding_target(meta);
    }
    if !heap::is_evacuating(p) {
        return p;
    }
    unsafe { evacuate_one(p) }
}

/// Copy one object out of a block being emptied and publish where it went.
///
/// Exactly one caller wins the forwarding word; a loser's copy is handed
/// straight back to the heap, because nothing will ever reference it. Losing
/// is rare -- it takes the collector and the program reaching the same object
/// at the same moment -- and wasting a copy is much cheaper than a lock.
///
/// # Safety
/// `p` must be a live object in a block being evacuated.
pub(crate) unsafe fn evacuate_one(p: *mut u8) -> *mut u8 {
    let Some(size) = (unsafe { crate::types::object_size(p) }) else {
        return p;
    };
    let type_id = unsafe { type_id_of(p) };
    let copy = heap::alloc_copy_shared(type_id, size);
    if copy.is_null() {
        return p;
    }
    // The header goes with it: the copy keeps the original's mark bit, its
    // reference count and its flags, and only its address is different.
    unsafe { std::ptr::copy_nonoverlapping(p, copy, size as usize) };
    match unsafe { crate::header::try_forward(p, copy) } {
        Ok(()) => {
            // The program is holding this reference, so the object is live
            // whatever the marker concluded; say so, because the sweep that
            // follows frees exactly what is unmarked.
            unsafe { crate::header::claim_mark(copy) };
            heap::note_forwarded(size);
            crate::gc::note_copy(copy);
            copy
        }
        Err(existing) => {
            unsafe { heap::free_object(copy, size) };
            existing
        }
    }
}

/// Where `value` lives now: its copy if it was moved, itself if not, and
/// `None` if it is not a collectable reference at all.
///
/// The forwarding test comes first, because a forwarded header no longer
/// holds flags -- its bits are an address -- so asking anything else of it
/// would be reading noise.
///
/// # Safety
/// `value` must be null or point at an object with a readable header.
unsafe fn forward(value: *mut u8) -> Option<*mut u8> {
    if value.is_null() {
        return None;
    }
    let meta = unsafe { load_meta(value) };
    if is_forwarded_meta(meta) {
        return Some(forwarding_target(meta));
    }
    if flags_of_meta(meta) & FLAG_IMMORTAL != 0 {
        return None;
    }
    Some(value)
}

/// Rewrite one slot if what it holds has moved.
///
/// # Safety
/// `slot` must be a readable, writable pointer-sized location.
unsafe fn fix_slot(slot: *mut *mut u8) {
    let value = unsafe { slot.read() };
    if let Some(target) = unsafe { forward(value) }
        && target != value
    {
        unsafe { slot.write(target) };
    }
}

/// # Safety
/// `obj` must be a live, un-forwarded object with a registered type.
unsafe fn fix_fields(obj: *mut u8) {
    let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
        return;
    };
    unsafe {
        types::for_each_ptr_offset(obj, info, |offset| {
            fix_slot(obj.add(offset as usize) as *mut *mut u8)
        })
    };
}

/// Repoint every reference that still points into a block being emptied.
///
/// This runs with the program stopped, and it is what makes releasing those
/// blocks safe. The places such a reference can be, and why this is all of
/// them:
///
/// * a **root slot** -- the stack maps hand over slot addresses precisely so
///   a moving collector can write them. Roots were already resolved when the
///   evacuation began, and every reference the program has loaded since came
///   through the load barrier, so this is a formality; it is cheap, and it is
///   the one place the argument would be hard to check.
/// * a **slot the marker saw pointing into an evacuating block**. The marker
///   scans every live object exactly once, so this catches every reference
///   that existed at the snapshot and was not overwritten.
/// * a **field of an object the trace touched afterwards**: anything the write
///   barrier logged, anything allocated while the trace ran (whose fields the
///   marker never scanned, because a new object is born marked), and every
///   copy made during the evacuation (whose fields are a snapshot of an
///   original's, and so may point at objects that had not moved yet).
/// * a **runtime root** -- something the runtime is holding while it builds an
///   object graph, on the list beside the stack that `worker::PINNED` is.
/// * an entry in one of the **collector's own lists**.
///
/// Under `--gc-stress` the whole heap is walked afterwards and the run aborts
/// if any of this missed something, which is what keeps the argument above
/// honest rather than merely plausible.
///
/// # Safety
/// The evacuation pause, after the copying and before the blocks are released.
pub(crate) unsafe fn fix_references(remembered: &[*mut *mut u8], scan: &[*mut u8]) {
    let worker = crate::worker::Worker::current();
    unsafe { crate::worker::walk_worker_roots(worker, |slot| fix_slot(slot)) };
    crate::worker::for_each_pinned_slot(|slot| unsafe { fix_slot(slot) });
    for &slot in remembered {
        unsafe { fix_slot(slot) };
    }
    for &obj in scan {
        unsafe { fix_fields(obj) };
    }
    with_buffers(|b| {
        for list in [
            &mut b.nursery,
            &mut b.fresh,
            &mut b.logged,
            &mut b.decrements,
            &mut b.satb,
            &mut b.deferred_dead,
        ] {
            for entry in list.iter_mut() {
                unsafe { fix_slot(entry) };
            }
        }
    });
}

/// Under `--gc-stress`: nothing this pause is about to read has been freed.
///
/// The list `fix_references` walks was recorded during the mark, and an entry
/// freed in between will have had its space taken by something else by now --
/// so the pause would read a stranger's bytes through a dead object's layout,
/// and write a forwarding address into the middle of a live one. Counting's
/// frees are deferred through the evacuation precisely so that cannot happen
/// (`mark::finish_marking` arms `evacuating` before it settles the counts),
/// and this is the check that says so out loud.
///
/// # Safety
/// As [`fix_references`].
pub(crate) unsafe fn verify_nothing_scanned_is_dead(scan: &[*mut u8]) {
    for &obj in scan {
        assert!(
            !unsafe { test_flag(obj, FLAG_DEAD) },
            "the evacuation pause was handed {obj:p}, which has been freed"
        );
    }
}

/// Walk the whole heap and abort if anything live still points into a block
/// about to be released. Under `--gc-stress` only: this is the check that the
/// list of places in [`fix_references`] is complete.
///
/// # Safety
/// As [`fix_references`].
pub(crate) unsafe fn verify_no_stale_references() {
    let mut stale = 0usize;
    let mut check = |value: *mut u8| {
        if heap::is_evacuating(value) {
            stale += 1;
        }
    };
    let worker = crate::worker::Worker::current();
    unsafe { crate::worker::walk_worker_roots(worker, |slot| check(slot.read())) };
    crate::worker::for_each_pinned_slot(|slot| check(unsafe { slot.read() }));
    heap::for_each_object(true, |obj| {
        if !unsafe { is_marked(obj) } {
            return;
        }
        let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
            return;
        };
        unsafe {
            types::for_each_ptr_offset(obj, info, |offset| {
                check((obj.add(offset as usize) as *const *mut u8).read())
            })
        };
    });
    with_buffers(|b| {
        for list in [
            &b.nursery,
            &b.fresh,
            &b.logged,
            &b.decrements,
            &b.satb,
            &b.deferred_dead,
        ] {
            for &entry in list {
                check(entry);
            }
        }
    });
    if stale > 0 {
        eprintln!(
            "W# collector: {stale} references still point into evacuated blocks after the fix-up"
        );
        std::process::abort();
    }
}
