# W# Roadmap

Sessions 1–2 delivered the core language and Hindley-Milner type inference on a
Cranelift JIT. Sessions 3–4 delivered the garbage collector and multiple
dispatch. Session 5 delivered arrays, explicit generics, the standard library
and the module system — everything the original feature list asked for.
Session 6 closed what item 5 had left open: a growable array, and `fn` literals
that generalise. Session 7 delivered item 8: the operating system declared by
hand on three platform arms, a safe region that lets a thread block without
stalling its collector, and `std/net` and `std/http` above them.

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
| `std/str` | `len`, `concat`, `eq`, `substr`, `find`, `split`, `join`, `repeat`, `starts_with`, `from_int`, `from_float`, and — since item 8 — `byte_at`, `from_byte`, `parse_int`, `to_lower`, `trim` |
| `std/array` | `len`, `new`, `concat`, `push`, `slice`, `repeat` |
| `std/list` | `List[T]` and `new`, `with_capacity`, `from`, `len`, `capacity`, `get`, `set`, `push`, `pop`, `insert`, `remove`, `extend`, `clear`, `iter`, `next`, `to_array` |
| `std/math` | `abs`, `min`, `max`, `sign`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `trunc`, `ipow` |
| `std/io` | `read_file`, `read_line`, `write_file`, `exists` |
| `std/http` | the 27 status types, moved out of the global namespace; since item 8, an HTTP/1.1 client and server over `std/net` |
| `std/broker` | `Topic[M]`, `Consumer[M]` and `topic`, `publish`, `subscribe`, `next`, `commit`, `seek`, `len` |
| `std/net` | `Socket`, `Listener`, `Poller`, `Event`, `Datagrams`, `Peer`, `Datagram` and `connect`, `listen`, `accept`, `read`, `write`, `write_all`, `read_exactly`, `read_all`, `set_nonblocking`, `poller`, `watch`, `wait`, `udp`, `send_to`, `receive`, `reply`, `close` (item 8) |

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
  there is nothing that fetches anything. That is item 11, and the hook it
  needs is one branch in `Loader::follow`.

---

## 7. Multithreading — **done**

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

### What has been built

**Per-worker heaps.** `worker::Worker` owns the heap, the write barrier's
buffers, the phase machine, the mark parity and the statistics. A thread-local
pointer says which worker a thread belongs to, created on demand — a thread
that allocates is a worker by that fact alone — and a collector thread installs
its worker's pointer on entry, so a copy it makes while evacuating lands in the
heap the original came from.

Three things stay process-wide, each for a reason that does not generalise:

- **The type registry and the stack maps.** Frozen before any code runs.
- **The space directory.** It answers "which space is this address in?", and
  the load barrier asks it about whatever reference it was handed.
- **The two flag words generated code reads.** Their addresses are compiled in
  as constants, so they cannot be per-worker without teaching the barriers
  thread-local access. They mean "*some* worker wants a pause" and "*some*
  worker is moving" instead, counted rather than set, so that one worker
  finishing its pause cannot silence another's request. The slow path asks the
  current worker whether the request is its own; the cost is a false slow path
  on an uninvolved worker, which is correct because both slow paths are
  idempotent and rare because both flags are raised only around a pause.

This landed with a single worker and changed nothing observable: the whole
end-to-end suite passed unaltered, twice, and `WSHARP_GC_STATS` reported the
same collections, roots, traces and objects moved as before, to the number.

**`Transferable`, and the deep copy.** A sixth `Constraint`, created the way
`Member` is and deferred for the same reason -- a generic `send[T]` meets the
question before `T` is known. Unlike an abstract type it is not a list of
members but a structural walk: scalars, `str`, arrays and structs whose fields
all qualify, and never a function, which is a code pointer plus an environment
object belonging to the heap it was made in. A struct that reaches itself
answers yes on the second visit rather than recursing for ever.

The walk that does the copying needed nothing new: `types::for_each_ptr_offset`
is already the one definition of where an object's references are, and it
covers an array's elements as well as a struct's fields. What *was* new is the
list `decode` needs. It builds a graph in Rust locals, which no stack map
describes, so every object it makes is pinned on a runtime root list until the
graph is finished -- and that list is a fourth place a heap pointer can live,
so it went into `gc::collect`'s root set, the evacuation pause's root pass,
`evacuate::fix_references` and `--gc-stress`'s verifier, which is the standing
rule for exactly this.

