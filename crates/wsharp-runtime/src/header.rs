//! The heap object header.
//!
//! Every heap object -- struct instance, string, closure -- starts with the same
//! 16 bytes:
//!
//! ```text
//!   offset 0 : meta   u64   type id | flags | reference count
//!   offset 8 : aux    u64   element count for strings and (later) arrays, 0 otherwise
//!   offset 16: fields ...
//! ```
//!
//! The meta word carries everything the LXR collector (reference counting plus
//! concurrent copying) needs, without growing the object:
//!
//! ```text
//!   bits  0..32 : type id, an index into the type registry
//!   bits 32..40 : flags
//!   bits 40..63 : reference count, saturating
//!   bit  63     : forwarded
//! ```
//!
//! **Forwarding overwrites the whole word.** When the collector moves an
//! object it stores `FORWARDED | new_address` over the meta, losing the type id
//! and the count -- which costs nothing, because both moved with the object and
//! a forwarded object is dead. Userspace addresses on x86-64 and aarch64 are
//! below 2^47, so the top bit is free to mean this. Taking that bit from the
//! reference count rather than adding a fourth header word keeps every object
//! 8 bytes smaller; a 23-bit count saturates at 8.4 million, and LXR saturates
//! by design anyway.
//!
//! The meta word is accessed atomically throughout, because the collector reads
//! and writes it while the mutator is running.

use std::sync::atomic::{AtomicU64, Ordering};

/// Bytes of header in front of every heap object's fields.
pub const HEADER_SIZE: u32 = 16;
/// Byte offset of the meta word from the start of an object.
pub const META_OFFSET: i32 = 0;
/// Byte offset of the auxiliary word (string/array length) from the object start.
pub const AUX_OFFSET: i32 = 8;
/// All heap objects are aligned to this, so the low bits of a pointer are free
/// for tagging later.
pub const ALIGN: usize = 16;

pub const TYPE_ID_BITS: u32 = 32;
pub const FLAG_SHIFT: u32 = 32;
pub const FLAG_BITS: u32 = 8;
pub const RC_SHIFT: u32 = 40;
/// Bits 40..63. Bit 63 is [`FORWARDED`].
pub const RC_BITS: u32 = 23;
/// The count sticks here rather than wrapping. LXR relies on saturation: an
/// object this popular is never freed by counting alone, and the backup trace
/// is what reclaims it.
pub const RC_MAX: u32 = (1 << RC_BITS) - 1;

pub const TYPE_ID_MASK: u64 = (1 << TYPE_ID_BITS) - 1;
pub const FLAG_MASK: u64 = ((1 << FLAG_BITS) - 1) << FLAG_SHIFT;
pub const RC_MASK: u64 = ((1u64 << RC_BITS) - 1) << RC_SHIFT;
/// Set when the meta word has been replaced by a forwarding address.
pub const FORWARDED: u64 = 1 << 63;

/// Never collected, and never traced through. Set on string literals and on the
/// singleton instance of a zero-field struct, which live in the JIT's data
/// section rather than the W# heap.
pub const FLAG_IMMORTAL: u64 = 1 << 0;
/// Reached by the backup trace in the current cycle.
pub const FLAG_MARKED: u64 = 1 << 1;
/// LXR's "this object's references have been logged" bit. Set by the write
/// barrier the first time a field is overwritten in an epoch, and by the
/// allocator, since a new object has nothing to log.
pub const FLAG_LOGGED: u64 = 1 << 2;
/// Reclaimed. The type id is deliberately left intact so that a linear walk
/// over a block can still work out how many bytes to step over; without that
/// the heap would stop being walkable the moment anything was freed.
pub const FLAG_DEAD: u64 = 1 << 3;

/// Identifies an object's type at runtime. Read by the collector to find a
/// layout, and by multiple dispatch to select an overload.
pub type TypeId = u32;

pub const TYPE_ID_INVALID: TypeId = 0;
pub const TYPE_ID_STR: TypeId = 1;
pub const TYPE_ID_CLOSURE: TypeId = 2;
/// User-declared types are numbered from here, leaving room for more built-ins.
pub const TYPE_ID_FIRST_USER: TypeId = 16;

