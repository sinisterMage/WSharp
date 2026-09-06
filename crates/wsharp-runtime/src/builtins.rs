//! The builtin function registry.
//!
//! One table describes every function the language provides natively. The type
//! checker reads it to seed the global type environment; the code generator
//! reads the same table to register symbols with the JIT. Adding a function to
//! the standard library (a later session) means adding one row here -- the two
//! consumers pick it up without changes.

use crate::header::{AUX_OFFSET, HEADER_SIZE};

/// The types a builtin signature can mention. Deliberately small and
/// independent of the type checker's representation, so that `wsharp-runtime`
/// stays a leaf crate that neither sema nor codegen has to be built before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTy {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F64,
    Bool,
    Void,
    Str,
    /// `[]T`.
    Array(&'static BuiltinTy),
    /// `?T`.
    Optional(&'static BuiltinTy),
    /// `!T`, over the error names the implementation can actually produce.
    ///
    /// Written out rather than left open: a builtin is compiled long before
    /// the program that catches its errors, so what it can raise is a fact
    /// about the row and nothing else can work it out.
    ErrUnion(&'static BuiltinTy, &'static [&'static str]),
    /// A type variable that must be copyable to another worker's heap.
    ///
    /// Numbered as [`BuiltinTy::Var`] is, and the same variable: the two
    /// differ only in that this one makes the type checker record a
    /// `Transferable` constraint on it, which is a question about a type that
    /// the runtime's small enum cannot ask for itself.
    Transferable(u8),
    /// A type variable that must be a transferable *heap object* -- a struct,
    /// a string or an array -- rather than merely transferable.
    ///
    /// What a broker's message must be. An object carries its type id in its
    /// header, and that id is both what lets the copy be made without knowing
    /// the type and what makes the decoded value dispatchable on the other
    /// side. A scalar carries nothing.
    Message(u8),
    /// A type variable that must be one of the integer types.
    ///
    /// Numbered as [`BuiltinTy::Var`] is, and the same variable; the type
    /// checker additionally records a `Member` constraint on the abstract type
    /// `Integer`, exactly as an `Integer`-annotated parameter would. What it
    /// is *for* is a builtin the code generator lowers inline and so can
    /// compile at any width -- `bits.rotl`, which is one instruction on both
    /// targets and cannot be an `extern "C"` function because a Rust one
    /// cannot be generic over the width.
    IntVar(u8),
    /// A type variable, numbered within one signature: every `Var(0)` in a row
    /// is the same type, and each *use* of the builtin gets its own.
    ///
    /// A generic builtin is still one machine implementation, so a variable
    /// may only appear where the implementation does not need to know what it
    /// is -- inside an array, whose stride the object's own type id carries.
    Var(u8),
}

pub struct Builtin {
    pub name: &'static str,
    /// The module this belongs to, or `""` for the prelude -- the handful of
    /// names every module sees without importing anything.
    ///
    /// A field rather than a table per module because the index into
    /// [`builtins`] *is* the identity a `hir::Callee::Builtin` carries, and
    /// code generation registers the same flat list as JIT symbols. Where a
    /// name is visible is a question for the type checker alone.
    pub module: &'static str,
    pub params: &'static [BuiltinTy],
    pub ret: BuiltinTy,
    /// Address of the native implementation, handed to the JIT as a symbol.
    pub ptr: *const u8,
}

/// The prelude: visible in every module without an import.
pub const PRELUDE: &str = "";

impl Builtin {
    /// The name the JIT knows this by.
    ///
    /// Qualified, because two modules may each have a `len` and the symbol
    /// table has no notion of a module. The bare name is what W# source
    /// writes; this is what the linker sees.
    pub fn symbol(&self) -> String {
        if self.module == PRELUDE {
            self.name.to_string()
        } else {
            format!("{}.{}", self.module, self.name)
        }
    }
}

/// `[]T`, where `T` must be copyable to another worker's heap.
const TRANSFERABLE_ARRAY: BuiltinTy = BuiltinTy::Array(&BuiltinTy::Transferable(0));

/// The message type a topic carries.
const MESSAGE: BuiltinTy = BuiltinTy::Message(0);

/// Every builtin visible to W# source.
pub fn builtins() -> Vec<Builtin> {
    let mut table = vec![
        Builtin {
            module: PRELUDE,
            name: "print",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Void,
            ptr: ws_print_str as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "print_int",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: ws_print_int as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "print_float",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::Void,
            ptr: ws_print_float as *const u8,
        },
        // `print_int` takes an `i64`, and a narrower value is written
        // `print_int(i64(x))` -- conversions are written, never inferred. This
        // is for the one value that cannot round-trip through an `i64`: a
        // `u64` with its top bit set.
        Builtin {
            module: PRELUDE,
            name: "print_uint",
            params: &[BuiltinTy::U64],
            ret: BuiltinTy::Void,
            ptr: ws_print_uint as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "print_bool",
            params: &[BuiltinTy::Bool],
            ret: BuiltinTy::Void,
            ptr: ws_print_bool as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "assert",
            params: &[BuiltinTy::Bool],
            ret: BuiltinTy::Void,
            ptr: ws_assert as *const u8,
        },
        // The out-of-bounds panic, exposed for the same reason the `gc_*`
        // counters are: a container written in W# should be able to report a
        // bad index exactly as `a[i]` does. `std/list` bounds-checks against
        // its count rather than its backing array's capacity, so the check is
        // in W# -- and `assert` would only say "assertion failed".
        //
        // [`ws_panic_index`] never returns; declaring it `void` is right
        // because the two have the same ABI, and a caller that writes a
        // `return` after it is simply writing unreachable code.
        Builtin {
            module: PRELUDE,
            name: "panic_index",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: ws_panic_index as *const u8,
        },
        // The collector, exposed so that a W# program can assert on it. Being
        // able to say "allocate this much rubbish, collect, check it went" in
        // the language itself is what makes the collector testable end to end
        // rather than only from Rust.
        Builtin {
            module: PRELUDE,
            name: "gc_collect",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_collect as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "gc_trace",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_trace as *const u8,
        },
        // The two halves of a trace, so a program can mutate the heap while
        // the collector thread is marking it and then check nothing was lost.
        Builtin {
            module: PRELUDE,
            name: "gc_trace_start",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_trace_start as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "gc_trace_finish",
            params: &[],
            ret: BuiltinTy::Void,
            ptr: ws_gc_trace_finish as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "gc_traces",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_traces as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "gc_live_objects",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_live_objects as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "gc_live_bytes",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_live_bytes as *const u8,
        },
        Builtin {
            module: PRELUDE,
            name: "gc_collections",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: ws_gc_collections as *const u8,
        },
    ];
    table.extend(library());
    table
}

/// The standard library's rows, appended to the prelude by [`builtins`].
///
/// A separate function only for readability: they are the same table, and the
/// index into it is what a call carries.
fn library() -> Vec<Builtin> {
    // `Var(0)` is one type variable per *use*, which is what lets `len` work
    // on any array without being compiled per element type: the machine code
    // reads the count out of the header either way.
    const ELEM: &BuiltinTy = &BuiltinTy::Var(0);
    const ARRAY_OF_ELEM: BuiltinTy = BuiltinTy::Array(ELEM);
    /// `[]u8` written out: `std/bytes` is not generic over its element, because
    /// what it means by a byte is a byte.
    const BYTES_OF_U8: BuiltinTy = BuiltinTy::Array(&BuiltinTy::U8);
    vec![
        Builtin {
            module: STR_MODULE,
            name: "len",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::I64,
            ptr: crate::strings::ws_str_len as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "concat",
            params: &[BuiltinTy::Str, BuiltinTy::Str],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_concat as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "eq",
            params: &[BuiltinTy::Str, BuiltinTy::Str],
            ret: BuiltinTy::Bool,
            ptr: crate::strings::ws_str_eq as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "substr",
            params: &[BuiltinTy::Str, BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_substr as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "find",
            params: &[BuiltinTy::Str, BuiltinTy::Str],
            ret: BuiltinTy::I64,
            ptr: crate::strings::ws_str_find as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "from_int",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_from_int as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "from_float",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_from_float as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "byte_at",
            params: &[BuiltinTy::Str, BuiltinTy::I64],
            ret: BuiltinTy::I64,
            ptr: crate::strings::ws_str_byte_at as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "from_byte",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_from_byte as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "parse_int",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, &["BadFormat"]),
            ptr: crate::strings::ws_str_parse_int as *const u8,
        },
        // The other half of `from_int`, for the top half of a `u64`: an `i64`
        // cannot hold it, so converting first would print a negative number.
        Builtin {
            module: STR_MODULE,
            name: "from_uint",
            params: &[BuiltinTy::U64],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_from_uint as *const u8,
        },
        // Rotation, which every hash and stream cipher is written in terms of.
        // A builtin rather than a shift-shift-or the code generator would have
        // to recognise and would sometimes miss -- and lowered inline rather
        // than called, because one machine instruction behind a call is not a
        // rotate anybody wants, and because a Rust `extern "C"` cannot be
        // generic over the width the way `IntVar` is.
        //
        // The pointers are never taken: `Trans::call` intercepts both by name,
        // exactly as it does `array.new`. They name `ws_panic` so that the row
        // is still a well-formed JIT symbol.
        Builtin {
            module: BITS_MODULE,
            name: BITS_ROTL,
            params: &[BuiltinTy::IntVar(0), BuiltinTy::IntVar(0)],
            ret: BuiltinTy::IntVar(0),
            ptr: ws_panic as *const u8,
        },
        Builtin {
            module: BITS_MODULE,
            name: BITS_ROTR,
            params: &[BuiltinTy::IntVar(0), BuiltinTy::IntVar(0)],
            ret: BuiltinTy::IntVar(0),
            ptr: ws_panic as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "to_lower",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_to_lower as *const u8,
        },
        Builtin {
            module: STR_MODULE,
            name: "trim",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Str,
            ptr: crate::strings::ws_str_trim as *const u8,
        },
        Builtin {
            module: ARRAY_MODULE,
            name: "len",
            params: &[ARRAY_OF_ELEM],
            ret: BuiltinTy::I64,
            ptr: crate::strings::ws_array_len as *const u8,
        },
        // `std/bytes`. Each of these writes into an object the caller made:
        // a `[]u8` holds no references, so nothing here can create a stale
        // one, and `array.new` is lowered inline so the allocation had to be
        // W#'s anyway. See `crate::bytes`.
        Builtin {
            module: BYTES_MODULE,
            name: "raw_to_str",
            params: &[BYTES_OF_U8, BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::Str,
            ptr: crate::bytes::ws_bytes_to_str as *const u8,
        },
        Builtin {
            module: BYTES_MODULE,
            name: "raw_from_str",
            params: &[BYTES_OF_U8, BuiltinTy::I64, BuiltinTy::Str],
            ret: BuiltinTy::Void,
            ptr: crate::bytes::ws_bytes_from_str as *const u8,
        },
        Builtin {
            module: BYTES_MODULE,
            name: "copy",
            params: &[
                BYTES_OF_U8,
                BuiltinTy::I64,
                BYTES_OF_U8,
                BuiltinTy::I64,
                BuiltinTy::I64,
            ],
            ret: BuiltinTy::Void,
            ptr: crate::bytes::ws_bytes_copy as *const u8,
        },
        Builtin {
            module: BYTES_MODULE,
            name: "equal",
            params: &[BYTES_OF_U8, BYTES_OF_U8],
            ret: BuiltinTy::Bool,
            ptr: crate::bytes::ws_bytes_equal as *const u8,
        },
        Builtin {
            module: CRYPTO_MODULE,
            name: "raw_random",
            params: &[BuiltinTy::I64],
            // The same set `std/io` raises, since this is the same kind of
            // failure: the system was asked for something and said no.
            ret: BuiltinTy::ErrUnion(&BuiltinTy::Str, &["PermissionDenied", "IoFailed"]),
            ptr: crate::crypto::ws_crypto_random as *const u8,
        },
        Builtin {
            module: CRYPTO_MODULE,
            name: "raw_system_roots",
            params: &[],
            // `NotSupported` is the ordinary answer on a system that keeps its
            // anchors in a file, which `std/x509` then goes and reads.
            ret: BuiltinTy::ErrUnion(&BuiltinTy::Str, &["NotSupported", "IoFailed"]),
            ptr: crate::crypto::ws_crypto_system_roots as *const u8,
        },
        Builtin {
            module: TIME_MODULE,
            name: "now",
            params: &[],
            ret: BuiltinTy::I64,
            ptr: crate::crypto::ws_time_now as *const u8,
        },
        Builtin {
            module: MATH_MODULE,
            name: "sqrt",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::F64,
            ptr: ws_math_sqrt as *const u8,
        },
        Builtin {
            module: MATH_MODULE,
            name: "pow",
            params: &[BuiltinTy::F64, BuiltinTy::F64],
            ret: BuiltinTy::F64,
            ptr: ws_math_pow as *const u8,
        },
        Builtin {
            module: MATH_MODULE,
            name: "floor",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::F64,
            ptr: ws_math_floor as *const u8,
        },
        Builtin {
            module: MATH_MODULE,
            name: "ceil",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::F64,
            ptr: ws_math_ceil as *const u8,
        },
        Builtin {
            module: MATH_MODULE,
            name: "round",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::F64,
            ptr: ws_math_round as *const u8,
        },
        Builtin {
            module: MATH_MODULE,
            name: "trunc",
            params: &[BuiltinTy::F64],
            ret: BuiltinTy::F64,
            ptr: ws_math_trunc as *const u8,
        },
        Builtin {
            module: IO_MODULE,
            name: "read_file",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Str,
                &["NotFound", "PermissionDenied", "IoFailed"],
            ),
            ptr: crate::io::ws_io_read_file as *const u8,
        },
        Builtin {
            module: IO_MODULE,
            name: "read_line",
            params: &[],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::Str, &["IoFailed", "EndOfFile"]),
            ptr: crate::io::ws_io_read_line as *const u8,
        },
        Builtin {
            module: IO_MODULE,
            name: "write_file",
            params: &[BuiltinTy::Str, BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Void,
                &["NotFound", "PermissionDenied", "IoFailed"],
            ),
            ptr: crate::io::ws_io_write_file as *const u8,
        },
        Builtin {
            module: IO_MODULE,
            name: "exists",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Bool,
            ptr: crate::io::ws_io_exists as *const u8,
        },
        // ---- std/fs: the rest of a filesystem ----
        //
        // A separate module from `std/io`, which is about a file's contents.
        // These are about the tree it sits in, and a program that only reads
        // and writes files should not have to know they exist.
        Builtin {
            module: FS_MODULE,
            name: "mkdir",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Void,
                &[
                    "NotFound",
                    "PermissionDenied",
                    "AlreadyExists",
                    "NotADirectory",
                    "IoFailed",
                ],
            ),
            ptr: crate::fs::ws_fs_mkdir as *const u8,
        },
        Builtin {
            module: FS_MODULE,
            name: "rmdir",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Void,
                &[
                    "NotFound",
                    "PermissionDenied",
                    "DirectoryNotEmpty",
                    "NotADirectory",
                    "IoFailed",
                ],
            ),
            ptr: crate::fs::ws_fs_rmdir as *const u8,
        },
        Builtin {
            module: FS_MODULE,
            name: "remove",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Void,
                &[
                    "NotFound",
                    "PermissionDenied",
                    "DirectoryNotEmpty",
                    "IoFailed",
                ],
            ),
            ptr: crate::fs::ws_fs_remove as *const u8,
        },
        Builtin {
            module: FS_MODULE,
            name: "rename",
            params: &[BuiltinTy::Str, BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Void,
                &[
                    "NotFound",
                    "PermissionDenied",
                    "AlreadyExists",
                    "DirectoryNotEmpty",
                    "NotADirectory",
                    "IoFailed",
                ],
            ),
            ptr: crate::fs::ws_fs_rename as *const u8,
        },
        Builtin {
            module: FS_MODULE,
            name: "is_dir",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Bool,
            ptr: crate::fs::ws_fs_is_dir as *const u8,
        },
        Builtin {
            module: FS_MODULE,
            name: "size",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::I64,
                &["NotFound", "PermissionDenied", "IoFailed"],
            ),
            ptr: crate::fs::ws_fs_size as *const u8,
        },
        Builtin {
            module: FS_MODULE,
            name: "raw_read_dir",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(
                &BuiltinTy::Str,
                &["NotFound", "PermissionDenied", "NotADirectory", "IoFailed"],
            ),
            ptr: crate::fs::ws_fs_raw_read_dir as *const u8,
        },
        // ---- std/os: the process's own arguments and environment ----
        Builtin {
            module: OS_MODULE,
            name: "raw_args",
            params: &[],
            ret: BuiltinTy::Str,
            ptr: crate::os::ws_os_raw_args as *const u8,
        },
        Builtin {
            module: OS_MODULE,
            name: "env",
            params: &[BuiltinTy::Str],
            ret: BuiltinTy::Optional(&BuiltinTy::Str),
            ptr: crate::os::ws_os_env as *const u8,
        },
        // Copy an array out of this worker's heap and build it again, which
        // is what sending it somewhere does. Exposed for the reason the `gc_*`
        // counters are: the deep copy is a mechanism the language depends on,
        // and it should be assertable from W# -- including under
        // `--gc-stress`, where every allocation it makes is a collection.
        //
        // An array rather than a bare `T`, because the implementation is handed
        // one word and has to know it is a reference.
        Builtin {
            module: PRELUDE,
            name: "gc_transfer",
            params: &[TRANSFERABLE_ARRAY],
            ret: TRANSFERABLE_ARRAY,
            ptr: crate::transfer::ws_transfer_roundtrip as *const u8,
        },
        // The broker. Handles are numbers rather than objects: a topic and a
        // consumer belong to the process, not to any one worker's heap, and a
        // number is what can be held from either side. `std/broker.ws` wraps
        // them in `Topic[M]` and `Consumer[M]`, which is what makes them typed.
        Builtin {
            module: BROKER_MODULE,
            name: "raw_topic",
            params: &[BuiltinTy::Str, BuiltinTy::I64],
            ret: BuiltinTy::I64,
            ptr: crate::broker::ws_broker_topic as *const u8,
        },
        Builtin {
            module: BROKER_MODULE,
            name: "raw_publish",
            params: &[BuiltinTy::I64, BuiltinTy::Str, MESSAGE],
            ret: BuiltinTy::I64,
            ptr: crate::broker::ws_broker_publish as *const u8,
        },
        Builtin {
            module: BROKER_MODULE,
            name: "raw_subscribe",
            params: &[BuiltinTy::I64, BuiltinTy::Str],
            ret: BuiltinTy::I64,
            ptr: crate::broker::ws_broker_subscribe as *const u8,
        },
        Builtin {
            module: BROKER_MODULE,
            name: "raw_poll",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Optional(&MESSAGE),
            ptr: crate::broker::ws_broker_poll as *const u8,
        },
        Builtin {
            module: BROKER_MODULE,
            name: "raw_commit",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: crate::broker::ws_broker_commit as *const u8,
        },
        Builtin {
            module: BROKER_MODULE,
            name: "raw_seek",
            params: &[BuiltinTy::I64, BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: crate::broker::ws_broker_seek as *const u8,
        },
        Builtin {
            module: BROKER_MODULE,
            name: "raw_len",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::I64,
            ptr: crate::broker::ws_broker_len as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_connect",
            params: &[BuiltinTy::Str, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_connect as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_listen",
            params: &[BuiltinTy::Str, BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_listen as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_accept",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_accept as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_read",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::Str, NET_ERRORS),
            ptr: crate::net::ws_net_read as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_read_into",
            params: &[BuiltinTy::I64, BYTES_OF_U8, BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_read_into as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_write_bytes",
            params: &[BuiltinTy::I64, BYTES_OF_U8, BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_write_bytes as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_write",
            params: &[BuiltinTy::I64, BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_write as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_local_port",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_local_port as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_set_nonblocking",
            params: &[BuiltinTy::I64, BuiltinTy::Bool],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_set_nonblocking as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_close",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: crate::net::ws_net_close as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_udp",
            params: &[BuiltinTy::Str, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_udp as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_send_to",
            params: &[
                BuiltinTy::I64,
                BuiltinTy::Str,
                BuiltinTy::I64,
                BuiltinTy::Str,
            ],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_send_to as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_send_peer",
            params: &[BuiltinTy::I64, BuiltinTy::I64, BuiltinTy::Str],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_send_peer as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_recv_from",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::Str, NET_ERRORS),
            ptr: crate::net::ws_net_recv_from as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_last_peer",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_last_peer as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_poller",
            params: &[],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_poller as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_watch",
            params: &[
                BuiltinTy::I64,
                BuiltinTy::I64,
                BuiltinTy::Bool,
                BuiltinTy::Bool,
            ],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_watch as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_forget",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_forget as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_wait",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_wait as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_ready_socket",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_ready_socket as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_ready_events",
            params: &[BuiltinTy::I64, BuiltinTy::I64],
            ret: BuiltinTy::ErrUnion(&BuiltinTy::I64, NET_ERRORS),
            ptr: crate::net::ws_net_ready_events as *const u8,
        },
        Builtin {
            module: NET_MODULE,
            name: "raw_close_poller",
            params: &[BuiltinTy::I64],
            ret: BuiltinTy::Void,
            ptr: crate::net::ws_net_close_poller as *const u8,
        },
        Builtin {
            module: ARRAY_MODULE,
            name: "new",
            params: &[BuiltinTy::I64],
            ret: ARRAY_OF_ELEM,
            // Lowered inline: only the call site knows the element type, and
            // so the stride and the type id to stamp. `ptr` is never used, and
            // is the allocator only so the symbol table stays well formed.
            ptr: crate::heap::ws_alloc as *const u8,
        },
    ]
}

/// The HTTP status lattice, as `(name, supertype)` pairs in declaration order.
///
/// These are ordinary W# structs with no fields: each is a type *and* a value
/// (its sole instance), and each declares its place in the lattice. The type
/// checker injects them ahead of the user's own declarations, so a handler can
/// be written as a set of overloads rather than a growing `switch`:
///
/// ```text
/// fn handle(r: Request, s: Status)      i64 { ... }
/// fn handle(r: Request, s: Status4xx)   i64 { ... }
/// fn handle(r: Request, s: NotFound404) i64 { ... }
/// ```
///
/// A parent must appear before its children so the lattice is well founded.
/// This is a table for the same reason [`builtins`] is: sema and codegen both
/// read it, and adding a status is one line.
///
/// They live in the global namespace because W# has no module system yet --
/// once it does (roadmap item 6) they move into a `http` module.
pub fn status_types() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("Status", None),
        ("Status1xx", Some("Status")),
        ("Continue100", Some("Status1xx")),
        ("Status2xx", Some("Status")),
        ("Ok200", Some("Status2xx")),
        ("Created201", Some("Status2xx")),
        ("Accepted202", Some("Status2xx")),
        ("NoContent204", Some("Status2xx")),
        ("Status3xx", Some("Status")),
        ("MovedPermanently301", Some("Status3xx")),
        ("Found302", Some("Status3xx")),
        ("NotModified304", Some("Status3xx")),
        ("Status4xx", Some("Status")),
        ("BadRequest400", Some("Status4xx")),
        ("Unauthorized401", Some("Status4xx")),
        ("Forbidden403", Some("Status4xx")),
        ("NotFound404", Some("Status4xx")),
        ("MethodNotAllowed405", Some("Status4xx")),
        ("Conflict409", Some("Status4xx")),
        ("Teapot418", Some("Status4xx")),
        ("TooManyRequests429", Some("Status4xx")),
        ("Status5xx", Some("Status")),
        ("ServerError500", Some("Status5xx")),
        ("NotImplemented501", Some("Status5xx")),
        ("BadGateway502", Some("Status5xx")),
        ("ServiceUnavailable503", Some("Status5xx")),
        ("GatewayTimeout504", Some("Status5xx")),
    ]
}

/// The array constructor, which the code generator lowers itself rather than
/// calling: it needs the element type, which only the call site has.
pub const ARRAY_NEW: &str = "new";

/// The standard library's string operations.
pub const STR_MODULE: &str = "std/str";
/// The standard library's array operations.
pub const ARRAY_MODULE: &str = "std/array";
/// The standard library's growable array.
pub const LIST_MODULE: &str = "std/list";
/// The standard library's arithmetic.
pub const MATH_MODULE: &str = "std/math";
/// The standard library's file and standard-input operations.
pub const IO_MODULE: &str = "std/io";
/// The standard library's byte buffers, and the bridge to `str`.
pub const BYTES_MODULE: &str = "std/bytes";
/// The standard library's AEADs: ChaCha20-Poly1305 and AES-GCM.
pub const CIPHER_MODULE: &str = "std/cipher";
/// The standard library's hashes: SHA-2, HMAC and HKDF.
pub const HASH_MODULE: &str = "std/hash";
/// The system's random bytes.
pub const CRYPTO_MODULE: &str = "std/crypto";
/// The wall clock.
pub const TIME_MODULE: &str = "std/time";
/// Fixed-width unsigned integers and Montgomery arithmetic.
pub const BIGNUM_MODULE: &str = "std/bignum";
/// X25519, on Curve25519.
pub const CURVE25519_MODULE: &str = "std/curve25519";
/// The NIST prime curves: ECDH and ECDSA on P-256 and P-384.
pub const NISTEC_MODULE: &str = "std/nistec";
/// RSA signature verification: PKCS#1 v1.5 and PSS.
pub const RSA_MODULE: &str = "std/rsa";
/// Distinguished Encoding Rules, which X.509 is written in.
pub const DER_MODULE: &str = "std/der";
/// Public keys and certificates.
pub const X509_MODULE: &str = "std/x509";
/// TLS 1.3.
pub const TLS_MODULE: &str = "std/tls";

/// The error names the library can raise, in the order their ids are assigned.
///
/// An error value is an index into the program's error table, and a builtin
/// has to know which index it is returning long before the program that will
/// catch it has been read. The type checker therefore interns these first, so
/// the ids below are what they are in every program.
pub fn builtin_errors() -> &'static [&'static str] {
    &[
        "NotFound",
        "PermissionDenied",
        "IoFailed",
        "EndOfFile",
        // A worker can die, and that is not an exceptional case worth a second
        // mechanism -- so an RPC returns `!T` and this is what it raises.
        "WorkerDied",
        "SpawnFailed",
        // The network. Appending is safe and rearranging is not: a tag is an
        // index into this list, and a builtin compiled against one number
        // cannot be caught by a program that interned another.
        "ConnectionRefused",
        "ConnectionReset",
        "BrokenPipe",
        "AddressInUse",
        "TimedOut",
        // Not a failure so much as an answer: on a non-blocking socket this is
        // what "nothing to do yet" is called, and a program is expected to
        // catch it and come back.
        "WouldBlock",
        "HostNotFound",
        "NetworkUnreachable",
        // What `str.parse_int` raises. Not a network error; it is here because
        // this list is one flat numbering and appending is the only safe way
        // to add to it.
        "BadFormat",
        // What a library raises for something it can do in principle and
        // cannot yet -- `https://`, until there is a TLS client to hand it to.
        "NotSupported",
        // The filesystem, past reading and writing whole files. Appended, like
        // everything else here, because the number is the identity.
        "AlreadyExists",
        "NotADirectory",
        // `rmdir` on a directory that still holds something, and `remove` on a
        // directory -- one name, because both mean "there is something in the
        // way and it is not this call's business to move it".
        "DirectoryNotEmpty",
    ]
}

/// The error a call into a worker that has gone raises.
pub const ERROR_WORKER_DIED: i64 = 5;
/// The error `@spawn` raises when a thread could not be started.
pub const ERROR_SPAWN_FAILED: i64 = 6;
/// The names, for the type checker to intern -- it needs them by name, and
/// they must be the same two the constants above number.
pub const WORKER_DIED: &str = "WorkerDied";
pub const SPAWN_FAILED: &str = "SpawnFailed";

/// The tags for [`builtin_errors`]: an index plus one, because zero is success.
pub const ERROR_NOT_FOUND: i64 = 1;
pub const ERROR_PERMISSION_DENIED: i64 = 2;
pub const ERROR_IO_FAILED: i64 = 3;
pub const ERROR_END_OF_FILE: i64 = 4;
pub const ERROR_CONNECTION_REFUSED: i64 = 7;
pub const ERROR_CONNECTION_RESET: i64 = 8;
pub const ERROR_BROKEN_PIPE: i64 = 9;
pub const ERROR_ADDRESS_IN_USE: i64 = 10;
pub const ERROR_TIMED_OUT: i64 = 11;
pub const ERROR_WOULD_BLOCK: i64 = 12;
pub const ERROR_HOST_NOT_FOUND: i64 = 13;
pub const ERROR_NETWORK_UNREACHABLE: i64 = 14;
pub const ERROR_BAD_FORMAT: i64 = 15;
pub const ERROR_NOT_SUPPORTED: i64 = 16;
pub const ERROR_ALREADY_EXISTS: i64 = 17;
pub const ERROR_NOT_A_DIRECTORY: i64 = 18;
pub const ERROR_DIRECTORY_NOT_EMPTY: i64 = 19;

/// The module the HTTP status lattice lives in.
///
/// The types are materialised from [`status_types`] on first mention rather
/// than declared, so the module has no other content and no entry in
/// [`builtins`]; it still has to be a path an `@import` can name.
pub const HTTP_MODULE: &str = "std/http";
/// The broker's module: named topics, partitioned logs, consumer groups.
pub const BROKER_MODULE: &str = "std/broker";
/// The standard library's sockets.
pub const NET_MODULE: &str = "std/net";
pub const BITS_MODULE: &str = "std/bits";
/// Directories, and the two facts about a path a store needs.
pub const FS_MODULE: &str = "std/fs";
/// The process's own arguments and environment.
pub const OS_MODULE: &str = "std/os";
/// Path arithmetic, which is all W# and touches no syscall.
pub const PATH_MODULE: &str = "std/path";
/// The two rotates, which the code generator recognises by name and lowers
/// inline rather than calling. See [`BuiltinTy::IntVar`].
pub const BITS_ROTL: &str = "rotl";
pub const BITS_ROTR: &str = "rotr";

/// What any socket operation may raise.
///
/// One set for all of them rather than a tailored set each. A builtin's error
/// set is written in its row because it is compiled long before the program
/// that catches it, and the wrappers in `std/net.ws` pass these results through
/// each other constantly -- one set means those compose without a widening at
/// every step, and the cost is a `catch` that can name an error this particular
/// call would not in practice raise.
const NET_ERRORS: &[&str] = &[
    "ConnectionRefused",
    "ConnectionReset",
    "BrokenPipe",
    "AddressInUse",
    "TimedOut",
    "WouldBlock",
    "HostNotFound",
    "NetworkUnreachable",
    "PermissionDenied",
    "IoFailed",
];

/// The parts of the standard library written in W# rather than Rust.
///
/// Anything that assembles an object holding references belongs here. In W#
/// the write barrier, the load barrier and the stack maps all apply by
/// construction; in Rust each would have to be reproduced by hand, and a
/// missed one loses objects rather than failing a test. The rule this draws is
/// simple: a builtin may read and write bytes, and W# does everything that
/// touches a reference.
pub fn std_module_sources() -> &'static [(&'static str, &'static str)] {
    &[
        (ARRAY_MODULE, include_str!("std/array.ws")),
        (LIST_MODULE, include_str!("std/list.ws")),
        (STR_MODULE, include_str!("std/str.ws")),
        (BYTES_MODULE, include_str!("std/bytes.ws")),
        (CRYPTO_MODULE, include_str!("std/crypto.ws")),
        (HASH_MODULE, include_str!("std/hash.ws")),
        (CIPHER_MODULE, include_str!("std/cipher.ws")),
        (BIGNUM_MODULE, include_str!("std/bignum.ws")),
        (CURVE25519_MODULE, include_str!("std/curve25519.ws")),
        (NISTEC_MODULE, include_str!("std/nistec.ws")),
        (RSA_MODULE, include_str!("std/rsa.ws")),
        (DER_MODULE, include_str!("std/der.ws")),
        (X509_MODULE, include_str!("std/x509.ws")),
        (TLS_MODULE, include_str!("std/tls.ws")),
        (MATH_MODULE, include_str!("std/math.ws")),
        (FS_MODULE, include_str!("std/fs.ws")),
        (OS_MODULE, include_str!("std/os.ws")),
        (PATH_MODULE, include_str!("std/path.ws")),
        (BROKER_MODULE, include_str!("std/broker.ws")),
        (NET_MODULE, include_str!("std/net.ws")),
        (HTTP_MODULE, include_str!("std/http.ws")),
    ]
}

