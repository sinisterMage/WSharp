//! How a W# value maps onto machine values.
//!
//! A W# value is not one Cranelift value but a small list of them. Scalars and
//! pointers use one; an optional or an error union is a tag followed by its
//! payload. Keeping this list-shaped from the start is what lets optionals and
//! error unions exist without special-casing every expression.
//!
//! Slot *counts* come from [`wsharp_sema::layout`], which inference also uses to
//! place struct fields -- the two must agree, and a test below checks they do.

use cranelift_codegen::ir::{self, types};
use smallvec::{SmallVec, smallvec};
use wsharp_sema::layout;
use wsharp_sema::ty::{TyCon, Type, TypeStore};

/// The machine values making up one W# value.
pub type Slots = SmallVec<[ir::Value; 2]>;
/// Their types.
pub type SlotTypes = SmallVec<[ir::Type; 2]>;

/// Pointer width. W# targets 64-bit only.
pub const PTR: ir::Type = types::I64;

/// `?T` is this tag followed by `T`: 0 is null, 1 is present.
pub const OPTION_TAG: ir::Type = types::I8;
pub const OPTION_NULL: i64 = 0;
pub const OPTION_SOME: i64 = 1;

/// `!T` is this tag followed by `T`. 0 means success; any other value is the
/// error's index plus one. Encoding the error into the tag rather than the
/// payload keeps `!f64` from needing a slot that is both a float and an int.
pub const ERROR_TAG: ir::Type = types::I32;
pub const ERROR_OK: i64 = 0;

/// The tag value representing error number `id`.
pub fn error_tag_value(id: u32) -> i64 {
    id as i64 + 1
}

/// The Cranelift types of a value's slots.
pub fn slot_types(store: &mut TypeStore, ty: &Type) -> SlotTypes {
    match store.resolve(ty) {
        Type::Con(TyCon::Void, _) => smallvec![],
        Type::Con(TyCon::I64, _) => smallvec![types::I64],
        Type::Con(TyCon::F64, _) => smallvec![types::F64],
        Type::Con(TyCon::Bool, _) => smallvec![types::I8],
        Type::Con(TyCon::Error, _) => smallvec![ERROR_TAG],
        Type::Con(TyCon::Str | TyCon::Struct(_) | TyCon::Fn, _) => smallvec![PTR],
        Type::Con(TyCon::Optional, args) => {
            let mut out: SlotTypes = smallvec![OPTION_TAG];
            out.extend(slot_types(store, &args[0]));
            out
        }
        Type::Con(TyCon::ErrUnion, args) => {
            let mut out: SlotTypes = smallvec![ERROR_TAG];
            out.extend(slot_types(store, &args[0]));
            out
        }
        // Monomorphisation replaces every variable and reports the ones it
        // cannot; reaching here means that pass let something through.
        Type::Var(v) => unreachable!("type variable ?{v} survived monomorphisation"),
    }
}

/// The slot indices of a value that hold heap pointers -- what the collector
/// will need to trace, and what the `--gc-stack-maps` option marks.
pub fn pointer_slots(store: &mut TypeStore, ty: &Type) -> SmallVec<[usize; 2]> {
    let mut offsets = Vec::new();
    layout::ptr_offsets(store, ty, 0, &mut offsets);
    offsets
        .iter()
        .map(|o| (*o / layout::SLOT_SIZE) as usize)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_types_agree_with_the_layout_used_for_struct_fields() {
        let mut s = TypeStore::new();
        let point = s.declare_struct("Point");
        let cases = [
            Type::i64(),
            Type::f64(),
            Type::bool(),
            Type::void(),
            Type::str(),
            Type::strukt(point),
            Type::func(vec![Type::i64()], Type::i64()),
            Type::optional(Type::i64()),
            Type::optional(Type::str()),
            Type::err_union(Type::f64()),
            Type::optional(Type::optional(Type::i64())),
            Type::optional(Type::void()),
        ];
        for ty in cases {
            let expected = layout::slot_count(&mut s, &ty) as usize;
            let actual = slot_types(&mut s, &ty).len();
            let shown = s.show(&ty);
            assert_eq!(actual, expected, "slot count disagrees for `{shown}`");
        }
    }

    #[test]
    fn tagged_values_lead_with_their_tag() {
        let mut s = TypeStore::new();
        assert_eq!(
            &slot_types(&mut s, &Type::optional(Type::i64()))[..],
            &[OPTION_TAG, types::I64]
        );
        assert_eq!(
            &slot_types(&mut s, &Type::err_union(Type::f64()))[..],
            &[ERROR_TAG, types::F64]
        );
    }

    #[test]
    fn pointer_slots_finds_the_payload_of_an_optional() {
        let mut s = TypeStore::new();
        assert_eq!(&pointer_slots(&mut s, &Type::str())[..], &[0]);
        assert_eq!(
            &pointer_slots(&mut s, &Type::optional(Type::str()))[..],
            &[1]
        );
        assert!(pointer_slots(&mut s, &Type::i64()).is_empty());
        assert!(pointer_slots(&mut s, &Type::optional(Type::i64())).is_empty());
    }
}
