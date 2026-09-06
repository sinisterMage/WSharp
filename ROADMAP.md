# W# Roadmap

Sessions 1–2 delivered the core language and Hindley-Milner type inference on a
Cranelift JIT. Sessions 3–4 delivered the garbage collector and multiple
dispatch. Session 5 delivered arrays, explicit generics, the standard library
and the module system — everything the original feature list asked for.
Session 6 closed what item 5 had left open: a growable array, and `fn` literals
that generalise.

This file records what was built and why it was built that way, the limitations
that were chosen rather than stumbled into, and — for the items still ahead —
where each already has a place to plug into.

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

Nothing. The status types moved out of the global namespace and into
`std/http` when item 6 landed, which was the one thing outstanding here.

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

### Known limitation, uncovered by the above — **since fixed**

A **recursive generic function** used to fail monomorphisation with `cannot
tell what type X is being used at`. A call within a binding group recorded no
type arguments, so a self-call left the group's own variables unresolved. This
predated abstract types -- `fn pick(x, c: bool) { if (c) { return pick(x,
false); } return x; }` had always failed -- but abstract parameters made
generic functions easy to write on purpose, so it became much easier to meet.

Fixed in session 5, where the fix predicted here turned out to be the right
one: inference records a group's own variables as the type arguments of its
in-group calls, once the group has generalised. See item 5.

---

## 5. Arrays and generics — **done**

### What was built

- **`[]T`**, a heap object with the length in the header's `aux` word and the
  elements inline — the shape string literals already had. `[]i64{ 1, 2, 3 }`
  writes one; the element type is written rather than inferred so that `[]i64{}`
  is still a value.
- **Indexing** `a[i]`, as a place as well as a value, so `a[i] += 1` works.
- **Bounds checks that panic**, reporting the index and the length:
  `W# panic: index 5 out of bounds (len 3)`, exit 101. One *unsigned* compare
  against the length, which rejects a negative index in the same instruction.
- **`for (xs) |x|`**, and `for (xs) |x, i|` to bind the index too.
- **Explicit type parameters**: `fn first[T](a: []T) T`, and
  `const Box = struct[T] { value: T };`.
- **The element-stride notion** the last version of this file asked for.
  `TypeLayout` gained `elem_stride` and `elem_ptr_offsets` — the offsets within
  *one* element — and one function, `types::for_each_ptr_offset`, is now the
  single definition of where an object's references are. The six places that
  used to walk `ptr_offsets` themselves go through it.
- **Per-instantiation layout for generic structs.** `Box[?i64]` and `Box[i64]`
  put their fields in different places, because a `?T` field is two slots or
  three, so a generic struct's offsets are computed by code generation, where
  every type is concrete, through the same `layout::place` inference uses.
- **Generic `fn` literals.** `const id = fn (x) { return x; };` is
  generalised at its binding and usable at two types; `fn [T](a: []T) T`
  writes the parameters out, exactly as a declaration does. What made this
  reachable was that every piece already existed: Rémy levels, `generalize`
  and `instantiate`, and the `targs` field a `Closure` node was already
  carrying for a named generic function used as a value.
- **A growable array**, `std/list`. The last version of this file said one
  "wants a second object holding a capacity and a length", and that is exactly
  what `List[T] = struct[T] { items: []T, count: i64 }` is: the backing
  array's header length is the capacity, and `count` is how much of it is in
  use. Pushing writes into the spare tail and doubles when it is full, so a
  run of pushes is amortised constant time.

### Decisions worth recording

- **Indexing panics rather than returning `?T`.** The precedent is `.?` on a
  null optional and division by zero: failures the type system permits but the
  program must not perform. Returning an optional would make `a[i]` two slots
  and put an `orelse` in every loop that had already checked the index.
- **`for` desugars to `while` in inference**, rather than becoming a HIR
  statement of its own. That keeps one loop form in the code generator, and so
  one place where the back-edge safepoint can be forgotten. The array goes into
  a hidden local, which makes it evaluated once and rooted like any other.
- **A generic struct stands outside the dispatch lattice.** Type ids are a
  preorder walk of it, fixed before monomorphisation, and a generic struct's
  instantiations are not known until after. Keeping them out lets an
  instantiation take an id from a block above the lattice, where no range test
  can be disturbed. `struct[T] : Base` is rejected with a message saying so.
