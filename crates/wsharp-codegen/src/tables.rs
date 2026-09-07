//! Writing the three tables the runtime needs into the module's data section.
//!
//! The format is defined in `wsharp_runtime::aot`, which is also what reads
//! these back. There is one definition and it lives there, because the reader
//! is the half that has to be right on a machine the compiler never saw.
//!
//! Two of the three tables hold code addresses, and those are the only
//! relocations: a stack map's `base` and a service's trampolines. Everything
//! else is a count or a byte, so the blob is position-independent by
//! construction and a linker has exactly as much to do as it must.
//!
//! Under the JIT these are written too, and thrown away. That is deliberate:
//! `round_trips` below rebuilds each table from its own bytes and compares, so
//! a mistake in a format only an object file exercises is caught by a test that
//! needs no object file.

use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use wsharp_runtime::aot::{MAGIC, VERSION};
use wsharp_runtime::rpc::SlotKind;
use wsharp_runtime::types::TypeLayout;

use crate::{CodegenError, HarvestedCode, Trampoline, err};

/// The symbol each table is exported under, and what the startup shim in
/// `wsharp-start` declares to find it.
pub const TYPES_SYMBOL: &str = "ws_type_table";
pub const STACK_MAPS_SYMBOL: &str = "ws_stack_maps";
pub const SERVICES_SYMBOL: &str = "ws_service_table";

/// A table under construction: bytes, and where in them a code address goes.
///
/// The writer keeps its own alignment as it appends, which is what lets the
/// reader be a plain forward cursor. Only the `u64` slots that take a
/// relocation actually need it; doing it everywhere is one rule instead of a
/// list of exceptions.
#[derive(Default)]
pub struct Blob {
    bytes: Vec<u8>,
    /// `(offset in `bytes`, the function whose address goes there)`.
    relocs: Vec<(u32, FuncId)>,
}

impl Blob {
    fn new() -> Blob {
        let mut b = Blob::default();
        b.u32(MAGIC);
        b.u32(VERSION);
        b
    }