`gc_transfer` exposes both halves on one worker, for the reason the `gc_*`
counters are exposed: a mechanism the language depends on should be assertable
from W#, including under `--gc-stress`, where every allocation `decode` makes
is a whole collection.

**Workers, and typed RPC.** `@spawn(m, args..)` starts an OS thread running
module `m`'s service and hands back a handle; `w.f(a, b)` calls into it;
`@join(w)` waits for it and shuts it down. A handle is an `i64` -- an index,
not a pointer, because another worker's objects are not this one's to hold, and
so the collector never sees one.

**A service is an ordinary module**, which is the decision the rest follows
from. `init` makes the state and a *method* is any function taking that state
as its first parameter. No new declaration form was needed, because W# has no
mutable globals: a worker's state had to be an explicit value passed in and out
anyway, and once it is, the set of functions that take it is exactly the set of
things the worker can be asked to do. `@spawn` and `@join` are new arms of the
form `@import` already was -- a thing the compiler handles rather than a
function it could call, because the first argument names a *module* and no
parameter could have a type that accepts one.

Marshalling meets generated code exactly twice, and both times through a buffer
of machine words: the call site writes its arguments into one and reads its
result from another, and a **trampoline** -- one generated function per method
-- reads the arguments back out and calls the real function. Both are generated
code on purpose. A reference moving from a buffer into a call goes through the
write barrier, the load barrier and the stack maps by construction there, and a
hand-written Rust caller would have none of the three -- the mistake
`array.concat` taught. What is left for the runtime is bytes, which is what it
may touch.

Two things fell out of the queue rather than the types. A worker's state is
pinned on the runtime root list for as long as the worker lives, because
nothing on its stack holds it between calls. And a call and a shutdown share
one lock: a call that got in before the worker stopped is answered by the drain
on the way out, and one that arrives after sees the worker gone under the same
lock and is told so -- without that, a call to a worker that had been joined
waits for a reply nobody is left to send.

**The broker.** Named topics, append-only partitioned logs, consumer groups
with their own offsets, replay, at-least-once delivery -- Kafka's shape,
because that shape is what makes two useful things possible at once: a
consumer that has fallen behind can catch up, and a consumer that has died can
be replaced by one that starts where its group had got to. A message sits in
the log as *bytes*, so it belongs to no heap while it waits and each consumer
decodes its own copy into its own.

The handles are numbers, because a topic belongs to the process rather than to
any one worker's heap; what makes them typed is `std/broker.ws`, where
`Topic[M]` and `Consumer[M]` carry the message type. The always-null `sample:
?M` field is what makes `M` a parameter of the struct rather than a name
nothing mentions, and so what makes the compiler check that a publisher and a
consumer agree about it.

**Choosing a subscriber needed no broker-side machinery at all.** `next`
returns the topic's message type and the program writes
`fn handle(m: OrderPlaced)` beside `fn handle(m: OrderCancelled)`; the existing
dispatcher picks by the type id the copy carried with it, in one subtract and
one unsigned compare. That the same pattern turns up here and in the status
lattice, in unrelated features, is the argument that it was the right one --
and it is why a message must be an *object* rather than merely transferable: a
scalar carries no header, so there would be nothing to dispatch on and nothing
to copy it by. `BuiltinTy::Message` says so, and the demand travels from the
builtin through `std/broker`'s generic wrappers to each use, the way an
abstract type's does.

### What is left

- **Durability.** The log is kept rather than written down, so a topic lives
  as long as the process. The interface does not change when that changes:
  nothing above the log would know.
- **One broker, in one process.** Named topics are what let two workers that
  have never met agree on one, and the same naming is what a networked broker
  would use. Item 8 has landed, so nothing is in the way of that now except
  writing it: `std/net` is the transport and `transfer::encode` is already the
  wire format.
- **A handle must be in a variable to be called through.** `w.f(a)` is
  recognised from the shape -- an object that is a local holding a handle,
  rather than a module path -- so a handle in a struct field or straight out of
  a call cannot be called through yet.