pub const fn meta_word(type_id: TypeId, flags: u64) -> u64 {
    (type_id as u64) | ((flags << FLAG_SHIFT) & FLAG_MASK)
}

pub const fn type_id_of_meta(meta: u64) -> TypeId {
    (meta & TYPE_ID_MASK) as TypeId
}

pub const fn flags_of_meta(meta: u64) -> u64 {
    (meta & FLAG_MASK) >> FLAG_SHIFT
}

pub const fn rc_of_meta(meta: u64) -> u32 {
    ((meta & RC_MASK) >> RC_SHIFT) as u32
}

/// Round an object size up to the heap's alignment.
pub const fn align_up(size: u32) -> u32 {
    let a = ALIGN as u32;
    size.div_ceil(a) * a
}

// ---------------------------------------------------------------------------
// Atomic access
// ---------------------------------------------------------------------------

/// The meta word as an atomic.
///
/// # Safety
/// `ptr` must point at a live W# heap object or a static one emitted by the
/// code generator. Both are 16-byte aligned, so the `u64` load is aligned.
#[inline]
pub unsafe fn meta_cell<'a>(ptr: *const u8) -> &'a AtomicU64 {
    unsafe { &*(ptr as *const AtomicU64) }
}

/// # Safety
/// See [`meta_cell`].
#[inline]
pub unsafe fn load_meta(ptr: *const u8) -> u64 {
    unsafe { meta_cell(ptr) }.load(Ordering::Acquire)
}

/// Read the type id of a live object. Safe to call on any pointer produced by
/// `ws_alloc` or on a static object emitted by the code generator.
///
/// # Safety
/// `ptr` must point at a W# heap object, and must not have been forwarded --
/// a forwarded object's type id is gone. Follow the forwarding first.
#[inline]
pub unsafe fn type_id_of(ptr: *const u8) -> TypeId {
    type_id_of_meta(unsafe { load_meta(ptr) })
}

/// # Safety
/// See [`meta_cell`].
#[inline]
pub unsafe fn test_flag(ptr: *const u8, flag: u64) -> bool {
    (unsafe { load_meta(ptr) }) & (flag << FLAG_SHIFT) != 0
}

/// Set `flag`, returning true if this call is the one that set it.
///
/// The write barrier uses the return value to decide whether it owns the job of
/// logging the object, so exactly one thread does it.
///
/// # Safety
/// See [`meta_cell`].
#[inline]
pub unsafe fn set_flag(ptr: *const u8, flag: u64) -> bool {
    let bit = flag << FLAG_SHIFT;
    let prev = unsafe { meta_cell(ptr) }.fetch_or(bit, Ordering::AcqRel);
    prev & bit == 0
}

/// # Safety
/// See [`meta_cell`].
#[inline]
pub unsafe fn clear_flag(ptr: *const u8, flag: u64) {
    unsafe { meta_cell(ptr) }.fetch_and(!(flag << FLAG_SHIFT), Ordering::AcqRel);
}

/// # Safety
/// See [`meta_cell`].
#[inline]
pub unsafe fn rc_of(ptr: *const u8) -> u32 {
    rc_of_meta(unsafe { load_meta(ptr) })
}

/// Increment the reference count, saturating at [`RC_MAX`]. Returns the new
/// count.
///
/// # Safety
/// See [`meta_cell`].
pub unsafe fn rc_inc(ptr: *const u8) -> u32 {
    unsafe { rc_update(ptr, |rc| rc.saturating_add(1).min(RC_MAX)) }
}

/// Decrement the reference count, saturating at zero and *never* moving off
/// [`RC_MAX`]. Returns the new count; zero means the object may be free.
///
/// A count that reached the maximum is no longer accurate, so decrementing it
/// could free a live object. Such objects are left to the backup trace.
///
/// # Safety
/// See [`meta_cell`].
pub unsafe fn rc_dec(ptr: *const u8) -> u32 {
    unsafe {
        rc_update(ptr, |rc| {
            if rc == RC_MAX {
                RC_MAX
            } else {
                rc.saturating_sub(1)
            }
        })
    }
}