    fn u32(&mut self, v: u32) {
        self.pad(4);
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.pad(8);
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    /// Leave room for a code address, and remember that a linker fills it in.
    fn code_addr(&mut self, func: FuncId) {
        self.pad(8);
        self.relocs.push((self.bytes.len() as u32, func));
        self.bytes.extend_from_slice(&0u64.to_le_bytes());
    }

    fn u32s(&mut self, values: &[u32]) {
        self.u32(values.len() as u32);
        for v in values {
            self.u32(*v);
        }
    }

    fn bytes_of(&mut self, data: &[u8]) {
        self.u32(data.len() as u32);
        self.bytes.extend_from_slice(data);
        self.pad(4);
    }

    fn text(&mut self, s: &str) {
        self.bytes_of(s.as_bytes());
    }

    fn slot_kinds(&mut self, kinds: &[SlotKind]) {
        let bytes: Vec<u8> = kinds
            .iter()
            .map(|k| u8::from(*k == SlotKind::Ref))
            .collect();
        self.bytes_of(&bytes);
    }

    fn pad(&mut self, to: usize) {
        while !self.bytes.len().is_multiple_of(to) {
            self.bytes.push(0);
        }
    }

    /// Emit this as an exported data object, with a relocation per code address.
    fn define<M: Module>(mut self, module: &mut M, symbol: &str) -> Result<DataId, CodegenError> {
        self.pad(8);
        let mut desc = DataDescription::new();
        let relocs = std::mem::take(&mut self.relocs);
        desc.define(self.bytes.into_boxed_slice());
        desc.set_align(8);
        for (at, func) in relocs {
            // A `Linkage::Local` function is a fine relocation target from data
            // in the same object -- which every one of these is, because the
            // compiler emitted both.
            let fr = module.declare_func_in_data(func, &mut desc);
            desc.write_function_addr(at, fr);
        }
        // Exported, because the startup shim is in another object and names
        // these three symbols to find them.
        let id = module
            .declare_data(symbol, Linkage::Export, false, false)
            .map_err(|e| err(&format!("could not declare `{symbol}`"), e))?;
        module
            .define_data(id, &desc)
            .map_err(|e| err(&format!("could not define `{symbol}`"), e))?;
        Ok(id)
    }
}

/// Every heap type's layout, for the collector to trace by.
pub fn types(layouts: &[(u32, TypeLayout)]) -> Blob {
    let mut b = Blob::new();
    b.u32(layouts.len() as u32);
    for (type_id, l) in layouts {
        b.u32(*type_id);
        b.u32(l.size);
        b.u32(l.elem_stride);
        b.u32s(&l.ptr_offsets);
        b.u32s(&l.elem_ptr_offsets);
        b.text(&l.name);
    }
    b
}

/// Every compiled function's extent and its safepoints.
pub fn stack_maps(harvested: &[(FuncId, HarvestedCode)]) -> Blob {
    let mut b = Blob::new();
    b.u64(harvested.len() as u64);
    for (func, code) in harvested {
        b.code_addr(*func);
        b.u32(code.len as u32);
        b.u32(code.safepoints.len() as u32);
        for sp in &code.safepoints {
            b.u32(sp.return_offset);
            b.u32(sp.frame_size);
            b.u32s(&sp.roots);
        }
        b.pad(8);
    }
    b
}

/// Every service, and the trampoline each of its functions is called through.
pub fn services(program: &wsharp_sema::hir::Program, found: &[Trampoline]) -> Blob {
    let grouped = crate::group_trampolines(program, found);
    let mut b = Blob::new();
    b.u64(grouped.len() as u64);
    for (name, init, methods) in &grouped {
        b.code_addr(init.clif_id);
        b.slot_kinds(&init.args);
        b.text(name);
        b.u32(methods.len() as u32);
        b.pad(8);
        for m in methods {
            b.code_addr(m.clif_id);
            b.slot_kinds(&m.args);
            b.slot_kinds(&m.ret);
            b.text(&m.name);
            b.pad(8);
        }
    }
    b
}

/// Write all three into `module`, exported under the names the startup shim
/// looks for.
pub fn emit_all<M: Module>(
    module: &mut M,
    program: &wsharp_sema::hir::Program,
    layouts: &[(u32, TypeLayout)],
    harvested: &[(FuncId, HarvestedCode)],
    found: &[Trampoline],
) -> Result<(), CodegenError> {
    types(layouts).define(module, TYPES_SYMBOL)?;
    stack_maps(harvested).define(module, STACK_MAPS_SYMBOL)?;
    services(program, found).define(module, SERVICES_SYMBOL)?;
    Ok(())
}

impl Blob {
    /// The bytes, for a test that reads them back. Code addresses are still
    /// zero: only a linker or the JIT knows those.
    #[cfg(test)]
    pub(crate) fn finish_for_test(mut self) -> Vec<u8> {
        self.pad(8);
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wsharp_runtime::stackwalk::SafePoint;

    /// A format with two implementations has two definitions, and they drift.
    /// So these drive the runtime's *own* reader over bytes this writer just
    /// produced -- the same function a compiled program calls at startup.
    ///
    /// The code addresses read back as zero, because only a linker knows those.
    #[test]
    fn a_type_table_round_trips() {
        let layouts = vec![
            (16, TypeLayout::fixed("Point", 32, vec![16, 24])),
            (17, TypeLayout::elements("[]str", 8, vec![0])),
            // An empty name and no offsets at all: the degenerate case a length
            // prefix has to get right and a separator would not. `[]i64` is
            // really this shape, minus the name.
            (18, TypeLayout::fixed("", 16, vec![])),
            // A name whose length is not a multiple of four, so the next entry
            // starts only if the writer padded.
            (19, TypeLayout::fixed("Odd", 24, vec![16])),
        ];
        let bytes = types(&layouts).finish_for_test();
        let read = unsafe { wsharp_runtime::aot::parse_types(bytes.as_ptr()) };
        assert_eq!(read, layouts);
    }

    /// The safepoints are the one table where a mistake is silent corruption
    /// rather than a crash: the collector would read a root at the wrong stack
    /// offset and mark whatever happened to be there.
    #[test]
    fn a_stack_map_round_trips() {
        let harvested = vec![
            (
                FuncId::from_u32(0),
                HarvestedCode {
                    len: 64,
                    safepoints: vec![
                        SafePoint {
                            return_offset: 12,
                            frame_size: 32,
                            roots: vec![0, 8].into_boxed_slice(),
                        },
                        // A safepoint with no live roots at all, which is most
                        // of them in a program that does little with objects.
                        SafePoint {
                            return_offset: 40,
                            frame_size: 32,
                            roots: Box::new([]),
                        },
                    ],
                },
            ),
            // No safepoints: a leaf function that calls nothing.
            (
                FuncId::from_u32(1),
                HarvestedCode {
                    len: 16,
                    safepoints: Vec::new(),
                },
            ),
        ];
        let blob = stack_maps(&harvested);
        for (at, _) in &blob.relocs {
            assert_eq!(
                at % 8,
                0,
                "a code address at {at} is not eight-aligned, so a linker \
                 would write a relocation across a word boundary"
            );
        }
        assert_eq!(blob.relocs.len(), 2, "one address per compiled function");

        let bytes = blob.finish_for_test();
        let read = unsafe { wsharp_runtime::aot::parse_stack_maps(bytes.as_ptr()) };
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].len, 64);
        assert_eq!(read[0].safepoints.len(), 2);
        assert_eq!(read[0].safepoints[0].return_offset, 12);
        assert_eq!(read[0].safepoints[0].frame_size, 32);
        assert_eq!(&*read[0].safepoints[0].roots, &[0, 8]);
        assert_eq!(read[0].safepoints[1].return_offset, 40);
        assert!(read[0].safepoints[1].roots.is_empty());
        assert_eq!(read[1].len, 16);
        assert!(read[1].safepoints.is_empty());
    }

