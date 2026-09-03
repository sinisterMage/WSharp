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
  ```

  The harness (`crates/wsharp-cli/tests/cases.rs`) runs the built binary as a
  subprocess, so stdout is captured for free and the test does exactly what a
  user would.
- Parser tests compare against the s-expression dump (`wsharp_syntax::dump`),
  which makes precedence bugs obvious.
- **The whole case suite runs a second time under `--gc-stress`**, which
  collects at every allocation and checks every root the stack maps describe.
  This is the collector's main defence, because rooting is spread over every
  expression the code generator lowers and a slot it forgot would otherwise
  show up as rare corruption rather than a failing test.
- `WSHARP_GC_STATS=1` prints what the collector did on exit;
  `WSHARP_GC_TRACE=1` prints every frame the root walk visits. **Check the
  counts, not just that the tests pass**: a stack walk that finds no roots at
  all makes every root check succeed for the wrong reason, which is exactly how
  the first version of this looked correct while doing nothing.

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
