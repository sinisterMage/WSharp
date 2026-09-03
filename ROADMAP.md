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
| Reference-count collection, with transitive freeing done iteratively, deferred while a trace is marking and suspended while it is moving objects | `gc.rs` — `collect` |
| The collector thread: concurrent marking from a root snapshot, concurrent evacuation, the concurrent sweep, the three pauses between them, and the abandon-at-exit path | `wsharp-runtime/src/mark.rs` |
| Hole refilling: a block with free lines is allocated into again rather than waiting for a trace to evacuate it | `heap.rs` — `find_hole`, `reserve` |
| Thread-local allocation buffers: the common allocation takes no lock at all | `heap.rs` — `tlab_alloc`, `refill_tlab` |
| An object-start bitmap, which is what makes a heap walkable when objects are not laid end to end | `heap.rs` — `set_start`, `for_each_in_block` |
| Concurrent evacuation of sparse blocks, with a load barrier so the program can keep running while objects move | `wsharp-runtime/src/evacuate.rs` — `ws_resolve`; `lower.rs` — `emit_load_barrier` |
| A remembered set: the marker notes every reference into a block being emptied, so the evacuation pause revisits a list rather than the heap | `mark.rs` — `mark_all`; `evacuate.rs` — `fix_references` |
| `--gc-stress` walks the whole heap afterwards and aborts if the remembered set missed a reference | `evacuate.rs` — `verify_no_stale_references` |
| Loop back-edge safepoint, three instructions; also the collector thread's way of asking for the final pause | `lower.rs` — `emit_gc_poll` |
| `--gc-stress`: collect at every allocation and check every root | `gc.rs` — `validate_roots` |
| Pause accounting: `WSHARP_GC_STATS` reports the number of pauses and the longest | `gc.rs` — `record_pause` |

### What is left

Nothing. The collector thread, the load barrier that lets it move objects while
the program runs, hole refilling, thread-local allocation buffers and the
remembered set that bounds the evacuation pause are all in. What is worth
recording instead is where the remaining costs are:

- **The pauses are proportional to what the program is doing, not to what it
  is holding.** Marking, copying and sweeping all run on the collector thread.
  The three pauses scan the stack, move whatever the roots point at, and
  repoint the references the marker noted. On a heap of 120,000 live objects
  the longest pause measured 40 microseconds, the same as on one of 15,000.
- **`--gc-stress` still walks the whole heap** in the evacuation pause to check
  that the remembered set missed nothing. That is deliberate: the fast path is
  a list, and the slow path is the proof.
- **Removing the allocation lock bought contention, not raw speed.** On a
  single thread allocating three million short-lived objects the new path
  measures about the same as the old mutex-per-allocation one (roughly 250 ms
  against 240 ms): what the lock cost, the object-start bitmap and the atomic
  line metadata now cost instead, and those are what hole refilling and a
  concurrent collector need. The win is that an allocating thread and a
  sweeping or evacuating collector no longer serialise on every object.
- **A block is chosen for evacuation by line occupancy alone.** Immix has more
  to say here -- defragmentation headroom, a budget per collection -- and this
  takes every sparse block it finds.
- **Large objects are never moved**, so the large-object space can fragment its
  address space in a long run. It is served by the system allocator, which does
  its own coalescing, so this is a theoretical concern rather than a measured
  one.

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

- **The status types are in the global namespace**, because there is no module
  system. Item 6.

Scalar dispatch and overload-sets-as-values are done; what they turned into is
worth recording, because both landed differently from the one-line sketches
that used to sit here.

**Abstract types.** `abstract_types()` sits beside `status_types()` and is the
same kind of table: one row per type, `("Number", [i64, f64])` so far. A
concrete scalar is a subtype of an abstract type that lists it, which is the
one place the lattice reaches past structs. Two consequences fall out of the
language's own rules rather than being chosen:

- *An abstract parameter is a constrained generic parameter.* There is no
  machine representation for "a number", so `fn f(x: Number)` cannot be
  compiled once; it becomes a fresh type variable with a `Member` constraint,
  generalises, and monomorphises per argument type exactly as an unannotated
  parameter does. That also means an operator inside such a body has to work
  for *every* member, since the caller picks: `%` on a `Number` is rejected,
  because it does not.
- *No runtime test is ever emitted for one.* A scalar's type is always
  statically known, so the dispatcher never has to ask. `DispatchCase` is
  still a struct id or nothing, and the code generator did not change.

Specificity needs the type as *written*, not the variable it became, so
`Candidate` carries `decl_params` alongside `params`.

**Overload sets as values.** Two forms, because they answer different
questions. `const g: fn(Base) i64 = f;` selects the member whose signature *is*
that type -- exactly, not by subtyping, because a value is one code pointer and
widening would hand a `Base` to a body compiled for `Sub`. `const g = f;`
binds an alias instead: `g` dispatches exactly as `f` does, and is not a value
at all. Using an alias as a value is still an error, now one that says so.

### Known limitation, uncovered by the above

A **recursive generic function** fails monomorphisation with `cannot tell what
type X is being used at`. A call within a binding group records no type
arguments, so a self-call leaves the group's own variables unresolved. This
predates abstract types -- `fn pick(x, c: bool) { if (c) { return pick(x,
false); } return x; }` has always failed -- but abstract parameters make
generic functions easy to write on purpose, so it is much easier to meet now.
The fix is to record a group's own variables as the type arguments of its
in-group calls, which is a change to `mono.rs` rather than to inference.

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