- **Type arguments are written in type position only.** `Box[i64]` as a type;
  `Box{ .value = 1 }` as a literal, with the arguments inferred from the field
  values. `Box[i64]{ .. }` in expression position would be ambiguous with
  indexing `Box` by `i64`.
- **An annotation reaches a literal's fields.** `var c: Pair[?i64, i64] =
  Pair{ .first = 5, .. }` binds the arguments before the fields are checked, so
  `5` coerces into `?i64` exactly as it would in any other annotated binding.
  Without it the fields are checked against variables nothing has bound yet.
- **The recursive-generic bug is fixed.** A call within a binding group used to
  record no type arguments, so monomorphisation queued a second, unsubstituted
  copy of the callee and reported `cannot tell what type X is being used at`.
  Inference now fills those in with the callee's own quantified variables after
  the group generalises, which is sound because Hindley-Milner holds a group
  monomorphic.
- **A generic `fn` literal is a definition, not a value.** A closure value is
  one code pointer, and two instantiations need two -- so a `const` bound to a
  generic literal binds a *name*, and each use materialises a closure at the
  type that use needs. This is the second time the language has needed that
  shape: `const g = f;` over an overload set is the first, and for the same
  reason. A literal written where a value is wanted is still a value, and
  still monomorphic.
- **The value restriction is `const` plus no annotation.** `var f = fn ...`
  is one storage location holding one function value; `const f: fn(i64) i64 =
  fn ...` says which one. Both name a single type, so neither generalises. The
  dead `Expr::is_syntactic_value`, which claimed sema did this and had no
  callers, is gone -- the rule now lives where it is used.
- **A variable a constraint still owns is not quantified.** `solve_constraints`
  runs once per binding group, which is why `fn add(a, b) { return a + b; }` is
  `fn(i64, i64) i64` and not generic: `Numeric` defaults it before anything is
  quantified. A literal generalised at its own binding closes its level first,
  so quantifying such a variable would give the same body two different types
  depending on which of the two ways it was written. `const add = fn (a, b)
  { return a + b; };` is `i64` for exactly the reason the declaration is.
- **A `fn` literal names itself through its environment.** `const f = fn (x)
  { .. f(x) .. };` used to fail, because the name was bound by the statement it
  is the initialiser of and so was not in scope in its own body. Binding it
  first is only half the fix; the other half is what the name means. A literal
  is only ever entered *through* a closure value, and a call passes that value
  as the environment pointer -- so the environment already *is* a closure for
  this function at this instantiation. The name binds to it, copied into a
  declared local in the prologue beside the captures, and the recursive
  reference costs a register rather than an allocation. Monomorphisation needed
  nothing: the closure the outer use site built already points at the
  specialisation the body is, which is also why polymorphic recursion stays out
  of reach here for the same reason Hindley-Milner puts it out of reach for a
  declaration. A `var` still cannot, and for the reason it does not generalise:
  the location holds nothing yet when the literal is built.
- **Captures are snapshotted at the definition.** Each use builds its own
  closure object, so a captured `var` assigned in between would otherwise
  change what the closure sees. The definition emits one hidden local per
  capture -- bracketed, as the `for` desugaring's are -- and every
  instantiation shares them, which is sound because a capture's type belongs
  to the enclosing frame and so is never one of the quantified variables.
- **Monomorphisation composes rather than replaces.** A closure body refers to
  the enclosing function's variables *and* to its own, so `callee_subst` starts
  from the caller's substitution and adds the literal's quantified variables.
  That also keeps the cache key right for free: the key is the whole map, so
  one literal used at two types inside one enclosing instantiation gets two
  copies, and one used at one type inside two enclosing instantiations still
  gets two. **The code generator did not change at all** -- each specialisation
  is an ordinary `FuncId` with a closure layout of its own, and a call through
  a definition is the indirect call it always was.
- **`for` asks the type's own module how to walk it.** A `for` whose subject
  is not an array desugars to `while (M.next([iter])) |v|`, with `iter` and
  `next` resolved in the module that *declares* the subject's type -- so
  `for (xs)` over a list needs no import beyond the list, and `std/list` says
  how a list is iterated without the type checker knowing it exists. `?T` is
  what says the walk is over, which makes the desugaring `while (c) |v|`
  exactly, back-edge safepoint and all. Two consequences worth stating: a
  subject whose type is still a variable takes the array path and records
  `Indexable`, because the protocol has to be chosen where the loop is built
  and there is nothing yet to choose it from; and a `for` names `iter` and
  `next` as dependencies of *every* module it imports, because which one
  applies is not knowable before inference. Over-approximating there is sound
  rather than merely convenient -- a dependency only matters inside a cycle,
  and a library's iterator never calls back into the program using it.
- **The growable array is a library type, not a language one.** `List[T]` is
  an ordinary generic struct in an ordinary `.ws` file; nothing in the lexer,
  the parser, inference or the code generator knows it exists. That it could
  be written at all is the argument that item 5's generic structs and
  header-counted arrays were the right primitives — and it is why the whole
  module is 178 lines with no Rust beside it.
- **A list bounds-checks against its count, not its capacity.** Indexing the
  backing array directly would happily hand back a spare slot, so the check is
  in W#. That needed a way to fail as well as generated code does: the prelude
  gained `panic_index`, which is the same entry point `a[i]` calls and gives
  the same `index 5 out of bounds (len 3)`. Exposed for the reason the `gc_*`
  counters are — a library written in W# should be held to the standard the
  code generator is.

### What is left

- **No array covariance**, deliberately: `[]Sub` is not a `[]Base`, because a
  write through the second would break the first.
- **A definition builds its closure at each use.** A call through one is an
  indirect call on a freshly materialised closure, so it allocates an
  environment object per use rather than per binding. Cheap, and the price of
  needing no new calling convention -- but it is a cost, not a nothing.
- **`pop` and `remove` leave the vacated tail slot holding its old
  reference.** The collector walks every element the header claims, so that
  object stays alive until the slot is overwritten, the list grows or the list
  dies. It cannot simply be nulled: `l.items[i] = null` only typechecks when
  `T` is itself an optional. At most one extra object per pop, and `clear`
  drops the backing array outright.

---

## 6. Standard library — **done**

### What was built

A module system, and four modules behind it.

| Module | Contents |
|---|---|
| `std/str` | `len`, `concat`, `eq`, `substr`, `find`, `split`, `join`, `repeat`, `starts_with`, `from_int`, `from_float` |
| `std/array` | `len`, `new`, `concat`, `push`, `slice`, `repeat` |
| `std/list` | `List[T]` and `new`, `with_capacity`, `from`, `len`, `capacity`, `get`, `set`, `push`, `pop`, `insert`, `remove`, `extend`, `clear`, `iter`, `next`, `to_array` |
| `std/math` | `abs`, `min`, `max`, `sign`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `trunc`, `ipow` |
| `std/io` | `read_file`, `read_line`, `write_file`, `exists` |
| `std/http` | the 27 status types, moved out of the global namespace |

`==` on `str` works, comparing contents. The prelude — `print`, `assert`, the
`gc_*` counters — stays global, because every module has it without asking.

### The module system

- `const http = @import("std/http");` binds a module to a name. Only a
  top-level `const` may hold one, which is what makes the set of files a
  program needs answerable before anything is type-checked.
- A path is either a file next to the importing one (`"./util.ws"`) or one of
  the library's (`"std/http"`). `@import("std")` works too, and `std.http.X`
  walks into it: a module path is a prefix, and each further segment extends it.
- Names are stored qualified in one flat table, and an unqualified lookup tries
  the current module and then the prelude. What a module *cannot* see is simply
  what it has no key for — so two files may each declare a `helper`.
- A local binding shadows an imported module, so adding an import cannot break
  code that already used the name.
- Import cycles are detected and reported with the chain.

### Decisions worth recording

- **Spans stayed 8 bytes.** A `Span` has no file in it; widening one would
  touch every node in the syntax tree to carry a number only the renderer
  reads. Files are laid end to end in a single offset space instead, and a
  span's file is the range it falls in (`SourceMap`). The first file starts at
  offset 1, which keeps 0 meaning `Span::EMPTY`.
- **Half the library is written in W#.** `std/array`, `std/list`, `std/math`
  and `std/str.split` are `.ws` files compiled with the program, embedded with
  `include_str!`. The rule that draws the line is worth stating plainly:
  **a builtin may read and write bytes; anything that moves a *reference* from
  one object into another is written in W#.**

  This was learned the hard way. `array.concat` was a Rust function that
  memcpy'd elements between arrays, and `--gc-stress` caught it: the copied
  references had never been through the load barrier, so they named objects in
  blocks that were about to be released. Generated code cannot make that
  mistake — every reference it stores came through the barrier, and the
  registers holding one are roots — so the fix was to stop writing that code by
  hand rather than to reproduce three barriers in Rust.
- **`array.new` is lowered inline** rather than called: only the call site
  knows the element type, and so the stride and the type id to stamp. It is the
  one builtin the code generator recognises by name.
- **A generic builtin is instantiated, not generalised.** `len(a: []T) i64` has
  one machine implementation, because it reads the count out of the header
  whatever the elements are; its type variables are made fresh per use, and
  monomorphisation has nothing to specialise.
- **`abs` is an overload set, `min` is not.** `min(a: Number, b: Number)`
  compares two numbers and says nothing about which kind they are, so it is one
  constrained generic. `abs` compares against a literal zero, and an integer
  literal is an `i64` — so a single definition would pin `Number` to `i64` at
  the comparison. Two overloads instead, which is what the language is for.
- **A fallible builtin returns a `!T` directly.** The tag and the payload cross
  the boundary as the two words a `#[repr(C)]` pair is returned in. The tag is
  an index into the program's error table plus one, so the library's error
  names are interned before any program's — `builtin_errors()` fixes them.