/// Every module path the standard library provides.
///
/// This is what tells an `@import` of `"std/nope"` from one of `"std/http"`,
/// and what lets `@import("std")` be written and then walked into.
pub fn std_module_paths() -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for module in builtins()
        .iter()
        .map(|b| b.module)
        .chain(std_module_sources().iter().map(|(path, _)| *path))
        .chain(std::iter::once(HTTP_MODULE))
    {
        if module == PRELUDE {
            continue;
        }
        // Every prefix is a module too, so `@import("std")` can be walked into
        // as `std.http`.
        let mut prefix = String::new();
        for segment in module.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            if !paths.contains(&prefix) {
                paths.push(prefix.clone());
            }
        }
    }
    paths.sort();
    paths
}

/// The abstract types, as `(name, members)` pairs.
///
/// An abstract type is a name for a set of concrete types. It is not a type a
/// value can have -- there is no machine representation for "a number" -- so it
/// may only be written as a parameter's annotation, where it says which types
/// that parameter accepts. That is what gives the dispatcher something to order
/// scalars by: `i64` is more specific than `Number` in the same way `NotFound404`
/// is more specific than `Status`, and for the same reason.
///
/// A table for the same reason [`builtins`] and [`status_types`] are: sema reads
/// it to seed the lattice, and adding a classifier is one row.
pub fn abstract_types() -> &'static [(&'static str, &'static [BuiltinTy])] {
    use BuiltinTy::*;
    &[
        // Every numeric type, which is what makes `math.min` one function
        // rather than nine. The price is stated rather than hidden: a body
        // annotated `Number` must work for *every* member, so it may not use
        // `%` (no float form) and may not negate (no unsigned negatives).
        ("Number", &[I8, I16, I32, I64, U8, U16, U32, U64, F64]),
        // The integers alone, for a body that needs `%` or the bit operators
        // and would otherwise have to be written eight times. Its members are
        // a subset of `Number`'s, which is what makes it the more specific of
        // the two when both could match -- see `TypeStore::is_sub_ty`.
        ("Integer", &[I8, I16, I32, I64, U8, U16, U32, U64]),
    ]
}

