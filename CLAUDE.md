# Working on W#

Conventions and hard-won details for anyone (human or agent) picking this up.

## Building

`rustc` invokes `cc` to link, and a bare NixOS box has none on `PATH`. Library
crates build fine without it and then *tests and binaries fail to link* with
`linker 'cc' not found`, which is a confusing way to find out. Use the dev shell:

```sh
nix-shell --run "cargo test --workspace"
```

`.cargo/config.toml` sets `-Cforce-frame-pointers=yes` for the whole workspace.
**Do not remove it.** The collector finds its roots by walking the frame-pointer
chain out of a runtime function that generated code called into; if the Rust
frames between the collector and the nearest generated frame omit their frame
pointers, the walk reads garbage on its first step. Generated code preserves
them because the code generator sets Cranelift's `preserve_frame_pointers`; this
is the other half of the same requirement.

## Cranelift

Pinned to **0.134.3**, which is in the local registry cache, so the workspace
builds offline. `cranelift-native` picks the host ISA.

**Read the vendored source before using an API you have not used in this
codebase.** Cranelift's surface churns across minor versions and recalled
signatures are frequently wrong. The source is at:

```
~/.cargo/registry/src/index.crates.io-*/cranelift-{codegen,frontend,jit,module}-0.134.3/
```

The generated instruction builder is not in that tree — it is produced at build
time. After a build, find it at:

```
target/debug/build/cranelift-codegen-*/out/inst_builder.rs
```

Things in this version that differ from older tutorials, each of which cost time:

| API | Reality in 0.134.3 |
|---|---|
| `FunctionBuilder::declare_var` | Takes a type and *returns* a `Variable`. Older code passes one in. |
| `brif` / `jump` block arguments | Take `BlockArg`, not `Value`. Use `BlockArg::Value(v)`. |
| `MemFlags` | An interned `u16` handle. The struct with the flags is `MemFlagsData`; `load`/`store` take `impl Into<MemFlagsData>`. There is no `MemFlags::new()`. |
| Referencing data | `symbol_value`, not `global_value`. |
| `icmp_imm` / `iadd_imm` | Deprecated; use the `_s` / `_u` variants that say how the immediate is extended. |
| `FunctionBuilder::finalize` | **Must** be called per function, and takes a `TargetFrontendConfig`. It is what releases the shared `FunctionBuilderContext`; skip it and the *next* `FunctionBuilder::new` fails an assert with no hint about the real cause. |
| `iconst` | The immediate must be **zero-extended** into the controlling type. `iconst.i32 -1` is a verifier error, doc comments about signedness notwithstanding -- so a negative literal arrives masked (`IntTy::mask`). The `_imm_u` / `_imm_s` builders mask for you; the bare `iconst` does not. |
| `uextend` / `ireduce` | Strictly **wider** / **narrower**, despite `uextend`'s doc saying same-width is a no-op. Converting between two types of the same width must emit *nothing*; asking for the identity is a verifier error. |
| `ishl` / `ushr` / `sshr` / `rotl` / `rotr` | The shift or rotate amount is **masked to the operand's width**, so `x << 64` on a `u64` is `x`. Documented for the shifts; for the rotates it is established by `isle_prelude.rs`'s constant folding and both backends' lowering rather than by the doc comment. |
| `sdiv` / `udiv` | Both trap, and a trap in a JIT with no signal handler is a bare SIGILL. The hand-rolled checks in `checked_div` are what turn those into panics that say something. `srem` defines `MIN % -1` as 0, and unsigned division cannot overflow at all. |

## Two backends, one lowering

`wsharp run` compiles into this process; `wsharp build` writes an object file
and links it against `libwsharp_start.a` with `cc`. Both go through
`codegen::build`, which is generic over `cranelift_module::Module` and is the
*only* place either backend's code comes from. Nothing below it may ask which
backend it is in -- that would be the first of two lowerings, and the second
would drift.

What actually differs is one flag and where the tables go:

| | `compile_jit` | `compile_object` |
|---|---|---|
| `is_pic` | `false` — cranelift-jit **asserts** this | `true` |
| Runtime symbols | resolved to addresses via `JITBuilder::symbol` | left to the linker |
| Type registry, stack maps, services | `register_type`/`register_code`/`register_services`, in process | emitted as data, read at startup by `runtime::aot` |
| Entry | `transmute` and call | a `ws_main` shim, called by `wsharp-start`'s `main` |

Things that will bite:

| Thing | Reality |
|---|---|
| A new runtime entry point | Needs `#[unsafe(no_mangle)]` *and* a `link:` in its `Builtin` row. Under the JIT a missing one is a symbol panic at compile time; under AOT it is a link error at the far end of a build. A test asserts every `link` is well formed and its own. |
| A new absolute address in generated code | There must not be one. The two GC flag bytes are `Linkage::Import` data reached with `symbol_value`, because an `iconst` of an address works under the JIT and is a wild pointer in a built program. |
| `Linkage::Import` data under `is_pic` | Emits `R_X86_64_GOTPCREL`, which the linker *relaxes* to a RIP-relative `lea` because the flag is in the same static link unit. That is what keeps the load barrier's fast path one `lea` and one byte load. Check it with `readelf -r` on the linked binary, not on the object: the object always shows the GOT form. |
| The three emitted tables | Sequential little-endian streams, defined once in `wsharp-runtime/src/aot.rs` and written by `wsharp-codegen/src/tables.rs`. Only code addresses are relocations. Adding a field means the writer, the reader, and the round-trip tests -- which drive the *real* reader over the writer's bytes, so there is no second implementation to keep in step. |
| `MAGIC`/`VERSION` in those tables | The only guard against a program built by one compiler and linked against another's runtime. Bump `VERSION` when the layout changes. |
| `crates/wsharp-start` | Defines `main`, so it sets `test = false`: a test harness brings its own `main` and the linker refuses two. It must be built with `-Cforce-frame-pointers=yes` or the collector's root walk breaks at its first Rust frame. |
| `cargo test` and the runtime archive | **`cargo test --workspace` does not build `crates/wsharp-start`.** It has no test target and nothing depends on it, so cargo leaves it out of the graph entirely -- while the case suite's AOT pass shells out to `wsharp build`, which links against whatever `libwsharp_start.a` an *earlier* `cargo build` left in `target/debug`. A cold tree fails every built case with "cannot find the runtime archive"; a warm one silently links a stale runtime, which is worse, because it passes. Adding a builtin and testing it with `cargo test` alone will report an undefined reference to a symbol that is right there in the source. Run `cargo build --workspace` first -- which is why both CI workflows have a "Build for the test suite" step before their "Test" step. |
| `cargo build` and `ingot` | Does not produce it any more. `ingot` is `wsharp build --module ingot/main -o ingot`, and `crates/wsharp-cli/tests/verbs.rs` bootstraps one to test against. |
| Windows | **Builds, links and passes the suite.** Everything needed to compile and link: `c_int` in `sys/windows.rs`, `-subsystem:console` (clang picks a subsystem by looking for `main` in the *objects*, and ours is in the archive), the system libraries a staticlib does not carry, and `-nodefaultlib:libcmt -defaultlib:msvcrt` (Rust links the dynamic CRT, clang defaults to the static one). What was wrong for a long time was **the collector's stack walk, not `fs.mkdir_all`** -- see the row below. The `mkdir_all` story that stood here was a misdiagnosis: that walk is fine, and `!x` on a builtin's `bool` (normalised in `lower.rs`) really did fix what it was blamed for. Note that almost nothing here can be checked locally on NixOS: no rustup, no std for the target, so not even `cargo check --target`. It needs a real Windows machine. |
| The `rbp` chain on Win64 | **Not a chain, and this cost the platform a release.** A Win64 prologue records its frame register in the function's *unwind info* and may establish it as `lea rbp, [rsp + n]`, for which `[rbp]` is a local rather than the caller's frame; and nothing zeroes the outermost one, where SysV requires it. `-Cforce-frame-pointers=yes` does reach this target -- it is on the `rustc` command line and checkable -- and does not make the chain followable. So the collector's walk crossed about two Rust frames, reached no generated code, found **zero roots**, and then climbed off the end of the stack: the symptom was a return address of `0x6873775c67756265`, which is `"ebug\wsh"`. Every `gc_*` case died under `--gc-stress` and `ingot install` died at `0xC0000005`, while ordinary programs looked fine because without stress the collector barely runs. `stackwalk` therefore crosses the Rust frames with `RtlVirtualUnwind` here and follows `rbp` only within generated code. |

## The operating system

`crates/wsharp-runtime/src/sys/` declares it by hand: three arms named for what
they are (`linux.rs`, `bsd.rs`, `windows.rs`), no `libc` crate, because
`wsharp-runtime` has zero dependencies and that is worth keeping. Everything
that can be written once -- `EINTR` retries, short reads, trying each resolved
address in turn -- is in `sys/mod.rs` instead, which is also the only part of
the layer this machine's tests exercise.

Cross-check the arms you cannot run before believing them:

```sh
nix-shell --run "cargo check -p wsharp-runtime --target x86_64-pc-windows-gnu --all-targets"
nix-shell --run "cargo check -p wsharp-runtime --target aarch64-apple-darwin --all-targets"
```

Things that differ between the arms, each of which cost time:

| Thing | Reality |
|---|---|
| `struct addrinfo` | `ai_canonname` comes **before** `ai_addr` on the BSDs and Windows, and after it on Linux. `ai_addrlen` is `socklen_t` on Unix and `size_t` on Windows. Each arm declares its own for this reason. |
| `epoll_event` | `#[repr(C, packed)]` on x86-64 **only**; elsewhere it has the natural padding. The wrong one shifts `data` by four bytes and hands back a descriptor that is half of one. |
| `O_NONBLOCK` | `0o4000` on Linux, `0x4` on the BSDs. `O_CLOEXEC` differs between macOS and FreeBSD, which is why the BSD arm sets close-on-exec with `fcntl` instead of asking for it in the flags. |
| `errno` | The same up to 34 and different above it -- `EAGAIN` is 11 on Linux and 35 on the BSDs. |
| A Windows `SOCKET` | Not a file descriptor and not a `HANDLE`: `closesocket`, not `CloseHandle`; `recv`, not `ReadFile`. |
| `SO_REUSEADDR` on Windows | Lets a second socket bind a port another is *actively listening on*. The right port of the Unix workaround is to do nothing at all. |
| `getaddrinfo` failure | Reports `EAI_*` codes, which are negative on glibc and small positive numbers on macOS -- so they would collide with `errno`. Each arm translates them to one synthetic `ERESOLVE` instead. |
| `struct stat` | **Declared in one arm, and only because one question needs it.** It has a different layout on macOS, FreeBSD, NetBSD and OpenBSD, and is a versioned symbol on glibc, so three of the four questions this layer asks have a one-number answer instead: `is_dir` is `opendir` succeeding, `size` is `lseek` to the end, and `is_executable` is `access(X_OK)`. The fourth is `modified_at`, which a watcher needs and which hashing a whole tree per tick is not an answer to. Linux uses `statx` — kernel UAPI, one layout on every architecture, versioned by a mask rather than by a symbol — macOS uses `getattrlist`, which hands back the attributes asked for and no struct at all, and Windows reads a field `GetFileAttributesExW` was already fetching. **Only FreeBSD, NetBSD, OpenBSD and DragonFly meet `struct stat`**, as one `ST_MTIME_OFFSET` per system in `D_NAME_OFFSET`'s style, and those four numbers are the part of this runtime no machine available here can check — `cargo check --target aarch64-apple-darwin` compiles the macOS arm, which is the one that does not use them. `chmod` still has no counterpart that *reads* a mode: "will this start" is answerable with `access` and "which bits are set" is not, and only the first has a caller. |
| `struct dirent` | `d_name` starts at 19 on Linux, 21 on macOS, 24 on FreeBSD and OpenBSD, 13 on NetBSD and 16 on DragonFly. POSIX guarantees it is NUL-terminated, so each arm carries the *offset* and nothing else -- one auditable fact per system, where a whole declared struct would be five. |
| `readdir` on macOS | The symbol is `readdir$INODE64` on x86-64 and plain `readdir` on arm64. Linking the unsuffixed name on x86-64 gets the *old* `struct dirent`, whose `d_name` is at 8 rather than 21, and every file name comes back as the tail of another field. Same for `opendir`; `closedir` is unsuffixed. |
| `ENOTEMPTY` | 39 on Linux and 66 on the BSDs -- the numbering agrees only up to 34, and this is the first code past it that ordinary filesystem work meets. |
| `MoveFileExW` | Needs `MOVEFILE_REPLACE_EXISTING` to be the operation Unix's `rename` is; without it Windows refuses when the destination exists. It still will not replace an existing *directory*, which is why the store publishes by renaming into a name nothing holds yet. |

**Never lay out a `sockaddr` by hand.** `getaddrinfo` produces addresses and
everything else consumes them, which is what keeps byte order and the BSDs'
extra `sin_len` byte out of this code entirely.

## Invariants

- **`hir::Program.funcs` is indexed by `FuncId`.** Never filter or reorder it —
  that silently renumbers every function. `Analysis::finish` discards the whole
  table rather than drop one entry.
- **Code generation never sees a type variable.** Monomorphisation substitutes
  every type in a reachable function, *including intermediate expression types*,
  and reports what it cannot resolve. `repr::slot_types` panics on a leftover
  variable on purpose: it means that pass has a hole.
- **Value layout has one definition, and it is `layout::place`.** Everything
  laid out end to end goes through it: a struct's fields (`infer.rs`), an
  instantiation of a generic struct (`lower::struct_shape`), and a closure's
  captures -- which needs it in three places at once, the layout registered
  with the runtime, the prologue that reads captures out of `env`, and the
  constructor that writes them in (`lower::capture_offsets`). Those three used
  to be three hand-written loops that agreed only by being written the same
  way; a disagreement is not a crash but a field read from the wrong place, and
  the collector reading a scalar as a reference. `slot_count` and
  `repr::slot_types` must agree too, and a test in
  `wsharp-codegen/src/repr.rs` checks that they do.
- **A scalar packs; a tagged value does not.** A scalar occupies its natural
  size at its natural alignment -- which is what makes `[]u8` a byte array
  rather than one eight times too large -- while a `?T` or `!T` keeps a whole
  machine word per slot. That second half is load-bearing: **slot `i` of a
  value lives at `base + i * SLOT_SIZE`**, and three separate things depend on
  exactly that -- the stride `load_at` and `store_slots` walk with, and the
  division `repr::pointer_slots` uses to turn a byte offset back into a slot
  index. Two tests in `layout.rs` state it rather than leaving it in a comment.
- **Packing means aligning, and that is not cosmetic.** Every load and store
  through these offsets uses `MemFlagsData::trusted()`, whose `aligned` bit
  lets the instruction "trap or return a wrong result if the effective address
  is misaligned". A field placed at its own alignment is what keeps that flag
  honest. `layout::align_of` gives `void` an alignment of 1 rather than 0,
  because `place` rounds by it.
- **A subtype starts from its parent's *unaligned* `field_end`.** Aligning
  there would move every subtype's fields and break "a subtype's fields are its
  supertype's, followed by its own". Only the total size is rounded up.
- **Signedness lives in the W# type, never in `ir::Type`.** Cranelift has
  `I8`/`I16`/`I32`/`I64` and nothing else, so `i32` and `u32` are the same
  machine type; it is the *instruction* that differs. `lower::NumKind` carries
  the answer, and five things ask it: `>>` (`sshr` against `ushr`), `/` and `%`
  (`sdiv`/`srem` against `udiv`/`urem`, and whether the overflow check is
  emitted at all), and the four ordering comparisons.
- **`+`, `-` and `*` wrap; only division panics.** For an unsigned type the
  wrapping is the definition -- SHA-256 *is* addition modulo 2^32 -- and for a
  signed one it is what the language has always done here. The division checks
  are per width, and the unsigned overflow branch is not emitted, because there
  is no pair of unsigned values whose quotient does not fit.
- **An integer literal is a `comptime_int`, and three places settle it early.**
  It gets a fresh variable and a `Constraint::IntLiteral`, so `0xff` is a `u8`
  here and a `u32` there. But "an integer literal" is not an answer to *which
  overload*, and a variable that survives to the end of a binding group is one
  some coercion elsewhere can bind to something stranger than a number. So a
  literal settles to its default at an overloaded call (`dispatched_call`),
  when coerced to anything that is not a *numeric* type (`coerce` -- which is
  what makes it *wrap* into a `?i64` rather than become one), and when it sits
  beside an equally undecided operand (`settle_literal_operand`). That last one
  must not fire when an abstract type owns the other side: that is a
  constrained generic, and settling it decides for the caller.
- **`f64` is one of the types a literal may become, and the node changes shape
  at monomorphisation.** `coerce` asks `wants_a_number`, not `as_int`, so the
  variable binds to `f64` and `x + 1` on a float is not an error. The HIR node
  is still `ExprKind::Int` at that point and must not reach code generation as
  one -- `iconst.f64` is a verifier error -- so `mono::rewrite_expr` turns it
  into an `ExprKind::Float` right after it substitutes the type. That is the
  only place the type is finally settled: a body annotated `Number` does not
  know which member it is compiled at until then. The value must be one the
  `f64` *is*, which `check_literal_is_exact_as_f64` tests by round trip rather
  than by range, because above 2^53 the integers are no longer all there.
- **The worker argument buffer is zeroed before it is written.** A value
  narrower than a machine word writes only part of one, and `rpc::pack` reads
  whole words and sends them to another thread. This was already true of `bool`
  and of every option tag and was simply never exercised; `narrow_worker.ws` is
  the case that would have caught it.
- **A builtin may write an array it did not allocate, and may not allocate
  one.** The first half follows from the rule below: a `[]u8` holds no
  references, so a `memcpy` into one is reading and writing bytes. The second
  is a fact about the code generator rather than about the collector --
  `array.new` is lowered inline because only the call site knows the element
  type, and so the stride and the type id to stamp, and a Rust function has no
  channel to learn either. So every entry point in `std/bytes` and every
  byte-oriented socket call is W# allocating and Rust filling. It is also why
  `net.read_into` reads into a caller's buffer rather than answering with a
  fresh array, which happens to be the shape a record layer wants anyway.
- **A library primitive allocates once per call, not once per block.** The
  whole case suite runs a second time under `--gc-stress`, which collects at
  *every* allocation, so a temporary inside a hash's or a cipher's block loop
  turns a test into a timeout. `std/hash`'s message schedule and working words
  live in the state and are made at `init`; a 64-byte digest and a 64 KiB one
  both cost six objects, which is checkable with `gc_live_objects()` and worth
  checking after touching one.
