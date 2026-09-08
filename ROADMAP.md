# W# Roadmap

Sessions 1–2 delivered the core language and Hindley-Milner type inference on a
Cranelift JIT. Sessions 3–4 delivered the garbage collector and multiple
dispatch. Session 5 delivered arrays, explicit generics, the standard library
and the module system — everything the original feature list asked for.
Session 6 closed what item 5 had left open: a growable array, and `fn` literals
that generalise. Session 7 delivered item 8: the operating system declared by
hand on three platform arms, a safe region that lets a thread block without
stalling its collector, and `std/net` and `std/http` above them. Session 8
delivered item 9: eight more integer types, the bit operators, and literals
that take the type they are used at — which is what makes item 10 writable.
Session 9 began item 10 and delivered its symmetric half: byte buffers, the
system's generator, SHA-2, HMAC, HKDF, ChaCha20-Poly1305 and AES-GCM, each
against its published vectors — and, because writing them is what finds the
holes, the one thing the language turned out to be missing, a `const` table.
Session 10 delivered item 10's stages two and three, the asymmetric half:
X25519 and P-256 for key agreement, a fixed-width bignum with Montgomery
arithmetic, and RSA PKCS#1 v1.5 and PSS verification above it — all of it W#,
with no compiler change at all, because a 32-bit limb is what makes the 64x64
product item 9 reserved a place for unnecessary. Session 11 finished it: stage four
is the protocol — Ed25519 and ECDSA, a strict DER reader, the record layer and
the handshake at both ends, replayed against RFC 8448's published traces byte
for byte — and stage five is the certificate, so `http.get("https://…")` now
returns a page. Sessions 12 and 13 delivered item 11, **ingot**: a program that
can read its own command line and walk a directory, TOML 1.0 and a
content-addressed store, a PubGrub resolver that answers a conflict with the
derivation that caused it, a git client that speaks smart HTTP rather than
shelling out — and then the one branch in the loader and the two table keys in
the type checker that turn all of that into `@import("acme/json")`.

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
| `std/fs` | `mkdir`, `rmdir`, `remove`, `rename`, `is_dir`, `size`, `read_dir`, `mkdir_all`, `remove_tree` (item 11) |
| `std/os` | `args`, `get`, `home`, `temp_dir` (item 11) |
| `std/path` | `join`, `dirname`, `basename`, `extension`, `is_absolute`, `normalise` (item 11) |
| `std/toml` | TOML 1.0.0, read and written: `parse`, `write`, the `Value` lattice, `get`/`set`/`lookup` (item 11) |
| `std/inflate` | DEFLATE and the zlib wrapper, as a cursor: `zlib`, `raw`, `adler32` (item 11) |
| `std/http` | the 27 status types, moved out of the global namespace; since item 8, an HTTP/1.1 client and server over `std/net` |
| `std/broker` | `Topic[M]`, `Consumer[M]` and `topic`, `publish`, `subscribe`, `next`, `commit`, `seek`, `len` |
| `std/net` | `Socket`, `Listener`, `Poller`, `Event`, `Datagrams`, `Peer`, `Datagram` and `connect`, `listen`, `accept`, `read`, `write`, `write_all`, `read_exactly`, `read_all`, `set_nonblocking`, `poller`, `watch`, `wait`, `udp`, `send_to`, `receive`, `reply`, `close` (item 8); and since item 10, `read_into`, `write_bytes`, `write_all_bytes`, `read_exactly_into` over a `[]u8` |
| `std/bytes` | `[]u8` as a buffer and the bridge to `str`: `new`, `of`, `to_str`, `slice`, `concat`, `copy`, `fill`, `xor`, `equal`, the big- and little-endian word accessors, `to_hex`, `from_hex` (item 10) |
| `std/hash` | SHA-256, SHA-384, SHA-512 one-shot and incremental; `hmac`, `hkdf_extract`, `hkdf_expand` (item 10); SHA-1, for git's object ids (item 11) |
| `std/cipher` | ChaCha20, Poly1305, ChaCha20-Poly1305; AES-128/256, GHASH, AES-GCM (item 10) |
| `std/crypto` | `random` — the system's generator (item 10) |
| `std/time` | `now` — seconds since the Unix epoch (item 10) |
| `std/bignum` | fixed-width limbs and Montgomery arithmetic: `from_be`, `to_be`, `cmp`, `add`, `sub`, `mont`, `mont_mul`, `mont_add`, `mont_sub`, `to_mont`, `from_mont`, `modexp` (item 10) |
| `std/curve25519` | `x25519`, `x25519_base` (item 10) |
| `std/nistec` | the NIST prime curves: `p256`, `p384`, `derive`, `ecdh`, `valid`, `ecdsa_verify` (item 10) |
| `std/rsa` | `public_key`, `verify_pkcs1`, `verify_pss` (item 10) |

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
- **TLS arrived in item 10** and turned `https://` from `error.NotSupported`
  into a connection. It cost `std/http` a field on `Conn`, a branch in
  `parse_url` and four call sites, which is what "a TLS connection is a
  `Socket` by another name" was a bet on.
- **No connection pooling.** The HTTP client opens a socket per request and
  sends `Connection: close`, which is the honest shape for a client with no
  pool. `Conn`, `send_request` and `read_response` are exposed so that a
  protocol wanting to reuse a socket can.
- **A failed request leaks its socket.** W# has no `defer`, so a `try` that
  leaves `http.request_with` early skips the `close` below it. The process
  closes everything at exit, so this is a leak within one run rather than a
  leak -- but a TLS connection is a much more expensive thing to leak than a
  socket was, and this is the first place that shows.

---

## 9. Sized and unsigned integers, and bitwise operators — **done**

`i64` is the right default and was the wrong *only* choice the moment a program
computed on bytes rather than merely moving them. SHA-256 is addition modulo
2^32, ChaCha20 is 32-bit add, xor and rotate; neither could be written here.
`tests/cases/chacha_quarter.ws` is now RFC 8439's quarter-round test vector, in
W#, and it is the shortest statement of what this item was for.

### What was built

| Piece | Where |
|---|---|
| `i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64`, as one parameterised constructor rather than eight variants | `wsharp-sema/src/ty.rs` — `IntTy`, `TyCon::Int` |
| `& \| ^ << >> ~` and their compound forms, at Zig's relative precedence | `wsharp-syntax/src/{token,lexer,parser,ast}.rs` |
| `>>` arithmetic on a signed type and logical on an unsigned one; the four ordering comparisons and both divisions likewise | `wsharp-codegen/src/lower.rs` — `NumKind`, `binary`, `checked_div` |
| `comptime_int`: a literal takes the type it is used at and defaults to `i64` | `infer.rs` — `Constraint::IntLiteral`, `int_literal`, `default_int_ty` |
| Scalars packed at their natural size and alignment, through one `place` | `wsharp-sema/src/layout.rs` — `size_of`, `align_of`, `place` |
| `u32(x)`, `i64(x)`, `f64(n)` — conversions written, never inferred | `infer.rs` — `infer_convert`; `lower.rs` — `convert` |
| `Integer` beside `Number`, and abstract types ordered by their member sets | `wsharp-runtime/src/builtins.rs` — `abstract_types`; `ty.rs` — `is_sub_ty` |
| `std/bits` — `rotl` and `rotr`, generic over `Integer`, lowered inline | `builtins.rs` — `BuiltinTy::IntVar`; `lower.rs` — the `BITS_MODULE` arm |
| `print_uint` and `str.from_uint`, for the half of `u64` an `i64` cannot hold | `builtins.rs`, `strings.rs` |

### Decisions worth recording

- **Unsigned arithmetic wraps, and so does signed.** The wrapping is the point
  for unsigned — SHA-256 *is* addition modulo 2^32, so a checked `+` would make
  it unwritable — and for signed it is what the language already did: only
  division ever panicked here, and it still does. What changed is that the
  check is now per width and is skipped entirely for unsigned division, which
  cannot overflow. The panic no longer says `i64::MIN`, because an `i32` can
  reach it too.
- **A literal stopped being an `i64` without suffixes.** An integer literal
  gets a fresh type variable and a constraint, so `0xff` is a `u8` in one place
  and a `u32` in another. Three things then had to settle it early, and each is
  a place where "an integer literal" is not an answer: an overloaded call
  chooses *by* argument type; a coercion into a `?i64` must wrap the literal
  rather than let it become one; and a literal beside an equally undecided
  operand would otherwise merge with it into a variable that nothing pins until
  the end of the binding group. All three default it exactly as the language
  always did, so no program that compiled before means anything different now.
- **A literal too large for an `i64` defaults to `u64`.** The lexer's bound
  moved from `i64::MAX` to `u64::MAX` for the same reason: a literal has no
  type yet, and a 64-bit mask should not have to name its type to be written
  down at all.
- **A minus sign on a literal is part of the literal.** Without that, `-128` is
  the negation of `128` and does not fit an `i8` — the one value each signed
  type has that its positive twin does not would be unwritable. It also retires
  the `- 1` dance `panic_div_overflow.ws` had to document.
- **Unary `-` requires a signed type.** `-x` on a `u8` is not an error the
  machine reports; it is 256 - x. Rejecting it is what keeps the widened
  `Number` honest, and it is the same rule that already rejected `%` there.
- **`Number` lists every numeric type, and `Integer` was added beside it.**
  That is what makes `math.min` one function rather than nine. The price is
  stated rather than hidden: a body annotated `Number` must work for *every*
  member, so it may not use `%` and may not negate. `Integer` is what such a
  body claims instead. Abstract types are now ordered by their member sets
  rather than by identity — `Integer ⊑ Number` because every type it lists is
  one `Number` lists too — which is one line in `is_sub_ty` and gives
  specificity everything it needs. Two abstract types with identical members
  would be mutually more specific, so a test asserts the table is a strict
  lattice.
- **Rotation is a builtin, and an inline one.** It is one instruction on both
  targets and is what every one of these algorithms is spelled in, so leaving
  the code generator to recognise a shift-shift-or pattern would mean sometimes
  missing it. It cannot be an `extern "C"` function either, because a Rust one
  cannot be generic over the width — so `BuiltinTy::IntVar` gives it a type
  constrained to `Integer` and `Trans::call` lowers it inline, exactly as
  `array.new` already was.
- **The shift amount is masked to the operand's width.** Not a choice so much
  as a discovery: Cranelift documents it for `ishl`/`ushr`/`sshr`, and its
  constant folding and both backends do it for `rotl`/`rotr` as well. `x << 64`
  on a `u64` is `x`, not undefined, and a case pins it.
- **Conversions are written, never inferred.** A silent widening is how a
  32-bit hash becomes a 64-bit one that is right for a while. `u32(x)`
  truncates and says so; a float-to-integer conversion saturates rather than
  trapping, so it is total — a value too large clamps and a NaN is zero.
- **Cranelift is stricter than its own documentation.** Two things cost time
  and are worth writing down: `iconst` demands a *zero-extended* immediate, so
  `iconst.i32 -1` is a verifier error and a negative literal has to arrive
  masked; and `uextend`/`ireduce` are strictly wider/narrower despite
  `uextend`'s doc claiming same-width is a no-op, so a conversion between two
  types of the same width must emit nothing at all rather than ask for one.

### What packing changed, and what it uncovered

A scalar now occupies its natural size at its natural alignment, so `[]u8` has
a stride of one — without which every buffer in item 10 would be eight times
too large — and a struct of bytes costs bytes. A *tagged* value keeps a whole
word per slot, because slot `i` of a value living at `base + i*8` is what three
separate pieces of code depend on: the stride `load_at` and `store_slots` walk,
and the division `repr::pointer_slots` uses to turn a byte offset back into a
slot index. Two tests state that invariant rather than leaving it in a comment.