/// Runtime support routines that generated code calls but that are not
/// callable from W# source: the allocator and the panic handler.
pub fn runtime_symbols() -> Vec<(&'static str, *const u8)> {
    vec![
        ("ws_alloc", crate::heap::ws_alloc as *const u8),
        ("ws_panic", ws_panic as *const u8),
        ("ws_panic_index", ws_panic_index as *const u8),
        ("ws_log_object", crate::gc::ws_log_object as *const u8),
        ("ws_gc_poll", crate::gc::ws_gc_poll as *const u8),
        ("ws_resolve", crate::evacuate::ws_resolve as *const u8),
        ("ws_spawn", crate::rpc::ws_spawn as *const u8),
        ("ws_rpc_call", crate::rpc::ws_rpc_call as *const u8),
        ("ws_join", crate::rpc::ws_join as *const u8),
    ]
}

// ---- implementations ----------------------------------------------------

/// Print a W# string object followed by a newline.
///
/// # Safety
/// `ptr` must be null or point at a W# string object: a header whose aux word
/// holds the byte length, followed by that many bytes of UTF-8.
pub unsafe extern "C" fn ws_print_str(ptr: *const u8) {
    unsafe { crate::gc::checkpoint() };
    if ptr.is_null() {
        println!();
        return;
    }
    // SAFETY: `ptr` is a W# string object -- length in the aux word, bytes
    // immediately after the header.
    let text = unsafe {
        let len = (ptr.add(AUX_OFFSET as usize) as *const u64).read() as usize;
        let bytes = std::slice::from_raw_parts(ptr.add(HEADER_SIZE as usize), len);
        std::str::from_utf8(bytes)
    };
    match text {
        Ok(s) => println!("{s}"),
        Err(_) => println!("<invalid utf-8 string>"),
    }
}