- **A service method cannot be generic and cannot return `!T`.** The first
  because a worker calls it through one machine implementation; the second
  because the call is already `!T`, and `!!T` is not what anyone wants.
- **A call blocks the caller.** RPC is for when the caller needs the answer;
  when it does not, the broker below is the shape.

---

## 8. Direct libc calls for I/O and networking — **done**

`std/io` went through Rust's `std::fs` and `std::io`, which was the right trade
while the library was four functions and stopped being one the moment
networking arrived. It is now hand-declared syscalls on three named platform
arms, with `std/net` and `std/http` on top of them.

### Settled first: the safe region

The blocker this item named was not the syscalls. It was that **a blocking
syscall is a hole in the safepoint protocol**: all three collector pauses run
on the mutator, because only a mutator can walk its own stack, so a thread
parked in `read(2)` cannot answer a pause request and its trace waits for the
disk. With workers that is one worker's heap held up by another's slow client.

The way out is that a *blocked* thread's stack is frozen, and a frozen stack
can be walked by anyone. So `worker::blocking` records the frame pointer its
stack starts at, says the worker is parked, and makes the call; the collector
claims it and runs the pause itself, walking from that recorded frame. Leaving
the region is a compare-exchange rather than a store, because a mutator
resuming while its stack is being read is the one race that matters.

`mark::quiesce` already had one thread drive another worker's pause, justified
by that worker's stack holding no roots. The recorded frame pointer is what
generalises it to a worker whose stack holds plenty.

Two consequences fell out rather than being chosen. A worker gives its
allocation buffer back and publishes its counters on the way *in*, because
those live on the thread and not in the worker -- a collector running the pause
would otherwise retire its own buffer and leave the mutator's block open, and
an open block is never swept, recycled or evacuated. And the runtime's pinned
roots do not travel, because they are on the thread too: a collector declines a
parked worker whose `pinned_depth` is not zero and waits for it, as everything
did before.

**Blocking is now safe; non-blocking is for scale.** That is worth stating
plainly, because it inverts the reason this item gave for wanting a readiness
API. `epoll` is not what keeps the collector alive -- the safe region is. A
readiness API is what lets one worker serve many connections.

### What was built

| Piece | Where |
|---|---|
| The safe region: park, record, hand the stack to the collector, and the handshake that leaves it | `worker.rs` — `blocking`, `claim_parked`, `walk_worker_roots` |
| A stack walk that starts from a recorded frame rather than the caller's | `stackwalk.rs` — `walk_roots_from` |
| The collector's half: run the pause for a mutator that cannot | `mark.rs` — `wait_or_serve`, `serve_parked` |
| Hand-declared syscalls, three arms, no new dependency | `sys/{mod,linux,bsd,windows}.rs` |
| `std/io` ported onto them, with `errno` where `ErrorKind` was | `io.rs`, `sys/mod.rs` |
| TCP, UDP, names and a readiness API | `net.rs`, `sys/*` |
| `std/net`: `Socket`, `Listener`, `Poller`, and the loops that must be W# | `std/net.ws` |
| `std/http`: HTTP/1.1 client and server, chunked decoding, and `status_of` | `std/http.ws` |
| Byte-level `str`: `byte_at`, `from_byte`, `parse_int`, `to_lower`, `trim` | `strings.rs` |

### Decisions worth recording

- **Hand-declared bindings, not the `libc` crate** — but not for the reason
  this item used to give. It said `libc` was not in the local registry cache;
  it is, and has been all along, as a transitive dependency of
  `cranelift-jit`. The half of the argument that stands is the one that was
  always doing the work: `wsharp-runtime` has **zero dependencies**, and that
  is worth more than a few dozen `extern "C"` declarations are worth avoiding.
- **`getaddrinfo`, not a resolver of our own.** Names are the one place where
  writing it by hand would have meant reimplementing the hosts file, NSS and
  the search domains -- and getting IPv6 wrong. It blocks, which the safe
  region has already made safe, so the reason to avoid it went away before the
  code was written.