Alignment is not cosmetic here. Every load and store through these offsets uses
Cranelift's `trusted` memory flags, whose `aligned` bit lets the instruction
"trap or return a wrong result if the effective address is misaligned" — so
packing without aligning would have made that flag a lie.

Doing it turned up two things:

- **`layout::place` was not the single definition CLAUDE.md claimed.** Four
  more hand-rolled `offset += size_of(...)` loops existed: struct fields in
  `infer.rs`, and three separate copies for closure captures — the layout
  registered with the runtime, the prologue that reads captures out, and the
  constructor that writes them in. Those three had to agree byte for byte and
  did so only by being written the same way three times. All four now call
  `place`, so the agreement is structural.
- **The worker argument buffer was reading uninitialised memory.** A value
  narrower than a word writes only part of one, and `rpc::pack` reads whole
  words and sends them to another thread. That was already true of `bool` and
  of every option tag; it was simply never exercised, because no test passed
  such a value to a worker. `tests/cases/narrow_worker.ws` is that test, and
  the buffer is zeroed before anything is written to it.

### What is left

- **`math.abs` and `math.sign` are still `i64`/`f64` overloads.** They cannot
  become one generic over `Number`, and the reason changed: it used to be that
  an integer literal was an `i64`, which `comptime_int` has retired; it is now
  that negation is meaningless on an unsigned type. A narrow signed value needs
  a conversion.
- **An array index is still an `i64`.** "Every literal index works" is true and
  is not the case that chafes; writing item 10's ciphers found the one that
  does, and it is a *byte-valued* index -- `table[b]`. `i64(b)` covers it, and
  it turned out to be barely met, because a table indexed by a secret byte is
  the thing constant-time code must not do anyway.
- **No `u128`, and no `usize`.** The second is deliberate — this language has
  no pointer arithmetic to size, and a type whose overload depends on the
  target is a type whose overflow does. The first was an open question, and
  item 10 answered it: Poly1305's 130-bit accumulator and GHASH's
  multiplication in GF(2^128) are the two places a 128-bit type is usually
  reached for, and both are written without one — five 26-bit limbs in `u64`s
  for the first, two `u64` halves and 128 shifts for the second. Neither is a
  workaround; both are the shape a portable implementation has anyway. What is
  still missing is a 64x64 -> 128 product, and this used to name RSA's `modexp`
  as the one place it was genuinely wanted. **Stage three of item 10 wrote that
  `modexp` and did not want it.** A 32-bit limb makes `t + a*b + carry` at most
  `2^64 - 1` exactly, so the whole bignum is ordinary `u64` arithmetic;
  `bits.mulhi` would halve the limb count and buy nothing measurable, and is
  still one table row and one arm in `lower.rs` if a caller ever appears.
- **An overload set distinguished only by integer width needs the conversion
  written at the call.** A literal argument settles to its default before the
  overload is chosen, because which overload is meant is a question about the
  argument's type.


## 10. TLS 1.3, written in W# — **done**

Item 8 left `https://` as `error.NotSupported` rather than a connection that
quietly speaks the wrong protocol. Removing it is what this item is for -- and
what the package manager needs before it can fetch anything from a host it did
not already trust.

It was also by a wide margin the largest item in the tree, so it was built in
stages rather than pretended into one commit. **Stage one is the byte plumbing
and every symmetric primitive TLS 1.3 uses. Stages two and three are the
asymmetric half: two curves for key agreement, and a bignum and RSA for
verifying a certificate's signature. Stage four is the protocol: two more
signature schemes, the record layer, and the handshake. Stage five is the
certificate: X.509, a chain, three root stores, and `https://`.**

### Why in W# rather than in the runtime

The rule that governs the boundary would *permit* the other answer: a cipher is
a pure byte-to-byte transform, which is exactly what a builtin may be. So the
reason is not the collector's; it is that a language which cannot express
SHA-256 has a hole in it, and the fastest way to find out where the hole is, is
to try. The handshake and X.509 have to be W# regardless -- both build object
graphs, and item 6's rule sends those to `.ws` files.

**Trying found exactly one hole, and it is now closed.** Every crypto primitive
in the world is written around a table of constants -- SHA-256's sixty-four
round words, SHA-512's eighty, AES's round constants, a hex alphabet -- and a
top-level `const` could only be a literal, so not one of them could be written
down. Nothing else was missing. That is a better result than the item expected,
and the fix is described below.

### Stage one: the byte plumbing, and every symmetric primitive

#### What was built

| Piece | Where |
|---|---|
| A top-level `const` array of scalars, emitted as immortal data | `infer.rs` — `const_array`; `codegen/src/lib.rs` — `define_arrays` |
| `[]u8` as a buffer, and the bridge to and from `str` | `std/bytes.ws`, `bytes.rs` |
| Byte-oriented sockets: read into a buffer, write out of one | `std/net.ws` — `read_into`, `write_all_bytes`; `net.rs` |
| The system's CSPRNG, on three arms | `sys/{linux,bsd,windows}.rs` — `random`; `crypto.rs` |
| The wall clock, likewise | `sys/*` — `wall_clock_secs`; `std/time.now` |
| SHA-256, SHA-384 and SHA-512, one-shot and incremental | `std/hash.ws` |
| HMAC and HKDF, written once over a value describing the hash | same |
| ChaCha20, Poly1305 and ChaCha20-Poly1305 | `std/cipher.ws` |
| AES-128 and AES-256, GHASH, and AES-GCM | same |
| Published vectors for every one of them | `tests/cases/{hash_*,cipher_*,bytes_ops,crypto_random}.ws` |

#### Decisions worth recording

- **Constant time is a construction, not a guarantee, and the code says so.**
  W# compiles through Cranelift, which is free to turn a branchless expression
  into a branch and a select into a jump, and there is no `black_box` to pin a
  secret away from an optimisation the compiler invented. So the primitives are
  written constant-time *by construction* -- no secret-dependent indices, no
  early-exit compares -- and this is stated as a best effort against a local
  attacker rather than a promise. Anyone who needs the promise needs a reviewed
  C library behind an FFI, which is a different item.
- **AES has no S-box table, and GHASH has no multiplication table.** This is
  the visible cost of the paragraph above. A 256-byte S-box indexed by a byte
  of the state is indexed by a byte that depends on the key, and which cache
  line that touches is exactly what a timing attack reads -- so `sbox` inverts
  in GF(2^8) by exponentiation instead, which is about a hundred times slower
  and touches the same instructions whatever the input. GHASH is 128 shifts and
  exclusive-ors for the same reason, and needs no `u128` as a bonus.
- **A `const` array of scalars is a literal, not a computed global.** The
  restriction it lifts was never about arrays: it was about needing storage and
  a startup initialiser, and about the collector needing globals as roots. An
  immortal array of scalars needs neither -- it is a string literal with a wider
  element, emitted by the same code path, carrying the same `FLAG_IMMORTAL`, and
  holding nothing the collector has to trace. Writing an element of one through
  the `const`'s own name is rejected, because a top-level `const` is shared by
  every worker and W# has no mutable globals.
- **A builtin may write an array it did not allocate.** The standing rule is "a
  builtin may read and write bytes; anything that moves a *reference* is written
  in W#", and a `[]u8` holds no references, so a `memcpy` into one is on the
  permitted side of it. What a builtin still may not do is *allocate* an array:
  `array.new` is lowered inline because only the call site knows the element
  type, and so the stride and the type id to stamp. Every entry point in
  `std/bytes` is therefore W# allocating and Rust filling.
- **The byte-oriented socket API reads into a buffer rather than returning
  one.** Forced by the rule above, and better than the alternative anyway: the
  `str` API allocates a fresh object per read, which is right for a protocol
  made of lines and wrong for one made of 16 KiB records. A connection can now
  keep one buffer for its whole life. The `str` API is untouched, and `std/http`
  did not change.
- **The generator is the system's.** `getrandom` on Linux, `arc4random_buf` on
  the BSDs, `BCryptGenRandom` on Windows -- a fourth `#[link]`, since neither
  kernel32 nor ws2_32 has it. A TLS stack is the last place to be clever about
  entropy: the kernel has it, and it knows things this process cannot, such as
  that the machine forked or was restored from a snapshot. The call blocks until
  the pool is initialised, which is correct and is safe here because it is made
  inside a safe region.
- **HMAC is written once, over a value.** The three things it needs to know --
  block size, digest size, and how to hash -- are three fields of a `Hash`
  struct, the third an ordinary function value. An overload set would have read
  better and does not work: which overload is meant is a question about a
  parameter's *type*, and inside a body generic over the algorithm there is no
  type yet to ask about. This is the same shape, for the same reason, that made
  `for` ask a type's own module how to walk it.
- **AES decryption is not written.** GCM is counter mode and encrypts even to
  decrypt, so the inverse cipher has no caller -- and unreachable code in a
  security-critical file is exactly the shape a bug hides in.
- **A digest allocates once, not once per block.** The message schedule and the
  working words live in the state. This is not tuning: the whole case suite runs
  a second time under `--gc-stress`, which collects at *every* allocation, so a
  temporary inside a block loop is the difference between a test and a timeout.
  It is checkable, and checked -- a 64-byte digest and a 64 KiB one both cost
  six objects.

### Stages two and three: two curves, a bignum, and RSA

The asymmetric half, and it landed in one commit rather than two because the
two stages turned out to want the same code. Stage two is X25519 and P-256, the
two key-agreement groups a TLS 1.3 client offers; stage three is a bignum and
RSA signature verification, which is what reading a certificate chain needs.
P-256's field is that bignum's Montgomery multiplication at eight limbs, so
splitting them would have meant either writing a second modular multiplication
or moving the bignum across the boundary anyway.

#### What was built

| Piece | Where |
|---|---|
| Fixed-width limb arithmetic, 32 bits to a limb, with the carry and the borrow as return values | `std/bignum.ws` |
| Montgomery form: CIOS multiplication, modular add and subtract, and `R^2 mod n` without a division | same |
| `modexp`, square and multiply, for a public exponent | same |
| X25519, on TweetNaCl's sixteen-limb field, with the ladder of RFC 7748 section 5 | `std/curve25519.ws` |
| The small-order check, on the output rather than on the input | same |
| P-256: Jacobian points, `dbl-2001-b` and `add-2007-bl`, and a Montgomery ladder over them | `std/nistec.ws` |
| Key-share validation: length, form, both coordinates below p, and on the curve | same |
| RSA PKCS#1 v1.5 and PSS verification, generic over the hash by value | `std/rsa.ws` |
| MGF1, and the three DigestInfo prefixes as `const` tables | same |
| Published vectors for the curves, and vectors built twice for RSA | `tests/cases/{bignum_ops,curve25519_*,p256_*,p384_*,rsa_*}.ws` |

#### Decisions worth recording

- **A limb is 32 bits, and so `bits.mulhi` was never needed.** Item 9 left a
  place for a 64x64 -> 128 product and named RSA's `modexp` as the one caller
  that genuinely wanted it. It does not: with 32-bit limbs the largest quantity
  any of this computes is `t + a*b + carry`, which is at most
  `(2^32-1)^2 + 2*(2^32-1)`, and that is exactly `2^64 - 1`. Not one bit spare
  and not one needed. **So both stages are pure W#** -- the only Rust in the
  whole change is four lines registering four modules, and the language grew
  nothing at all.
- **One Montgomery multiplication serves both a curve and a signature scheme.**
  P-256's modulus is a Solinas prime and the quick way to reduce modulo it is a
  page of shifted additions with signed corrections. That page exists only to
  be faster, in a file where being wrong is a security problem and where
  nothing is fast enough for the difference to matter. Sharing `bignum`'s CIOS
  is the same trade the computed AES S-box was.