pub extern "C" fn ws_print_int(value: i64) {
    unsafe { crate::gc::checkpoint() };
    println!("{value}");
}

/// The unsigned twin of [`ws_print_int`], for the half of `u64`'s range an
/// `i64` cannot hold: `print_int(i64(x))` would show it as a negative number.
pub extern "C" fn ws_print_uint(value: u64) {
    unsafe { crate::gc::checkpoint() };
    println!("{value}");
}

/// Always shows a decimal point, so a float `1` is not mistaken for an integer.
pub extern "C" fn ws_print_float(value: f64) {
    unsafe { crate::gc::checkpoint() };
    if value.is_finite() && value.fract() == 0.0 {
        println!("{value:.1}");
    } else {
        println!("{value}");
    }
}

pub extern "C" fn ws_print_bool(value: i8) {
    unsafe { crate::gc::checkpoint() };
    println!("{}", value != 0);
}

pub extern "C" fn ws_assert(cond: i8) {
    unsafe { crate::gc::checkpoint() };
    if cond == 0 {
        ws_panic(PANIC_ASSERT);
    }
}

/// Force a collection.
///
/// Called directly from generated code, so the frame chain above is intact and
/// the stack maps describe the caller's roots -- which is exactly what a
/// collection needs.
pub extern "C" fn ws_gc_collect() {
    // Not while a trace is moving objects: see `gc::on_allocation`.
    if crate::gc::evacuating() {
        return;
    }
    unsafe { crate::gc::collect() };
}