- **A builtin call is a stack walk under `--gc-stress`, so hoist `array.len`
  out of a loop.** `gc::checkpoint` runs at the top of *every* runtime entry
  point (`strings.rs` — `ws_array_len` is the one that bites) and validates
  every root the stack maps describe. That is the right thing for the collector
  and the wrong thing in a loop condition: `while (i < array.len(a))` walks the
  stack once per iteration. Read the length into a local, or take it as a
  parameter, and let the inner loops of anything numeric call nothing at all.
  A back-edge safepoint costs nothing here by contrast — `ws_gc_poll` does not
  collect under stress, only `on_allocation` does.
- **`std/map` hashes with a builtin, for the row above's reason.** A hash over a
  `str` written in W# would be one `str.byte_at` per byte and so one stack walk
  per byte under `--gc-stress`. `str.hash` is FNV-1a -- what `broker.rs` already
  picks a partition with -- followed by the SplitMix64 finaliser, because the
  table masks with `capacity - 1` and reads only the low bits, which FNV-1a
  leaves poorly distributed. Not keyed and not cryptographic: a table built from
  attacker-chosen keys can be made to collide, and `std/hash` is what to reach
  for when that matters. `set` and `get` hash once and hoist it out of the probe
  loop, and compare stored hashes before comparing keys.
- **A removed map entry stays reachable until its slot is reused**, exactly as
  `list.pop` leaves a dead reference in the tail and for the same reason:
  `m.keys[i] = null` only typechecks when the element type is an optional, and
  the collector walks every element the array header claims. The slot's `state`
  byte is what says it is dead. Known and documented, not a bug to fix by
  nulling.
- **A limb is 32 bits, because there is no 64x64 -> 128 product.** `std/bignum`,
  `std/nistec` and anything else doing multi-precision arithmetic hold 32-bit
  values and accumulate in a `u64`, which is what makes `t + a*b + carry` fit:
  `(2^32-1)^2 + 2*(2^32-1)` is exactly `2^64 - 1`. Widening a limb to 64 bits
  needs `bits.mulhi`, which does not exist. `std/curve25519` is the same rule
  one level down — sixteen 16-bit limbs in `i64`s, so a schoolbook product and
  its 38-fold stay near 2^41.
- **Field arithmetic takes its output and its scratch from the caller.** A
  scalar multiplication runs its ladder 255 or 256 times over ten-odd field
  operations, so a routine that allocated a temporary would be thousands of
  collections under `--gc-stress`. One `Work` struct per operation, holding
  every temporary, is the shape — and it is what lets `x25519` cost fourteen
  objects whatever the ladder does. Aliasing an output with an input is then
  free and is relied on everywhere: assemble into scratch and write the result
  out last.
- **A top-level `const` array is immortal data, and read-only.** It is emitted
  beside the string literals with `FLAG_IMMORTAL` (`codegen/src/lib.rs` --
  `define_arrays`), so it needs no roots and no startup initialiser, which is
  the whole reason it is allowed to be a literal at all: it holds no
  references. Its elements must therefore be scalars, and inference says so.
  Writing an element through the `const`'s own name is rejected, because a
  top-level `const` is shared by every worker and W# has no mutable globals; an
  alias defeats that check, which is why the data is emitted *writable* -- a
  read-only page would turn the mistake into a fault with no message.
- **A builtin may read and write bytes; anything that moves a *reference* from
  one object into another is written in W#.** This is why `std/array`,
  `std/str.split`, `std/net` and `std/http` are `.ws` files compiled with the
  program rather than rows in the builtin table. Generated code goes through the write barrier, the load
  barrier and the stack maps by construction; a Rust function has none of the
  three, and its arguments live in locals no stack map describes. `array.concat`
  was written in Rust first, and `--gc-stress` caught it: the memcpy'd
  references had never been through the load barrier, so they named objects in
  blocks about to be released.
- **A builtin that allocates is a safepoint.** The pause sites are no longer
  only the ones listed below: `ws_alloc` inside `str.concat` can run a whole
  collection. Such a builtin must copy what it needs into plain bytes *before*
  allocating and never hold a raw pointer across the allocator — which is what
  every function in `strings.rs` does, and why none of them touches a
  reference.
- **All allocation goes through `ws_alloc`, all field writes through
  `emit_store_field`, and every expression's result through `Trans::expr`.**
  These are the collector's three choke points: where objects are born, where
  the write barrier is emitted, and where heap pointers are declared as roots.
  Bypassing any of them loses objects or corrupts them, and the failure is
  neither immediate nor reproducible. Run `--gc-stress` if you touch them.
- **`Number` includes the unsigned types, so a generic over it may not
  negate.** Nor may it use a bit operator: the abstract type lists every numeric
  type including `f64`, and a body annotated with it must work for *every*
  member, because the caller picks. `Integer` is what such a body claims
  instead, and `Signed` -- the four signed integer widths -- is what a body that
  negates claims, which is why `math.abs` is one definition and not an overload
  set. (`%` was on this list until `f64` had one.) Abstract types are ordered by
  their member sets rather than by identity (`ty.rs::is_sub_ty`), which is what
  makes `Integer` the more specific of `Number` and `Integer` -- and which means
  two abstract types with *identical* members would be mutually more specific
  and silently lose an ambiguity error. A test asserts the table is a strict
  lattice.
- **A `Signed` body still may not hold a float literal, which is why `f64` is
  not a member.** `math.abs` compares against `0`, and an integer literal is
  checked against every *integer* member of the abstract type it is used at;
  `f64` in the set would make that check say nothing while the body still had to
  work there. The `f64` overloads sit beside the generic one, disjoint from it,
  so the dispatcher has nothing to call ambiguous.
- **A subtype's fields are its supertype's, followed by its own.** That is what
  lets a field read compiled against a supertype run unchanged on any subtype,
  with no adjustment and no vtable. `collect_structs` lays types out in lattice
  order for this reason, not source order.
- **Struct type ids are a preorder walk of the lattice.** Every type's subtypes
  occupy `type_id .. type_id + subtree_len`, which is what makes the
  dispatcher's subtype test one subtract and one compare. Numbering therefore
  runs *after* inference, because inference is what materialises the status
  types a program mentions. Nothing may read a `type_id` before then — dispatch
  tables carry `StructId`s and code generation resolves them.
- **Every W# function takes a leading environment pointer.** Static calls pass
  null. Forgetting it produces a signature mismatch that Cranelift reports far
  from the cause.
- **A return of more than `repr::MAX_RET_SLOTS` slots goes through a pointer,
  which is the second parameter.** x86-64 hands back two values and Cranelift
  refuses a signature asking for more rather than spilling one itself, so `!?T`
  -- three slots -- is written into space the caller provides. Immediately after
  the environment, so `env` stays `params[0]` in the dozen places that assume it,
  and an ordinary parameter rather than Cranelift's `StructReturn`, because W#
  owns both sides of every call. Three call paths have to agree: a static call,
  an indirect one through a closure, and a dispatched one -- where every case
  writes *one* shared area and the join block carries no block parameters at
  all. Every path out of a function goes through `Trans::emit_return` so the two
  forms cannot drift. The area is not a root and must not become one: the callee
  writes it in the instructions before its `return` and the caller reads it in
  the instructions after the call, with no safepoint between, which is the same
  argument the runtime boundary already made for `returns_by_pointer`.
- **A dispatched call must tell each surviving candidate what it is being called
  at, and must recompute its tests after the arguments settle.** `dispatched_call`
  trials candidates under a snapshot and rolls back, so one cannot pin an
  argument for the next; that also undoes the bindings a *generic* candidate
  needs, and where the argument is concrete they have to be made again -- safe
  exactly because a concrete argument has no variables of the caller's to bind.
  Without it `fn next[T](it: Iter[T]) ?T` called with an `Iter[Row]` answers
  `?T` with `T` open, and a `for` over a list of structs gets a loop variable
  whose type nothing decides. The second half is the dangerous one: `overlap`
  proves applicability with `try_unify`, which *always* succeeds against a
  variable, so an argument that is still open overlaps every candidate with no
  test -- and a most-specific case needing no test is compiled as a *static*
  call. That is a wrong overload with no diagnostic, so the tests are taken
  again from the settled types before static-versus-dynamic is decided.
- **Inference guarantees termination.** A non-`void` function whose body can
  fall through is rejected, which is also what lets code generation assume a
  fall-through means `return void`.
- **An object born in a collection is exempt from it.** A collection triggered
  by an allocation runs before the allocator has returned, so the new object is
  in no register, on no stack and in no stack map. It gets one cycle of grace
  (`Buffers::fresh`). Removing that frees objects the instant they are made.
- **Initialising stores take the write barrier too.** It is tempting to skip
  them — the object is new and its fields are null — but `ws_alloc` is a call,
  hence a safepoint, and a collection there clears the logged bit before the
  fields are written. Skipping the barrier loses those references entirely.
- **The collector's "is this mine?" test is the header, not the address.**
  `heap::is_collectable` (non-null and not `FLAG_IMMORTAL`) is what the
  marker, counting and the barrier use; `in_heap` is an address-range test
  for finding a block and its lines. Confusing them either decrements a string
  literal — a write to read-only memory — or forgets every large object.
- **A fresh object's fields read as null.** A hole is zeroed when an allocator
  takes it (`open_hole`), never assumed clean. The barrier's slow path and the
  marker both read fields of objects whose initialising stores have not run yet.
