//! How values are laid out.
//!
//! Inference uses this to place struct fields; code generation uses it to
//! decide how many machine values a W# value occupies and where the collector
//! will find pointers. Both must agree exactly, so both call these functions.

use crate::ty::{TyCon, Type, TypeStore};

/// Every value slot is one machine word.
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
pub fn size_of(store: &mut TypeStore, ty: &Type) -> u32 {
    slot_count(store, ty) * SLOT_SIZE
}

/// Whether a value of this type is a pointer to a heap object, and so something
/// the garbage collector has to trace and the write barrier has to notice.
pub fn is_heap_pointer(store: &mut TypeStore, ty: &Type) -> bool {
    matches!(
        store.resolve(ty),
        Type::Con(TyCon::Str | TyCon::Struct(_) | TyCon::Fn, _)
    )
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
    use crate::ty::Type;

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
        assert_eq!(slot_count(&mut s, &Type::err_union(Type::i64())), 2);
        // A tag on a tag on a payload.
        assert_eq!(
            slot_count(&mut s, &Type::optional(Type::optional(Type::i64()))),
            3
        );
        // An optional with no payload is just a tag.
        assert_eq!(slot_count(&mut s, &Type::optional(Type::void())), 1);
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