/// Run a whole mark trace, synchronously: begin it, wait for the collector
/// thread to mark, finish it, wait for the sweep. What follows in the program
/// sees a heap with every cycle reclaimed.
pub extern "C" fn ws_gc_trace() {
    unsafe { crate::mark::run_full_trace() };
}

/// Begin a trace and return while the collector thread marks.
pub extern "C" fn ws_gc_trace_start() {
    unsafe { crate::mark::trace_start() };
}

/// Wait for the trace in flight to be entirely over.
pub extern "C" fn ws_gc_trace_finish() {
    unsafe { crate::mark::trace_finish() };
}

pub extern "C" fn ws_gc_traces() -> i64 {
    crate::gc::traces() as i64
}

pub extern "C" fn ws_gc_live_objects() -> i64 {
    crate::heap::heap_stats().live_objects as i64
}

pub extern "C" fn ws_gc_live_bytes() -> i64 {
    crate::heap::heap_stats().live_bytes as i64
}

pub extern "C" fn ws_gc_collections() -> i64 {
    crate::gc::collections() as i64
}

pub extern "C" fn ws_math_sqrt(x: f64) -> f64 {
    unsafe { crate::gc::checkpoint() };
    x.sqrt()
}

pub extern "C" fn ws_math_pow(x: f64, y: f64) -> f64 {
    unsafe { crate::gc::checkpoint() };
    x.powf(y)
}

