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

## Invariants

- **`hir::Program.funcs` is indexed by `FuncId`.** Never filter or reorder it —
  that silently renumbers every function. `Analysis::finish` discards the whole
  table rather than drop one entry.
- **Code generation never sees a type variable.** Monomorphisation substitutes
  every type in a reachable function, *including intermediate expression types*,
  and reports what it cannot resolve. `repr::slot_types` panics on a leftover
  variable on purpose: it means that pass has a hole.
- **Value layout has one definition.** `wsharp-sema/src/layout.rs` decides slot
  counts and pointer offsets. Inference uses it to place struct fields, code
  generation to shape registers. They must agree; a test in
  `wsharp-codegen/src/repr.rs` checks that they do.
- **A builtin may read and write bytes; anything that moves a *reference* from
  one object into another is written in W#.** This is why `std/array` and
  `std/str.split` are `.ws` files compiled with the program rather than rows in
  the builtin table. Generated code goes through the write barrier, the load
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
  safepoint, because only the mutator can walk its own stack. The collector
  thread never touches the stack. The pause sites are `ws_gc_poll`,
  `on_allocation`, the `gc_trace*` builtins and exit — and so, transitively,
  any builtin that allocates, since `ws_alloc` is where `on_allocation` lives.
  Never one that does not: `print` holds its argument in a Rust local no stack
  map describes, and pausing there would leave it stale. `on_allocation` is
  safe because the object in flight is in the open block, which is never an
  evacuation candidate, and on no list, so nothing judges it.
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
- **The closure environment is dead after the prologue.** Captures are copied
  into declared locals before the first safepoint and `env` is never read
  again, so it is not a root and need not be. Re-reading it after a call would
  be a use-after-move.

## Testing

```sh
nix-shell --run "cargo test --workspace"
```

- Unit tests live next to the code they cover.
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

  The harness (`crates/wsharp-cli/tests/cases.rs`) runs the built binary as a
  subprocess, so stdout is captured for free and the test does exactly what a
  user would.
- Parser tests compare against the s-expression dump (`wsharp_syntax::dump`),
  which makes precedence bugs obvious.
- **Fixtures a case imports live in `tests/cases/modules/`.** The harness runs
  every `.ws` directly in `tests/cases`, and a file with no `main` is not a
  case; `read_dir` does not recurse, so a subdirectory is where an imported
  module goes.
- **The whole case suite runs a second time under `--gc-stress`**, which
  collects at every allocation and checks every root the stack maps describe.
  This is the collector's main defence, because rooting is spread over every
  expression the code generator lowers and a slot it forgot would otherwise
  show up as rare corruption rather than a failing test. Traces start on the
  same allocation schedule under stress as without it, so the concurrent
  paths run in both passes.
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
