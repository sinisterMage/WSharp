//! The three tables an ahead-of-time compiled program carries, and the reader
//! that installs them before it runs.
//!
//! Under the JIT the code generator has the runtime in the same process, so it
//! hands over the type registry, the stack maps and the service table by
//! calling [`crate::register_type`], [`crate::stackwalk::register_code`] and
//! [`crate::rpc::register_services`] directly. A compiled program has no
//! compiler in it. The same three tables are therefore written into its data
//! section, and this reads them back and makes exactly those three calls -- so
//! nothing downstream knows which way it was told. `function_at`,
//! `safepoint_at`, `types::info` and the whole collector are untouched.
//!
//! # The format
//!
//! One stream per table, read front to back, so neither side does offset
//! arithmetic and there is no padding to disagree about. Every integer is
//! little-endian, which is not a choice so much as a statement: the stack
//! walker reads frame pointers with inline assembly written for x86-64 and
//! aarch64, and both are little-endian.
//!
//! Each stream opens with [`MAGIC`] and [`VERSION`]. That pair is the whole
//! defence against a program built by one version of the compiler being linked
//! against another version's runtime, which would otherwise be a heap walked
//! with the wrong layouts rather than an error.
//!
//! Alignment is maintained as the writer goes: a `u32` starts on a multiple of
//! four and a `u64` on a multiple of eight. Only the code addresses need it --
//! they are relocations, and a linker writes a whole word -- but keeping the
//! rule everywhere is cheaper than remembering where it matters.
//!
//! ```text
//! types:       u32 magic, u32 version, u32 count
//!              count x { u32 type_id, u32 size, u32 elem_stride,
//!                        u32 n_ptrs,      n_ptrs x u32,
//!                        u32 n_elem_ptrs, n_elem_ptrs x u32,
//!                        u32 name_len,    name_len bytes, pad to 4 }
//!
//! stack maps:  u32 magic, u32 version, u64 count            (pad to 8 first)
//!              count x { u64 base <- relocation, u32 len, u32 n_safepoints,
//!                        n_safepoints x { u32 return_offset, u32 frame_size,
//!                                         u32 n_roots, n_roots x u32 },
//!                        pad to 8 }
//!
//! services:    u32 magic, u32 version, u64 count            (pad to 8 first)
//!              count x { u64 init <- relocation,
//!                        u32 n_init_args, n_init_args bytes, pad to 4,
//!                        u32 name_len,    name_len bytes,    pad to 4,
//!                        u32 n_methods,   pad to 8,
//!                        n_methods x { u64 call <- relocation,
//!                                      u32 n_args, n_args bytes, pad to 4,
//!                                      u32 n_ret,  n_ret bytes,  pad to 4,
//!                                      u32 name_len, name_len bytes, pad 4,
//!                                      pad to 8 } }
//! ```
//!
//! The writer is `wsharp_codegen::tables`, and a round-trip test there builds
//! each table, writes it, reads it back with this module and compares. That is
//! what keeps the two halves of the definition in step; the comment above is
//! for people.

use crate::rpc::{MethodCode, ServiceCode, SlotKind};
use crate::stackwalk::{FunctionCode, SafePoint};
use crate::types::TypeLayout;

/// `"WS#T"`, so a blob that is not one of these says so rather than being read.
pub const MAGIC: u32 = u32::from_le_bytes(*b"WS#T");

/// Bumped whenever the layout above changes. A compiled program and the
/// runtime it links against must agree, and there is no way to negotiate.
pub const VERSION: u32 = 1;

/// Read the three tables a compiled program carries and install them.
///
/// Called once, from the startup shim, before any generated code runs -- the
/// same point in the same order the JIT does it in.
///
/// # Safety
/// Each pointer must be to a blob this compiler's `tables` module wrote. They
/// are trusted exactly as the code beside them is: both came out of the same
/// compilation, and neither is checked beyond the magic and the version.
pub unsafe fn register_static_tables(types: *const u8, stack_maps: *const u8, services: *const u8) {
    for (id, layout) in unsafe { parse_types(types) } {
        crate::register_type(id, layout);
    }
    // Freeze the registry, exactly as the JIT path does once it has registered
    // everything: from here the collector reads layouts with no lock.
    crate::publish();
    // Sorting is `register_code`'s job and has to be, because the order in the
    // blob is the compiler's and the addresses in it are the linker's.
    crate::stackwalk::register_code(unsafe { parse_stack_maps(stack_maps) });
    crate::rpc::register_services(unsafe { parse_services(services) });
}

/// A cursor over one table, reading front to back.
struct Cursor {
    at: *const u8,
    /// Where the blob began, so alignment is measured from the right place.
    base: *const u8,
}

impl Cursor {
    /// # Safety
    /// `p` must be a blob written by the compiler, opening with the magic and
    /// a version this runtime knows.
    unsafe fn open(p: *const u8, what: &str) -> Cursor {
        let mut c = Cursor { at: p, base: p };
        let magic = unsafe { c.u32() };
        let version = unsafe { c.u32() };
        assert_eq!(
            magic, MAGIC,
            "the {what} table is not one: this program was linked against a \
             runtime that does not match the compiler that built it"
        );
        assert_eq!(
            version, VERSION,
            "the {what} table is version {version} and this runtime reads \
             version {VERSION}: rebuild the program"
        );
        c
    }