- **Nothing divides.** The one place a bignum usually needs a remainder is
  `R^2 mod n`, and that is `64*limbs` doublings with a masked conditional
  subtract instead. RSA verification needs no remainder and neither does the
  curve, so a division would have been code with no caller -- which is exactly
  why `std/cipher` still has no AES decryption.
- **RSA verifies and never signs, and that changes what the code is.** TLS 1.3
  does no RSA key exchange, so the private exponent has no caller. Everything
  that is left is public: the modulus, the exponent, the signature and the
  message. `modexp` is therefore an ordinary square-and-multiply that says out
  loud that it is not constant time, and it is seventeen multiplications rather
  than two thousand, because a public exponent is 65537.
- **The encoded message is built and compared, never parsed.** Every historical
  PKCS#1 v1.5 break is an *acceptance* bug -- a verifier that walks the encoding
  left to right and is content with eight bytes of padding and a correct
  DigestInfo, whatever follows. There is exactly one byte string a valid
  signature can decrypt to, so producing it and comparing is both the shortest
  implementation and the strictest one. `rsa_pkcs1.ws` includes that forgery.
- **X25519's field is sixteen limbs of sixteen bits.** TweetNaCl's shape rather
  than ref10's ten limbs of twenty-five and a half. Every partial product is at
  most 2^32, sixteen of them 2^36, and the fold that wraps 2^256 back down
  multiplies by 38 to reach about 2^41 -- twenty-two bits of headroom for a
  schoolbook multiplication anyone can check by reading it. That is the third
  time this item has taken the auditable side of that trade, after AES's
  computed S-box and GHASH's 128 shifts, and it is the same argument each time.
- **X25519's weak-point check is on the output.** Curve25519 has a subgroup of
  order eight, and a peer sending a point from it forces the shared secret to
  zero whatever the local key is. A check against a list of the small-order
  *encodings* misses `p`, `p+1` and `p-1`, which are only small once they are
  reduced; a check that the answer is not zero cannot miss any of them.
  `curve25519_reject.ws` is all seven.
- **P-256's check is on the input, and for the opposite reason.** Its cofactor
  is one, so there is no small subgroup to land in and nothing to catch on the
  way out. What there is instead is the invalid-curve attack: a point that is
  not on P-256 at all but is on some curve with a smooth group, which leaks the
  private key a few bits per exchange. So the peer's key share is checked to be
  a well-formed uncompressed point, with both coordinates below the modulus,
  that satisfies the curve equation -- and RFC 8446 section 4.2.8.2 says so too.
- **`pt_add` is exception-free because of the ladder, not because of the
  formula.** `add-2007-bl` cannot add a point to itself, and the Montgomery
  ladder is what rules that out: its two accumulators satisfy `R1 - R0 = P`
  throughout and `P` is never the identity, so they are never equal. The case
  that *is* reachable is an operand at infinity -- `R0` starts there -- and that
  is settled by selecting the other operand with a mask, because which one it
  was is a fact about the scalar. The argument is in the code, because it is
  the thing a reviewer has to check rather than read.
- **Scratch belongs to the caller, all the way down.** Every field and point
  routine writes into storage handed to it, and one `Work` is built per
  operation. The result is that an X25519 costs fourteen objects and a P-256
  exchange about sixty, whatever the 255 ladder steps inside them do -- which is
  what lets `curve25519_x25519.ws` keep RFC 7748's thousand-round iterated
  vector at full length and take the same two seconds under `--gc-stress` as
  without it. A ladder that allocated per step would be a quarter of a million
  collections there.
- **`array.len` is a builtin, and a builtin is a stack walk under
  `--gc-stress`.** `gc::checkpoint` runs at the top of every runtime entry
  point and validates every root the stack maps describe. That is the right
  thing for the collector and the wrong thing in a loop condition, so lengths
  are read once into a local and the inner loops call nothing at all. This is
  new, and general, and is now in CLAUDE.md.
- **Nothing new was asked of the language.** Stage one found exactly one hole
  and closed it; these two stages found none. Two curves, a bignum, two
  signature schemes and 2,100 lines of W# needed no lexer, parser, inference or
  code-generator change -- which is a better answer than item 9's "no `u128`"
  decision had any right to expect.

### Stage four: two signature schemes, the record layer, and the handshake

Stages two and three left the asymmetric primitives in place and nothing above
them. This is the protocol: the two signature schemes a TLS 1.3 client has to
verify besides RSA, the record layer, and ClientHello through Finished --
including HelloRetryRequest, which is the only part of the handshake that
happens twice.

It also has a **server**, which the item did not ask for. The reason is in the
testing section below and is worth stating here: a client tested only against
recorded bytes is a client whose *own* bytes nothing has ever read.

#### What was built

| Piece | Where |
|---|---|
| Ed25519, signing and verification, on the field `std/curve25519` already had | `std/curve25519.ws` |
| ECDSA verification, with arithmetic modulo the group order | `std/nistec.ws` |
| Shamir's trick, so a verification is one ladder rather than two | same |
| A strict DER reader: definite lengths, minimal encodings, no trailing data | `std/der.ws` |
| Public keys as a dispatch lattice, and `SubjectPublicKeyInfo` | `std/x509.ws` |
| The key schedule, `HKDF-Expand-Label` and every secret of RFC 8446 section 7.1 | `std/tls.ws` |
| The record layer: nonces, the header as additional data, padding, the size limits | same |
| ClientHello through Finished, both ends, with HelloRetryRequest and KeyUpdate | same |
| A blocking `Session` over a `net.Socket`, and nothing else that knows a socket exists | same |
| A growable byte buffer with length backpatching, which every message here is written with | `std/bytes.ws` |
| RFC 8448's traces, replayed byte for byte | `tests/cases/tls_{schedule,rfc8448,ecdsa}.ws` |
| A handshake between this library's two ends, over every suite and both groups | `tests/cases/tls_{loopback,socket}.ws` |
| Nineteen ways to be refused | `tests/cases/tls_reject.ws` |

#### Decisions worth recording

- **The core takes bytes in and hands bytes out.** `feed` is given whatever
  arrived and `pending` says what to send; nothing below `Session` knows a
  socket exists. That is not an abstraction for its own sake. A handshake is a
  *negotiation*, so a blocking `connect` writes its ClientHello and then waits
  for a reply -- and one thread cannot be both ends of one, which is how every
  other networking case in this tree is written. Splitting the core out is what
  makes the whole of `tls_loopback.ws` single-threaded and deterministic, and
  it is what lets RFC 8448 be replayed with no I/O at all.
- **A cipher suite is a value, not an overload set**, for the reason
  `std/hash.Hash` is: the suite is chosen at run time by the peer, so inside
  the record layer there is no type to dispatch on. This is the third time that
  shape has been forced rather than chosen.
- **A public key *is* an overload set**, and that is the same argument the
  other way. A key's algorithm is known when it is parsed and never changes, so
  `RsaKey`, `EcdsaP256Key` and `Ed25519Key` are subtypes of `SigKey` and
  `verify_signature` is three functions -- the dispatcher picks by the type id
  in the header, and a fourth algorithm is a struct and a function rather than
  an edit to a chain.
- **`std/x509` sits below `std/tls`, not beside it.** A client verifies a
  CertificateVerify with a key out of a certificate, so one module has to name
  the other's types, and W# has no re-export. The one that owns `SigKey` is the
  one everything imports, and a public key comes from a certificate -- so the
  arrow points that way and there is no cycle to break.
- **Ed25519 signs and RSA still does not.** The rule has not changed: a
  signing key is written when it has a caller. This one has -- the server's
  CertificateVerify -- and Ed25519 is the scheme this library can produce
  without a constant-time exponentiation it does not have and without a nonce
  whose generation is the classic way to lose a private key.
- **ECDSA verifies and does not sign**, for the reason RSA does not, and its
  code is allowed to be different *because* everything it touches is public.
  `r`, `s`, the digest and the peer's key all travel in the clear, so a branch
  on any of them leaks nothing -- which is what makes an addition with real
  cases in it and a double-and-add that skips a zero digit legitimate here and
  not in `ecdh`.
- **The window landed where it is free, and the ladder was left alone.** Item
  10 recorded that P-256's scalar multiplication is a bare ladder and that a
  four-bit window would be a quarter of the additions. Verification is now a
  second caller and gives it a reason, so `u1*G + u2*Q` is Shamir's trick: 256
  doublings and about 192 additions where two ladders would be 512 of each.
  Measured, a verification costs about 4.8 ms against an ECDH's 3.8, where two
  ladders and an addition would be nearer 8.
  The *secret* ladder is untouched, and that is a decision. A windowed ladder
  can reach a step where the accumulator equals the table entry being added,
  which `pt_add` cannot do; ruling it out needs either complete formulas or a
  mask over an exception, where the Montgomery ladder rules it out by
  construction. An exchange costs about four milliseconds against a network
  round trip, and this file has twice already taken the auditable side of that
  trade.
- **DER is read strictly, and that is the feature.** A certificate is a signed
  byte string, so any encoding this accepts but a second implementation
  re-encodes differently is a signature that has been moved onto a different
  meaning. Indefinite lengths, non-minimal lengths, integers with a spare
  leading zero and trailing bytes are all refused, and `der_reject.ws` gives
  each its own line. There is no writer, for the reason `std/cipher` has no AES
  decryption.
- **PKCS#1 v1.5 may sign a certificate and may not sign a CertificateVerify.**
  RFC 8446 section 4.4.3 says so, and the reason is worth keeping in the code:
  without the check, a signature made by a TLS 1.2 server could be replayed as
  a 1.3 one. The 64 spaces and the context string in front of the signed
  content are the other half of the same defence.
- **The transcript is a buffer, not a running hash.** `std/hash`'s incremental
  state has no clone, and the transcript is hashed at five different points; but
  the deciding reason is that the *hash itself* is not known until the
  ServerHello has been read, and the ClientHello comes before it. Holding the
  bytes and digesting on demand is what a client that offers both SHA-256 and
  SHA-384 suites has to do anyway.
- **A HelloRetryRequest is the only message that rewrites history.** The first
  ClientHello is replaced in the transcript by a synthetic message holding its
  hash, which is what lets a server keep no state between the two flights. Both
  ends implement it, and the server here sends a cookie and checks the echo --
  not because it needs to, since it is stateful, but because a client's cookie
  handling is otherwise code nothing runs.
- **Compatibility mode is sent and accepted, and never hashed.** A 32-byte
  session id and a ChangeCipherSpec record make a 1.3 handshake look like a
  resumed 1.2 one to a middlebox. The record is not part of the transcript, and
  one that is not a single `0x01` is refused.

### Stage five: certificates, a chain, three root stores, and `https://`

Item 8 left one line in `std/http`: `https://` was `error.NotSupported`
"rather than a connection that quietly speaks the wrong protocol". This is that
line removed, and everything a client needs before removing it is honest --
which is a certificate parser, a chain, and somewhere to get the anchors from.

#### What was built

| Piece | Where |
|---|---|
| The certificate around the key: validity, names, and the four extensions that change the answer | `std/x509.ws` |
| Chain building, with `pathLenConstraint`, `basicConstraints` and `keyUsage` enforced | same |
| `subjectAltName` matching, with one wildcard in the leftmost label and nowhere else | same |
| PEM, and the candidate paths every Linux and BSD keeps a bundle at | same |
| The platform stores: Security.framework on macOS, `CertOpenSystemStoreW` on Windows | `sys/{bsd,windows}.rs`, `crypto.rs` |
| Base64, which is the whole of what PEM is | `std/bytes.ws` |
| `https://`, as a field on `Conn` and four call sites | `std/http.ws` |
| A bound on the RSA public exponent | `std/rsa.ws` |
| Certificates minted for the tests, each wrong in one way | `tests/cases/x509_*.ws` |
| A request over TLS to a server this library also wrote | `tests/cases/https_loopback.ws` |

