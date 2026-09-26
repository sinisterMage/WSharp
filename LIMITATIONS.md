# What W# does not do

**Status: draft for ratification.** The limitations are real and each one below
was verified against `wsharp 0.2.3`; what is not yet settled is whether every
one of them is a *limitation of 1.0* or something that gets fixed first. That
call belongs to Ada (fix or document) and Johnny (what 1.0 should promise), and
the "Tracked" line on each entry is where it is being made.

A 1.0 is allowed limitations. It is not allowed undocumented ones. This file is
where a limitation lands so that a user meets it here rather than in their own
program at two in the morning.

## How to read an entry

Every entry carries four things, and an entry missing any of them is not
finished:

| Field | Means |
|---|---|
| **What** | The thing that does not work, stated as a user would meet it. A program where one helps. |
| **Why** | The reason it was left. Not an apology — the constraint that makes the fix more than an afternoon. |
| **Workaround** | What to write instead, or the word *None*. |
| **Tracked** | The issue where fix-or-document is decided. |

Every program shown below was run against `wsharp 0.2.3` and the output is what
it printed. Where a `tests/cases` entry guards the behaviour mechanically — so
that the limitation cannot quietly stop being true without a test noticing —
the entry is named. A limitation with no such guard is a limitation somebody has
to remember, which is why the ones that can have one, do.

Two definitions this file deliberately does not restate, because two copies
drift: **breaking change** and the **severity scale** are in Part one of
[RELEASE-CRITERIA-1.0.md](RELEASE-CRITERIA-1.0.md).

Nothing here is a promise that the limitation stays. The promises — what 1.0
will not break — are in [COMPATIBILITY.md](COMPATIBILITY.md).

---

# Language

## A computed top-level `const` is rejected

**What.** A top-level `const` may be bound only to a literal or a `fn`.

```wsharp
const N = 2 + 3;
```

```
error: a top-level `const` must be a literal or a `fn`
  --> p1.ws:1:11
  |
1 | const N = 2 + 3;
  |           ^^^^^
  |
  = help: computed globals need a startup initialiser, which W# does not have
    yet -- move the computation into a function
```

**Why.** A computed global needs storage that outlives every function and a
startup initialiser to fill it — and the collector would need those globals in
its root set. That is a fifth root list beside the three in `gc::collect` and
`worker::PINNED`, and every root list has to be added to four places at once
(the collect root set, the evacuation pause's root pass,
`evacuate::fix_references`, and the `--gc-stress` verifier).

**Workaround.** Compute it in a function and call that function where the value
is needed. For a table of scalars there is a second answer that costs nothing:
a `const` array literal is emitted as immortal data with no initialiser at all,
which is why it is allowed to be a literal —

```wsharp
const K = []u32{ 0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5 };
```

`tests/cases/const_array.ws`.