/// # Safety
/// See [`meta_cell`].
unsafe fn rc_update(ptr: *const u8, f: impl Fn(u32) -> u32) -> u32 {
    let cell = unsafe { meta_cell(ptr) };
    let mut meta = cell.load(Ordering::Acquire);
    loop {
        let next = f(rc_of_meta(meta));
        let updated = (meta & !RC_MASK) | ((next as u64) << RC_SHIFT);
        match cell.compare_exchange_weak(meta, updated, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return next,
            Err(seen) => meta = seen,
        }
    }
}

// ---------------------------------------------------------------------------
// Forwarding
// ---------------------------------------------------------------------------

pub const fn is_forwarded_meta(meta: u64) -> bool {
    meta & FORWARDED != 0
}

pub const fn forwarding_meta(to: usize) -> u64 {
    FORWARDED | (to as u64)
}

pub const fn forwarding_target(meta: u64) -> *mut u8 {
    (meta & !FORWARDED) as *mut u8
}

/// Claim the right to move `ptr` to `to`.
///
/// Exactly one caller wins; the loser is handed the address the winner
/// published and must use that instead of copying again.
///
/// # Safety
/// See [`meta_cell`]. `to` must be a live, correctly sized copy.
pub unsafe fn try_forward(ptr: *const u8, to: *mut u8) -> Result<(), *mut u8> {
    let cell = unsafe { meta_cell(ptr) };
    let mut meta = cell.load(Ordering::Acquire);
    loop {
        if is_forwarded_meta(meta) {
            return Err(forwarding_target(meta));
        }
        match cell.compare_exchange_weak(
            meta,
            forwarding_meta(to as usize),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(seen) => meta = seen,
        }
    }
}