    unsafe fn u32(&mut self) -> u32 {
        // Unaligned reads rather than a promise of alignment: the writer keeps
        // the stream aligned, and a `read_unaligned` costs nothing on either
        // target when it happens to be right.
        let v = unsafe { (self.at as *const u32).read_unaligned() };
        self.at = unsafe { self.at.add(4) };
        u32::from_le(v)
    }

    unsafe fn u64(&mut self) -> u64 {
        let v = unsafe { (self.at as *const u64).read_unaligned() };
        self.at = unsafe { self.at.add(8) };
        u64::from_le(v)
    }

    unsafe fn u32s(&mut self) -> Vec<u32> {
        let n = unsafe { self.u32() } as usize;
        (0..n).map(|_| unsafe { self.u32() }).collect()
    }

    unsafe fn bytes(&mut self) -> Vec<u8> {
        let n = unsafe { self.u32() } as usize;
        let out = unsafe { std::slice::from_raw_parts(self.at, n) }.to_vec();
        self.at = unsafe { self.at.add(n) };
        self.pad(4);
        out
    }

    unsafe fn text(&mut self) -> String {
        let bytes = unsafe { self.bytes() };
        // The compiler wrote these from Rust `String`s, so they are UTF-8.
        String::from_utf8(bytes).expect("a name in a table is UTF-8")
    }

    unsafe fn slot_kinds(&mut self) -> Vec<SlotKind> {
        unsafe { self.bytes() }
            .into_iter()
            .map(|b| {
                if b == 0 {
                    SlotKind::Scalar
                } else {
                    SlotKind::Ref
                }
            })
            .collect()
    }

    /// Skip forward to the next multiple of `to` from the blob's start.
    fn pad(&mut self, to: usize) {
        let off = self.at as usize - self.base as usize;
        let over = off % to;
        if over != 0 {
            self.at = unsafe { self.at.add(to - over) };
        }
    }
}

/// Read the type table, without registering anything.
///
/// Separate from the registering so that the compiler's own tests can drive
/// *this* reader over bytes its writer just produced, rather than a second
/// reader written to match. A format with two implementations has two
/// definitions, and they drift.
///
/// # Safety
/// As [`register_static_tables`].
pub unsafe fn parse_types(p: *const u8) -> Vec<(u32, TypeLayout)> {
    let mut c = unsafe { Cursor::open(p, "type") };
    let count = unsafe { c.u32() };
    (0..count)
        .map(|_| {
            let type_id = unsafe { c.u32() };
            let size = unsafe { c.u32() };
            let elem_stride = unsafe { c.u32() };
            let ptr_offsets = unsafe { c.u32s() };
            let elem_ptr_offsets = unsafe { c.u32s() };
            let name = unsafe { c.text() };
            (
                type_id,
                TypeLayout {
                    name,
                    size,
                    ptr_offsets,
                    elem_stride,
                    elem_ptr_offsets,
                },
            )
        })
        .collect()
}

/// Read the stack maps, without registering them. See [`parse_types`].
///
/// # Safety
/// As [`register_static_tables`].
pub unsafe fn parse_stack_maps(p: *const u8) -> Vec<FunctionCode> {
    let mut c = unsafe { Cursor::open(p, "stack map") };
    c.pad(8);
    let count = unsafe { c.u64() };
    let mut funcs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        // Written by the linker: the address the function finally landed at.
        let base = unsafe { c.u64() } as usize;
        let len = unsafe { c.u32() } as usize;
        let n = unsafe { c.u32() };
        let safepoints: Vec<SafePoint> = (0..n)
            .map(|_| {
                let return_offset = unsafe { c.u32() };
                let frame_size = unsafe { c.u32() };
                let roots = unsafe { c.u32s() };
                SafePoint {
                    return_offset,
                    frame_size,
                    roots: roots.into_boxed_slice(),
                }
            })
            .collect();
        c.pad(8);
        funcs.push(FunctionCode {
            base,
            len,
            safepoints: safepoints.into_boxed_slice(),
        });
    }
    funcs
}

/// Read the service table, without registering it. See [`parse_types`].
///
/// # Safety
/// As [`register_static_tables`].
pub unsafe fn parse_services(p: *const u8) -> Vec<&'static ServiceCode> {
    let mut c = unsafe { Cursor::open(p, "service") };
    c.pad(8);
    let count = unsafe { c.u64() };
    let mut out: Vec<&'static ServiceCode> = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let init = unsafe { c.u64() } as *const u8;
        let init_args = unsafe { c.slot_kinds() };
        let name = unsafe { c.text() };
        let n_methods = unsafe { c.u32() };
        c.pad(8);
        let methods: Vec<MethodCode> = (0..n_methods)
            .map(|_| {
                let call = unsafe { c.u64() } as *const u8;
                let args = unsafe { c.slot_kinds() };
                let ret = unsafe { c.slot_kinds() };
                let name = unsafe { c.text() };
                c.pad(8);
                MethodCode {
                    name: Box::leak(name.into_boxed_str()),
                    call,
                    args: Box::leak(args.into_boxed_slice()),
                    ret: Box::leak(ret.into_boxed_slice()),
                }
            })
            .collect();
        // Leaked for the reason the JIT path leaks them: the runtime holds
        // these for as long as any worker might call into a service, which is
        // until the process exits.
        out.push(Box::leak(Box::new(ServiceCode {
            name: Box::leak(name.into_boxed_str()),
            init,
            init_args: Box::leak(init_args.into_boxed_slice()),
            methods: Box::leak(methods.into_boxed_slice()),
        })));
    }
    out
}
