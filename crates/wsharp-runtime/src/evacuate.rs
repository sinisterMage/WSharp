//! Evacuation: moving survivors out of sparse blocks, and repointing every
//! reference at the copies.
//!
//! Freeing works a line at a time, so one survivor pins the free lines around
//! it, and over time a heap fills with mostly-empty blocks it cannot reuse.
//! The trace's final pause copies the survivors of the sparsest blocks out,
//! leaves a forwarding word in each old header, and then visits every place a
//! reference can live and rewrites it. The blocks are then free, whole.
//!
//! Because all of this happens with the program stopped, and the fix-up
//! visits everything, no load barrier is needed: by the time the program
//! resumes, nothing points into an evacuated block any more. Under
//! `--gc-stress` that claim is checked rather than trusted.

use crate::gc::with_buffers;
use crate::header::{
    FLAG_IMMORTAL, flags_of_meta, forwarding_target, is_forwarded_meta, is_marked, load_meta,
    type_id_of,
};
use crate::heap;
use crate::stackwalk::walk_roots;
use crate::types;

/// Blocks with at most this many occupied lines are worth evacuating: mostly
/// empty, but pinned by a handful of survivors.
pub(crate) const EVACUATE_BELOW_LINES: u16 = (heap::LINES_PER_BLOCK / 4) as u16;

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
    for &offset in info.ptr_offsets {
        unsafe { fix_slot(obj.add(offset as usize) as *mut *mut u8) };
    }
}

/// Repoint every reference in the program at the copies.
///
/// The places a reference can live, and why this is all of them:
///
/// * a **root slot** -- the stack maps hand over slot addresses precisely so
///   a moving collector can write them;
/// * a **field of a live object** in a block that is not being emptied --
///   which includes the copies themselves, since they went into the open
///   block. Unmarked objects are skipped: they are garbage the sweep is about
///   to free, and nothing will read their fields again;
/// * the **nursery**, the one collector list a counting collection leaves
///   populated. The others were emptied by the collection that ran just
///   before evacuation, and a debug build checks that.
///
/// Registers hold nothing: at a safepoint every live value is in its slot.
///
/// # Safety
/// The final pause, after `heap::evacuate` and before `release_evacuated`.
pub(crate) unsafe fn fix_references() {
    unsafe { walk_roots(|slot| fix_slot(slot)) };
    heap::for_each_object(true, |obj| {
        if unsafe { is_marked(obj) } {
            unsafe { fix_fields(obj) };
        }
    });
    with_buffers(|b| {
        for entry in b.nursery.iter_mut() {
            unsafe { fix_slot(entry) };
        }
        debug_assert!(b.logged.is_empty() && b.decrements.is_empty());
        debug_assert!(b.fresh.is_empty() && b.satb.is_empty() && b.deferred_dead.is_empty());
    });
}

/// Walk the same places [`fix_references`] does and abort if any of them still
/// points into a block about to be released. Under `--gc-stress` only.
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
    unsafe { walk_roots(|slot| check(slot.read())) };
    heap::for_each_object(true, |obj| {
        if !unsafe { is_marked(obj) } {
            return;
        }
        let Some(info) = types::info(unsafe { type_id_of(obj) }) else {
            return;
        };
        for &offset in info.ptr_offsets {
            check(unsafe { (obj.add(offset as usize) as *const *mut u8).read() });
        }
    });
    with_buffers(|b| {
        for &entry in &b.nursery {
            check(entry);
        }
    });
    if stale > 0 {
        eprintln!(
            "W# collector: {stale} references still point into evacuated blocks after the fix-up"
        );
        std::process::abort();
    }
}