pub extern "C" fn ws_math_floor(x: f64) -> f64 {
    unsafe { crate::gc::checkpoint() };
    x.floor()
}

pub extern "C" fn ws_math_ceil(x: f64) -> f64 {
    unsafe { crate::gc::checkpoint() };
    x.ceil()
}

pub extern "C" fn ws_math_round(x: f64) -> f64 {
    unsafe { crate::gc::checkpoint() };
    x.round()
}

pub extern "C" fn ws_math_trunc(x: f64) -> f64 {
    unsafe { crate::gc::checkpoint() };
    x.trunc()
}

pub const PANIC_UNWRAP_NULL: i64 = 1;
pub const PANIC_ASSERT: i64 = 2;
pub const PANIC_NO_METHOD: i64 = 3;
pub const PANIC_DIVIDE_BY_ZERO: i64 = 4;
pub const PANIC_DIVIDE_OVERFLOW: i64 = 5;
pub const PANIC_INDEX_OUT_OF_BOUNDS: i64 = 6;

/// The exit status of a program that panicked: the one a Rust program exits
/// with on a panic, so it is already familiar. A signal (`abort`) would be the
/// alternative, but a signal is what a *compiler* bug looks like; a plain exit
/// status says the program itself asked to stop.
pub const PANIC_EXIT_STATUS: i32 = 101;