- **Addresses are never laid out by hand.** `getaddrinfo` produces them and
  everything else consumes them, so no `sockaddr_in` is built here and no port
  is byte-swapped here. An IPv6 address then costs nothing extra: it is a
  longer one. `SockAddr` carries the family the resolver reported rather than
  reading it back out of the bytes, because `sockaddr` starts with a `u16`
  family on Linux and Windows and a `u8` length then a `u8` family on the BSDs.
- **`poll(2)` on the BSDs, not `kqueue`, and `WSAPoll` on Windows, not IOCP.**
  Both are deviations from what this item asked for, and each has its own
  reason. `struct kevent` is *not the same struct* across the family --
  FreeBSD 12 added an `ext[4]` tail macOS does not have -- so a binding written
  from the macOS headers and tested on the macOS runner would be a declaration
  for FreeBSD that nobody had ever run, laid out wrongly, failing silently.
  IOCP is a different model altogether: completion rather than readiness, which
  would push buffers-handed-to-the-kernel through every layer above for a
  scalability win nothing here needs yet. `Poller` is the interface both hide
  behind, and either can be replaced without anything above it changing.
- **One error set for the whole of `std/net`.** A builtin's set is written in
  its row, because it is compiled long before the program that catches it; the
  wrappers in `std/net.ws` pass results through each other constantly, and one
  set means they compose without a widening at every step. The cost is a
  `catch` that can name an error a particular call would not raise.
- **`WouldBlock` is an error name, not a special case.** On a non-blocking
  socket it is the ordinary answer, and a program is expected to catch it and
  come back.
- **A datagram's sender is a handle too.** Replying needs no host, no port and
  no address formatting: `receive` remembers where the message came from and
  `reply` sends back to it, so the address never leaves the runtime as text and
  IPv6 costs nothing extra. The pair is assembled in W# because one builtin
  answers with one value -- the same split the poller's `wait` and
  `ready_socket` use.
- **A socket handle is a number.** A socket belongs to the process rather than
  to any one worker's heap, exactly as a broker topic does, so it is an index
  into a table and W# holds the index in a one-field struct. What makes it
  typed is `std/net.ws`: a `Listener` accepts and a `Socket` reads and writes.
- **`!void` had to be made writable first.** `std/io.write_file` returns one,
  so the type existed -- but no W# function could produce one: `return;` was
  checked against the declared type directly rather than against the payload,
  and falling through is rejected. A valueless `return` in a function returning
  `!void` now means "finished, and nothing went wrong", which is the success
  tag with no payload beside it. `std/net.write_all` is the first caller.
- **A module can now name its own lazily materialised types.** `std/http` is
  the only module whose contents are a table in the compiler rather than
  declarations in a file, and once it had a source file of its own it could not
  mention `Ok200` even though every program importing it can. `lookup_struct`
  goes through `lookup_struct_in` for the current module, which changes nothing
  anywhere else.

### Traps met on the way, each of which cost time

| Thing | Reality |
|---|---|
| `struct addrinfo` | `ai_canonname` comes **before** `ai_addr` on the BSDs and Windows, and after it on Linux. `ai_addrlen` is `socklen_t` on Unix and `size_t` on Windows. |
| `epoll_event` | `#[repr(C, packed)]` on x86-64 **only**. Elsewhere it has the natural padding, and reading one layout through the other shifts `data` by four bytes. |
| `O_NONBLOCK` | `0o4000` on Linux, `0x4` on the BSDs. `O_CLOEXEC` differs again between macOS and FreeBSD, which is why the BSD arm sets it with `fcntl` instead. |
| `errno` numbering | The same up to 34 and different above it: `EAGAIN` is 11 on Linux and 35 on the BSDs. |
| A Windows `SOCKET` | Not a file descriptor and not a `HANDLE`. `closesocket`, not `CloseHandle`; `recv`, not `ReadFile`. |
| `SO_REUSEADDR` on Windows | Means something else — it lets a second socket bind a port another is *actively listening on*. The right port of the Unix workaround is to do nothing. |
| `EINTR`, short reads, path encoding | All ours now. `std::fs` did them; `sys` does them once, above the arms. |

### What is left

- **`kqueue` and IOCP**, per the decision above: an upgrade behind the existing
  `Poller`, wanted when a program has thousands of sockets rather than tens.