#### Decisions worth recording

- **`subjectAltName`, and never the common name.** Every browser stopped
  looking at the CN years ago and the reason is worth keeping: a CN is a
  display string with no structure, so a certificate for
  `CN=example.com, O=Some Company` and one issued to a company literally named
  `example.com` are the same bytes to a naive reader. A `dNSName` says what it
  is. One wildcard, covering the whole of the leftmost label and nothing else,
  so `*.a.com` is not a certificate for `a.com` and not one for `c.b.a.com`.
- **A name is compared as bytes.** RFC 5280 has rules about folding case in a
  `PrintableString`, and no authority relies on them: an issuer name in a
  certificate and the same authority's subject name in its own are the same
  encoding, because one was copied from the other. Comparing encodings cannot
  accidentally make two different names equal, which the folding rules can.
- **An algorithm this library cannot verify is not a reason to refuse a
  certificate.** It was at first, and that was wrong in a way the system store
  made obvious: a *trust anchor's* own signature is never checked -- it is
  trusted for being in the store, not for having signed itself -- so refusing
  one for its signature algorithm drops authorities for a reason that never
  applies to them. The scheme becomes zero instead, `verify_signature` refuses
  it, and a chain that actually needs the signature still fails. That change
  took the local store from 79 usable roots to 83.
- **What was left out was 36 of them, and the reason was P-384** -- until it
  was written. Thirty-five of this machine's 119 root certificates have
  `secp384r1` keys, and until `std/nistec` carried a second curve none of them
  could be used and no chain through a P-384 intermediate could be verified,
  which is most of the modern web. Fixing it is stage two's decision paying
  off: because P-256 was written against `std/bignum`'s generic Montgomery
  multiplication rather than a Solinas reduction for one prime, the second
  curve is five tables and a limb count. Eighty-three usable roots became a
  hundred and eighteen. The one still refused has a P-521 key, whose 521 bits
  are not a whole number of 32-bit limbs -- the one place the shape of this
  bignum shows through.
- **A store is a bag, a chain is a structure.** A certificate in the store that
  this library cannot read is dropped and the rest are used; a certificate *in
  a chain* that it cannot read is a refusal. Those are different questions --
  one fewer authority to trust against one connection to a peer whose identity
  cannot be established -- and answering them the same way would either make a
  container with an odd root unusable or make a broken chain acceptable.
- **An unknown extension marked critical is a refusal.** That is what critical
  means: the issuer saying "refuse this certificate rather than ignore me". It
  is the one place this parser is strict about something it could shrug at, and
  it is the historical shape of several real failures.
- **The clock is read at the handshake, not kept in the configuration.** A
  configuration holds the parsed trust store because parsing it is expensive; a
  long-lived process holding a *time* would go on believing a certificate that
  expired while it was running.
- **A subtype and an overload set were tried for `Conn` first, and do not
  work.** `TlsConn : Conn` with `conn_read` and `conn_write` as overloads is
  what this language is for, and it is what `https://` looks like it should be.
  It fails on a rule that is not going to change: **a dispatched call has one
  type, so every overload must share it** -- and reading through TLS can raise
  everything a handshake can, two dozen names, where reading a socket raises
  ten. Making them agree means writing the whole set out twice and keeping two
  copies in step, or catching inside and answering with one flattened error,
  which throws away the reason a connection failed. An optional field and an
  `if` keep every error intact, and the comment in the file says so rather than
  leaving the next reader to rediscover it.
- **The root store is the fourth arm-shaped problem, and its arms have less in
  common than any before it.** macOS has a keychain and Windows a store API,
  and both hand back a blob of length-prefixed DER that W# cuts up -- the
  runtime touching bytes and W# building the objects, which is the boundary
  rule this whole item is written to. Every Linux and BSD has a file instead,
  at a path that differs by distribution, so the list of candidate paths lives
  in W# where it can be read rather than compiled in three times.
- **The RSA exponent is bounded at 2^32 + 1**, which is item 10's last deferred
  item. It is not a correctness fix: `modexp` costs one modular multiplication
  per exponent bit, so a certificate carrying a 2048-bit exponent is a peer
  deciding how much work this machine does.

### Stage five, second pass: the second curve

Stage five shipped with one limitation big enough to be worth its own section,
and this is it closed. Thirty-five of a typical machine's 119 root
certificates have P-384 keys; none of them could be used, and a chain through
a P-384 intermediate -- which is most of the modern web -- could not be
verified at all.

`std/p256` became `std/nistec`, and the module now carries three numbers in its
`Curve`: how many 32-bit limbs a field element takes, how many bytes a
coordinate is, and how many bits a scalar has. Everything below that is the
same code. Both curves are short Weierstrass with `a = -3`, so every formula
was already shared; what was hard-coded was the size.

- **This is the receipt for stage two's decision.** That stage chose
  `std/bignum`'s generic Montgomery multiplication over a Solinas reduction
  written for P-256's prime, and recorded the trade: "a page that exists only
  to be faster, in a file where being wrong is a security problem". Had the
  fast page been written, P-384 would have needed a second one -- different
  prime, different shifts, separately wrong. Instead it needed five byte tables
  and a limb count.
- **A key is two types, not one carrying a curve.** `EcdsaP256Key` and
  `EcdsaP384Key` are both subtypes of `SigKey`, so the dispatcher goes on doing
  the work and a third curve is a struct and a function rather than an edit to
  a chain. That is the same argument the whole `SigKey` lattice is.
- **In X.509 the algorithm identifier names only the hash.**
  `ecdsa-with-SHA384` says nothing about which curve signed, so a P-256 key
  signing with SHA-384 is an ordinary certificate and the curve has to come
  from the key. TLS's `SignatureScheme` conflates the two; a peer that names
  the wrong one simply fails to verify, which is the answer a stricter check
  would give anyway.
- **P-521 is still refused, and it is the one curve a third table would not
  buy.** 521 bits is not a whole number of 32-bit limbs, so the top limb is
  nine bits wide and `std/bignum`'s "a number's length *is* its width"
  invariant would need a mask everywhere it is read.

Eighty-three usable roots became a hundred and eighteen, and `example.com` --
whose chain goes through two P-384 intermediates and which was the worked
example of the limitation -- now answers.

### What being wrong costs here, and how that is paid

This is the first thing in the tree where being wrong is a security problem
rather than a crash. A collector bug shows up as a failing test; a wrong hash
shows up as nothing at all until something signs with it. The mitigation is not
cleverness, it is test vectors, and every primitive here has its published ones:
FIPS 180-4 for SHA-2, RFC 4231 for HMAC, RFC 5869 for HKDF, RFC 8439 for
ChaCha20 and Poly1305, FIPS 197 for AES, McGrew and Viega's original cases for
GCM, RFC 7748 for X25519 and RFC 5903 for P-256.

Two habits are worth writing down because both caught something. Vectors were
**checked against an independent implementation** rather than transcribed from
memory -- which found that a remembered RFC ciphertext was wrong and the code
was right, and would equally have found the reverse. And every AEAD has a case
for each *way* of being wrong: a changed ciphertext, a changed tag, changed
additional data that is not itself transmitted, the wrong nonce, the wrong key,
and a truncation that leaves no room for a tag.

**A protocol has published traces, and they are better than vectors.** RFC 8448
writes whole TLS 1.3 handshakes down as bytes -- every record, and every secret
behind them -- which makes three different kinds of test possible from one
document. `tls_schedule.ws` checks the key schedule one derivation at a time, so
a failure names which of the eleven is wrong rather than only that one is.
`tls_rfc8448.ws` drives the client with the recorded server flight and compares
every record it produces against the recorded one, byte for byte. And
`tls_ecdsa.ws` does the same over section 6, whose server signs with ECDSA
rather than RSA-PSS.

The ClientHello is handed to the client rather than built by it, through a
documented test hook, and that is a limitation worth stating: the recorded
client offers extensions this one does not, so a hello built here would be a
different message and nothing downstream could be compared at all. Everything
after the hello is this implementation. What that leaves untested is the bytes
this library *itself* produces first -- which is why stage four also has a
server. `tls_loopback.ws` runs the two ends against each other over all three
cipher suites, both groups and a HelloRetryRequest, and `tls_socket.ws` does it
once more over a real socket with the server on a worker of its own, which is
also two threads blocking in `read(2)` inside the collector's safe region.

**A certificate cannot be borrowed, so the fixtures are minted.** A real
certificate expires and takes the test with it, and the interesting cases --
expired, not yet valid, the wrong name, an intermediate that is not a
certificate authority, a `pathLenConstraint` violated, an unknown critical
extension -- do not exist in the wild to be borrowed anyway. So `x509_reject.ws`
has thirteen certificates made for it, each wrong in exactly one way, written
by a hand-rolled DER encoder in a node script because node cannot issue one.
Every certificate it produces is handed straight back to node's
`crypto.X509Certificate`, which is the independent check that what was written
is what was meant. The chain checks pass a *fixed* time rather than the clock,
so the case says the same thing whenever it is run.

And once, at the end, a real one: `http.get("https://www.google.com/")`
returning a 200 over a chain checked against this machine's own store is the
only thing that proves the root store, the parser, the chain builder and the
name check together. It is not a case -- the harness runs every example and a
network-dependent one would make CI depend on the weather -- so it is a command
in the README instead.

**RSA has no published vector this library could use**, because the ones that
exist are 1024-bit and SHA-1 and this only carries the three SHA-2 prefixes TLS
1.3 allows. So its vectors were *made*, and made twice: a key from one
implementation, every encoded message built from RFC 8017's text and signed with
the raw private exponent, and then every one of them handed back to that first
implementation, which agreed about all eighteen. That is what makes the forged
ones worth having -- and the forgeries are the point, because a verifier that
accepts too much passes every test written from the valid side. Between them
`rsa_pkcs1.ws` and `rsa_pss.ws` make twenty-two refusals, each a different way
of being wrong -- the AEAD discipline applied where acceptance rather than
rejection is the historical failure.

### What is left

`https://` works. `example.com`, `github.com`, `nixos.org`,
`www.cloudflare.com`, `www.google.com` and `crates.io` all answer, which
between them cover RSA, P-256 and P-384 chains and both AES suites. What is
left is a list of things that were left on purpose:

- **No P-521.** One root certificate in a typical store has such a key, and a
  chain through one cannot be verified. It is the one curve a third table would
  not buy: 521 bits is not a whole number of 32-bit limbs, so the top limb is
  nine bits wide and `std/bignum`'s "the length *is* the width" invariant no
  longer holds without a mask everywhere it is read. P-256 and P-384 between
  them cover a hundred and eighteen of a hundred and nineteen.
- **Nothing checks revocation.** No OCSP, no CRL, no stapling. A certificate
  that was issued and then withdrawn is still accepted until it expires, which
  is a real hole and a large piece of work -- OCSP is another protocol and
  stapling is another extension. Saying so is better than a half-check that
  looks like one.
- **No name constraints and no certificate policies.** Both are extensions a
  chain can carry, and both are *critical* when they appear, so a certificate
  carrying one is refused rather than misread. That is the safe direction and
  it does mean a handful of authorities cannot be used.
- **No ALPN.** `std/http` speaks HTTP/1.1 and nothing else, so there is nothing
  to negotiate yet; a client that wanted HTTP/2 would need the extension and a
  second protocol behind it.

Smaller things left behind these stages:

- **No 1.2-style RSA key transport and no RSA signing**, deliberately, per the
  decision above. If a signing key ever has a caller, it needs a constant-time
  `modexp` and the Chinese remainder theorem, and neither is written.
- **A public exponent is bounded at 2^32 + 1** -- done in stage five. `modexp`
  costs one modular multiplication per exponent bit, so an unbounded one is a
  peer deciding how much work this machine does.
- **P-256's *secret* scalar multiplication is still a bare ladder.** Stage four
  windowed the half of it that is public -- a verification is now one
  interleaved double-and-add rather than two ladders -- and deliberately left
  the other half alone: a windowed ladder can reach a step where the
  accumulator equals the table entry being added, which `pt_add` cannot do, and
  ruling that out needs complete formulas or a mask over an exception where the
  Montgomery ladder rules it out by construction. Worth revisiting only
  together with a complete addition formula.
- **No client certificates, no resumption, no 0-RTT.** A CertificateRequest is
  answered with an empty certificate list, which is what RFC 8446 requires of a
  client that has none; a NewSessionTicket is read and dropped. Both are
  features, not oversights: a resumption secret that is never used cannot be
  used wrongly.
- **A `Config` cannot restrict what is offered.** A client offers all three
  cipher suites and both groups, always. A caller that wants to insist on one
  has no way to say so, where a server does (`server_requiring`).

The decisions taken in advance about these two stages, and how they turned out:

- **TLS 1.3 only.** No 1.2, no fallback, no downgrade dance. A client that
  cannot talk to a 1.2-only server fails loudly against a server that should be
  upgraded, and every hour spent on 1.2 is an hour spent on the version with the
  worse security story.
- **The root store is a fourth arm-shaped problem**, and the exploration for
  that stage predicted it correctly. Most Linux distributions ship a concatenated PEM
  bundle, so a list of candidate paths tried with the existing `io.exists` and
  `io.read_file` covers Linux with no new syscall at all; macOS ships no such
  file and needs Security.framework, and Windows' store is not a file path, so
  neither would have been helped by a directory listing. Bundling a copy of
  Mozilla's list instead would make the build reproducible and the trust
  decisions stale, and staleness in a trust store is the failure mode that
  matters.
- **A TLS connection is a `Socket` by another name**, and that turned out to be
  exactly right: `https://` is a field on `Conn`, a branch in `parse_url`, and
  four call sites. The chunked decoder, the header parser and the status
  mapping did not change at all. What it is *not* is a subtype and an overload
  set, which is what the shape deserved and which the error sets forbid -- the
  reason is above, in stage five's decisions.
- **The wall clock now has its caller.** It went in with stage one against the
  day X.509 validity checking would want it; `std/tls` reads it once per
  handshake, at the moment the chain is checked rather than when the
  configuration was built.

---

## 11. Package management — **done**

Item 6 left one line: *"No package management. An import is a relative path or
a library one; there is nothing that fetches anything."* This is that, and it
has a name: **ingot**.

Five stages, as item 10 had five, and for the same reason: each one is worth
having before the next exists.

| Stage | What it is | State |
|---|---|---|
| One | `argv`, the environment, and a real filesystem | **done** |
| Two | TOML, the manifest and lockfile, and the content-addressed store | **done** |
| Three | Semantic versions, and a PubGrub resolver that explains itself | **done** |
| Four | Git spoken rather than shelled out to: inflate, pkt-line, a packfile | **done** |
| Five | The loader hook, and the re-export a package facade needs | **done** |

### Stage one — the language can see the world — **done**

Everything item 11 needs and nothing it is, which is what makes it worth having
on its own: a program can now read its own command line and walk a directory.

| Piece | Where |
|---|---|
| `mkdir`, `rmdir`, `remove`, `rename`, `is_dir`, `file_size`, `read_dir`, `env`, on three arms | `sys/{mod,linux,bsd,windows}.rs` |
| `std/fs` — the builtins, and `read_dir`, `mkdir_all`, `remove_tree` above them | `fs.rs`, `std/fs.ws` |
| `std/os` — `args`, `get`, `home`, `temp_dir` | `os.rs`, `std/os.ws` |
| `std/path` — `join`, `dirname`, `basename`, `extension`, `is_absolute`, `normalise` | `std/path.ws` |
| `wsharp run prog.ws -- a b c`, and `// args:` in a case header | `wsharp-cli/src/main.rs`, `tests/cases.rs` |

**`struct stat` is not declared anywhere, and that is the interesting decision.**
The obvious way to answer "is this a directory, and how big is it" is a
`stat(2)`, and the obvious way to bind one is to declare `struct stat`. That
struct has a different layout on macOS, FreeBSD, NetBSD and OpenBSD, and is a
versioned symbol on glibc whose shape differs by architecture — so the BSD arm
would carry four declarations, three of which nobody here can run. It is
`struct kevent` again, and it gets the same answer: pick the interface that
hides the difference. `is_dir` is `opendir` succeeding and `file_size` is
`lseek` to the end, and neither needs a field offset.

What made that possible was **dropping `modified`**. A timestamp is the third
thing a `stat` is for, and the store was going to use it to decide whether a
manifest was newer than its lockfile. It should not: a checkout does not
preserve mtimes and two machines do not agree about them. The lockfile records
the manifest's *hash* instead, which is both more correct and what removed the
struct.

`readdir` is where the difference could not be hidden, because a directory
entry is a struct and nothing wraps it. Each arm carries `d_name`'s offset and
nothing else — 19 on Linux, 21 on macOS, 24 on FreeBSD and OpenBSD, 13 on
NetBSD, 16 on DragonFly — which is one auditable fact per system rather than
five declared layouts, and which works because POSIX guarantees the name is
NUL-terminated. macOS needs one thing more: `readdir` is `readdir$INODE64` on
x86-64 and plain `readdir` on arm64, and linking the unsuffixed name on x86-64
gets the *old* directory ABI, whose `d_name` is at 8. Only arm64 macOS is in
CI, so that is a trap nothing here would have caught.

**Three more things fell out rather than being chosen.**

- **A builtin still may not allocate an array**, so `fs.raw_read_dir` and
  `os.raw_args` answer with one `str` of four-byte big-endian lengths and their
  bytes, and `std/os.unpack` cuts it up. That is `crypto.raw_system_roots`'s
  shape, and length prefixes rather than a separator mean the encoding says
  nothing about what a name may contain.
- **The command line is process-wide state.** `main` takes no arguments — the
  type checker says so — and the one word a compiled `main` receives is the
  closure environment pointer every W# function takes. So `os::set_args`
  publishes them into a `OnceLock` before anything is compiled, which is the
  third thing in this runtime allowed to be process-wide and qualifies for the
  same reason the other two do: frozen before any generated code runs.
- **The separator is `/` everywhere, Windows included.** Every Win32 path call
  accepts one, and one spelling is what keeps a lockfile written on one machine
  readable on another. A `\` that arrives from outside is normalised away and
  none is ever produced.

`std/fs.mkdir_all` checks before each `mkdir` rather than catching
`AlreadyExists`, because W# has no way to re-raise a caught error — which is
the first thing this item has found that the language cannot say, and it is
noted rather than fixed: the store publishes by `rename` precisely so that two
processes racing is not a case anything has to get right.

### Stage two — TOML, the store, and the tool — **done**

| Piece | Where |
|---|---|
| TOML 1.0.0, read and written | `std/toml.ws` |
| `str.parse_float`, the asymmetry `str.parse_int` had left | `strings.rs` |
| `ingot.toml` and `ingot.lock` | `ingot/manifest.ws` |
| The content-addressed store: the tree hash, an atomic install, `check`, `gc` | `ingot/store.ws` |
| One failure, with something to read on it | `ingot/fault.ws` |
| The verbs | `ingot/main.ws` |
| The tool: two hundred lines of Rust over them | `crates/ingot/` |
| A library module is read only if something imports it | `wsharp-cli/src/load.rs` — `add_library` |

`ingot init`, `add`, `remove`, `resolve`, `install`, `verify`, `list`, `why`,
`gc`, `store` and `run` all work, against path dependencies. A registry
dependency needs stage three and a git one needs stage four; both are refused
by name rather than as "unsatisfiable".

#### TOML, whole, because a subset is a promise the extension makes

The manifest format was the one thing this item said to decide deliberately
rather than by accident, and the decision went the harder way: **all of TOML
1.0**, not a line-oriented format of our own and not a subset. A reader that
takes three quarters of the grammar answers "syntax error" on a file every
other tool in the world reads, and the person holding it has no way to tell
which quarter they landed in.

So: dotted keys, all four string forms including the triple-quoted ones with
their line-ending backslash and their `\uXXXX`, integers in four bases with
underscores, floats with exponents, `inf` and `nan`, the four date and time
forms, arrays, inline tables, `[table]`, `[[array of tables]]`, and the
redefinition rules that are the part nobody expects to be hard. There is a
writer too, which `std/der` decided against on the grounds that nothing needed
one; something needs one here, and a lockfile written by a hand-rolled emitter
and read by a real parser is a bug that waits for the one entry with a
quotation mark in it. The round trip is the test.

Two things about writing it in W# are worth recording.

**A value is a lattice.** The language has no sum types, and what it has
instead is nominal subtyping with multiple dispatch — so `Value` is an empty
supertype, each shape is a subtype carrying its payload, and `as_int` is an
overload set whose base case says "this is not an integer". `std/x509`'s
`SigKey` is the original of the shape, and this is the second use of it, which
is the point at which it stops being a trick and becomes how this language
reads a tagged format. The cost is two rules that have to be remembered: every
overload states the same error set, because a dispatched call has one type; and
a subtype coerces into its supertype only at an *annotated* binding, so each
constructor is `const v: Value = Int{ .. }; return v;`.

**Parsing does not raise.** An error union carries a tag and nothing else, and
`BadFormat` is not a thing to hand somebody holding a 200-line manifest.
`parse` answers with a `Doc` — a table, or a message and the line it happened
on — and everything under ingot's verbs takes a `Fault` and writes into it.
This is the first place the language's error unions were not enough, and it is
worth saying plainly that the answer was not to change the language: a reader
that has lost its place should report the first failure and stop, which is a
shape rather than a missing feature.

Writing it found one real gap. `str.parse_int` existed and `str.parse_float`
did not, so a number could be written and not read back. It is one builtin row
over Rust's own correctly-rounded parser — the one place where reimplementing
in W# would have been worse rather than more honest, since there is exactly one
right answer per decimal and getting it is a hard numerical problem.

#### The store, and the two ways it can be wrong

`~/.wsharp/store/sha256/<hex>` holds a package's files under the hash of its
tree, `tmp/` holds a fetch in flight, and `env/` holds the lockfiles that reach
entries. **The tree hash is defined here rather than borrowed**, because a key
two versions of ingot compute differently is a store that silently splits in
half:

```
H(dir) = SHA-256 over each entry, sorted by name as bytes:
           "f" name 0x00 <decimal size> 0x00 <contents>     for a file
           "d" name 0x00 <hex H(sub)>   0x00                for a directory