- **`types::for_each_ptr_offset` is the single definition of where an object's
  references are.** Six places need it -- the write barrier, the counting
  collector, the marker, the evacuation fix-up, the stress verifier and the
  birth-time "has references?" test -- and a seventh that grew its own loop
  would be a reference the collector cannot see. It covers the fixed
  `ptr_offsets` *and* an array's elements, which a fixed list cannot describe:
  the count belongs to the object, so the type carries a stride and the offsets
  within one element instead.
- **`ws_alloc` writes the element count itself.** It has to: `on_allocation`
  inside it is a safepoint that can run a whole collection, and until `aux`
  holds the count an array claims to be a bare header, so a heap walk would
  step into the middle of it.
- **The object-start bitmap is what makes the heap walkable.** Objects are not
  laid end to end -- a refilled hole puts new ones among the corpses of old
  ones, and an allocation buffer leaves an unused tail -- so a walk that
  stepped by each object's size would lose its place at the first gap, and
  losing its place means missing live objects. Set the bit *after* stamping
  the header, and clear it when freeing.
- **A block an allocator is holding is `Open`**, and that state is what keeps
  its unused tail unclaimed, keeps it from being recycled under the thread
  using it, and keeps a trace from choosing it to evacuate. That last one is
  why `on_allocation` is a safe place to pause: the object in flight is in
  such a block. It also means a buffer must be retired (`retire_local_buffer`)
  before the sweep can see into its block, which the pause that finishes
  marking does.
- **The mark bit is a parity.** "Marked" means the bit equals
  `header::mark_parity()`, which flips in a trace's initial pause. Nothing
  ever clears mark bits, and `stamp` writes the current parity, so an object
  allocated during a trace is born marked — which snapshot-at-the-beginning
  marking requires. Evacuation copies carry their header with them.
- **Nothing is freed while a trace is marking.** Counting keeps running, but
  its free loop is deferred (`Buffers::deferred_dead`) until the final pause.
  The marker reads any object it is handed, including one counting has found
  dead, because that object may be the only path the snapshot had to something
  live; so the marker never tests `FLAG_DEAD`, and no block is recycled and no
  large object deallocated underneath it.
- **The runtime has roots of its own, and they are a fourth list.** Generated
  code's roots are its stack slots and the maps say where they are; a runtime
  function that *builds* an object graph -- `transfer::decode` is the only one
  -- holds its half-built pieces in Rust locals instead, which the collector
  cannot see. Those go on `worker::PINNED`, thread-local beside the stack
  because only a mutator ever holds one. Adding a place a heap pointer can live
  means adding it to all four of `gc::collect`'s root set, the evacuation
  pause's root pass, `evacuate::fix_references` and `--gc-stress`'s verifier;
  this list is the first thing that had to.
- **A worker's service is called through generated trampolines, and its
  arguments cross as machine words.** The call site writes a buffer and reads
  another; one generated function per method reads the arguments back out and
  calls the real one. Both halves are generated code for the reason
  `array.concat` had to be: a reference moving from a buffer into a call goes
  through the write barrier, the load barrier and the stack maps by
  construction there, and a hand-written Rust caller would have none of the
  three. The runtime is left with bytes, which is what it may touch. A worker's
  state is pinned on the runtime root list for its whole life, because nothing
  on its stack holds it between calls.
- **A broker message must be an object, not merely transferable.** An object
  carries its type id in its header, and that id is both what lets the copy be
  made without knowing the type and what makes the decoded value dispatchable
  on the other side -- which is how a subscriber set is an overload set and
  needs no broker-side machinery at all. `BuiltinTy::Message` says so, and the
  demand travels from the builtin through `std/broker`'s generic wrappers to
  each use exactly as an abstract type's does.
- **A value crosses to another worker as bytes, never as a pointer.**
  `transfer::encode` flattens the graph reachable from an object into plain
  memory with each reference replaced by an index, and `transfer::decode`
  builds it again in the receiving heap. Allocating into another worker's heap
  would need its lock, its allocation buffer and its mark parity, and the
  object would be judged by a collector that never saw it born. `decode` runs
  in two passes on purpose: the first allocates with every pointer slot left
  null, because an allocation is a safepoint and a collection between two of
  them would otherwise read a slot holding an index; the second writes the
  pointers and allocates nothing, so nothing can move underneath it.
- **Collector state belongs to a worker, not to the process.** `heap`,
  `buffers`, the phase machine, the mark parity and the statistics all live on
  `worker::Worker`, reached through a thread-local pointer; a collector thread
  installs its worker's on entry, so a copy it makes while evacuating lands in
  the heap the original came from. Three things stay process-wide on purpose,
  and each for a reason that does not generalise: the type registry and the
  stack maps (frozen before any code runs), the space directory (the load
  barrier asks it about an arbitrary address), and the two flag words generated
  code reads (their addresses are `iconst`-baked into the code). Those two mean
  "*some* worker wants a pause" and "*some* worker is moving"; the slow path
  asks the current worker whether the request is its own, and returns at once
  when it is not.
- **All three pauses run on the mutator thread**, inside a runtime call at a
  safepoint, because only the mutator can walk its own stack. The pause sites
  are `ws_gc_poll`, `on_allocation`, the `gc_trace*` builtins and exit — and so,
  transitively, any builtin that allocates, since `ws_alloc` is where
  `on_allocation` lives. Never one that does not: `print` holds its argument in
  a Rust local no stack map describes, and pausing there would leave it stale.
  `on_allocation` is safe because the object in flight is in the open block,
  which is never an evacuation candidate, and on no list, so nothing judges it.
  The one exception is a worker parked in a safe region, next.
- **A blocking call runs inside a safe region**, `worker::blocking`, and that is
  the only thing a mutator may block in. A thread sitting in `read(2)` cannot
  answer a pause request, so its trace would wait for the disk — and with
  workers, one worker's heap would wait on another's slow client. Before
  blocking it publishes the frame pointer its stack starts at and says it is
  parked; the collector then walks that frozen chain and runs the pause itself.
  Leaving the region is a compare-exchange rather than a store, because
  resuming while the collector is still reading the stack is the one race the
  handshake exists for.
- **A safe region is the same bargain as an allocation.** Everything the
  blocking call needs must be copied into plain bytes first, and nothing on the
  heap may be touched inside it — for exactly the reason an allocating builtin
  must copy first: a Rust local is described by no stack map, so a reference
  held in one is invisible to the collection that runs while the thread is
  parked. A worker also gives its allocation buffer back and publishes its
  counters on the way in, because those live on the *thread* rather than in the
  worker: a collector running the pause would otherwise retire its own buffer
  and leave the mutator's block open, and a block an allocator holds is never
  swept, recycled or evacuated.
- **A parked worker's stack is a root set like any other**, and
  `worker::walk_worker_roots` is the single door every root walk now goes
  through, because "whose stack, and where does it start" is precisely what a
  safe region changes the answer to. The runtime's own pinned roots are the one
  thing that does not travel: they sit on the thread, so a collector declines a
  parked worker whose `pinned_depth` is not zero and waits for it instead, as
  everything did before safe regions existed.
- **A root walk has two halves, and only the first one has arms.** *Reaching*
  generated code means crossing the handful of Rust frames between the collector
  and the runtime function generated code called into: that is
  `stackwalk::innermost_generated_frame`, and it is per-platform, because the
  SysV arms may follow `rbp` and Windows may not (see the platform table).
  *Walking* generated code is then `walk_generated`, which is `rbp` everywhere,
  because Cranelift's prologue is `push rbp; mov rbp, rsp` whatever the calling
  convention -- it ignores the call conv entirely, which `isa/x64/abi.rs` says
  out loud. So the half that reads stack maps has no arms and cannot drift.
  Two things follow. **A parked worker records the generated frame itself**, in
  `worker::blocking`, because crossing the Rust frames needs that thread's own
  frame pointers or its own registers and a collector has neither -- what it is
  handed is the far side of the crossing, which any thread may walk. And
  **nothing outside a confirmed generated frame is ever dereferenced**, which is
  what keeps a broken chain a stopped walk rather than a wild read; the old walk
  kept climbing instead, and that is exactly how it ran off the end of a Windows
  stack. A `None` from the first half is an ordinary answer -- a runtime thread,
  or a mutator that has not entered W# yet -- not a failure.
- **A forwarded header is an address, not flags.** Anything that reads a flag,
  a count, a size or a type id from an object in a block being emptied must
  test forwarding first -- `evacuate::forward`, `heap::evacuate_block` and
  `note_evacuated_blocks` all do. An object is accounted for at the moment it
  is forwarded, because afterwards nothing can ask how big it was.
- **The load barrier is what makes moving objects safe while the program
  runs.** Every reference loaded out of a heap object goes through
  `emit_load_barrier`, so nothing the program holds is ever in a block being
  emptied. Anything that starts reading a reference by some other route --
  a new instruction, a runtime function reaching into a field -- has to
  resolve it too, or a write through it will be lost when the block goes.
  `ws_log_object` is the existing example: it reads fields directly, so it
  resolves them while objects are moving.
- **Reference counting stands aside while objects move**, because a header may
  be a forwarding word rather than a count. `on_allocation` and the
  `gc_collect` builtin both return early; the window is one evacuation long.
- **The evacuation pause fixes a list, not the heap**: the slots the marker
  saw pointing into a block being emptied, plus the objects the trace touched
  afterwards (everything the write barrier logged, everything allocated while
  the trace ran, and every copy), plus the roots and the collector's own
  lists. `evacuate::fix_references` argues why that is all of them, and
  `--gc-stress` walks the whole heap afterwards to check. Anything that adds a
  place a heap pointer can be stored must be added to that list *and* to the
  verifier.