- **An error set is a second type argument, and `unify` was not touched.**
  `!T` is `Con(ErrUnion, [payload, set])`, and a set is an interned
  `TyCon::ErrorSet`. Two error unions therefore unify by unifying their sets --
  which merges two variables, or binds one to a written set -- while *widening*
  a smaller set into a larger one is `coerce`'s job. That is exactly where
  widening a subtype into its supertype already lived, and works for the same
  reason: by the time anything asks, unification has bound whichever side was a
  variable. Putting the set inside the constructor instead would have made
  `!{A}T` and `!{A, B}T` a hard mismatch and broken `try` outright.

  An unwritten set is inferred, and inference is a little dataflow rather than
  a rule: `error.X` contributes `{X}` to the set it is raised into, `try`
  contributes a *subset edge* from callee to caller, and the edges are followed
  to a fixed point before anything is checked. `!{A, B}T` written down is
  closed instead, and then the same contributions are checked against it.

  A set nothing decides is *not* an error. `fn f() !i64 { return 1; }` has the
  empty set and prints as the bare `!i64` every signature was before -- so the
  variable is left alone through generalisation, which is what lets
  `fn twice(f: fn(i64) !i64, ..)` be generic over what its argument raises, and
  is closed to the empty set at monomorphisation instead. That needed the store
  to know which variables stand for a set, because one standing alone as a call
  site's type argument is indistinguishable from any other unresolved variable.

  Two consequences fell out. A builtin's set has to be written in its row --
  it is compiled long before the program that catches it. And `catch |e|` now
  binds the tag *unadjusted*: it used to bind the tag less one so the number
  was the error's index, which nothing could observe, but `e == error.X` is a
  comparison a program can write and the two spellings have to be the same
  number.