```

Sorted, because a filesystem's own order is neither sorted nor the same on two
machines — so the sort is half of the hash's definition and lives beside it
rather than in `std/array`. The size written out, so that two files cannot run
together into one. A subtree folded to its own digest, so a deep tree costs no
more memory than a shallow one. Permissions and timestamps deliberately absent:
a package is its text.

**An install is a `rename`**, which is what W# having no `defer` makes
structural rather than tidy: a fetch builds under `tmp/` and becomes visible in
one step, so an interrupted install leaves rubbish rather than half a package,
and a package that is *there* is complete by construction. Two processes that
both win the race are both right, since the contents are what the name says.

The bug worth recording is the one this item predicted in advance. `install`
first asked whether the target *directory existed*, which is not the same
question as whether the entry is what it claims to be — so on a store somebody
had edited, `install` reported success and changed nothing, and `verify` went
on saying `damaged` for ever. Testing with `check` rather than with `is_dir` is
the fix, and telling "not installed" from "damaged" is precisely what this item
said was the feature rather than the polish.

**`verify` answers with its exit status**: `0` ready, `1` needs installing, `2`
needs resolving, `3` broken. A path dependency edited after it was resolved is
none of missing, damaged or fine — the store holds exactly what it was told to
and it is the *lockfile* that is out of date — so it is `2`, and the row says
`changed`. The lockfile records the manifest's **digest** rather than its
modification time, for the reason stage one dropped `stat`: a checkout does not
preserve timestamps and two machines do not agree about them.

#### Two decisions about shape

**A path dependency is copied into the store**, rather than used where it lies
as most package managers do. It follows from the first principle this item took
from Ajt: resolving, installing and building are separate verbs, and nothing
compiles because something else was fetched. If a build read a dependency's
working directory, then editing that directory would change what the build does
with nothing having been resolved or installed, and the separation the other
verbs exist for would be a fiction. The cost is an `ingot install` after every
edit, and it is the honest one.

**The tool is two hundred lines of Rust over a W# program.** `ingot/main.ws`
holds every verb and reads its own command line through `os.args()`; the driver
publishes the arguments and runs it. Two things stay the driver's, and both for
the same reason — they *are* the compiler, which a W# program has no way to ask
for: `ingot run <file.ws>`, and `--gc-stress`. `-C <dir>` is the driver's too,
since a process has one working directory. It is a seam rather than a split:
the verb list a user sees is one list, and the W# half prints it.

#### A library module is read only if something imports it

Not planned, and the largest single effect of this stage. Every `.ws` module in
the library was parsed and inferred for every program — free at four files, and
by twenty-five it was most of what compiling a ten-line program did.
`load::add_library` now follows the root's imports and reads only what they
reach; `@import("std")` still means all of `std/`, because a module path is a
prefix and any of them can be walked into from there. `wsharp check
examples/fib.ws` went from 0.62 seconds to 0.005, and the whole case suite from
140 seconds to 40.

That is also what made `ingot/` a library namespace beside `std/` rather than a
table the `ingot` binary passes in. Calling PubGrub and a packfile reader
"standard library" would be a promise this project does not intend to make, and
a namespace of its own says what they are — while `@import("ingot/store")`
working under plain `wsharp` is what lets `tests/cases/` test them the way it
tests everything else, including the second pass under `--gc-stress`.

The consequence to remember is the other side of the same coin: **a library
module nothing imports is never checked.** A new one needs a case that imports
it, or it is not compiled at all.

### Stage three — the resolver that explains itself — **done**

| Piece | Where |
|---|---|
| Semantic versions, and sets of them as unions of intervals | `ingot/semver.ws` |
| PubGrub: terms, incompatibilities, unit propagation, conflict resolution | `ingot/pubgrub.ws` |
| The report: a walk of the derivation graph | same — `explain` |
| The manifest graph, and the provider over it | `ingot/plan.ws` |
| A bug in the collector that this was the first program to reach | `mark.rs`, `gc.rs` |

`ingot resolve` now goes through the solver, so a version requirement is
*checked* rather than ignored and a project that cannot be satisfied is told
why:

```
Because no versions of core match >=2.0.0 <3.0.0 and util 0.3.0 depends on
core >=2.0.0 <3.0.0, util 0.3.0 cannot be used.
Because util 0.3.0 cannot be used and no versions of util match <0.3.0 or
>0.3.0, util any version cannot be used.
Because util any version cannot be used and myapp 0.1.0 depends on util any
version, version solving failed.
```

#### Every resolution goes through the solver, even one with nothing to choose

A path dependency offers exactly one version, so a project made only of them
gives the search no decisions to make. It goes through PubGrub anyway, and that
is the decision worth recording: a requirement on such a package still has to
be *checked*, and two packages that want incompatible versions of a third still
have to be told apart from two that agree. Running the trivial case through the
same code is what makes the interesting case say something useful rather than
something new.

The one thing kept out of the solver's hands is a package nothing in the graph
supplies. PubGrub's honest answer there is "no versions of X match ^1.0.0",
which is true and says nothing about what to do; `plan.unsourced` says "ingot
cannot yet fetch one" instead, which is the thing stage four will change.

#### A version set is intervals, because complements are the operation

`semver.Range` is a sorted, disjoint, non-adjacent list of intervals whose ends
carry inclusivity. Both halves of that are forced:

- **Intervals rather than a predicate**, because the solver takes *complements*
  constantly -- `not foo ^1.0.0` is a term it derives and reasons about -- and
  a predicate cannot be complemented into something you can then ask for the
  best version of.
- **Inclusive or exclusive ends rather than half-open**, because a half-open
  representation needs a successor function and a version has none: there are
  infinitely many pre-releases between `1.0.0` and `1.0.1`.

Touching intervals are run together, so two spellings of one set are one set
and equality is a walk rather than two subset tests.

**A pre-release is not a candidate unless it was asked for**, and that is a
policy in the solver rather than a rule in the algebra. The algebra stays
honest that `1.1.0-rc.1` is below `1.1.0` and is inside `^1.0.0`; the solver
asks whether the range it is choosing from *names* a pre-release, and skips
them if not. Putting the rule in the set arithmetic instead is how other
implementations end up with two kinds of range.

#### Three bugs in the solver, each worth naming

PubGrub is a short algorithm and every line of it is load-bearing. Three
things written the obvious way did not work, and each failed in the same way:
the search stopped learning and ran until it hit its own step limit.

- **An incompatibility must merge terms about the same package.** Resolution
  routinely produces a pair like `{not foo ^1.0.0, foo 2.0.0}`, which merged is
  `{foo 2.0.0}` -- the clause that actually rules something out, and the one
  the terminal test can recognise. Left unmerged, the two are asked about the
  same package independently and nothing is learned.
- **The difference taken during resolution is `satisfier ∖ term`**, not the
  reverse. The satisfier is *why* the term held, so what it allowed beyond the
  term is still open and has to be carried into the new clause.
- **A term is not its allowed set.** This was the subtle one. Representing a
  negative term as the complement of its range makes three of the four
  relation cases fall out of set arithmetic, and breaks the fourth: "foo is not
  in A" is satisfied by foo being *absent altogether*, which no set of versions
  says. Under that representation `not foo any-version` -- which is what every
  dependency starts life as -- allows the empty set and so looks like a term
  that can never hold, and the solver decides nothing at all. `relates` has
  four cases for that reason.

#### And one in the collector, which this was the first program to reach

The solver allocates far more, and far more short-lived object graphs, than
anything else in the suite, and it found a real bug in the evacuation pause:
an object it was handed to fix up had been **freed**, and its space taken by
something else, so the pause walked a stranger's bytes with a dead object's
layout. It showed up as a misaligned pointer dereference inside `header.rs`,
which is a long way from the cause.

The cause is a one-pause window. The evacuation pause reads two lists recorded
during the *mark* -- the slots the marker saw pointing into a block being
emptied, and the objects the trace touched afterwards. Counting's frees were
deferred through the mark, as they must be, and then run at the pause that
*finishes* marking -- which is one pause too early. `finish_marking` now arms
`evacuating` before it settles the counts, and `defer_frees` asks
`tracing() || evacuating()`, so the frees wait for the trace's last pause
rather than its second. `fix_references` visiting `deferred_dead` is what that
was always written for; the deferral had simply stopped reaching it.

Under `--gc-stress` the pause now asserts that nothing it was handed has been
freed, and `gc_evacuation_lists.ws` is what gives that assertion something to
fire on. Reproducing it needs volume rather than cleverness: a few hundred
thousand short-lived nodes, a handful of scattered survivors to make blocks
sparse enough to evacuate, and a field overwritten on every pass so the barrier
keeps logging. With the fix backed out, that case trips the assertion about two
runs in five; with it, six runs in six are clean, and the case reports 21
traces and a thousand objects moved, which is the number to look at.

### Stage four — git, spoken rather than shelled out to — **done**

| Piece | Where |
|---|---|
| SHA-1, because git names every object by one | `std/hash.ws` |
| DEFLATE and the zlib wrapper, as a cursor | `std/inflate.ws` |
| Headers of a caller's own on an HTTP request | `std/http.ws` — `send_request_with`, `request_headers` |
| pkt-line framing and side bands | `ingot/pktline.ws` |
| A packfile, with both delta kinds resolved | `ingot/packfile.ws` |
| Smart HTTP v2: discovery, `ls-refs`, `fetch` | `ingot/git.ws` |
| A fetched tree becoming a store entry | `ingot/store.ws` — `install_files`, `remember` |
| A git dependency being resolved and installed | `ingot/plan.ws` |
| A second bug in the collector | `evacuate.rs` |

`ingot.toml` can now say `dep = { git = "https://…", rev = "…" }`, and
`ingot resolve` fetches it, reads its manifest, folds it into the search, and
records the tree's digest in the lockfile. Fetching happens at *resolve* rather
than at install, because resolving needs the dependency's own manifest before
it can choose anything -- and it is remembered under `~/.wsharp/git/`, since a
revision names one tree for ever and the second resolve of a project should
touch no network at all.

#### The interface a packfile forces

`std/inflate` answers with **how many input bytes it consumed**, not with a
buffer. That is the whole shape of the module and it is not a preference: a
packfile is a concatenation of zlib streams with nothing between them, so only
the decompressor knows where one ends. A `decompress(bytes) -> bytes` would
have been the obvious thing to write and useless for the one caller there is.

The decoder is Mark Adler's `puff` in outline -- canonical Huffman from a table
of counts and a table of symbols rather than from a tree -- and the tables live
in the reader rather than in a block, because a block that allocated would be a
collection per block under `--gc-stress`. It costs nothing measurable: the case
runs in a fifth of a second under stress.

There is no `crc32` and no gzip wrapper, for the reason `std/der` has no writer.

#### Three places a packfile reader is wrong

- **A packfile has two varints and they are different.** A size is the ordinary
  seven-bits-at-a-time little-endian form. An offset delta's backreference
  accumulates `((n + 1) << 7) | next`, which is what makes every number's
  encoding unique. Reading one with the other's loop gives a plausible wrong
  answer, and is the classic bug.
- **A copy instruction with a size of zero means 65536.** Zero would be a copy
  of nothing, which no encoder writes, so the value was given a use.
- **A back reference whose distance is shorter than its length is how a run is
  encoded**, so the copy is byte at a time and not a block move -- the bytes
  being read are partly the bytes being written.

#### `ERR` is not a side band

Git says no out of band: `ERR <message>` is a plain pkt-line and can arrive
anywhere a line can, including before the side bands exist to carry one. A
client that only watched band 3 would read a refusal as an unknown section and
report an empty answer. This was found by asking a real server for a commit it
does not have, which is the only way it would have been found.

#### Tested against a real server, without a network

The rule item 10 set -- a protocol is tested three ways, because a transcript
can only do two of them -- applies here with one improvement available. Git's
HTTP endpoint is `git upload-pack --stateless-rpc` behind a thin proxy, and
that command can be run directly. So:

1. The **requests** in the fixture are the bytes this client generates.
2. The **answers** are what `git upload-pack` said when it was handed them.

That closes the gap a recorded transcript leaves. A recorded request is an
input, so comparing against it says nothing about what the client would have
written; here the recording was made *from* the client's own output, and the
server's willingness to answer is the assertion. The case then checks that the
client still writes those bytes, so a change to the request is a failure rather
than a silent divergence.

The packfiles are what `git repack` actually wrote -- one with offset deltas,
one with reference deltas, both holding a chain of length two -- and what comes
out is checked against the object ids git itself assigned. An id is a SHA-1
over the whole object, so a delta applied one byte wrongly is a different id.

What is **not** tested is a fetch over a real network, for the reason
`http.get("https://…")` is not in `examples/`: CI would depend on the weather.

#### And a second bug in the collector, in the same pause as the first

Stage three found the evacuation pause reading objects that had been *freed*.
This stage found it reading objects that had been **forwarded** -- and the two
are different mistakes with the same symptom.

`evacuate::fix_fields` read an object's type id and walked the fields that type
describes. A logged object sitting in a block being emptied gets forwarded
while the program runs, and a forwarded header is an *address*: its low
thirty-two bits are part of a pointer, which can perfectly well name a real
type id. The pause then walked a stranger's bytes with somebody else's layout.

The rule was already written down -- *"a forwarded header is an address, not
flags; anything that reads a flag, a count, a size or a type id from an object
in a block being emptied must test forwarding first"* -- and named the three
places that follow it. `fix_fields` was the fourth and did not. It now returns
straight away: the copy is on the same list, and the copy is what needs fixing.

The `--gc-stress` check added in stage three had the same bug, which is how
this one was found: it read `FLAG_DEAD` out of a forwarded header and reported
a freed object that was not one. A check that is wrong in the way the code is
wrong is worth remembering as a failure mode of its own.

#### What it costs

`ingot` compiles its own W# on every invocation, and stage four roughly doubled
what that is: `ingot/git` reaches `std/tls`, which reaches `std/x509`,
`std/nistec`, `std/rsa` and the rest. A release build starts in about half a
second and a debug build in three and a half. Lazy loading is what keeps that
from being every *program's* problem, but `ingot`'s own verbs genuinely reach
all of it, and the fix -- if it is worth one -- is caching a compiled program
rather than loading less.

### Stage five — the import that reaches a package — **done**

| Piece | Where |
|---|---|
| `os.cwd`, on three arms | `sys/{mod,linux,bsd,windows}.rs`, `os.rs`, `std/os.ws` |
| `ingot.env`: where each package's files are on *this* machine | `ingot/manifest.ws` — `Installed`, `write_env` |
| `install` writing one, `verify` noticing it is missing | `ingot/main.ws` |
| The loader's third rule, and the scope check | `wsharp-cli/src/load.rs` — `Packages`, `follow_package` |
| Re-export: `pub const parse = reader.parse;` | `wsharp-sema/src/infer.rs` — `alias_reexported_{types,values}` |
| A bug in `store.register` that made every project one environment | `ingot/main.ws` |
| A bug in the `for` protocol's dependency edges | `infer.rs` — `infer_all` |

`ingot.toml` can say `util = { path = "../util" }`, and `@import("util")` now
compiles -- under `ingot run` and under plain `wsharp run` alike, because
nothing about finding a package needs the tool.

#### The loader hook really was one branch

This is the thing the item said in advance would be easy, and it was:
`Loader::follow` had two rules and now has three, and *nothing downstream
changed*. A module's identity is already its canonical path, so a package file
loaded out of `~/.wsharp/store/sha256/<hex>/src/util.ws` is a module named by
that path and the type checker never learns it came from a store. Two versions
of a package are two directories, so they are two modules, exactly as predicted.

What was not free is the *input*. The loader is Rust; `std/toml` and
`ingot/manifest` are W#. Reading the lockfile in the compiler would mean a
second TOML implementation kept in step with the first for ever -- and it would
have to be a whole one, because a package's own `ingot.toml` is a file a person
wrote. So `ingot install` writes **`ingot.env`** beside the lockfile: one line
per package, tab-separated, absolute.

```
myapp	/home/u/work/app	src/myapp.ws	util
util	/home/u/.wsharp/store/sha256/c14b…	src/util.ws
```

Name, directory, facade, and then one field per dependency -- the fourth field
onwards rather than a list inside one, so the file has exactly one separator and
a package name is whatever a name is. It is derived, machine-local and
regenerable, which is what lets "unreadable" and "from a newer ingot" have the
same answer: write it again. `verify` reports `needs install` when it is gone,
which is the smallest true thing to say about a store that is otherwise fine.

That also keeps the split the item opened with. `wsharp` still works on a
machine with no network and no store; it now also reads a file when there is
one, and a program that is not in a project pays one directory walk for that.

**`resolve` removes it**, which is the one thing about this file that is not
obvious. A new resolution names new store entries, and the *old* ones are still
there holding exactly what they always did -- so an environment left behind
would build the previous version of a dependency and say nothing at all, which
is the worst way for a package manager to be wrong. `add` and `remove` leave it,
because the environment they leave is still a true statement about what was
installed; it is `resolve` that makes one false.

#### A package may import only what it asked for

The lockfile is the whole graph's, because the solver chooses one version of a
package for the *project*. What a package may **name** is narrower: what its own
manifest asked for. Without that rule `ingot.env` would make every package
reachable from every other, and a manifest would describe what gets fetched
rather than what may be written down -- a dependency you never declared would
work until the day something else stopped depending on it.

The check needs to know which package a file is *in*, which is the entry whose
directory is the longest prefix of it. Longest rather than first, because
`WSHARP_HOME` may sit inside the project; ingot's own verb tests put it there.

#### Re-export is two keys in tables that already existed

A package presents one file. That was the design before this stage -- the
manifest has had a `root` since stage two -- and it is why the item said a
facade needs re-export: `const x = @import("./inner.ws");` binds a *module*, and
a module cannot be reached through.

The answer needs no new syntax at all:

```zig
const inner = @import("./inside.ws");