- **The evacuation pause must test forwarding before reading a header.** This
  is the same rule `evacuate::forward`, `heap::evacuate_block` and
  `note_evacuated_blocks` already followed, and `evacuate::fix_fields` was the
  one place that did not: a logged object in a block being emptied gets
  forwarded while the program runs, and a forwarding header's low 32 bits are
  part of an *address*, which can easily name a real type id. Scanning one then
  walks a stranger's bytes with somebody else's layout. The copy is on the same
  list, and the copy is what needs fixing.
- **Counting's frees wait until the trace's *last* pause, not its second.**
  The evacuation pause reads two lists recorded during the mark -- the slots
  the marker saw pointing into a block being emptied, and the objects the trace
  touched afterwards -- so an object freed between the two pauses will have had
  its space taken by something else by the time the second one gets there, and
  the pause then walks a stranger's bytes with a dead object's layout. So
  `finish_marking` arms `evacuating` *before* it calls `gc::collect`, and
  `defer_frees` is `tracing() || evacuating()`. `fix_references` visiting
  `deferred_dead` is what that was always written for. Under `--gc-stress`,
  `verify_nothing_scanned_is_dead` says so out loud; `gc_evacuation_lists.ws`
  is what gives it something to fire on, and needs a few hundred thousand
  short-lived nodes to do it.
- **Counting after the final pause never names an unmarked object.** Its
  buffers were drained in the pause and the nursery was filtered by mark, so
  the concurrent sweep, which frees exactly the unmarked, cannot free anything
  counting will touch. `Heap::free` is idempotent (it claims `FLAG_DEAD`) for
  the benign case where both reach the same object anyway.
- **Locks never nest.** `Heap::*` never touches the counting buffers; the
  marker takes only the buffers lock (to drain `satb`) and answers its heap
  questions from the lock-free directory; the sweeper takes only the heap lock,
  per block; the phase is an atomic so safepoint checks take no lock at all.
- **A generic struct stands outside the dispatch lattice.** Type ids are a
  preorder walk of it, fixed before monomorphisation, and a generic struct's
  instantiations are not known until after. So `struct[T] : Base` is rejected,
  and an instantiation takes an id from a block above the lattice, where it can
  disturb no range test. Its field offsets are computed by code generation
  rather than inference, because a `?T` field is two slots or three depending
  on what `T` is.
- **A container written in W# bounds-checks itself.** `std/list` indexes its
  backing array, whose header length is the *capacity*, so `a[i]`'s own check
  would let a read of a spare slot through. `get`, `set`, `pop`, `insert` and
  `remove` compare against `count` first and call the prelude's `panic_index`,
  which is the same entry point generated code calls and gives the same
  message. A new list operation that indexes `items` and forgets this hands
  back a spare slot -- a zero, or a null reference -- instead of reporting the
  mistake.
- **A git object id is SHA-1 and a package's integrity is not.** `std/hash`
  carries SHA-1 because git names every object by one; nothing in `std/tls` or
  `std/x509` uses it, and a fetched tree's own key is `ingot/store`'s SHA-256.
  Saying that in the module header is cheaper than being asked.
- **`std/inflate` answers with a cursor, not with bytes.** A packfile is a
  concatenation of zlib streams with nothing between them, so only the
  decompressor knows where one ends. A `decompress(bytes) -> bytes` would be
  the obvious shape and useless for the one caller there is. The Huffman tables
  live in the reader and are built once, because a block that allocated would
  be a collection per block under `--gc-stress`.
- **A packfile has two varints and they are different.** A size is the ordinary
  seven-bits-at-a-time little-endian form; an offset delta's backreference
  accumulates `((n + 1) << 7) | next`, which makes each number's encoding
  unique. Reading one with the other's loop gives a plausible wrong answer and
  is the classic bug. A copy instruction with a size of zero means 65536.
- **Git says no out of band.** `ERR <message>` is a plain pkt-line and can
  arrive anywhere a line can, including before the side bands exist. A client
  that only looked at band 3 reads a refusal as an unknown section and reports
  an empty answer.
- **A version set is intervals, not a predicate.** `ingot/semver`'s `Range` is
  a sorted, disjoint, non-adjacent list of intervals with inclusive or
  exclusive ends, because PubGrub takes *complements* constantly and a
  predicate cannot be complemented into something you can then ask for the best
  version of. The ends carry inclusivity rather than being half-open because a
  version has no successor: there are infinitely many pre-releases between
  `1.0.0` and `1.0.1`. Touching intervals are run together by `normalise`, so
  two spellings of one set are one set and equality is a walk.
- **A PubGrub term is not its allowed set.** "foo is not in A" is satisfied by
  foo being *absent*, which no set of versions says -- so `relates` has four
  cases and not one piece of set arithmetic. Collapsing a negative term to its
  complement makes `not foo any-version`, which is what every dependency starts
  life as, look like a term that can never hold, and the solver then decides
  nothing at all. Two more rules that are load-bearing rather than tidy: an
  incompatibility **merges terms about the same package** (resolution routinely
  produces `{not foo ^1.0.0, foo 2.0.0}`, which merged is the clause that
  rules something out and unmerged is a search that never terminates), and the
  difference taken during resolution is **satisfier ∖ term**, not the reverse.
- **A pre-release is not a candidate unless it was asked for**, and that is a
  policy in `pubgrub.choose` rather than a rule in the set algebra. The algebra
  stays honest that `1.1.0-rc.1` really is below `1.1.0` and really is inside
  `^1.0.0`; `semver.mentions_prerelease` is what the policy asks.
- **`std/list` keeps a dead reference in its tail.** `pop` and `remove`
  decrement the count and leave the vacated slot alone, because
  `l.items[i] = null` only typechecks when `T` is an optional. The collector
  walks every element the header claims, so that object stays alive until the
  slot is reused. This is known and documented, not a bug to fix by nulling.
- **A module's names are stored qualified in one flat table.** An unqualified
  lookup tries the current module and then the prelude, and what a module
  cannot see is simply what it has no key for. A local binding shadows an
  imported module, so adding an import cannot break code that already used the
  name.
- **A re-export is a second key, not a second thing.** `pub const T = other.T;`
  puts one more entry in `struct_ids`, and `pub const f = other.f;` one more in
  `globals`, both holding the *same* `StructId` or `GlobalRef` -- so the alias
  and the original unify, dispatch and lay out identically because they are one
  type and one function set, and nothing below sema knows it happened. Two
  passes, because a type must be aliased before struct fields are laid out
  (`alias_reexported_types`, inside `collect_structs`) and a value only after
  every module's globals exist (`alias_reexported_values`); each runs to a
  fixpoint so a facade may re-export from a facade. There is no case for a
  cycle on purpose: such a `const` is written in a module that imported the
  other, so a cycle among re-exports is a cycle among imports and the loader
  refused it first. One thing has to be told: `is_alias` decides that a key is
  a second name for an overload set by comparing *bare* names, which a
  re-export defeats -- `pkg.parse` really is called `parse` -- so
  `check_overloads` asks the `reexported` key set instead and every overload
  diagnostic is said once.
- **Two re-exports of one name merge; a re-export beside a declaration does
  not.** A facade presenting a package of several files may find `render` an
  overload set in two of them, and "only functions may share a name, as an
  overload set" is exactly what a merge is -- so `alias_reexported_values`
  unions the id lists rather than reporting a redeclaration. Only when the name
  this module already has is *itself* a re-export (`reexported` is the test),
  and only without an annotation, which picks one member and so has nothing to
  merge with. A re-export beside a declaration of the module's own is a module
  shadowing its own name with a foreign one, stays a clash, and
  `err_reexport_clash.ws` is the case that says so. The first line to bind the
  name keeps the span, so a later clash still points at where it came from.
- **The `for` protocol's dependency edges are the whole program's.** `iter` and
  `next` resolve in the module that declares the subject's type, which
  inference has not run yet to know, so `infer_all` over-approximates. It must
  over-approximate across *every* module, not the ones this one imports: a
  subject's type arrives through functions that forward it and facades that
  re-export it, and an edge that is missing leaves the iterator ungeneralised
  and reports `no overload of iter accepts (T)` about a `T` that is right
  there. Sound for the reason the narrower version was: an edge only matters
  when it closes a cycle, and an iterator does not call back into its user.
- **A conversion inside an abstract-typed body must not settle the parameter.**
  A parameter annotated `Integer` is a fresh variable carrying
  `Constraint::Member`, and `infer_convert`'s "an unannotated argument would
  otherwise stay a variable for ever" default used to unify it with `i64` --
  the one defaulting site in `infer.rs` without the guard
  `settle_literal_operand`, the `IntLiteral` pre-pass and the `Numeric` arm all
  carry. `generalize_definition` then could not quantify it, so the function was
  monomorphic `fn(i64)` while `fn_decl_params` still said `Integer`. With one
  overload that surfaced as a bogus mismatch at the call; with two it got
  through, because `overlap`'s abstract branch kept the case on the strength of
  the *declared* type and **threw away the failed `try_unify`** -- `Some(None)`
  means "always applies, no test needed", which is compiled as a static call, so
  a `u8` reached a body Cranelift had compiled for `i64`. Both halves are fixed:
  the conversion records a `Numeric` constraint instead of defaulting, and
  `overlap` honours the unification. **Overload resolution must never be able to
  produce IR that fails verification**, so the second fix stays even though the
  first makes this call resolve correctly.