**Tracked.** [#9](https://github.com/sinisterMage/WSharp/issues/9).

## A field access needs a type the compiler can name

**What.** Structs are nominal and there is no row polymorphism, so a parameter
whose type inference cannot pin cannot have a field read from it.

```wsharp
fn getx(p) { return p.x; }
```

```
error: cannot tell which type the field `x` belongs to
  --> p2.ws:1:23
  |
1 | fn getx(p) { return p.x; }
  |                       ^
  |
  = help: W# resolves fields by the object's declared type -- annotate the
    parameter, e.g. `fn f(p: Point)`
```

This is the one place where W#'s "annotations are optional everywhere" is
qualified. Everything else infers; a field access does not.

**Why.** `HasField` is a deferred constraint (`crates/wsharp-sema/src/infer.rs`)
— it can be discharged once the type is known and is reported when it never
becomes known. Inferring it instead needs row polymorphism or an equivalent
structural mechanism, which changes the type system rather than adding to it,
and interacts with a dispatch lattice that is nominal by construction.

**Workaround.** Annotate the parameter. The annotation may be a **supertype**,
which is usually the one you want: a field read compiled against a supertype
runs unchanged on every subtype, with no adjustment and no vtable, because a
subtype's fields are its supertype's followed by its own.

**Tracked.** [#10](https://github.com/sinisterMage/WSharp/issues/10).

## A top-level `const` array can be written through an alias

**What.** Writing an element through the `const`'s own name is refused. Writing
it through a local bound to the same array is not, and the shared table changes.

```wsharp
const K = []i64{ 1, 2, 3 };

fn main() void {
  var a = K;
  a[0] = 99;
  print(K[0]);      // 99
}
```

Through the name, refused (`tests/cases/err_assign_const_array.ws`):

```
error: `K` is a top-level `const` array, which cannot be written
  = help: it lives in the data section and is shared by every worker -- copy it
    first, with `array.slice(..)`, if you need one to change
```

**Why.** The check is syntactic, on the name at the assignment, and W# has no
way to say that a reference is read-only. The data is emitted **writable** on
purpose (`crates/wsharp-codegen/src/lib.rs` — `define_arrays`): a read-only page
would turn this mistake into a fault with no message, which is worse than a
shared table quietly changing.

A top-level `const` array is one object shared by every worker in the process,
and W# has no mutable globals — the workers' design rests on that, since a
worker's state is an explicit value passed in and out.

**Workaround.** Copy before mutating, and change the copy:

```wsharp
var copy = array.slice(K, 0, array.len(K));
copy[0] = 42;
```

**Guarded by** `tests/cases/const_array_alias.ws`, which asserts both halves —
the alias write going through, and the copy leaving `K` alone. Written for this
entry, so that the limitation cannot stop being true without a test saying so.

**Tracked.** [#12](https://github.com/sinisterMage/WSharp/issues/12).

## Comparing a value that reaches itself aborts

**What.** `==` on a struct compares field by field and recurses into struct
fields, and cycles are not detected. A cyclic value compared with anything,
including itself, recurses until the stack is gone.

```wsharp
const Node = struct { next: ?Node, v: i64 };

fn main() void {
  var a = Node{ .next = null, .v = 1 };
  a.next = a;
  print(a == a);
}
```

```
thread 'main' has overflowed its stack
fatal runtime error: stack overflow, aborting
Aborted (core dumped)
```

There is no W# diagnostic and no exit status a program can act on.

**Why.** `==` on a struct is compiled into a *function* per concrete type
(`crates/wsharp-codegen/src/equality.rs`) rather than an inline sequence,
precisely because a type that reaches itself would otherwise expand for ever at
compile time; the recursion moves to run time and nothing bounds it. The
neighbouring case is caught at compile time instead: a struct with an array, a
function or an error-union field is rejected as uncomparable with a diagnostic
naming the field (`crates/wsharp-sema/src/infer.rs:6668`), and that reaches down
the lattice.

**This entry is the one in this file that may not survive.** By clause 3 of the
P1 definition in `RELEASE-CRITERIA-1.0.md` — a crash with no W# diagnostic from
a program that uses no FFI — it is a P1 rather than a limitation, and no
exemption for documented behaviour is written into that clause. The disposition
is open.

**Workaround.** Do not compare values that may reach themselves; compare the
fields you mean, or an identifier. A doubly linked list, a parent pointer and a
graph node are all cyclic.

**Tracked.** [#13](https://github.com/sinisterMage/WSharp/issues/13) — filed as
a defect with both readings stated, for Ada's severity call.

## A generic struct cannot have a supertype

**What.** `struct[T] : Base` is rejected.

```
error: a generic struct cannot have a supertype
  --> gen.ws:2:25
  |
2 | const Box = struct[T] : Base { v: T };
  |                         ^^^^
  |
  = help: give the subtype concrete fields, or drop the type parameters
```

**Why.** Struct type ids are a preorder walk of the dispatch lattice — every
type's subtypes occupy `type_id .. type_id + subtree_len`, which is what makes
the dispatcher's subtype test one subtract and one compare. Numbering runs
before monomorphisation, and a generic struct's instantiations are not known
until after it. So an instantiation takes an id from a block *above* the
lattice, where it can disturb no range test — and a type in that block cannot
also be inside somebody's subtree.

**Workaround.** Make the subtype concrete — `const IntBox = struct : Base { v: i64 };`,
one per instantiation you actually dispatch on — or express the shape with a
generic struct that *holds* a lattice value rather than being one.

**Tracked.** No issue; this is a consequence of the dispatch design rather than
an unfinished piece, and changing it means changing how type ids are assigned.
Raise one if you meet it in real code — that would be the evidence for
revisiting it.

## Every overload of a dispatched call must share one return type, error set included

**What.** A call that has to choose at run time has one type, so the overloads
it chooses between must all return the same thing — and an error set is part of
a function's type here, so they must all raise the same set.

**Why.** A dispatched call compiles to one join point with one result. That is
also the thing that stopped `https://` from being a `TlsConn : Conn` subtype
with `conn_read` and `conn_write` as overloads: reading through TLS can raise
everything a handshake can, reading a socket cannot, and the two would have had
to write the same two-dozen-name set out twice.

**Workaround.** An optional field and an `if`, which is what `std/http` does.
The lattice is right when the members agree about failure and wrong when they do
not — that is the design test, not a workaround for a missing feature.

**Tracked.** No issue; deliberate, and stated here because it is the constraint
people meet when they first reach for subtyping across a transport boundary.

## There is no `defer`

**What.** No `defer`, no destructor, no RAII. A resource is released by the code
that releases it.

**Why.** The collector reclaims memory; a socket, a file handle or a library
handle is not memory, and W# has no finaliser to hang one on. Adding `defer`
means deciding what runs on a panic, which means deciding what a panic is
allowed to unwind through — and generated code's roots are stack maps rather
than an unwind table.

**Workaround.** The shape the standard library uses everywhere: build under a
temporary name and `rename` into place in one step, so an interruption leaves
rubbish rather than a half-written thing (`ingot/store.ws` — `install`). For a
handle, close it on every path, and prefer an API that takes the caller's buffer
over one that hands back a resource (`net.read_into`).

**Tracked.** No issue. Named here because "where is `defer`" is a first-week
question and the answer is a design decision rather than an omission.

---

# Standard library

## `net.shutdown` does not stop an acceptor on the BSDs

**What.** `net.shutdown(s, read, write)` means the same thing on every system on
a **connected** socket. On a `Listener` it does not: Linux wakes a thread parked
in `accept`, and the BSDs answer `ENOTCONN` and leave it parked. There is no
`shutdown_listener`, because there is nothing to promise.

**Why.** The difference is in the operating systems. Emulating the Linux
behaviour on the BSDs needs a self-pipe or an equivalent per listener; the
alternative is a function that silently does nothing on half the release
targets.

**Workaround, and it is the shape a real server has anyway.** Drive the acceptor
with a poller and a tick — `net.accept_nonblocking`, `net.watch_listener`,
`net.wait(p, ms)` — and take the stop signal from whatever the program already
has, usually a `std/broker` topic, because that crosses heaps.
`tests/cases/net_poller.ws` is the worked shape.

The consequence worth stating: `@join` on a worker parked in `accept` waits for
ever. That is a program waiting on its own worker rather than anything the exit
path can answer for. A worker whose `init` never returns is abandoned at exit
instead, and `main` returning ends the process
(`tests/cases/worker_daemon_exit.ws`).

**Tracked.** [#14](https://github.com/sinisterMage/WSharp/issues/14).

## `std/tls` cannot verify a chain through a P-521 key

**What.** TLS verifies chains through RSA, P-256 and P-384. A chain through a
P-521 key is **refused with a diagnostic** — not accepted, and not a crash. A
typical trust store has one such root.

**Why.** `std/nistec` is one curve implementation parameterised by limb count,
coordinate size and scalar width, over `std/bignum`'s generic Montgomery
multiplication — which is what made P-384 a table of constants rather than a
second implementation. P-521 is not the same again: 521 bits is not a whole
number of 32-bit limbs, and a limb is 32 bits because there is no 64x64 -> 128
product to build a wider one from (`bits.mulhi` does not exist). So it needs
either a partial top limb threaded through the arithmetic, or the wide multiply.

A root in the *store* that this library cannot read is dropped and the rest are
used; one in a *chain* is a refusal. That asymmetry is deliberate — answering
both the same way either makes a machine with one odd root unusable, or makes a
broken chain acceptable.

**Workaround.** None within `std/tls`. A service reachable only through a P-521
chain cannot be reached by this client.

**Tracked.** [#15](https://github.com/sinisterMage/WSharp/issues/15).

## FFI takes C scalars and nothing else

**What.** A binding's annotated type may use the integer widths, `bool` (C
`_Bool`), `f64` (C `double`), `u64` (which carries a native pointer on W#'s
64-bit targets) and `void`. **Structs by value, variadic functions, callbacks
into W#, and W# heap references are not supported**, and a wrong native
signature is undefined behaviour rather than a diagnostic
(`crates/wsharp-runtime/src/std/ffi.ws`).

**Why.** `mono::foreign_wrapper` rejects anything that is not a C scalar, and
codegen emits a thunk taking native scalar storage. A W# reference crossing into
C would be a pointer the collector may move, held somewhere no stack map
describes; a callback would re-enter W# on a thread in a safe region, where the
collector has already decided nothing on the heap is being touched.

**Workaround.** Marshal through `ffi.buffer` / `ffi.c_string` / `ffi.read` /
`ffi.write`, which are plain native memory the collector does not own. For a
callback, invert the loop: poll from W# rather than being called from C.

**Tracked.** No issue; this is the FFI's design boundary, and the one place in
W# where getting it wrong is undefined behaviour rather than an error message.

## `str.hash` is neither keyed nor cryptographic

**What.** `std/map` hashes keys with `str.hash` — FNV-1a followed by the
SplitMix64 finaliser (`crates/wsharp-runtime/src/strings.rs:427`). A table built
from attacker-chosen keys can be made to collide.

**Why.** A hash written in W# would be one `str.byte_at` per byte, and a builtin
call is a stack walk under `--gc-stress`, so a hash over a key would be one
stack walk per byte. The builtin is what makes `std/map` usable under the
collector's own test suite.

**Workaround.** Reach for `std/hash` when the keys come from outside —
SHA-256 or HMAC, and hash into a fixed-width key yourself.

**Tracked.** No issue. Stated because "is the map's hash safe against a hostile
client" is a question a service author has to be able to answer, and the answer
is no.

## `std/hash` carries SHA-1, and it is there for git

**What.** SHA-1 is in `std/hash` because git names every object by one. Nothing
in `std/tls` or `std/x509` uses it, and a fetched package's integrity key is
`ingot/store`'s SHA-256.

**Workaround.** Do not use it for anything new. It is not a general-purpose
digest and its presence is not an endorsement.

**Tracked.** No issue.

## A removed `std/list` or `std/map` entry stays reachable until its slot is reused

**What.** `list.pop` and `list.remove` decrement the count and leave the vacated
slot alone. `map` marks a removed entry's `state` byte dead and leaves the key
and value in place. The collector walks every element the array header claims,
so the removed object stays alive until something overwrites that slot.

**Why.** `l.items[i] = null` only typechecks when the element type is itself an
optional, so there is nothing to write into the slot in the general case.

**Workaround.** For a large object whose lifetime matters, hold it in an
optional element type so the slot can be nulled, or drop the container.

**Tracked.** No issue; documented behaviour, not a leak in the collector. Listed
because a user watching a heap profile will otherwise conclude it is one.

---

# Platform and toolchain

## x86-64 and aarch64 only

**What.** No other architecture builds. `crates/wsharp-runtime/src/stackwalk.rs:46`:

```rust
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!(
```

The four release triples are `x86_64-unknown-linux-gnu`,
`x86_64-pc-windows-msvc`, `x86_64-apple-darwin` and `aarch64-apple-darwin`.

**Why.** The collector's root walk reads the frame pointer with inline assembly,
per architecture. `walk_generated` is then the same everywhere, because
Cranelift's prologue establishes the frame pointer whatever the calling
convention — so a third architecture needs the register read and a Cranelift
backend this project actually tests, not a rewrite of the walk.

**Workaround.** None.

**Tracked.** [#11](https://github.com/sinisterMage/WSharp/issues/11).
`RELEASE-CRITERIA-1.0.md` states that 1.0 does not add a supported platform.

## No reproducible builds

**What.** A third party cannot rebuild a release artefact and get the same
bytes. What is published is a digest per artefact, and the promise is that the
digest matches the bytes served.

**Why.** A Rust release build is not bit-identical across machines without work
the 1.0 campaign has not scoped.

**Workaround.** Verify the published digest, which both installers do before
extracting.

**Tracked.** Stated as a non-promise in `RELEASE-CRITERIA-1.0.md`, "What this
document does not promise".

## The runtime's C symbols are not a stable ABI

**What.** The `ws_*` symbols in `libwsharp_start.a` and the runtime archive are
an internal boundary between the compiler and its own runtime. Linking against
them from outside is not supported and they may change in a patch release.

**Tracked.** Stated as a non-promise in `RELEASE-CRITERIA-1.0.md`; repeated here
because the symbols are visible in a built binary and therefore look like an
interface.

---

# Where limitations that closed went

`ROADMAP.md` keeps every limitation that was closed as one line apiece, under
"Closed by item 13", "Closed by item 14" and the development log itself — so a
reader who remembers a limitation finds out where it went rather than wondering
whether they misremembered it. Ten stood on the wsharp.io limitations page at
`0.1.1` and do not now.

**A limitation is never removed from this file without a fix.** It moves to that
list, with the release that closed it.