pub const Pair  = inner.Pair;      // a type
pub const twice = inner.twice;     // a function, or a whole overload set
pub const SCALE = inner.SCALE;     // a constant
```

`const x = a.b;` already parsed. It was rejected as a computed global, which it
is not: naming a name is not evaluating one, exactly as `const g = f;` in one
module was never a computation. And it turns out to be **one more key in
`struct_ids` and one more in `globals`**, both already keyed
`"<module path>.<name>"`, both holding the *same* `StructId` or `GlobalRef`. So
the alias and the original are the same type and the same function set: they
unify, dispatch and lay out identically, because they are not copies. Nothing in
`hir.rs`, `mono.rs`, `ty.rs` or code generation knows this happened.

Two passes rather than one, because a type has to be aliased before struct
fields are laid out (a field may be written `pkg.Pair`) and a value only after
every module's globals are known. Each runs to a fixpoint, so a facade may
re-export from a facade. There is deliberately no case for a chain that closes
on itself: `pub const x = a.x;` is written in a module that imported `a`, so a
cycle among re-exports is a cycle among *imports*, and the loader has already
refused to read the second file.

**A facade states its surface, name by name.** That is the same rule `pub`
already set -- a module's surface is something it says rather than something it
leaks -- and it is why the alternative was not taken. Making
`pub const inner = @import("./inside.ws");` reachable through would have been
smaller, and it would mean every user of a package had to know the names of the
files inside it, which is exactly what a facade exists to stop.

#### Two bugs, and what hid each of them

**Every project on a machine was the same environment.** `store.register` names
an environment by the hash of its lockfile's path, and it was being handed the
bare relative `"ingot.lock"` -- so the hash was the same for every project,
`install` in one silently unregistered another, and the next `ingot gc` deleted
that project's store entries. The verb tests could not see it because each of
them gets a `WSHARP_HOME` of its own; two projects in one store is the smallest
thing that shows it, and is now a test. The fix wanted the project's absolute
directory, which W# could not ask for -- so `os.cwd` is in this stage, and
`ingot.env` needed it anyway.

**A `for` over a type from two modules away did not compile.** The `for`
protocol resolves `iter` and `next` in the module that declares the subject's
type, which inference has not run yet to know -- so the dependency graph
over-approximates and made every `iter` and `next` *this module can see* a
dependency. "Can see" was the bug: a subject's type can come from a module the
program never named, through a function that forwards it and, now, through a
facade that re-exports it. The edge was missing, the iterator was still
ungeneralised when its user was inferred, and what came out was
`no overload of iter accepts (Walk)` about a `Walk` that was right there. It is
now every `iter` and `next` in the program, which is sound for the reason the
original over-approximation was: an edge only matters when it closes a cycle,
and an iterator does not call back into the program using it.

This one is worth recording as a *class*. It was found by writing a package with
an iterable type in it, and it had nothing to do with packages: a plain
forwarding function through a third module fails the same way, and had done
since item 6. A facade is a forwarding function that hides where things came
from, so it finds every place the compiler was quietly relying on "the user
imported it directly".

### Stage six: a place for versions to come from

The four stages above built everything a registry needs and no registry. A
version dependency parsed, walked the graph, and was refused by one line saying
`ingot cannot yet fetch one` -- which was the honest thing to say and the thing
this stage deletes.

The registry is **Foundry**, a git repository beside this one, in the shape of
Julia's General: `packages/<owner>/<name>/` holding a `package.toml` that says
where the source is and a `versions.toml` with one `[[version]]` per release.
Plain TOML, read with `std/toml`, fetched with the client stage four wrote.

Four decisions are the whole of it.

**An index rather than a protocol.** A resolver has to know what versions exist
before it can choose between them, and asking a git host that one package at a
time is not a conversation a solver can hold -- `pubgrub.Provider`'s two
closures may not fail and may not block. So the answer is published as data and
read as a file, and the reachable subgraph is materialised before the solve, in
exactly the shape `plan.discover` already used for path dependencies. The
provider stayed total; nothing about the solver changed.

**A release records its tree hash.** This is the one that pays for itself twice.
A *git* dependency has to be fetched during `resolve`, because only the fetched
tree holds the manifest saying what it depends on; a registry entry already is
that manifest data, and it carries `tree = "sha256:…"` -- the store's own key.
So resolving a registry graph fetches nothing at all, and `install`'s existing
refusal of a tree whose digest is not the one the lockfile named
(`main.ws`, "has changed since it was resolved") becomes an integrity check for
free. The client ends up trusting a hash rather than a host. What makes the
hash worth trusting is the registry's CI, which fetches every entry a pull
request adds and hashes it before merging -- and that validator is a W# program
importing `ingot/registry`, so the format has one implementation and the thing
enforcing it is the thing reading it.

**A registry is a directory.** Fetching one over git is only how the directory
arrives. `INGOT_REGISTRY` naming a directory is used where it lies, with no
certificate store read and no socket opened. That was written for testability --
the git client speaks HTTP and no case in this suite may stand up a server, so
without it none of this could be tested here at all -- and it turned out to be
the definition of a private registry and an offline one as well. The lesson is
the one `std/tls` and `ingot/git` already taught: separate the bytes from the
transport and the bytes become testable.

**A path or git dependency overrides the registry.** A name the graph already
supplies from a directory is not looked up, because offering the solver
published versions of a package somebody is editing beside their project lets it
choose one -- which is not what a checkout beside your project means.

Two smaller things fell out. `git.discover` and `ls_refs_request` had been
written, tested against a real `git upload-pack` and called from nowhere since
stage four; a registry is named by a *branch* rather than by a revision that
never moves, so turning that branch into an object id is their first caller, and
`git.ref_id` is the one function that was missing. And `ingot gc` had to learn
that an index is a store entry reached by a pointer file rather than by a
lockfile, or the first collection after an update would delete the registry.

The bug worth recording: `registry.package` answers null two ways -- "not in
this registry", with no fault, and "here and unreadable", with one -- and two of
the three callers tested `!f.ok` where they meant `f.ok`. Since `fault.fail`
keeps the *first* failure, the effect was not a wrong message but no message at
all: `ingot add acme/nope` exited 4 in silence. An optional that means two
things needs its callers written the same way, and this is the argument for
having said so in the doc comment before writing them.

#### What is left

- **A registry index is fetched whole.** `packfile.read` builds every object in
  memory, so the index costs what it costs; the same bound a git dependency has
  always had, and the right trade until a registry is large enough to notice.
- **A registry entry is verified by its publisher's CI, and by nobody else
  afterwards.** `install` checks the fetched tree against the hash the lockfile
  names, which is the hash the registry gave — so a registry that lied at merge
  time is believed. The check that closes that is a signature, and a signature
  needs somebody to hold a key.
- **`struct : pkg.Base` is not spellable.** A supertype is an `Ident` rather
  than a type path, so a subtype of a re-exported type has to be declared in the
  module the parent was declared in. A re-exported name is nameable everywhere
  else a type is.
- **Two versions of a package still meet as two identically-named types**, and
  the mismatch says so without saying why. As written in advance: the
  diagnostic is the work, not the semantics.
- **`ingot.env` is trusted, not checked.** Compiling does not re-hash a store
  entry, because `verify` is a separate verb precisely so that building need
  not pay for one.

### The name

C#'s package manager is NuGet, which sounds like *nugget*; in Minecraft nine
gold nuggets craft one ingot. Item **9** is where the name was coined, which is
the whole joke and the reason it is written down here rather than decided later
under time pressure. `ingot` is the *tool*; `wsharp` stays the compiler, so
`wsharp run` keeps meaning what it means and `ingot install` is a different
program with a different job. That separation is worth having on purpose: one
of them must work on a machine with no network and no store, and the other is
the thing that fills the store.

### The shape, and where it comes from

Modelled on [Ajt](https://github.com/sinisterMage/Ajt.jl), an alternative
client for Julia's package ecosystem, for the parts that are about *being a
package manager* rather than about Julia:

- **Resolving, installing and building are separate verbs.** Nothing compiles
  because something else was fetched. `ingot add` records an
  intent, `resolve` chooses versions, `install` makes the store satisfy the
  lockfile, and `build` is a thing you asked for.
- **`verify` answers with its exit status** -- ready, needs installing, needs
  resolving, broken -- so a CI script can ask without parsing anything.
- **Output is tab-separated**, so a shell can cut it up.
- **`why` prints the dependency paths that explain an entry**, because "what
  pulled this in" is the question a lockfile never answers on its own.
- **A resolver that explains itself.** The thing Ajt is actually built around:
  a solver that tracks *why* each version was ruled out, so a conflict comes
  back as something to act on rather than as "unsatisfiable". PubGrub is the
  algorithm; the traceable derivation is the point. *Done in stage three.*

### What it needs, and what is missing today

- ~~**`argv`.**~~ Done in stage one, along with the environment, which is what
  `~/.wsharp` is found through.
- ~~**A filesystem beyond four functions.**~~ Done in stage one. It turned out
  to need *less* than this asked for: not a stat, but the two questions a stat
  was wanted for.
- ~~**Git, spoken rather than shelled out to.**~~ Done in stage four. The
  surprises were where this said they would be, and there were three of them:
  two different varints, a copy size of zero meaning 65536, and `ERR` not being
  a side band.
- ~~**A manifest and a lockfile format**~~ Done in stage two, and the decision
  went the harder way: all of TOML 1.0, because a subset is a promise the file
  extension makes and the code does not keep.
- ~~**A content-addressed store**~~ Done in stage two, `gc` included.
- ~~**A loader that can resolve a package path**~~ Done in stage five, and it
  really was one branch. What it needed that this did not foresee is an *input*
  the loader can read without a TOML parser of its own, which is `ingot.env`.
- ~~**Re-export**~~ Done in stage five, as `pub const x = other.x;` -- no new
  syntax, and two more keys in tables that already existed.

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

Kept as written, because they were right. The loader hook was one branch and
nothing downstream changed; two versions of a package are two modules and the
diagnostic is indeed the work; re-export was the one thing the type checker
needed, and deciding it early is what kept it to two keys in existing tables.
The last one is the one to take from this item: **every gap it found is one a
user would have found instead** -- and the gaps were not the ones a package
manager suggests. They were `os.cwd`, a lockfile path that was relative, and a
dependency edge for `for` that assumed you had imported the iterable yourself.

### What it costs

A package manager is judged on the day it goes wrong, which means the work is
mostly in the failure paths: a half-written store, an interrupted fetch, a
lockfile from a newer version, two packages that cannot agree. W# has no
`defer`, and item 8 already left a socket leaking on an error path because of
it -- a store is where that stops being cosmetic. Atomic rename, a temporary
directory per fetch, and a `verify` that can tell "not installed" from "damaged"
are not polish here; they are the feature.

---

## 12. Ahead-of-time compilation — **done**

Every item before this produced a program that ran *inside the compiler*. That
is the right shape for testing a language and the wrong one for using it: a
user could not hand anybody a binary, every run paid for parsing, inference,
monomorphisation and code generation, and a machine that ran a W# program
needed the whole of Cranelift on it. `wsharp build` is the answer, and
`ingot` -- which had a Rust driver for exactly this reason -- is what proves it.

| Stage | What it is | State |
|---|---|---|
| One | One lowering, generic over `Module` | **done** |
| Two | A linker name for every runtime entry point | **done** |
| Three | The two collector flags as symbols rather than addresses | **done** |
| Four | The type registry, the stack maps and the services as data | **done** |
| Five | The object backend, the startup archive, and `wsharp build` | **done** |
| Six | `ingot` without a Rust entrypoint | **done** |

### What was actually in the way

Not code generation. Cranelift emits an object file about as readily as it
fills memory, and `cranelift-object` is the same `Module` trait `cranelift-jit`
implements -- stage one is a type parameter and five signatures. What was in
the way is everything the JIT never had to say out loud, because the compiler
and the program were the same process:

- **Nothing had a linker name.** There was not one `#[no_mangle]` in the
  workspace. The JIT takes the *address* of each runtime function and invents
  the string name on the spot, so `std/str.len` was a fine symbol -- it only
  ever had to be a `HashMap` key. Ninety-five entry points needed real names,
  and a `link:` beside each `ptr:` so the two backends cannot disagree.