- **A `catch` or an `orelse` takes a block, and two different things wanted
  there decide its shape.** One is a value to use instead, after doing
  something first: a block whose last expression is written *without* a `;` is
  that value. The other is to give up -- and a block that never produces a
  value has to leave, by `return`, `break` or `continue`. Inference gives that
  second kind a *fresh type variable*, which is the whole of "diverging" here:
  it produces nothing, so it fits wherever it is written, and
  `f() catch return false;` checks in a function returning `bool` and in one
  returning `str` alike with no `noreturn` type anywhere.

  The one-statement spelling has no braces, because the `;` there belongs to
  the statement the whole expression is part of rather than to the `return`.
  And the `{` fork is unambiguous for a reason that was already true: a struct
  literal is `Path{ .. }`, and no expression starts with a bare brace.

  Code generation needed one thing it had not needed before. A block with no
  value has already ended by the time the operator wants to merge, and
  Cranelift will not let anything be appended to a block that ended -- so the
  placeholder values the merge still expects are made in a block of their own,
  with no predecessors, which falls out in optimisation.
- **Visibility is private by default, and only qualified lookup checks it.**
  `pub` in front of a `fn`, a `const` or a struct is what lets another module
  name it. The check goes in the three places a *qualified* name is resolved --
  a value, a callee, and a type -- and nowhere near `global()`, because an
  unqualified name can only ever mean this module's own or the prelude's and
  both are always visible. That also means the flat table needed no second
  dimension: one set of the keys that are *not* public says everything, since a
  qualified name cannot reach the module it is written in without that module
  importing itself, which is a cycle.

  A private name is reported rather than hidden. Resolving to nothing would
  come back as "cannot find", which sends the reader looking for a spelling
  mistake instead of at the declaration that is right there. One name is
  resolved more than once -- a call asks whether its callee is an overload set,
  then a builtin, then a value -- so the reported spans are remembered and the
  reader is told once. There is no secondary label pointing at the
  declaration, tempting as it is: a `Span` carries no file, and the renderer
  lays a diagnostic's labels out in the file its primary span falls in, which
  is by construction not the one the declaration is in.

  `std/list.reserve` is the demonstration: growing is that module's business,
  and a caller that reserved the wrong amount would be a caller that had to
  know about the doubling.