/// Report a failure and end the process. Called from generated code for
/// failures that the type system permits but the program must not perform,
/// such as `.?` on a null optional.
/// An out-of-bounds index, which unlike the other failures is worth naming
/// the numbers for: "index 5 out of bounds (len 3)" says what to look at, and
/// "index out of bounds" does not.
///
/// A separate entry point rather than three arguments on [`ws_panic`] because
/// every other panic site would then have to pass two zeroes, and the reason
/// they are zero would need explaining at each one.
pub extern "C" fn ws_panic_index(index: i64, len: i64) -> ! {
    report_and_exit(&format!("index {index} out of bounds (len {len})"))
}

pub extern "C" fn ws_panic(code: i64) {
    let reason = match code {
        PANIC_UNWRAP_NULL => "unwrapped a null optional".to_string(),
        PANIC_ASSERT => "assertion failed".to_string(),
        PANIC_NO_METHOD => "no overload matched these argument types".to_string(),
        PANIC_DIVIDE_BY_ZERO => "integer division by zero".to_string(),
        // Not "i64::MIN" any more: the check is per width, so an `i32` can
        // reach this too and naming one type would misdescribe the other.
        PANIC_DIVIDE_OVERFLOW => "integer overflow in division: MIN / -1".to_string(),
        _ => format!("unknown failure (code {code})"),
    };
    report_and_exit(&reason)
}

