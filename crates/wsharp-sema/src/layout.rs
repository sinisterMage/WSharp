//! How values are laid out.
//!
//! Inference uses this to place struct fields; code generation uses it to
//! decide how many machine values a W# value occupies and where the collector
//! will find pointers. Both must agree exactly, so both call these functions.

use crate::ty::{TyCon, Type, TypeStore};

/// Every value *slot* is one machine word.
///
/// A slot is a machine value, not a byte count: slot `i` of a value lives at
/// `base + i * SLOT_SIZE`, and three things depend on exactly that -- the
/// stride `load_at` and `store_slots` walk with, and the division
/// `repr::pointer_slots` uses to turn a byte offset back into a slot index.
/// Scalars pack (see [`size_of`]); a tagged value keeps a whole word per slot
/// so that the invariant survives.
pub const SLOT_SIZE: u32 = 8;

/// How many slots a value of this type occupies.
///
/// Optionals and error unions are a tag followed by their payload, which is why
/// this is recursive rather than a constant per constructor: `??i64` needs
/// three slots, not two.
pub fn slot_count(store: &mut TypeStore, ty: &Type) -> u32 {
    match store.resolve(ty) {
        // `void` is not stored at all.
        Type::Con(TyCon::Void, _) => 0,
        Type::Con(TyCon::Optional | TyCon::ErrUnion, args) => 1 + slot_count(store, &args[0]),
        _ => 1,
    }
}

/// Bytes a value of this type occupies in memory.
///
/// A scalar takes its natural width, so `[]u8` is a byte array rather than one
/// eight times too large and a struct of bytes costs bytes. A *tagged* value
/// keeps one whole word per slot instead, and deliberately: the tight packing
/// would give `?u8` a size of nine with an alignment of eight, and then
/// `[]?u8`'s stride would not be a multiple of its alignment. It would also
/// break the rule [`SLOT_SIZE`] states.
pub fn size_of(store: &mut TypeStore, ty: &Type) -> u32 {
    match store.resolve(ty) {
        Type::Con(TyCon::Void, _) => 0,
        Type::Con(TyCon::Int(t), _) => t.size(),
        Type::Con(TyCon::Bool, _) => 1,
        // An error is its tag, which is 32 bits wide (`repr::ERROR_TAG`).
        Type::Con(TyCon::Error, _) => 4,
        // Tagged: a word per slot, per the note above.
        ty @ Type::Con(TyCon::Optional | TyCon::ErrUnion, _) => slot_count(store, &ty) * SLOT_SIZE,
        _ => SLOT_SIZE,
    }
}

/// The alignment a value of this type must be placed at.
///
/// Not cosmetic. Every load and store through [`crate::layout`]'s offsets uses
/// Cranelift's `trusted` memory flags, whose `aligned` bit lets the instruction
/// "trap or return a wrong result if the effective address is misaligned" --
/// so packing without aligning turns that flag into a lie.
pub fn align_of(store: &mut TypeStore, ty: &Type) -> u32 {
    match store.resolve(ty) {
        // One, not zero: `place` rounds by this, and rounding by zero divides.
        Type::Con(TyCon::Void, _) => 1,
        Type::Con(TyCon::Int(t), _) => t.size(),
        Type::Con(TyCon::Bool, _) => 1,
        Type::Con(TyCon::Error, _) => 4,
        // A tagged value leads with a word-wide slot, so it aligns like one.
        _ => SLOT_SIZE,
    }
}

/// Whether a value of this type is a pointer to a heap object, and so something
/// the garbage collector has to trace and the write barrier has to notice.
pub fn is_heap_pointer(store: &mut TypeStore, ty: &Type) -> bool {
    matches!(
        store.resolve(ty),
        Type::Con(TyCon::Str | TyCon::Struct(_) | TyCon::Fn | TyCon::Array, _)
    )
}