- **No TLS**, so `https://` is `error.NotSupported` rather than a connection
  that quietly speaks the wrong protocol. That is item 10, and item 9 comes
  first because the ciphers cannot be written without it.
- **No connection pooling.** The HTTP client opens a socket per request and
  sends `Connection: close`, which is the honest shape for a client with no
  pool. `Conn`, `send_request` and `read_response` are exposed so that a
  protocol wanting to reuse a socket can.
- **A failed request leaks its socket.** W# has no `defer`, so a `try` that
  leaves `http.request` early skips the `close` below it. The process closes
  everything at exit, so this is a leak within one run rather than a leak.

---

## 9. Sized and unsigned integers, and bitwise operators — **next**

W# has one integer type. `i64` is the right default and the wrong *only* choice
the moment a program computes on bytes rather than merely moving them: SHA-256
is defined on 32-bit words that wrap, ChaCha20 on 32-bit add, xor and rotate,
X25519 on the limbs of a much wider number. None of that can be written here
today, which is why this comes before TLS rather than beside it.

It is not only crypto, and item 8 met it twice while being built. `std/net.wait`
decodes a readiness bitmask with `bits % 2 == 1` and `bits >= 2` because there
is no `&`. `std/http.parse_hex` exists because a chunk header is base 16 and
`str.parse_int` is decimal. Both are arithmetic standing in for bit work.

### What it needs

- **The types.** `u8`, `u16`, `u32`, `u64` beside `i8`, `i16`, `i32` and the
  `i64` that already exists. Not `usize`: this language has no pointer
  arithmetic to size, and a type whose width depends on the target is a type
  whose overflow depends on the target.
- **Unsigned arithmetic wraps; signed arithmetic still traps.** SHA-256 *is*
  addition modulo 2^32, so a checked `+` would make it unwritable. A signed
  overflow is a bug in every program that is not doing this, and it keeps the
  panic it has.
- **`& | ^ << >> ~`**, and a rotate. Rotate is not a C operator, is one
  instruction on both targets, and is what every one of these algorithms is
  written in terms of -- so it is a builtin (`bits.rotl(x, n)`) rather than a
  shift-shift-or pattern the code generator has to recognise and would
  sometimes miss.
- **`>>` differs by signedness**: logical on an unsigned type, arithmetic on a
  signed one. That difference is most of the reason the two kinds are worth
  distinguishing.
- **A literal has to stop being an `i64`.** "Integer literals are always `i64`"
  is a smaller follow-up today; it becomes load-bearing here, because `0xff`
  has to be a `u8` in one place and a `u32` in another. Either a `comptime_int`
  that takes the type it is used at, or suffixes, and the first is much nicer
  to write.

### Decisions worth recording in advance

- **Conversions are written, never inferred.** A silent widening is how a
  32-bit hash becomes a 64-bit one that is right for a while. `u32(x)` truncates
  and says so.
- **`Number` has to say what it means.** The abstract type lists `i64` and
  `f64` today, and it is what `math.min` is generic over. Listing all eleven
  numeric types makes `min` work everywhere and makes every *other* constrained
  generic over `Number` have to work for `u8` too. This is a real decision, not
  a table edit, and it should be made before the types land rather than after
  something depends on the answer.
- **The collector does not care.** These are scalars: no header, no reference,
  no barrier. `layout.rs` and `repr.rs` have to agree about their sizes, which
  is the invariant a test already checks.
- **Dispatch does not care either.** A scalar's type is always statically
  known, so no runtime test is ever emitted for one -- exactly as item 4 found
  for the abstract types it added.

### What it costs

Eight new types is eight more rows in every table that enumerates them, and
every one of `unify`, `layout::place`, `repr::slot_types` and the arithmetic
lowering grows a case. The interesting risk is not that, though: it is that
inference currently has exactly one integer type and therefore never has to
*choose* one. The moment a literal can be any of eight, every place a type
variable is defaulted needs an answer, and getting that wrong is a program that
compiles and computes something else.

---

## 10. TLS 1.3, written in W# — **after 9**

Item 8 left `https://` as `error.NotSupported` rather than a connection that
quietly speaks the wrong protocol. This is what removes it -- and what the
package manager needs before it can fetch anything from a host it did not
already trust.