fn report_and_exit(reason: &str) -> ! {
    eprintln!("W# panic: {reason}");
    crate::gc::report_if_asked();
    // `exit` rather than `panic!`: unwinding out of an `extern "C"` function
    // called from JIT-compiled frames has no defined behaviour. `exit` does
    // not unwind either -- it flushes stdout and leaves -- so whatever the
    // program printed before it died still arrives.
    std::process::exit(PANIC_EXIT_STATUS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{FLAG_IMMORTAL, TYPE_ID_STR, meta_word};

    /// Every `ERROR_*` constant is the position of its name in
    /// [`builtin_errors`], plus one.
    ///
    /// The two are written down separately -- the names for the type checker to
    /// intern, the numbers for the runtime to return -- and nothing but this
    /// keeps them agreeing. Getting it wrong is silent: a builtin returns a tag
    /// that names a different error, or one the program has no name for at all.
    #[test]
    fn every_error_constant_matches_its_position() {
        let names = builtin_errors();
        let expected = [
            ("NotFound", ERROR_NOT_FOUND),
            ("PermissionDenied", ERROR_PERMISSION_DENIED),
            ("IoFailed", ERROR_IO_FAILED),
            ("EndOfFile", ERROR_END_OF_FILE),
            ("WorkerDied", ERROR_WORKER_DIED),
            ("SpawnFailed", ERROR_SPAWN_FAILED),
            ("ConnectionRefused", ERROR_CONNECTION_REFUSED),
            ("ConnectionReset", ERROR_CONNECTION_RESET),
            ("BrokenPipe", ERROR_BROKEN_PIPE),
            ("AddressInUse", ERROR_ADDRESS_IN_USE),
            ("TimedOut", ERROR_TIMED_OUT),
            ("WouldBlock", ERROR_WOULD_BLOCK),
            ("HostNotFound", ERROR_HOST_NOT_FOUND),
            ("NetworkUnreachable", ERROR_NETWORK_UNREACHABLE),
            ("BadFormat", ERROR_BAD_FORMAT),
            ("NotSupported", ERROR_NOT_SUPPORTED),
            ("AlreadyExists", ERROR_ALREADY_EXISTS),
            ("NotADirectory", ERROR_NOT_A_DIRECTORY),
            ("DirectoryNotEmpty", ERROR_DIRECTORY_NOT_EMPTY),
        ];
        assert_eq!(
            names.len(),
            expected.len(),
            "a name was added to `builtin_errors` without a tag beside it"
        );
        for (name, tag) in expected {
            let index = names
                .iter()
                .position(|n| *n == name)
                .unwrap_or_else(|| panic!("`{name}` is not in `builtin_errors`"));
            assert_eq!(tag, index as i64 + 1, "the tag for `{name}` is wrong");
        }
        // The two names the type checker looks up by spelling.
        assert_eq!(names[ERROR_WORKER_DIED as usize - 1], WORKER_DIED);
        assert_eq!(names[ERROR_SPAWN_FAILED as usize - 1], SPAWN_FAILED);
    }

    /// Build a string object the way the code generator emits literals, so the
    /// test exercises the same layout `ws_print_str` reads.
    fn make_str(s: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&meta_word(TYPE_ID_STR, FLAG_IMMORTAL).to_ne_bytes());
        bytes.extend_from_slice(&(s.len() as u64).to_ne_bytes());
        bytes.extend_from_slice(s.as_bytes());
        bytes
    }

    #[test]
    fn string_layout_matches_what_print_reads() {
        let obj = make_str("hello");
        let len = unsafe { (obj.as_ptr().add(AUX_OFFSET as usize) as *const u64).read() };
        assert_eq!(len, 5);
        let bytes = &obj[HEADER_SIZE as usize..];
        assert_eq!(std::str::from_utf8(bytes).unwrap(), "hello");
    }

    #[test]
    fn every_builtin_has_a_distinct_name_and_a_real_pointer() {
        let all = builtins();
        // Distinct *symbols*, not names: `std/str.len` and `std/array.len`
        // are two different functions that source code calls `len`.
        let mut names: Vec<String> = all.iter().map(|b| b.symbol()).collect();
        names.sort();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate builtin symbol");
        assert!(all.iter().all(|b| !b.ptr.is_null()));
    }

    #[test]
    fn runtime_symbols_are_distinct_and_real() {
        let syms = runtime_symbols();
        assert!(syms.iter().any(|(n, _)| *n == "ws_alloc"));
        assert!(syms.iter().all(|(_, p)| !p.is_null()));
    }
}