/// Follow `ptr` to wherever it has been moved, or return it unchanged.
///
/// # Safety
/// See [`meta_cell`].
#[inline]
pub unsafe fn follow_forwarding(ptr: *mut u8) -> *mut u8 {
    let meta = unsafe { load_meta(ptr) };
    if is_forwarded_meta(meta) {
        forwarding_target(meta)
    } else {
        ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_word_roundtrips() {
        let m = meta_word(TYPE_ID_STR, FLAG_IMMORTAL);
        assert_eq!(type_id_of_meta(m), TYPE_ID_STR);
        assert_eq!(flags_of_meta(m), FLAG_IMMORTAL);
        assert_eq!(rc_of_meta(m), 0);
        assert!(!is_forwarded_meta(m));
    }

    #[test]
    fn flags_and_type_id_do_not_overlap() {
        let m = meta_word(u32::MAX, FLAG_IMMORTAL | FLAG_MARKED | FLAG_LOGGED);
        assert_eq!(type_id_of_meta(m), u32::MAX);
        assert_eq!(flags_of_meta(m), 0b111);
        assert_eq!(rc_of_meta(m), 0);
        assert!(!is_forwarded_meta(m));
    }

    #[test]
    fn align_up_rounds_to_sixteen() {
        assert_eq!(align_up(0), 0);
        assert_eq!(align_up(1), 16);
        assert_eq!(align_up(16), 16);
        assert_eq!(align_up(17), 32);
    }

    /// A 16-byte-aligned scratch object to exercise the atomic accessors. The
    /// payload exists to be pointed at, not read.
    #[repr(align(16))]
    #[allow(dead_code)]
    struct Obj([u64; 2]);

    fn obj(type_id: TypeId, flags: u64) -> Box<Obj> {
        Box::new(Obj([meta_word(type_id, flags), 0]))
    }

    fn ptr_of(o: &Obj) -> *const u8 {
        o as *const Obj as *const u8
    }

    #[test]
    fn reference_counts_saturate_at_both_ends() {
        let o = obj(TYPE_ID_FIRST_USER, 0);
        let p = ptr_of(&o);
        unsafe {
            assert_eq!(rc_of(p), 0);
            // Decrementing zero stays at zero rather than wrapping to RC_MAX,
            // which would resurrect a dead object.
            assert_eq!(rc_dec(p), 0);
            assert_eq!(rc_inc(p), 1);
            assert_eq!(rc_inc(p), 2);
            assert_eq!(rc_dec(p), 1);

            // Once stuck at the maximum the count never comes down: it is no
            // longer accurate, so the backup trace owns this object.
            for _ in 0..4 {
                rc_update(p, |_| RC_MAX);
            }
            assert_eq!(rc_of(p), RC_MAX);
            assert_eq!(rc_inc(p), RC_MAX);
            assert_eq!(rc_dec(p), RC_MAX);

            // Saturating the count must not disturb the type id or the flags.
            assert_eq!(type_id_of(p), TYPE_ID_FIRST_USER);
            assert!(!test_flag(p, FLAG_IMMORTAL));
        }
    }

    #[test]
    fn setting_a_flag_reports_who_set_it() {
        let o = obj(TYPE_ID_FIRST_USER, 0);
        let p = ptr_of(&o);
        unsafe {
            assert!(!test_flag(p, FLAG_LOGGED));
            assert!(set_flag(p, FLAG_LOGGED), "the first caller sets it");
            assert!(!set_flag(p, FLAG_LOGGED), "a later caller does not");
            assert!(test_flag(p, FLAG_LOGGED));
            clear_flag(p, FLAG_LOGGED);
            assert!(!test_flag(p, FLAG_LOGGED));
            // Flags are independent of the count and the type id.
            rc_inc(p);
            assert_eq!(type_id_of(p), TYPE_ID_FIRST_USER);
            assert_eq!(rc_of(p), 1);
        }
    }

    #[test]
    fn forwarding_round_trips_and_only_one_caller_wins() {
        let from = obj(TYPE_ID_FIRST_USER, 0);
        let mut to = obj(TYPE_ID_FIRST_USER, 0);
        let target = &mut *to as *mut Obj as *mut u8;
        let p = ptr_of(&from);
        unsafe {
            assert_eq!(follow_forwarding(p as *mut u8), p as *mut u8);
            assert!(try_forward(p, target).is_ok());
            assert!(is_forwarded_meta(load_meta(p)));
            assert_eq!(follow_forwarding(p as *mut u8), target);
            // A second attempt loses and is told where the object went.
            assert_eq!(try_forward(p, p as *mut u8), Err(target));
        }
    }

    #[test]
    fn a_forwarded_word_is_never_mistaken_for_a_live_one() {
        // Every type id, including the ones whose bit patterns look like small
        // addresses, must stay distinguishable from a forwarding word.
        for id in [
            TYPE_ID_INVALID,
            TYPE_ID_STR,
            TYPE_ID_CLOSURE,
            TYPE_ID_FIRST_USER,
            u32::MAX,
        ] {
            let live = meta_word(id, FLAG_IMMORTAL | FLAG_MARKED | FLAG_LOGGED);
            assert!(!is_forwarded_meta(live), "type id {id} looked forwarded");
        }
        // And a maxed-out reference count must not set the forwarding bit.
        let maxed = ((RC_MAX as u64) << RC_SHIFT) | meta_word(TYPE_ID_FIRST_USER, 0);
        assert!(!is_forwarded_meta(maxed));
        assert_eq!(rc_of_meta(maxed), RC_MAX);
        assert_eq!(type_id_of_meta(maxed), TYPE_ID_FIRST_USER);
    }

    #[test]
    fn the_reference_count_does_not_reach_the_forwarding_bit() {
        // The four fields must tile the word without overlapping.
        assert_eq!(RC_SHIFT + RC_BITS, 63);
        assert_eq!(RC_MASK & FORWARDED, 0);
        assert_eq!(RC_MASK & FLAG_MASK, 0);
        assert_eq!(RC_MASK & TYPE_ID_MASK, 0);
        assert_eq!(FLAG_MASK & TYPE_ID_MASK, 0);
        assert_eq!(TYPE_ID_MASK | FLAG_MASK | RC_MASK | FORWARDED, u64::MAX);
    }
}