### Why in W# rather than in the runtime

The rule that governs the boundary would *permit* the other answer: a cipher is
a pure byte-to-byte transform, which is exactly what a builtin may be. So the
reason is not the collector's; it is that a language which cannot express
SHA-256 has a hole in it, and the fastest way to find out where the hole is, is
to try. The handshake and X.509 have to be W# regardless -- both build object
graphs, and item 6's rule sends those to `.ws` files.

### What it needs

| Piece | Notes |
|---|---|
| Hashes | SHA-256 and SHA-384; HMAC and HKDF on top |
| Ciphers | AES-128-GCM and AES-256-GCM, ChaCha20-Poly1305 |
| Key exchange | X25519, and P-256 for a server that will not do better |
| Signatures | Ed25519 and ECDSA P-256 to speak TLS 1.3; RSA PKCS#1 v1.5 and PSS to *verify certificates*, which is a different and larger problem |
| Record layer | Framing, sequence numbers, key updates, the 1.2-shaped outer header 1.3 keeps for middleboxes |
| Handshake | ClientHello through Finished, plus HelloRetryRequest |
| X.509 | DER parsing, validity and name checking, chain building |
| Root store | Three platforms, three answers |

### Decisions worth recording in advance

- **Constant time cannot be promised, and saying so is part of the design.**
  W# compiles through Cranelift, which is free to turn a branchless expression
  into a branch and a conditional move into a jump. There is no `black_box`, no
  way to pin a secret away from a comparison the optimiser invented. So the
  implementation should be written constant-time *by construction* -- no
  secret-dependent indices, no early-exit compares -- and the ROADMAP should
  say plainly that this is a best effort against a local attacker rather than a
  guarantee. Anyone who needs the guarantee needs a reviewed C library and an
  FFI, which is a different item.
- **TLS 1.3 only.** No 1.2, no fallback, no downgrade dance. A client that
  cannot talk to a 1.2-only server is a client that fails loudly on a server
  that should be upgraded, and every hour spent on 1.2 is an hour spent on the
  version with the worse security story.
- **RSA is for certificates, not for the handshake.** 1.3 does not do RSA key
  exchange, but most of the certificate chain on the public internet is still
  RSA-signed -- so a bignum `modexp` is unavoidable even though nothing in the
  handshake wants one. An ECDSA-only client would fail against a large share of
  real hosts, and failing to verify is not an option.
- **The root store is a fourth arm-shaped problem.** `/etc/ssl/certs` and a
  handful of distribution-specific paths on Linux, the Keychain on macOS, the
  system store on Windows -- three implementations behind one question, which
  is exactly the shape `sys/` already has. Bundling a copy of Mozilla's list
  instead would make the build reproducible and the trust decisions stale, and
  staleness in a trust store is the failure mode that matters.
- **A TLS connection is a `Socket` by another name.** `tls.connect` returns
  something with `read`, `write` and `close`, so `std/http` takes either and
  neither knows which -- which is what makes `https://` a one-line change
  there rather than a second client.

### What it costs

This is the first thing in the tree where being wrong is a security problem
rather than a crash. A collector bug shows up as a failing test; a certificate
chain accepted when it should not have been shows up as nothing at all. The
mitigation is not cleverness, it is test vectors: RFC 8448's traced handshake,
Wycheproof for the primitives, and a corpus of certificates that must be
rejected with the reason each is rejected for. Those go in before the code they
check, not after.

---

## 11. Package management — **after 10**

Item 6 left one line: *"No package management. An import is a relative path or
a library one; there is nothing that fetches anything."* This is that.

### The shape, and where it comes from