- **`std/bytes` imports `std/str`, so `std/str` may not import `std/bytes`.**
  The loader refuses the cycle, and that is why `join` and `repeat` were
  `concat` in a loop -- which copies the accumulator, so joining 150,000 pieces
  into 600 KB took 9.4 seconds. They assemble into one `array.new` buffer now,
  through `str.raw_into`/`str.raw_from`: two builtin rows that are
  `bytes.raw_from_str`/`bytes.raw_to_str` under a second name, because a `link`
  is a symbol and no two rows may claim one. A one-line forwarder each, rather
  than a second implementation.
- **`==` on a struct is generated per type, and it recurses.** `lower.rs`
  compiles it into a call for the reason `==` on a `str` already is -- nothing
  in inference synthesises a call -- and into a *function* rather than an inline
  sequence because a type that reaches itself would otherwise expand for ever.
  Two per concrete type (`crate::equality`): a dispatching entry point that
  answers identity, nulls and unequal type ids and then picks by the runtime
  type id within the declared type's subtree, and an exact one that compares
  that type's fields. The subtree dispatch is what makes two `Sub`s compared at
  `Base` compare `Sub`'s fields rather than only the part `Base` declares.
  Built through a `Trans` with a placeholder `FuncDef` so the field reads go
  through the *same* `load_at`, and therefore the same load barrier and the same
  rooting, as every other field read. Both parameters are declared stack-map
  roots: they are live across `ws_str_eq` and across the recursive calls, and
  both are safepoints. **A value that reaches itself recurses for ever**, which
  is what derived structural equality does everywhere it exists and is said out
  loud rather than papered over.
- **`@spawn` returns before `init` starts, and the message loop is entered only
  after `init` returns.** Both were implementation details of `ws_spawn` and are
  now promises the README states, because the exit path depends on the second:
  a worker whose `init` never returns has never dequeued anything, so it can
  never see the `Stop` that `stop_all` sends. Joining one waits for ever, and
  did. `Handle::serving` is set immediately before the loop, and `stop_all`
  joins the workers that reached it and abandons the ones that could not --
  a distinction rather than a timeout, so an ordinary program's exit stays
  deterministic and a worker in the middle of a method still finishes. **`main`
  returning ends the process.** `@join` on a worker that never returns still
  blocks, because that is a program waiting on its own worker rather than the
  exit path.
- **The argument pins come off before generated code is entered.** `unpack`
  pins each decoded argument because each `decode` allocates and the ones
  already decoded sit in a `Vec` no stack map describes -- and `worker_main`
  used to hold them for the whole of `init`. A blocking call inside `init` then
  entered a safe region with `pinned_depth` non-zero, which a collector responds
  to by declining the parked worker and waiting for the syscall: the regression
  safe regions exist to remove, and an abort in any build with debug assertions
  on. Every acceptor is exactly that shape. `unpack` returning is the last
  moment anything can allocate before the callee's prologue stores its
  parameters into slots its own stack maps describe, so there is no safepoint in
  the window -- the same argument the return area and the runtime boundary
  already make. `ws_rpc_call`'s method path is the same fix for the same reason.
- **`--emit=api` is the only emit with a promise attached.** It is versioned,
  it is printed after type checking so it only ever describes a program the
  compiler accepted, and every name a program defines is absolute in it. A
  golden test in `crates/wsharp-cli/tests/api.rs` pins the whole output for a
  two-module program, so a format change fails there rather than quietly in
  somebody's generator, and a change that could make an existing reader wrong is
  a change to `api::VERSION` as well. `--emit=ast` is deliberately not this: it
  is a debugging aid, shared with the parser tests, free to change. `--emit=obj`
  is `build`'s alone and is now refused elsewhere -- it used to fall through
  every branch in `drive` and *run* the program.
- **`Span` is two `u32`s with no file in it.** Files are laid end to end in one
  offset space and a span's file is the range it falls in (`diag::SourceMap`);
  the first starts at offset 1, which keeps 0 meaning `Span::EMPTY`. Widening
  the span would touch every node in the syntax tree to carry a number only the
  renderer reads.
- **A generic `fn` literal is a definition, not a value.** A closure value is
  one code pointer and two instantiations need two, so a `const` bound to one
  binds a name -- `Binding::Definition`, beside `Binding::Overloads` -- and
  each *use* materialises the closure. Its captures are snapshotted into hidden
  locals at the binding, because each use builds its own environment and a
  captured `var` assigned in between would otherwise change what the closure
  sees. A literal in expression position is still a value and still
  monomorphic; so is one bound to a `var`, or to an annotated `const`.
- **A definition does not generalise over a variable a constraint owns.**
  `solve_constraints` runs once per binding group, so `Numeric` defaults
  `fn add(a, b) { return a + b; }` to `i64` before anything is quantified. A
  `fn` literal closes its level at its own binding, *before* that runs, so
  `generalize_definition` subtracts every variable the pending constraints
  still mention. Dropping that makes `const add = fn (a, b) { return a + b; };`
  mean something different from the declaration spelling the same body.
- **`return f(x)` unifies two error sets; `try f(x); return;` widens one.** A
  tail call in a `!void` function makes the caller's set *equal* the callee's,
  because the whole type unifies -- so a later `try` of something that raises
  more is rejected with "this function cannot raise", pointing at the second
  call rather than at the first. `try` alone contributes a subset edge, which
  is what a caller of several fallible things wants. `std/tls`'s two message
  dispatchers are written that way for exactly this reason, and the shape is
  worth recognising: the diagnostic names the wrong line.
- **A coercion is a sequence of steps, and the wrapping step recurses.**
  `coerce` is a reporting wrapper around `try_coerce`, whose `?T`/`!T` arm fits
  the value to the payload *by every rule including itself* -- so `Sub` reaches
  `!Base` by widening and then wrapping, and `Row` reaches `!?Row` by wrapping
  twice. Each of those used to be a type error, and the spelling was an
  annotated binding per conversion. Speculation is why the split exists: a
  failed attempt rolls back both the bindings and the constraints it made, so
  the diagnostic the outermost call reports is still about the type the caller
  wrote rather than about a payload nobody mentioned. Anything added to
  `try_coerce` has to be safe to run and undo.
- **An `if` whose arms both leave has no merge block.** Code generation creates
  one per `if` and used to switch to it unconditionally; the function epilogue
  then closes whatever block is open with a valueless `return`, which is right
  "only for a `void` function -- inference rejects anything else". But
  inference rejects a function that can *fall through*, and one whose arms both
  return cannot: the merge was unreachable rather than void, and the epilogue
  gave a `str`-returning function a `return` with no value. `lower.rs` now
  switches to the merge only when an arm can reach it. A block that is created
  and never switched to is never added to the layout, so the unused one costs
  nothing.
- **A dispatched call has one type, so every overload must share its return
  type -- error set included.** That is what stopped `https://` from being a
  `TlsConn : Conn` subtype with `conn_read` and `conn_write` as overloads,
  which is otherwise exactly the shape this language is for: reading through
  TLS can raise everything a handshake can and reading a socket cannot, so the
  two would have to write the same two-dozen-name set out twice. An optional
  field and an `if` keep every error intact. The lattice is right when the
  members agree about failure and wrong when they do not.
- **One curve implementation, two curves.** `std/nistec` carries the limb
  count, the coordinate size and the scalar width in its `Curve`, so P-256 and
  P-384 are the same code over different tables. That is only possible because
  the arithmetic is `std/bignum`'s generic Montgomery multiplication rather
  than a fast reduction written for one prime -- the trade stage two made, and
  what made P-384 a table of constants when thirty-five root certificates
  turned out to need it. A third curve is the same again; P-521 is not, because
  its 521 bits are not a whole number of 32-bit limbs.
- **A trust store is a bag; a chain is a structure.** A certificate in the
  store this library cannot read is dropped and the rest are used; one *in a
  chain* is a refusal. Answering both the same way either makes a machine with
  one odd root unusable or makes a broken chain acceptable. `parse_all` and
  `pem_certificates` take the first rule, `verify_chain` the second.
- **`std/x509` is below `std/tls`, and the arrow cannot be reversed.** A TLS
  client verifies a CertificateVerify with a key out of a certificate, so one
  module must name the other's types and W# has no re-export. `SigKey` and
  `verify_signature` live where a public key comes from, and `std/tls` imports
  them.
- **A builtin that answers with a list of strings answers with one blob.** A
  builtin may not allocate an array, so `fs.raw_read_dir` and `os.raw_args`
  hand back a `str` of four-byte big-endian lengths and their bytes, and
  `std/os.unpack` cuts it up in W#. Length prefixes rather than a separator
  byte, so the encoding says nothing about what a name may contain and an empty
  list is an empty blob rather than a case to special-case. `crypto.raw_system_roots`
  and `std/x509.split_blob` are the original of the shape.
- **The command line is process-wide state, and that is allowed.** `main` takes
  no arguments and the one word a compiled `main` receives is the closure
  environment pointer, so the arguments arrive out of band: `os::set_args`
  publishes them into a `OnceLock` before anything is compiled. It qualifies
  for the same exemption the type registry and the stack maps do -- frozen
  before any generated code runs, never written again.
- **A library module is read only if something imports it.** Every `.ws` module
  in `std/` and `ingot/` used to be parsed and inferred for every program,
  which was free at four files and was costing a ten-line program most of its
  compile time at twenty-five: `wsharp check examples/fib.ws` went from 0.62s
  to 0.005s when `load::add_library` started following the root's imports
  instead. `@import("std")` still means all of `std/`, because a module path is
  a prefix and any of them can be walked into from there. The consequence to
  remember is that **a library module nothing imports is never checked**, so a
  new one needs a case that imports it or it is not compiled at all.