/// Place a run of values one after another, returning each one's offset and
/// the offset just past the last.
///
/// This is where a struct's fields are put, for both the non-generic case
/// (inference, which knows them all up front) and an instantiation of a
/// generic one (code generation, which cannot know them until the arguments
/// are concrete). Both call this so the two can never disagree.
pub fn place(store: &mut TypeStore, types: &[Type], start: u32) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(types.len());
    let mut offset = start;
    for ty in types {
        offset = offset.next_multiple_of(align_of(store, ty));
        offsets.push(offset);
        offset += size_of(store, ty);
    }
    (offsets, offset)
}

/// How one element of `[]T` is laid out: its stride in bytes, and the offsets
/// within it that hold heap pointers.
///
/// This is what `TypeLayout.ptr_offsets` cannot say. A fixed list describes a
/// struct, whose fields are known; an array holds `aux` elements of the same
/// shape, so the collector needs the shape once and the count from the object.
/// Elements are laid out exactly as a struct's fields are, which is what lets
/// both go through [`ptr_offsets`].
pub fn elem_layout(store: &mut TypeStore, elem: &Type) -> (u32, Vec<u32>) {
    let stride = size_of(store, elem);
    let mut offsets = Vec::new();
    ptr_offsets(store, elem, 0, &mut offsets);
    (stride, offsets)
}