Modelled on [Ajt](https://github.com/sinisterMage/Ajt.jl), an alternative
client for Julia's package ecosystem, for the parts that are about *being a
package manager* rather than about Julia:

- **Resolving, installing and building are separate verbs.** Nothing compiles
  because something else was fetched. `wsharp add` records an intent, `resolve`
  chooses versions, `install` makes the store satisfy the lockfile, and
  `build` is a thing you asked for.
- **`verify` answers with its exit status** -- ready, needs installing, needs
  resolving, broken -- so a CI script can ask without parsing anything.
- **Output is tab-separated**, so a shell can cut it up.
- **`why` prints the dependency paths that explain an entry**, because "what
  pulled this in" is the question a lockfile never answers on its own.
- **A resolver that explains itself.** The thing Ajt is actually built around:
  a solver that tracks *why* each version was ruled out, so a conflict comes
  back as something to act on rather than as "unsatisfiable". PubGrub is the
  algorithm; the traceable derivation is the point.

### What it needs, and what is missing today

- **`argv`.** There is none. Nothing in the prelude or in `std` exposes the
  command line, so a package manager written in W# cannot read its own verb.
  This is the first thing to build and the easiest to overlook.
- **A filesystem beyond four functions.** `std/io` reads a file, writes a file,
  reads a line and asks whether a path exists. A store needs `mkdir`,
  `readdir`, `rename`, `remove` and a stat that distinguishes a directory from
  a file -- and `rename` is what makes an install atomic.
- **Git, spoken rather than shelled out to.** Smart-HTTP v2 over item 10's TLS:
  pkt-line framing, ref discovery, want/have negotiation, and then a packfile,
  which means zlib inflate and delta resolution. Inflate is a few hundred lines
  and wants item 9's bit operations; delta resolution is where the surprises
  are.
- **A manifest and a lockfile format**, and a parser for it in W#. TOML is what
  everyone expects and is more grammar than this needs; a small line-oriented
  format is a day's work and a lifetime of explaining why it is not TOML. Worth
  deciding deliberately rather than by accident.
- **A content-addressed store**, keyed by tree hash, under `~/.wsharp` -- with
  `gc` to remove what no environment can reach, which is the half of a store
  people forget until a disk fills.

### Decisions worth recording in advance

- **The loader hook is one branch.** `Loader::follow` in
  `crates/wsharp-cli/src/load.rs` has exactly two rules today -- a `std` path
  stands for itself, anything else is relative to the importing file. A package
  path is a third, and *nothing downstream changes*: a module's identity is
  already its canonical path, so sema, code generation and the parser need no
  edit at all. That is the single most encouraging fact about this item.
- **Two versions of a package are two modules.** They live at different paths,
  so they are distinct modules with distinct nominal struct types -- which is
  almost certainly right, and which will produce a type error saying two
  identically-named types do not match, with nothing to say why. The diagnostic
  is the work, not the semantics.
- **A package facade needs re-export, and W# has none.** A package of more than
  one file cannot present a single entry point: there is no `pub use`, and a
  `const x = @import(..)` binding is a module rather than a value, so it cannot
  be reached through. This is the one part of this item that needs new work in
  the type checker, and it should be decided early because it changes what a
  package is allowed to look like.
- **One executable stops being a complete installation.** `.forgejo/workflows/release.yml`
  notes that the standard library is `include_str!`'d into the binary, so a
  release is one file. A package store is the first thing that puts state
  beside it. That is fine and worth doing on purpose, with the store's location
  answerable from the CLI rather than assumed.
- **Written in W#**, which is the point rather than a flourish: a resolver, a
  hash, a protocol and a file format is a broad enough program to find out
  what the language is actually missing -- and every gap it finds is one a user
  would have found instead.

### What it costs

A package manager is judged on the day it goes wrong, which means the work is
mostly in the failure paths: a half-written store, an interrupted fetch, a
lockfile from a newer version, two packages that cannot agree. W# has no
`defer`, and item 8 already left a socket leaking on an error path because of
it -- a store is where that stops being cosmetic. Atomic rename, a temporary
directory per fetch, and a `verify` that can tell "not installed" from "damaged"
are not polish here; they are the feature.

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
  must be written where an `f64` is wanted. Item 9 makes this load-bearing:
  `0xff` has to be a `u8` in one place and a `u32` in another.
- **`%` is integer-only.** Cranelift has no float remainder, and a float `%`
  is rejected by inference rather than emulated.
- **No sized integer types**, no unsigned types, no bitwise operators. Now
  item 9 rather than a follow-up: crypto cannot be written without them.
- **x86-64 and aarch64 only.** The collector reads the frame pointer with
  inline assembly; other architectures get a `compile_error!`.