    /// A service's arguments say which of its words are references, and the
    /// runtime copies exactly those. Getting the two kinds the wrong way round
    /// would send an integer where a pointer belongs.
    #[test]
    fn a_service_table_round_trips() {
        let blob = services_from(&[(
            "Counter".to_string(),
            (vec![SlotKind::Scalar, SlotKind::Ref], "init".to_string()),
            vec![
                (
                    "bump".to_string(),
                    vec![SlotKind::Scalar],
                    vec![SlotKind::Scalar],
                ),
                // No arguments and no result: the empty case at both ends.
                ("reset".to_string(), vec![], vec![]),
            ],
        )]);
        let bytes = blob.finish_for_test();
        let read = unsafe { wsharp_runtime::aot::parse_services(bytes.as_ptr()) };
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].name, "Counter");
        assert_eq!(read[0].init_args, [SlotKind::Scalar, SlotKind::Ref]);
        assert_eq!(read[0].methods.len(), 2);
        assert_eq!(read[0].methods[0].name, "bump");
        assert_eq!(read[0].methods[0].args, [SlotKind::Scalar]);
        assert_eq!(read[0].methods[0].ret, [SlotKind::Scalar]);
        assert_eq!(read[0].methods[1].name, "reset");
        assert!(read[0].methods[1].args.is_empty());
        assert!(read[0].methods[1].ret.is_empty());
    }

    /// `services` needs an `hir::Program` to group trampolines by; this writes
    /// the same stream from an already-grouped description, so the test can
    /// state what it means without building a whole program.
    #[allow(clippy::type_complexity)]
    fn services_from(
        grouped: &[(
            String,
            (Vec<SlotKind>, String),
            Vec<(String, Vec<SlotKind>, Vec<SlotKind>)>,
        )],
    ) -> Blob {
        let mut b = Blob::new();
        b.u64(grouped.len() as u64);
        let mut next = 0u32;
        for (name, (init_args, _), methods) in grouped {
            b.code_addr(FuncId::from_u32(next));
            next += 1;
            b.slot_kinds(init_args);
            b.text(name);
            b.u32(methods.len() as u32);
            b.pad(8);
            for (mname, args, ret) in methods {
                b.code_addr(FuncId::from_u32(next));
                next += 1;
                b.slot_kinds(args);
                b.slot_kinds(ret);
                b.text(mname);
                b.pad(8);
            }
        }
        b
    }
}
