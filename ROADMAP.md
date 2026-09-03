# W# Roadmap

Sessions 1–2 delivered the core language and Hindley-Milner type inference on a
Cranelift JIT. Sessions 3–4 delivered the garbage collector and multiple
dispatch. This file records what is done, what is left, and — more usefully —
where each remaining feature already has a place to plug into.

---

## 3. Garbage collector — **done**

Reference counting combined with a concurrent mark trace and compaction,
following LXR (Zuo, Blackburn, Zigman & Yang, *Low-Latency, High-Throughput
Garbage Collection*, PLDI 2022).

### Settled: written by hand, not bound to MMTk

The open question was whether to implement LXR directly or bind
[MMTk](https://www.mmtk.io/) and use its plan. **MMTk is not in the local
registry cache**, so adding it would break the offline build that the pinned
Cranelift version exists to preserve. That settled it: the collector is written
against this heap, and the header stays as designed rather than adopting MMTk's
object model.

### What was built

| Piece | Where |
|---|---|
| Immix-style heap: 32 KiB blocks, 256-byte lines, over-aligned reservations so an object's block follows from its address | `wsharp-runtime/src/heap.rs` |
| Large-object space for anything over 8 KiB; never moved | same |
| A lock-free directory of the spaces and block states, so the marker's per-reference questions take no lock | same |
| Atomic header: 32-bit type id, 8 flags, 23-bit saturating reference count, a forwarding encoding in bit 63, and a mark bit whose meaning is a parity that flips per trace | `wsharp-runtime/src/header.rs` |
| Coalescing write barrier — one load, one test, one not-taken branch on the fast path; its snapshot doubles as the concurrent mark's snapshot-at-the-beginning record | `wsharp-codegen/src/lower.rs` — `emit_log_barrier`, and `ws_log_object` in `gc.rs` |
| Precise roots: every heap pointer the code generator produces is declared to Cranelift, and the maps are harvested per function | `lower.rs` — `gc_root`; `codegen/src/lib.rs` — `harvest_stack_maps` |
| Frame-pointer stack walker that turns a return address into a set of root slot addresses | `wsharp-runtime/src/stackwalk.rs` |
| Reference-count collection, with transitive freeing done iteratively, and deferred while a trace is marking | `gc.rs` — `collect` |
| The collector thread: concurrent marking from a root snapshot, the two pauses around it, the concurrent sweep, and the abandon-at-exit path | `wsharp-runtime/src/mark.rs` |
| Evacuation of sparse blocks in the final pause, with forwarding, a fix-up of every reference, and a `--gc-stress` check that none was missed | `wsharp-runtime/src/evacuate.rs`; `heap.rs` — `select_evacuation`, `evacuate`, `release_evacuated` |
| Loop back-edge safepoint, three instructions; also the collector thread's way of asking for the final pause | `lower.rs` — `emit_gc_poll` |
| `--gc-stress`: collect at every allocation and check every root | `gc.rs` — `validate_roots` |
| Pause accounting: `WSHARP_GC_STATS` reports the number of pauses and the longest | `gc.rs` — `record_pause` |

### What is left

1. **Concurrent copying** needs a load barrier on every reference load: check
   whether the loaded pointer is in a block being evacuated and resolve
   forwarding if so. About four instructions in `load_at`. Evacuation is safe
   today *without* one only because it happens in the final pause, which
   visits every reference before the program resumes.
2. **A remembered set for the final pause.** The fix-up after evacuation walks
   every live object, so the pause is bounded by the size of the live heap. The
   marker could instead record every slot it sees pointing into a candidate
   block, and the pause would then only revisit those, plus the objects
   modified or allocated during the mark. The argument that this is complete
   has more moving parts than the walk (the walk *is* the `--gc-stress` check),
   so it waits for a measured pause that justifies it.
3. **Partial block reuse.** A block is recycled only when every line in it is
   free; the free lines of a block that still holds something are not handed
   back. Evacuation is what recovers those blocks, so this is a throughput
   improvement rather than a leak.
4. **Thread-local allocation buffers.** `ws_alloc` still takes a mutex per
   allocation, and the counting collector's buffers a second one.
5. **The closure environment is not a root inside the closure's body.** The
   captures are copied out on entry and the environment pointer is dead from
   then on, so this is fine today. Anything that re-reads it after a call — a
   lazily loaded capture, the load barrier above applied to `env` — must first
   declare it a root.

---

## 4. Multiple dispatch — **done**

Julia-style multiple dispatch over a nominal subtype lattice, with the HTTP
status types as its standard-library instance.

### What was built

- `const Sub = struct : Base { };` declares a supertype. A subtype's fields are
  its supertype's followed by its own, so a field read compiled against the
  supertype runs unchanged on any subtype.
- Several top-level `fn`s may share a name. Every parameter of an overloaded
  function must be annotated.
- Selection is Julia's specificity rule, including the escape hatch: two
  overloads that cross are ambiguous *unless* a third is more specific than
  both and covers their overlap.
- Ambiguity is a compile error rather than a coin flip.
- A call whose most specific candidate needs no runtime test lowers to an
  ordinary direct call — no dispatch code at all.
- Otherwise a decision chain over the runtime type id. Type ids are assigned in
  a preorder walk of the lattice, so a type's subtypes occupy a contiguous
  range and each test is one subtract and one unsigned compare.
- A struct with no fields is also a value: its sole instance, emitted into the
  data section next to the string literals. Naming a status costs nothing.

### Decisions worth recording

- **No inline cache.** The roadmap originally asked for "a runtime dispatcher
  plus an inline cache". Preorder type ids make each case two instructions, so
  a cache probe would cost more than the chain it skipped. The chain is
  emitted inline instead.
- **No deferred subtype constraint.** Subtyping looked like a job for the
  `Constraint` list, but `try_unify` binds whichever side is still a variable,
  so by the time anything asks "is this a subtype?" both sides are concrete and
  the lattice answers immediately. `unify` is untouched, and there is no
  fixpoint solver.
- **No least upper bound at joins.** `if (c) NotFound404 else Forbidden403` is
  still a type error rather than inferring `Status4xx`. Asymmetric LUB
  inference is a footgun; this can be revisited deliberately.

### What is left

- **Dispatch on scalar types.** Only struct types have a lattice, so
  `fn f(x: i64)` and `fn f(x: f64)` work by exact match, and there is no way to
  say that one numeric type refines another.
- **Overload sets as values.** `const g = f;` where `f` names several functions
  is rejected: a closure is one code pointer and a set is not.
- **The status types are in the global namespace**, because there is no module
  system. Item 6.

---

## 5. Arrays and generics

Most of the type-level work is already done.

**Already exists:**

- `Type::Con(TyCon, Vec<Type>)` is a general constructor-with-arguments
  representation. `Array(T)` needs no change to the type system.
- Let-polymorphism, generalisation and instantiation all work today —
  `fn id(x) { return x; }` infers `fn(T) T`.
- **Monomorphisation is written and tested** (`wsharp-sema/src/mono.rs`).
- The object header's `aux` word already holds an element count, and
  `types::object_size` already reads it: the collector handles variable-sized
  objects, because strings are ones.

**What is left:**

- `[]T` syntax, and an array object layout (header, length in `aux`, elements
  inline — the shape string literals already use).
- Bounds checking, and deciding whether it traps or returns an optional.
- Explicit generic parameters on declarations, rather than generics only ever
  being inferred.
- `for` loops over arrays.
- Tracing an array's elements: `TypeLayout.ptr_offsets` is a fixed list, which
  cannot describe "a pointer every 8 bytes for `aux` elements". The registry
  needs an element-stride notion.

---

## 6. Standard library

**Already exists:** `wsharp-runtime/src/builtins.rs` holds a single table of
`(name, parameter types, return type, function pointer)`. Inference reads it to
seed the global type environment; code generation reads the *same* table to
register JIT symbols. Adding a function is a one-line change in one file.

`status_types()` beside it is the same idea for types, and demonstrates the
pattern scaling: a program pays only for the statuses it names, because they
are materialised on first mention rather than declared up front.

Currently seeded: `print`, `print_int`, `print_float`, `print_bool`, `assert`,
`gc_collect`, `gc_trace`, `gc_live_objects`, `gc_live_bytes`, `gc_collections`.

**What is left:**

- Strings: length, concatenation, comparison, slicing. (`==` on `str` is
  rejected today precisely because it needs a runtime call.) Concatenation is
  the first thing that will allocate a *string* on the heap rather than in the
  data section — the collector already handles that case.
- Arrays and their operations, once item 5 lands.
- Math, and file/stdin I/O.
- **A module system**, so the library is not one flat namespace. This is now
  the most pressing item: the status lattice adds up to 27 names to the global
  scope, and any of them can shadow user code.

---

## Smaller follow-ups

These are deliberate limitations, each with a clear fix:

- **Computed top-level `const`.** Only literals and `fn` values are allowed at
  the top level; anything computed is rejected with a message saying so.
  Supporting the general case needs global storage plus a startup initialiser —
  and the collector would need those globals as roots.
- **`fn` literals are monomorphic.** Top-level functions generalise, but a local
  `const f = fn (x) { return x; };` does not, so it cannot be used at two types.
- **Field access needs a known type.** Structs are nominal with no row
  polymorphism, so `fn getx(p) { return p.x; }` cannot be inferred and asks for
  an annotation instead.
- **`==` is limited to `i64`, `f64` and `bool`.** Strings need a runtime
  comparison; structs need a decision about identity versus structural
  equality.
- **No block expressions.** `catch`/`orelse` take an expression, not a block.
- **Integer literals are always `i64`.** No `comptime_int` coercion, so `1.0`
  must be written where an `f64` is wanted.
- **`%` is integer-only.** Cranelift has no float remainder, and a float `%`
  is rejected by inference rather than emulated.
- **No sized integer types**, no unsigned types, no bitwise operators.
- **x86-64 and aarch64 only.** The collector reads the frame pointer with
  inline assembly; other architectures get a `compile_error!`.