/// Collect the byte offsets, relative to `base`, of every slot in a value of
/// this type that holds a heap pointer.
///
/// This is what fills in [`wsharp_runtime::TypeLayout::ptr_offsets`], so a
/// future collector can trace an object without knowing anything about W#
/// types.
pub fn ptr_offsets(store: &mut TypeStore, ty: &Type, base: u32, out: &mut Vec<u32>) {
    match store.resolve(ty) {
        Type::Con(TyCon::Void, _) => {}
        // The tag occupies the first slot; only the payload can hold pointers.
        Type::Con(TyCon::Optional | TyCon::ErrUnion, args) => {
            ptr_offsets(store, &args[0], base + SLOT_SIZE, out)
        }
        _ => {
            if is_heap_pointer(store, ty) {
                out.push(base);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::{IntTy, Type};

    #[test]
    fn scalars_take_one_slot_and_void_takes_none() {
        let mut s = TypeStore::new();
        assert_eq!(slot_count(&mut s, &Type::i64()), 1);
        assert_eq!(slot_count(&mut s, &Type::f64()), 1);
        assert_eq!(slot_count(&mut s, &Type::bool()), 1);
        assert_eq!(slot_count(&mut s, &Type::str()), 1);
        assert_eq!(slot_count(&mut s, &Type::void()), 0);
    }

    #[test]
    fn tagged_types_nest() {
        let mut s = TypeStore::new();
        assert_eq!(slot_count(&mut s, &Type::optional(Type::i64())), 2);
        let empty = s.err_set(Vec::new());
        assert_eq!(slot_count(&mut s, &Type::err_union(Type::i64(), empty)), 2);
        // A tag on a tag on a payload.
        assert_eq!(
            slot_count(&mut s, &Type::optional(Type::optional(Type::i64()))),
            3
        );
        // An optional with no payload is just a tag.
        assert_eq!(slot_count(&mut s, &Type::optional(Type::void())), 1);
    }

    /// The rule [`SLOT_SIZE`]'s note states, checked rather than asserted in
    /// prose: slot `i` of a value lives at `base + i * SLOT_SIZE`, so a value
    /// must be big enough to hold its last slot there. `load_at`,
    /// `store_slots` and `repr::pointer_slots` all depend on it.
    #[test]
    fn a_value_is_big_enough_for_its_last_slot() {
        let mut s = TypeStore::new();
        let empty = s.err_set(Vec::new());
        let cases = vec![
            Type::int(IntTy::I8),
            Type::int(IntTy::U8),
            Type::int(IntTy::I32),
            Type::int(IntTy::U64),
            Type::f64(),
            Type::bool(),
            Type::str(),
            Type::optional(Type::int(IntTy::U8)),
            Type::optional(Type::str()),
            Type::optional(Type::optional(Type::i64())),
            Type::err_union(Type::int(IntTy::U8), empty),
        ];
        for ty in cases {
            let slots = slot_count(&mut s, &ty);
            let size = size_of(&mut s, &ty);
            assert!(
                size >= (slots.saturating_sub(1)) * SLOT_SIZE,
                "{ty:?}: {slots} slots do not fit in {size} bytes"
            );
        }
    }

    /// What keeps `HEADER_SIZE + i * stride` aligned for every element: the
    /// stride is a whole number of the element's alignment. An array whose
    /// elements were 9 bytes at an alignment of 8 would misalign every one
    /// after the first, and the loads through it claim to be aligned.
    #[test]
    fn an_element_stride_is_a_multiple_of_its_alignment() {
        let mut s = TypeStore::new();
        let point = s.declare_struct("Point");
        let cases = vec![
            Type::int(IntTy::U8),
            Type::int(IntTy::I16),
            Type::int(IntTy::U32),
            Type::i64(),
            Type::f64(),
            Type::bool(),
            Type::str(),
            Type::strukt(point),
            Type::optional(Type::int(IntTy::U8)),
            Type::optional(Type::str()),
        ];
        for ty in cases {
            let (stride, _) = elem_layout(&mut s, &ty);
            let align = align_of(&mut s, &ty);
            assert_eq!(stride % align, 0, "{ty:?}: stride {stride}, align {align}");
        }
    }

    #[test]
    fn scalars_pack_and_tagged_values_do_not() {
        let mut s = TypeStore::new();
        assert_eq!(size_of(&mut s, &Type::int(IntTy::U8)), 1);
        assert_eq!(size_of(&mut s, &Type::int(IntTy::I16)), 2);
        assert_eq!(size_of(&mut s, &Type::int(IntTy::U32)), 4);
        assert_eq!(size_of(&mut s, &Type::i64()), 8);
        assert_eq!(size_of(&mut s, &Type::bool()), 1);
        assert_eq!(size_of(&mut s, &Type::str()), 8);
        // A tag plus a payload, a word each, whatever the payload is.
        assert_eq!(size_of(&mut s, &Type::optional(Type::int(IntTy::U8))), 16);
    }

    /// Fields are placed at their own alignment, and a run of narrow ones
    /// costs what it says rather than a word each.
    #[test]
    fn place_aligns_each_field() {
        let mut s = TypeStore::new();
        let fields = vec![
            Type::int(IntTy::U8),
            Type::int(IntTy::U8),
            Type::int(IntTy::I32),
            Type::str(),
        ];
        let (offsets, end) = place(&mut s, &fields, 16);
        assert_eq!(offsets, vec![16, 17, 20, 24]);
        assert_eq!(end, 32);
    }

    #[test]
    fn only_heap_types_are_traced() {
        let mut s = TypeStore::new();
        let point = s.declare_struct("Point");
        assert!(is_heap_pointer(&mut s, &Type::str()));
        assert!(is_heap_pointer(&mut s, &Type::strukt(point)));
        assert!(is_heap_pointer(&mut s, &Type::func(vec![], Type::void())));
        assert!(!is_heap_pointer(&mut s, &Type::i64()));
        assert!(!is_heap_pointer(&mut s, &Type::bool()));
    }

    #[test]
    fn pointer_offsets_skip_tags_and_scalars() {
        let mut s = TypeStore::new();
        let mut out = Vec::new();

        ptr_offsets(&mut s, &Type::i64(), 16, &mut out);
        assert!(out.is_empty(), "an integer is not a root");

        ptr_offsets(&mut s, &Type::str(), 16, &mut out);
        assert_eq!(out, vec![16]);

        // `?str` puts the tag at 24 and the pointer at 32.
        out.clear();
        ptr_offsets(&mut s, &Type::optional(Type::str()), 24, &mut out);
        assert_eq!(out, vec![32]);

        // `?i64` holds no pointer at all.
        out.clear();
        ptr_offsets(&mut s, &Type::optional(Type::i64()), 24, &mut out);
        assert!(out.is_empty());
    }
}