- **Builtins are keyed by a qualified symbol.** `std/str.len` and
  `std/array.len` are two functions that source code calls `len`; the symbol
  table has no notion of a module, so `Builtin::symbol()` supplies one.

### What is left

- **No package management.** An import is a relative path or a library one;
  there is nothing that fetches anything.

---

## 7. Multithreading — **designed, not built**

The model, settled before anything is written so that the collector and the
type system are not surprised by it later.

### Workers, each with its own heap

A worker is an OS thread that owns its heap. No object is reachable from two
workers, and nothing is sent by pointer.

The collector decides this. All three pauses run on the mutator thread because
only a mutator can walk its own stack, and the stack walker walks exactly one
stack. Per-worker heaps keep every worker's pauses independent and need no
rendezvous at all. The alternative — one shared heap — needs every mutator to
poll and stop before any pause, which turns a 40-microsecond pause into one
that waits for the slowest thread to reach a safepoint. That is the cost, and
it is why the shared heap is the rejected design rather than the obvious one.

### Two ways to talk

1. **Typed RPC**, point to point. A worker declares a service — a set of `fn`s
   — and a caller holds a typed handle checked against the same signatures at
   compile time. A call returns `!T`, because a worker can die and that is not
   an exceptional case worth a second mechanism.

2. **A message broker**, many to many, in the shape of Kafka. Named, typed
   topics; append-only partitioned logs; workers subscribe as consumer groups,
   each with its own offset; replay from an offset; at-least-once delivery. An
   in-process broker first — durability is a later concern, and the interface
   does not change when it arrives.

RPC is for when the caller needs the answer. The broker is for when it does
not, or when more than one worker wants the same message.

### Where it plugs in

**A subscriber set is an overload set.** `fn handle(m: OrderPlaced)`,
`fn handle(m: OrderCancelled)`, and the broker picks by the message's runtime
type id — which is exactly what the dispatcher already does, in one subtract
and one unsigned compare. A subscriber written for a supertype catches every
message below it, so a topic's message lattice is the status lattice a second
time. That the pattern turns up twice, in unrelated features, is the argument
that it is the right one.

**A `Transferable` constraint**, deferred in the same way and for the same
reason as `Numeric`: scalars, `str`, arrays and structs of transferable fields
qualify; closures and function values do not, because they capture an
environment belonging to another heap. The question cannot be answered where it
is met — a generic `send[T]` meets it before `T` is known — so it is a
constraint rather than a check.

**A deep-copy walker**, which can read `TypeLayout.ptr_offsets` and the element
stride that item 5 added: the collector already knows how to find every
reference in an object, and copying one to another heap is the same walk.