- **A value with several shapes is a lattice, not a tagged struct.** W# has no
  sum types and does have nominal subtyping with multiple dispatch, so the
  shape is an empty supertype, one subtype per case carrying its payload, and
  an overload set whose base case is the failure -- `as_int(v: Value)` returning
  `error.BadFormat` beside `as_int(v: Int)` returning the number. `std/x509`'s
  `SigKey` is the original and `std/toml`'s `Value` is the second. Every
  overload must state the same error set, because a dispatched call has one
  type; and a subtype only coerces into its supertype at an *annotated*
  binding, which is why each constructor is `const v: Value = Int{ .. };
  return v;` rather than a bare return.
- **A parser answers, it does not raise.** An error union carries a tag and
  nothing else, and `BadFormat` is not a thing to hand somebody holding a
  200-line manifest. `std/toml.parse` answers with a `Doc` that is either a
  table or a message and a line, and everything under `ingot`'s verbs takes a
  `Fault` and writes into it. The first failure wins in both: a recursive
  descent reader that has lost its place invents the rest.
- **A registry is a directory, and fetching one over git is only how the
  directory arrives.** `registry.open` takes a path; `INGOT_REGISTRY` naming an
  existing directory is used where it lies, with no certificate store read and
  no socket opened. That is what makes a private registry, an offline checkout
  and the whole of `tests/cases/ingot_registry.ws` the same case as the public
  one -- the git client speaks HTTP, and no case in this suite may stand up a
  server. `crates/wsharp-cli/tests/verbs.rs` therefore points `INGOT_REGISTRY`
  at a directory on **every** invocation, whether or not the test uses a
  registry: the variable falls back to the public URL, so a test that left it
  unset resolves against the real Foundry over the real network.
- **A published version is never edited, and the fetch memo is why.**
  `store.remembered` maps a source string to a tree digest and never
  invalidates it, so `reg+acme/json@1.2.0` must name one tree for ever. The
  registry's CI enforces it; a mistake is a new version, and a withdrawal is
  `yanked = true`, which is filtered out of *selection* and stays readable
  because a lockfile that already names it must still install. An index, by
  contrast, does change -- which is why `store.index_at`/`remember_index` is a
  separate pointer file rather than a second key in the memo, and why `ingot
  update` exists at all.
- **A registry release carries its tree hash, so resolving fetches nothing.** A
  git dependency must be fetched during `resolve` because only the fetched tree
  holds the manifest saying what it depends on; a registry entry *is* that data
  and also names the store key. So `Source.tree` non-empty means "do not hash a
  directory, this is the answer", and `install`'s existing refusal of a digest
  that is not the lockfile's becomes the integrity check. Anything that makes a
  registry entry's tree not the tree that gets installed breaks that silently.
- **A path or git dependency overrides the registry for the name it supplies.**
  `plan.from_registry` skips a name `locally` already answers, or the solver
  would be offered published versions of a package somebody is editing beside
  their project and could choose one.
- **`registry.package` answers null two ways, and the caller must tell them
  apart.** Null with `f.ok` still true means the registry does not hold it --
  the caller writes that sentence, because which package and what asked for it
  is known one level up. Null with a fault means the entry is there and
  unreadable, and that message is already better. Getting the test backwards
  produces *no* message rather than a wrong one, because `fault.fail` keeps the
  first failure; that shipped once and `ingot add acme/nope` exited 4 in
  silence.
- **The store's tree hash is defined in `ingot/store.ws` and nowhere else.** A
  key two versions of ingot compute differently is a store that silently splits
  in half, so the definition is written out: SHA-256 over each entry sorted by
  name as bytes, `"f" name 0 <decimal size> 0 <contents>` for a file and
  `"d" name 0 <hex of the subtree's hash> 0` for a directory. The sort is half
  of that definition, which is why it lives beside the hash rather than in
  `std/array`. Permissions and timestamps are deliberately not in it.
- **An install is a `rename`, and a damaged entry is repaired rather than
  believed.** W# has no `defer`, so a fetch builds under `tmp/` and moves into
  place in one step: an interrupted install leaves rubbish rather than half a
  package. `store.install` tests the entry with `check` rather than testing
  that the directory exists -- getting that wrong made `install` a no-op on a
  damaged store, which is exactly the case `verify` exists to distinguish.
- **The path separator is `/` on every platform, including Windows.** Every
  Win32 path call accepts one, `sys/windows.rs` appends its listing wildcard to
  one, and one spelling is what keeps a lockfile written on one machine
  readable on another. `std/path.normalise` turns a `\` that arrives from
  outside into one; nothing here ever produces one. `os.cwd` and `os.home`
  answer with what the system said and normalise nothing, because a path is
  arithmetic and the environment is a fact about the process -- `std/os` does
  not import `std/path`, and every caller of either normalises.
- **`ingot.env` is derived, absolute and read by the compiler.** `install`
  writes it beside the lockfile; `Loader::follow`'s third rule is the only
  thing that reads it. Not TOML, and that is the decision: the loader is Rust
  and every reader this project owns is W#, so a lockfile the compiler parsed
  would be a second TOML implementation kept in step with `std/toml` for ever
  -- and a whole one, since a package's own `ingot.toml` is a file a person
  wrote. One line per package, tab-separated: name, directory, facade, and then
  one field per dependency, so the file has one separator and a package name is
  whatever a name is. Unreadable and malformed get the same answer, `run ingot
  install`, because writing it again is the fix for both. **`resolve` removes
  it**: a new resolution names new store entries and the old ones are still
  there holding what they always did, so an environment left behind would build
  the previous version of a dependency and say nothing. `add` and `remove` do
  not, because the environment they leave is still a true statement about what
  was installed -- it is `resolve` that makes one false.
- **A package may import only what its own manifest asked for.** The lockfile
  is the whole project's, because the solver chooses one version per package
  for the project -- so without the scope check in `follow_package` a manifest
  would describe what gets fetched rather than what may be named, and an
  undeclared dependency would work until something else stopped needing it. The
  owner of a file is the entry whose directory is the *longest* prefix of it:
  longest, because `WSHARP_HOME` may sit inside the project, which is where
  `crates/wsharp-cli/tests/verbs.rs` puts it.
- **A package presents one file.** `Manifest.root` is the facade, and nothing
  outside can name any other file in the tree -- `@import("util/inside")` is
  not a spelling. What a package of several files shows is what its facade
  re-exports, which is the same rule `pub` sets one level down: a surface is
  stated rather than leaked.
- **The closure environment is dead after the prologue.** Captures are copied
  into declared locals before the first safepoint and `env` is never read
  again, so it is not a root and need not be. Re-reading it after a call would
  be a use-after-move.
- **Generated code names runtime state by symbol, never by address.** The two
  flag bytes the barriers read -- `ws_gc_poll_flag` and
  `ws_gc_evacuating_flag` -- are exported statics declared as imported data and
  reached with `symbol_value`, exactly as a string literal is. They used to be
  `iconst`s of `&POLL_FLAG`, which is correct in a JIT and meaningless in a
  file some other process will load. They are the *only* runtime state a
  compiled program reaches without a call, which is why their names are
  constants both halves read rather than strings written twice.
- **A builtin takes a list of strings as one blob, for the reason it answers
  with one.** `os.exec` could have taken a `[]str` and read the elements in
  Rust; that would be reading *references* by a route the load barrier does not
  cover, and one the collector had already moved would be a stale pointer
  handed to `execvp`. So `std/os.pack` builds the same four-byte big-endian
  framing `raw_args` and `raw_read_dir` answer with, in W#, where the barrier
  applies by construction -- and the builtin is left with bytes. The rule is
  the one it always was, read in the other direction.
- **`ingot` is a W# program, and `wsharp` is what compiles it.** Its Rust
  driver is gone: `-C` is `os.chdir`, the verb list is `ingot/main.ws`'s, and
  `ingot run` *becomes* `wsharp run` through `os.exec` rather than embedding a
  compiler it cannot have. So `cargo build` no longer produces `ingot`, a
  release bootstraps it, and the compiler is looked for beside the binary
  before `PATH`.

## Testing

```sh
nix-shell --run "cargo test --workspace"
```

- Unit tests live next to the code they cover.
- A **test hook** is a `pub` function that exists so a test can reach a
  primitive a whole operation would hide, and it says so in its doc comment.
  `curve25519.field_mul`, `cipher.aes_sub_byte` and `tls.client_replay` are the
  three; the last one sends a ClientHello it was handed, because RFC 8448's
  recorded handshakes cannot be replayed against a hello this library would
  build.
- `crates/wsharp-sema/tests/` holds inference and monomorphisation tests, which
  assert on rendered signatures (`fn(T) T`) — far more readable than matching
  nested enums.
- **End-to-end tests are data.** Adding one means adding a `.ws` file to
  `tests/cases/` with its expectations in a header comment:

  ```zig
  // expect: 55          one line of expected stdout, in order
  // exit: 3             expected exit status (default 0)
  // error: <substring>  must fail to compile, saying this
  // panic: <substring>  must die with a W# panic saying this
  ```

  `// args: one two` is what the program sees as `os.args()`; it splits on
  whitespace, so an argument containing one cannot yet be written.

  The harness (`crates/wsharp-cli/tests/cases.rs`) runs the built binary as a
  subprocess, so stdout is captured for free and the test does exactly what a
  user would.