- **Two addresses were compiled into the code.** The poll flag and the
  evacuating flag are read by generated code directly, and their addresses were
  `iconst`s -- correct in a JIT, and a wild pointer in a file another process
  will load. They are exported statics now, declared as imported data and
  reached with `symbol_value`, exactly as a string literal is.
- **Three tables were handed over by calling a function.** The type registry,
  the stack maps and the service table were built as Rust values and pushed
  into the runtime. A compiled program has no compiler to push them, so they
  are written into its data section and read back by a startup pass. Only code
  addresses are relocations; everything else is a count or a byte.

### The one number worth checking

`R_X86_64_GOTPCREL`. Declaring the flags as imported data under `is_pic=true`
emits a GOT reference, which would put an extra dependent load in front of
*every heap reference the program loads* -- the hottest path in the language.
The linker relaxes it to a RIP-relative `lea`, because the flag is in the same
static link unit, and the linked binary has no GOT relocation for it at all.
Worth knowing that the check is on the executable and not on the object: the
object always shows the GOT form.

### What it costs

An installation is a *directory* now. `wsharp build` links, so it needs a C
compiler and something to link against -- `libwsharp_start.a`, which is
`wsharp-runtime` bundled with the `main` a compiled program starts in. The
standard library is still `include_str!`'d into the compiler, so the only thing
that grew is a `lib/` beside the binary. A missing `cc` is a user-facing
failure now rather than a build-time one, which is why it has a sentence of its
own rather than an `os error 2`.

Defining `main` in that archive means Rust's `lang_start` never runs. Panics,
unwinding, backtraces, threads and stdout all survive it; the main thread's
stack guard page does not, so a runaway recursion is a segfault rather than a
message. It was a segfault under the JIT too.

### Why `ingot` is the test

Its Rust driver did three things a W# program could not: `-C`, `--gc-stress`,
and `run`. Two were easy -- `os.chdir`, and a flag that only ever belonged to
whatever was being run. The third was the interesting one, because `ingot run`
compiles a program and a W# program has no compiler in it. Embedding one would
have meant linking Cranelift into the package manager; the answer is that it
*hands over*, replacing itself with `wsharp run` through a new `os.exec`. The
user sees one process and one exit status either way.

That made `os.exec` the first builtin to take a list of strings, and it takes
one blob rather than a `[]str` for the mirror of the reason `os.raw_args`
answers with one: reading references in Rust would bypass the load barrier, and
one the collector had moved would be a stale pointer handed to `execvp`.

The suite that proves all of this is the case suite, a third time: every one of
the 209 cases is built to a native executable and held to the same header, and
the `gc_*` subset runs again under `WSHARP_GC_STRESS=1`. A built `gc_moving.ws`
reports the same 100 objects moved and the same 18 safepoints the JIT does,
which is what says the stack maps survived being written to a file.

---

## Smaller follow-ups

These are deliberate limitations, each with a clear fix:

- **Computed top-level `const`.** Only literals and `fn` values are allowed at
  the top level; anything computed is rejected with a message saying so.
  Supporting the general case needs global storage plus a startup initialiser —
  and the collector would need those globals as roots.
- **A supertype is a name, not a path.** `struct Sub : Base` resolves `Base`
  unqualified, so a subtype of a type another module declares -- including one
  a package facade re-exports -- has to be declared in that module. Every other
  position takes `pkg.Base`; `StructDecl::parent` would have to become a
  `TypeExpr` for this one to.
- **Field access needs a known type.** Structs are nominal with no row
  polymorphism, so `fn getx(p) { return p.x; }` cannot be inferred and asks for
  an annotation instead.
- **`==` is limited to the integer types, `f64`, `bool` and `str`.** Structs
  still need a decision about identity versus structural equality.
- **An integer literal is never an `f64`.** Item 9 made a literal take the
  integer type it is used at, but not a float one: `1.0` must still be written
  where an `f64` is wanted. The diagnostic now says so in those words rather
  than reporting a bare mismatch.
- **`%` is integer-only.** Cranelift has no float remainder, and a float `%`
  is rejected by inference rather than emulated.
- **`math.abs` and `math.sign` are `i64`/`f64` overloads**, so a narrow signed
  value needs a conversion. One generic over `Number` is not available: the
  abstract type includes the unsigned types, and negation is meaningless there.
- **An array index is an `i64`.** Every literal index works without saying so,
  and `i64(i)` covers the rest.
- **x86-64 and aarch64 only.** The collector reads the frame pointer with
  inline assembly; other architectures get a `compile_error!`.
- **`g[i][j] = v` is rejected**, because the base of a place must be a variable
  or a field chain -- a compound assignment evaluates its target twice, and
  restricting the base is what keeps that unobservable. `var row = g[i];
  row[j] = v;` is the spelling, and it is correct rather than merely accepted,
  since an array is a reference. Found while writing item 10's AES, where it
  cost nothing: the state is a flat sixteen-byte buffer, which is how AES is
  written anyway.
- **`wsharp check` accepts a program `wsharp run` rejects**, when a generic
  call's type variable is never pinned. `var b = array.new(32);` with nothing
  to say what the elements are passes the first and fails the second with
  `cannot tell what type main is being used at`, because `check` does not
  monomorphise. The diagnostic is right; which command reports it is not.
- **A top-level `const` array can be written through an alias.** `K[0] = 1` is
  rejected, and `var a = K; a[0] = 1;` is not: the second is a local holding
  the same address, and W# has no way to say that a reference is read-only.
  The data is emitted writable rather than read-only for that reason, so the
  mistake is a shared table quietly changing rather than a fault with no
  message.