### What has to change first

- The heap's statics become per-worker. `HEAP`, the buffers, the phase and the
  type registry are process-wide today; only the registry can stay that way,
  because it is frozen before any code runs.
- `builtins.rs` gains `spawn`, the handle types, and the broker's operations —
  and the broker itself is the first part of the runtime that is not a leaf.
- The stack walker is already per-thread and needs nothing.

---

## 8. Direct libc calls for I/O and networking — **before v0.5**

`std/io` goes through `std::fs` and `std::io` today, so the platform work is
Rust's and W# inherits its portability for free. That is the right trade while
the library is four functions; it stops being the right trade the moment
networking arrives, and this is the item that has to land before it does.

### Why move

- **Flags and error codes are not reachable.** `std::fs::read` opens a file one
  way. `O_NONBLOCK`, `O_DIRECT`, `O_CLOEXEC` and the rest are not expressible,
  and `io_error_tag` currently maps `std::io::ErrorKind` — a portable
  approximation — where `errno` is the real answer.
- **Networking needs non-blocking I/O and a readiness API**, which means
  `epoll` on Linux, `kqueue` on the BSDs and macOS, and IOCP on Windows. None
  of that is in `std`, so the socket half would end up hand-written regardless;
  doing the file half the same way keeps one layer rather than two.
- **A blocking syscall is a hole in the safepoint protocol.** All three
  collector pauses run on the mutator thread, because only a mutator can walk
  its own stack — so a thread parked in `read(2)` cannot answer a pause
  request, and the trace waits for the disk. Today that is a stall in a
  single-threaded program. With item 7's workers it is one worker's heap
  blocked on another's slow client, which is exactly the failure a worker model
  exists to avoid. Non-blocking I/O plus a readiness loop is the fix, and it is
  the same fix networking wants.

### Decisions worth recording in advance

- **Hand-declared bindings, not the `libc` crate.** `libc` is not in the local
  registry cache, so adding it would break the offline build that the pinned
  Cranelift version exists to preserve — the same argument that settled MMTk in
  item 3. It would also end `wsharp-runtime`'s leaf-crate property: it has zero
  dependencies today, and that is worth more than a few dozen `extern "C"`
  declarations are worth avoiding.
- **Windows is a third arm, not a variation.** It has no libc worth targeting:
  `CreateFileW`/`ReadFile`, and WSA for sockets. Pretending otherwise behind a
  `#[cfg(unix)]`/`#[cfg(not(unix))]` split would put the difference in the
  wrong place. Three arms, named for what they are.
- **The W# side does not change.** `crates/wsharp-runtime/src/io.rs` is the
  only file in the tree that touches the outside world, and the builtin table
  rows, the string object layout and the `!T` tag encoding are all above it.
  A program that calls `io.read_file` is unaffected, which is what makes this a
  port rather than a redesign.

### What it costs

Portability becomes ours. `std::fs` currently handles path encoding, retry on
`EINTR`, short reads and the difference between a file and a pipe; each of
those becomes a thing to get right, per platform, with a test. That is the
price of the control, and it is worth paying only because networking cannot be
had without it.

---

## Smaller follow-ups

These are deliberate limitations, each with a clear fix:

- **Computed top-level `const`.** Only literals and `fn` values are allowed at
  the top level; anything computed is rejected with a message saying so.
  Supporting the general case needs global storage plus a startup initialiser —
  and the collector would need those globals as roots.
- **Field access needs a known type.** Structs are nominal with no row
  polymorphism, so `fn getx(p) { return p.x; }` cannot be inferred and asks for
  an annotation instead.
- **`==` is limited to `i64`, `f64`, `bool` and `str`.** Structs still need a
  decision about identity versus structural equality.
- **Integer literals are always `i64`.** No `comptime_int` coercion, so `1.0`
  must be written where an `f64` is wanted.
- **`%` is integer-only.** Cranelift has no float remainder, and a float `%`
  is rejected by inference rather than emulated.
- **No sized integer types**, no unsigned types, no bitwise operators.
- **x86-64 and aarch64 only.** The collector reads the frame pointer with
  inline assembly; other architectures get a `compile_error!`.