- **`ingot`'s libraries are tested as W# cases; its verbs are tested as a
  binary.** `ingot/*` is a library namespace beside `std/*`, so
  `tests/cases/ingot_*.ws` reaches the store and the manifest reader directly
  and gets the `--gc-stress` pass for free. What a case cannot reach is the
  tool -- a verb reads a directory, writes two files and answers with an exit
  status -- so `crates/wsharp-cli/tests/verbs.rs` builds one with `wsharp build
  --module ingot/main` and drives it with
  `-C <dir>` and `WSHARP_HOME` pointed inside a temporary directory.
- **A case cannot expect leading *or* trailing whitespace.**
  `parse_expectations` trims each header line, so a tab-separated row with an
  empty last field has to print something -- `-` is what `ingot_manifest.ws`
  uses -- and an indented line cannot be expected at all, which is why
  `packfile.ws` writes `- ` rather than two spaces.
- **The git client is recorded against a real `git upload-pack`.** The requests
  in `tests/cases/modules/gitfixture.ws` are the bytes `ingot/git` generates,
  and the answers are what `git upload-pack --stateless-rpc` said when it was
  handed them -- the same program `git http-backend` puts behind the HTTP
  endpoint. That checks the half a recorded transcript cannot: whether a real
  server *accepts* what this client sends. The packfiles in
  `packfixture.ws` are what `git repack` wrote, one with offset deltas and one
  with reference deltas, because only the first is common and both must work.
- **A case that touches the filesystem builds its own directory and removes
  it.** `os.temp_dir()` says where, and the name carries `crypto.random` bytes,
  because the suite runs a second time under `--gc-stress` and the two runs may
  overlap. `tests/cases/io.ws` hardcoded `/tmp` and was for a long time the one
  case the Windows runner had trouble with -- twice over, because `/tmp` there
  resolves to the current drive's root, which need not exist, and because a
  fixed name is two of the suite's passes opening one file, which Windows
  refuses rather than tolerates. It follows the rule now.
- Parser tests compare against the s-expression dump (`wsharp_syntax::dump`),
  which makes precedence bugs obvious.
- **Fixtures a case imports live in `tests/cases/modules/`.** The harness runs
  every `.ws` directly in `tests/cases`, and a file with no `main` is not a
  case; `read_dir` does not recurse, so a subdirectory is where an imported
  module goes.
- **A vector with no publication is made twice.** RSA's published vectors are
  1024-bit and SHA-1, which `std/rsa` does not carry, so `rsa_pkcs1.ws` and
  `rsa_pss.ws` build every encoded message from RFC 8017's text, sign it with
  the raw private exponent, and then ask a second implementation about each one
  — the *forgeries* included, which is the half that matters, because a
  verifier that accepts too much passes every test written from the valid side.
  `node`'s `crypto` and `python3`'s `pow`/`hashlib` are the two available here;
  `openssl` is not installed.
- **A protocol is tested three ways, because a transcript can only do two of
  them.** RFC 8448 publishes whole TLS 1.3 handshakes, so the key schedule can
  be checked one derivation at a time (`tls_schedule.ws`) and the client can be
  driven with recorded bytes and its output compared to recorded bytes
  (`tls_rfc8448.ws`). What that cannot reach is the bytes this library produces
  *first* -- a recorded ClientHello is an input, so comparing it says nothing.
  That is why there is a server: `tls_loopback.ws` runs both ends against each
  other with no sockets at all, which is also the only way one thread can drive
  a negotiation.
- **A cryptographic case is checked against an independent implementation,
  not against memory.** Every primitive in `std/hash` and `std/cipher` has its
  published vectors -- FIPS 180-4, RFC 4231, RFC 5869, RFC 8439, FIPS 197, and
  McGrew and Viega's GCM cases -- and each was confirmed against a second
  implementation before being written into a header comment. That is not
  belt-and-braces: a remembered RFC ciphertext turned out to be wrong while the
  code was right, and the same habit catches the reverse. `node -e` has
  ChaCha20-Poly1305 and AES-GCM built in, and Python's `hashlib`/`hmac` cover
  the rest.
- **An AEAD case tests each way of being wrong separately.** A changed
  ciphertext, a changed tag, changed additional data that is not itself
  transmitted, the wrong nonce, the wrong key, and a truncation too short to
  hold a tag are six different paths, and a single "rejects a bad tag" check
  covers one of them.
- **A case that prints a narrow integer converts it.** `print_int` takes an
  `i64`, so a `u8` is written `print_int(i64(x))`; `print_uint` exists for the
  half of `u64`'s range an `i64` cannot hold. Conversions are written and never
  inferred, which is the same rule the language gives its users.
- **The whole case suite runs a third time, compiled.**
  `every_case_behaves_the_same_built_as_run` builds each case with `wsharp
  build` and runs the executable, holding it to the same header. That is the
  only thing that exercises the object backend, the three tables as data, the
  relocations a linker fills in, and the `main` in `wsharp-start` -- a JIT pass
  reaches none of them. It costs one `cc` per case and about a minute.
  `the_collector_survives_stress_in_a_built_program` then runs the `gc_*`
  subset natively under `WSHARP_GC_STRESS=1`, because the stack maps are the
  table where a serialisation mistake is silent rather than fatal.
  `a_built_program_reports_the_collector_doing_its_work` reads the stats line
  and insists the counts are non-zero, for the reason below: a stack-map table
  that deserialised to nothing makes every root check pass vacuously.
- **A case that spawns a worker whose `init` never returns is an assertion
  about the exit path**, and it fails by *hanging* rather than by reporting.
  `worker_daemon_exit.ws` is that case, and it is what probes 17 and 29 of the
  Raython design study were. It expects nothing from the worker: `@spawn`
  returns before `init` starts, so whether that thread printed anything before
  `main` returned is a race and a case cannot expect the answer.
- **The whole case suite runs a second time under `--gc-stress`**, which
  collects at every allocation and checks every root the stack maps describe.
  This is the collector's main defence, because rooting is spread over every
  expression the code generator lowers and a slot it forgot would otherwise
  show up as rare corruption rather than a failing test. Traces start on the
  same allocation schedule under stress as without it, so the concurrent
  paths run in both passes.
- **A networking case binds port 0 and asks what it was given.** A hardcoded
  port makes a test that fails whenever the machine happens to be using it.
  Every case is also single-threaded: `connect` to a listening socket completes
  into the backlog without anyone having called `accept`, so one thread can be
  both ends and no case can deadlock in CI. `net::close_all` runs at exit so a
  listener's port is released before the next case wants it.
- **A readiness case must not assume one wait reports everything.** `poll` and
  `epoll` are allowed to report a *subset* of what is ready, and the platforms
  differ in what they report and when: Linux completes a loopback write inside
  the syscall, so both ends of a pair are ready at once, while macOS hands
  loopback delivery to the kernel and a wait issued straight afterwards may see
  one or neither. `net_poller.ws` asserted that one wait saw both, passed
  everywhere, and then failed on macOS under `--gc-stress` alone, which shifted
  the timing enough to expose it. Loop until each socket has actually been
  served, which is the shape a real server has anyway, and bound the loop so a
  poller that reports nothing fails the case rather than hanging the suite.
- **The safe region has a test that would pass without it.** A trace that
  finishes while its mutator is parked proves nothing if the mutator happened
  to park *after* the trace was over, so `served_pauses` is counted and
  asserted on -- the same discipline as counting the roots a walk found.
  `WSHARP_GC_STATS=1` reports it as "N served parked".
- **The concurrent collector is tested from W#, not from Rust.** The phase
  machine needs a real mutator with real stack maps, so `gc_concurrent.ws`
  mutates the heap between `gc_trace_start()` and `gc_trace_finish()`,
  `gc_barrier_concurrent.ws` hammers both barriers across the same window
  while objects are actually moving, and `gc_auto_trace.ws` allocates enough
  to trigger traces on its own. Runtime
  unit tests drive private `Heap` instances; the few that touch process-wide
  state (the mark parity, the stress flag) take `test_support::SERIAL`,
  because the test binary runs its tests in parallel on one heap.
- `WSHARP_GC_STATS=1` prints what the collector did on exit, including traces,
  objects moved, and the number and longest of the pauses;
  `WSHARP_GC_TRACE=1` prints every frame the root walk visits. **Check the
  counts, not just that the tests pass**: a stack walk that finds no roots at
  all makes every root check succeed for the wrong reason, which is exactly how
  the first version of this looked correct while doing nothing. Likewise a run
  of `gc_moving.ws` should report objects moved, and of `gc_auto_trace.ws`
  traces started.

## Style

- Diagnostics carry a span, a message, an optional label and an optional help
  line. The *message* should state the rule; the help should say what to do.
  Rendering is hand-rolled in `syntax/diag.rs` and deliberately colour-free so
  it stays diffable.
- Prefer adding a deferred `Constraint` in `infer.rs` over special-casing at the
  point of use. The test is whether the question can be answered where it is
  met: `Numeric` and `HasField` cannot be, so they are constraints. Subtyping
  *can* be — `try_unify` binds whichever side is still a variable, so anything
  reaching the lattice check is already concrete — so it is not one, and
  `unify` stays untouched.
- Comments should explain *why*, especially where a choice looks arbitrary
  (levels, the leading environment pointer, the tag-carries-the-error-code
  encoding). The code says what it does already.
