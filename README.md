# W# (WSharp)

[![CI](https://forgejo-hagc.srv1954822.hstgr.cloud/ofekbickel/WSharp/actions/workflows/ci.yml/badge.svg)](https://forgejo-hagc.srv1954822.hstgr.cloud/ofekbickel/WSharp/actions?workflow=ci.yml)

**A Julia alternative for the programs Julia isn't for.**

Julia's central idea is right: multiple dispatch is a better way to organise a
program than either classes or a growing `switch`. Julia's execution model is
built around a different priority — numerical work in a REPL — and pays for it
with dynamic types, boxed values, and a stop-the-world collector.

W# keeps the idea and changes the priorities. Dispatch is multiple, but resolved
at compile time wherever the types allow. Types are inferred rather than
declared, but *checked* rather than advisory. Values are unboxed. The collector
is built for low pause times. The target is services, tools, and long-running
processes.

```wsharp
// The status types come from the standard library; nothing is declared here.
const http = @import("std/http");

fn render(r: Request, s: http.Status)      str { return "HTTP/1.1 500 Internal Server Error"; }
fn render(r: Request, s: http.Status2xx)   str { return "HTTP/1.1 200 OK"; }
fn render(r: Request, s: http.Status4xx)   str { return "HTTP/1.1 400 Bad Request"; }
fn render(r: Request, s: http.NotFound404) str { return "HTTP/1.1 404 Not Found"; }
fn render(r: Request, s: http.Teapot418)   str { return "HTTP/1.1 418 I'm a teapot"; }

// Resolved at compile time: the argument's type is exactly what it says.
print(render(req, http.NotFound404));

// Resolved at run time, from the type id in the object's header, because
// `s` could be any status by the time this runs.
fn serve(r: Request, s: http.Status) void { print(render(r, s)); }
```

Adding a special case means adding a function. Nothing existing is edited, and
the compiler rejects a new overload that would be ambiguous with an old one
rather than silently changing which code runs.

The whole example is in [`examples/status.ws`](examples/status.ws).

## Compared with Julia

|  | Julia | W# |
|---|---|---|
| Dispatch | multiple, always dynamic | multiple, static wherever inference pins the types |
| Types | dynamic; annotations optional and advisory | Hindley-Milner; annotations optional and **checked** |
| Generics | one method, specialised at run time | monomorphised at compile time; unreachable copies dropped |
| Values | boxed by default | unboxed — `i64`, `f64`, `bool`, optionals and error unions live in registers |
| Errors | exceptions | `?T` optionals and `!T` error unions, Zig-style |
| Collector | generational, stop-the-world | reference counting with a coalescing barrier, a concurrent mark trace for cycles, and compaction |
| Aimed at | arrays, notebooks, science | services, tools, systems |

What Julia still does far better: a vast numerical ecosystem, a mature REPL and
package manager, and a decade of tuning. W# is a young language with a small
standard library. The table is a statement of design priorities, not a claim to
have replaced anything.

## The language

Zig-flavoured, with one deliberate departure: **type annotations are optional
everywhere.** They are checked when written and inferred when not.

| | |
|---|---|
| Bindings | `const x = 1;` immutable, `var y: i64 = 2;` mutable |
| Types | `i64` `f64` `bool` `void` `str`, `[]T` array, `?T` optional, `!T` error union, `fn(A) B` |
| Functions | `fn add(a, b) { return a + b; }`, `fn add(a: i64, b: i64) i64 { ... }` |
| Overloads | several `fn`s may share a name; the call picks the most specific |
| Abstract types | `Number` stands for `i64` and `f64`, so an overload can claim "any number" while another claims `i64` |
| Control flow | `if (c) { } else { }`, `while (c) : (i += 1) { }`, `for (xs) \|x\| { }`, `break`, `continue` |
| Expressions | `if (c) a else b`, `fn (a, b) { ... }` closures |
| Closures | `const id = fn (x) { return x; };` generalises, may name itself, and `fn [T](a: []T) T` writes the parameters out |
| Literals | `42`, `0xff`, `0b1010`, `0o17`, `1_000_000`, `2.5`, `"text"` with `\n \t \r \0 \\ \"` |
| Arrays | `[]i64{ 1, 2, 3 }`, `a[i]`, `for (a) \|v, i\| { }`; an index out of range panics |
| Growable | `std/list` — a backing array plus a count, so `push` is amortised constant time |
| Iterating | `for (xs) \|x\|` over an array walks it by index; over anything else it calls `iter` and `next` from the module that declares its type |
| Generics | `fn first[T](a: []T) T`, `const Box = struct[T] { value: T };`, `fn [T](x: T) T` — inferred when not written |
| Modules | `const http = @import("std/http");`, then `http.NotFound404`; `pub` is what another module may name |
| Structs | `const P = struct { x: i64 };`, `P{ .x = 1 }`, `p.x` |
| Subtyping | `const Sub = struct : Base { };` — a subtype widens implicitly |
| Singletons | a struct with no fields is also a value: its sole instance |
| Optionals | `null`, `a orelse b`, `a.?`, `if (a) \|v\| { }`, `while (a) \|v\| { }` |
| Errors | `error.Name`, `try f()`, `f() catch 0`, `f() catch \|e\| ...` |
| Operators | `+ - * /`, `%` (integers only), `== != < <= > >=` (non-chaining), `and or !` |

`==` compares `str` by contents, so a string built at run time equals a literal.

One limit worth knowing before it surprises you: the `e` bound by `catch |e|`
is opaque, because `!T` has a single global error set rather than one per
function. See [ROADMAP.md](ROADMAP.md).

Some things that follow from optional annotations:

```zig
fn id(x) { return x; }          // fn(T) T -- generic, and monomorphised per use
fn add(a, b) { return a + b; }  // fn(i64, i64) i64 -- `+` defaults to i64
fn f(x) { return x * 2.0; }     // fn(f64) f64 -- the literal decides
```

A plain value coerces into an optional or an error union when the context wants
one, so `return n;` is legal in a function declared `!i64`. A subtype coerces
into its supertype for the same reason: both are one pointer, and a subtype's
layout begins with a byte-identical copy of its supertype's.

### Multiple dispatch

An overload set is several top-level functions sharing a name. Every parameter
of an overloaded function must be annotated — dispatch chooses *by* parameter
type, so those types cannot themselves be inferred from the calls being
resolved.

Overloading is not limited to struct types. An **abstract type** stands for a
set of concrete ones -- `Number` for `i64` and `f64` -- so a general case can
be written alongside a specific one:

```zig
fn show(x: i64)    str { return "an integer"; }
fn show(x: Number) str { return "a number"; }   // catches f64
```

An abstract type classifies values for dispatch and is never one itself: a
parameter annotated with it is a *generic* parameter constrained to the
members, compiled once per type it is used at, exactly as an unannotated
parameter is. Nothing is tested at run time, because a scalar's type is always
known at compile time.

An overload set can also be named. `const g: fn(i64) i64 = f;` picks the member
with that signature and gives you an ordinary function value; `const g = f;`
binds an alias that dispatches just as `f` does.

Selection is Julia's rule: an overload wins if it is at least as specific as
every other applicable one in every argument, and strictly more in at least
one. Two overloads that could both match the same call, with neither more
specific, are a **compile error**:

```zig
fn pick(a: Sub,  b: Base) i64 { return 1; }
fn pick(a: Base, b: Sub)  i64 { return 2; }
// pick(Sub, Sub) matches both -> error: this call to `pick` is ambiguous
fn pick(a: Sub,  b: Sub)  i64 { return 3; }   // ...and this settles it
```

When inference pins every argument to a type whose subtypes cannot change the
answer, the winner is known at compile time and the call lowers to an ordinary
direct call — no dispatch code at all. Otherwise the compiler emits a decision
chain over the runtime type id. Type ids are assigned in a preorder walk of the
subtype lattice, so every type's subtypes occupy a contiguous range and each
test is one subtract and one unsigned compare.

### The collector

Reference counting for the common case, a concurrent mark trace for what
counting cannot reclaim, and compaction to recover fragmented blocks. The
design follows LXR (Zuo, Blackburn, Zigman & Yang, *Low-Latency,
High-Throughput Garbage Collection*, PLDI 2022).

- **Immix-style heap.** 32 KiB blocks, 256-byte lines. Blocks come from
  over-aligned reservations, so an object's block and line follow from
  arithmetic on its address. Objects over 8 KiB get their own allocation and
  are never moved.
- **Coalescing reference counting.** The write barrier snapshots an object's
  outgoing references the first time it is modified in a cycle; the collector
  compares that snapshot against the current state. A field written a thousand
  times between collections costs one decrement and one increment, not a
  thousand of each. The fast path is a load, a test, and a not-taken branch.
- **Precise roots.** Every heap pointer the code generator produces is declared
  to Cranelift as a stack-map root, and the collector finds them by walking the
  frame-pointer chain and looking each return address up in the emitted maps.
  Because the roots are precise and updatable in place, objects can move.
- **A concurrent mark trace for cycles.** Two objects pointing at each other
  keep each other's counts above zero for ever; only reachability reclaims
  them, and reachability is a walk over the whole live heap. That walk runs on
  a collector thread while the program continues. Marking is
  snapshot-at-the-beginning: the barrier's snapshot of overwritten references
  is exactly the record the marker needs, objects allocated during the mark are
  born marked (the mark bit is a parity that flips per trace, so nothing is
  ever cleared), and nothing is freed while the marker runs. The sweep
  afterwards runs on the collector thread too, a block at a time.
- **Compaction, also concurrent.** Freeing works a line at a time, so a block
  pinned by a few scattered survivors stays mostly unusable. The trace copies
  those survivors out and releases the block. The copying runs while the
  program does, which is what the **load barrier** is for: every reference read
  out of a heap object is resolved to wherever that object lives now, so the
  program can never hold an address the collector has abandoned. Whoever
  reaches an object first -- the collector, or the program through the barrier
  -- moves it, and one compare-and-swap on the header decides whose copy wins.
  When nothing is moving the barrier costs a load, a test, and a branch that
  falls through.
- **Holes are refilled.** A block with free lines is allocated into again
  rather than waiting for a trace to come and evacuate it. On a heap of 300
  survivors scattered through 33,000 allocations, that is the difference
  between holding 28 blocks and holding 8.
- **Allocation takes no lock.** Each thread bumps through a buffer of its own,
  publishing the object-start bit and the line counts atomically and batching
  the statistics until the buffer is replaced. The heap lock is for handing out
  a new buffer, freeing, sweeping and evacuating -- so an allocating thread and
  a sweeping collector no longer queue behind each other on every object.
- **Three short pauses, and they do not grow with the heap.** Only the mutator
  can walk its own stack, so the parts that need the stack run on it: take a
  root snapshot; finish marking and move what the roots point at; repoint the
  references the marker noted. That last one visits a *list* -- the marker
  records every reference it sees into a block being emptied -- rather than the
  live heap, so the pause is proportional to what the program did, not to what
  it holds. On 120,000 live objects the longest pause measured 40 microseconds,
  the same as on 15,000; the previous stop-the-world collector took 1.2
  milliseconds on 30,000 and did not finish 60,000 at all. What is left in the
  pause and *does* grow is the counting collection: reconciling the write
  barrier's buffers costs what the program has modified since the last one, so
  a program that rewrites a large structure between traces will see
  milliseconds rather than microseconds there. Under `--gc-stress` the whole
  heap is walked afterwards and the run aborts if the list missed anything.
- **Interruptible loops.** Cranelift makes every call a safepoint and nothing
  else, so a loop that calls nothing would be uninterruptible. Each back edge
  carries a three-instruction poll, which is also how the collector thread
  asks the program to stop for its next pause.

`--gc-stress` collects at every allocation and checks every root the maps
describe. The end-to-end suite runs twice, once under it, and traces start on
the same allocation schedule in both runs so the concurrent paths are covered
both ways. `WSHARP_GC_STATS=1` prints what the collector did on exit, including
the number of pauses and the longest one.

## Building and running

The project needs a C toolchain, because rustc shells out to `cc` to link. On a
bare NixOS box there isn't one on `PATH`, so a dev shell is provided:

```sh
nix-shell                         # rustc + cargo from the system, cc from nixpkgs
cargo test --workspace
cargo run -p wsharp-cli -- run examples/status.ws
```

Or without entering the shell:

```sh
nix-shell --run "cargo run -p wsharp-cli -- run examples/status.ws"
```

Outside Nix, any environment with `cc` and Rust 1.95+ works with plain `cargo`.
Note that `.cargo/config.toml` sets `-Cforce-frame-pointers=yes`: the collector
walks the frame-pointer chain out of the runtime to find its roots, and the
chain has to be unbroken through the Rust frames as well as the generated ones.

## The compiler

```sh
wsharp run   <file.ws>    # compile and run main; exits with main's return value
wsharp check <file.ws>    # type-check only
```

The process exits with the low byte of `main`'s return value, as a C program
does, so `return 256;` exits 0. A compile error exits 1. A failure the type
system allows but the program must not perform — `.?` on a null optional, a
failed `assert`, integer division by zero, `i64::MIN / -1`, an index outside an
array, a call no overload matches — prints `W# panic: <reason>` to stderr and
exits with status 101.

`--emit` stops after a stage and prints it, which is the fastest way to see what
the compiler is thinking:

```sh
wsharp check examples/inference.ws --emit=types   # inferred signatures
wsharp check examples/fib.ws       --emit=ast     # parsed syntax tree
wsharp run   examples/fib.ws       --emit=clif    # generated Cranelift IR
wsharp run   examples/fib.ws       --emit=hir     # typed, monomorphised IR
wsharp check examples/fib.ws       --emit=tokens
```

`--emit=types` prints one line per top-level function, so an overload set shows
up as several:

```
$ wsharp check examples/status.ws --emit=types
render: fn(Request, Status) str
render: fn(Request, Status2xx) str
render: fn(Request, Status4xx) str
render: fn(Request, NotFound404) str
render: fn(Request, Teapot418) str
serve: fn(Request, Status) void
main: fn() i64
```

Two flags exist for the collector: `--gc-stress` as above, and the
`WSHARP_GC_STATS` environment variable, which prints what the collector did on
exit. `WSHARP_GC_TRACE` prints every frame the root walk visits. Both are off
when unset, empty or `0`.

### The standard library

`@import` binds a module to a name; everything in it is reached through that
name. A path is either a file next to the importing one or one of the
library's. A module's names are private to it unless it writes `pub`.

```zig
const str  = @import("std/str");
const http = @import("std/http");

fn main() i64 {
    for (str.split("a,b,c", ",")) |part| { print(part); }
    return 0;
}
```

| Module | |
|---|---|
| `std/str` | `len` `concat` `eq` `substr` `find` `split` `join` `repeat` `starts_with` `from_int` `from_float` |
| `std/array` | `len` `new` `concat` `push` `slice` `repeat` |
| `std/list` | `List[T]`, a growable array: `new` `with_capacity` `from` `len` `capacity` `get` `set` `push` `pop` `insert` `remove` `extend` `clear` `iter` `next` `to_array` |
| `std/math` | `abs` `min` `max` `sign` `sqrt` `pow` `floor` `ceil` `round` `trunc` `ipow` |
| `std/io` | `read_file` `read_line` `write_file` `exists` — the fallible ones return `!str` |
| `std/http` | the 27 HTTP status types, materialised on first mention |

A **prelude** needs no import, because every module has it:

| | |
|---|---|
| `print(s: str)`, `print_int(i64)`, `print_float(f64)`, `print_bool(bool)` | write a line to stdout |
| `assert(c: bool)` | panic if `c` is false |
| `panic_index(i: i64, len: i64)` | the out-of-bounds panic, so a container written in W# reports a bad index exactly as `a[i]` does |
| `gc_collect()` | one reference-counting collection |
| `gc_trace()` | a whole mark trace, synchronously: cycles are reclaimed when it returns |
| `gc_trace_start()`, `gc_trace_finish()` | the two halves of a trace, so a program can mutate the heap while the collector thread marks it |
| `gc_live_objects()`, `gc_live_bytes()`, `gc_collections()`, `gc_traces()` | the collector's counters, for asserting on it |

Half the library is written in W# rather than Rust — `std/array`, `std/list`,
`std/math` and `str.split` are `.ws` files compiled with your program,
monomorphised per element type and dropped when nothing calls them. The rule that draws the line
is worth knowing if you add to it: **a builtin may read and write bytes, and
anything that moves a *reference* from one object into another is written in
W#**, where the write barrier, the load barrier and the stack maps all apply by
construction.

## How it works

```
source ──► wsharp-syntax ──► wsharp-sema ──► wsharp-codegen ──► native code
           lex, parse         infer, mono      Cranelift JIT
                                  │
                            wsharp-runtime
                    heap, collector, header, builtins
```

| Crate | Job |
|---|---|
| `wsharp-syntax` | Lexer, recursive-descent parser with Pratt-style precedence, spans, diagnostic rendering |
| `wsharp-sema` | Name resolution, Hindley-Milner inference, the subtype lattice, overload selection, typed HIR, monomorphisation, value layout |
| `wsharp-codegen` | HIR to Cranelift IR, the dispatcher, the write barrier, stack-map harvesting, JIT module setup |
| `wsharp-runtime` | Object header, block/line heap, reference counting, the mark trace and its thread, evacuation, stack walker, type registry, builtins — a leaf crate with no dependencies at all |
| `wsharp-cli` | The `wsharp` binary and the end-to-end test suite |

A few decisions worth knowing about:

- **Inference runs per binding group.** Top-level functions are grouped into
  strongly connected components of the call graph, inferred with their types
  held monomorphic, and generalised only once the whole group is done. That is
  what makes `fib` calling itself, and mutually recursive functions, check. An
  overload set lands in one group, so its members generalise together.
- **Subtyping is not in unification.** Making `unify` directional would mean
  threading a polarity through every recursive call, including function types,
  where parameters are contravariant. Instead `coerce` consults the lattice
  after unification fails — which works because unification binds whichever side
  is still a variable, so anything reaching the subtype check is already
  concrete.
- **Levels, not environment scans.** Type variables carry Rémy levels, so
  generalisation is a walk over one type rather than a scan of the environment.
- **Monomorphisation, because values are unboxed.** `i64`/`f64`/`bool` live in
  registers, so `fn(T) T` cannot be compiled once. Inference records the type
  arguments at each call site and a worklist pass emits one copy per
  instantiation. Unreachable functions fall out as dead code.
- **A value is a list of machine values, not one.** Scalars and pointers take one
  slot; `?T` and `!T` are a tag followed by the payload. No boxing, no
  allocation for an optional.
- **A subtype's fields are its supertype's, first.** That is what lets a field
  read compiled against a supertype run unchanged on any subtype, with no
  adjustment and no vtable.
- **Every function takes an environment pointer.** Top-level functions ignore it
  and are called with null. That uniformity lets a plain `fn` be passed as a
  value without generating a wrapper.
- **Standard library entries are one table row, or one line of W#.**
  `builtins.rs` holds `(module, name, parameters, return type, function
  pointer)`; inference reads it to seed each module's environment and code
  generation reads the same table to register JIT symbols. The HTTP status
  lattice is a second such table, and a program pays only for the statuses it
  names. Anything that moves a reference between objects is a `.ws` file
  instead, compiled with your program so the collector's barriers apply to it.
- **A module is a prefix on a name.** Names are stored qualified in one flat
  table, and an unqualified lookup tries the current module and then the
  prelude; what a module cannot see is what it has no key for. Everything is
  private to its module unless it says `pub`, and only a *qualified* lookup
  checks that -- an unqualified name can only mean this module's own or the
  prelude's, and both are always visible. Files are laid
  end to end in one offset space, so a `Span` stays two `u32`s with no file in
  it and the renderer works out which file a span fell in.

## Status

Sessions are numbered by the original feature list:

- [x] **1.** Core language, Zig-style syntax
- [x] **2.** Hindley-Milner type inference
- [x] **3.** Garbage collector — reference counting, a concurrent mark trace
      for cycles, and compaction
- [x] **4.** Multiple dispatch over a subtype lattice, with the HTTP status
      types as its standard-library instance, abstract types for scalars, and
      overload sets as values
- [x] **5.** Arrays, `for` loops, a growable array, and explicit generic
      parameters on functions, structs and `fn` literals
- [x] **6.** Standard library — strings, arrays, math and I/O — behind a module
      system
- [ ] **7.** Multithreading: workers with their own heaps, talking by typed RPC
      or through a message broker. Designed in [ROADMAP.md](ROADMAP.md), not
      built.
- [ ] **8.** Direct libc calls for I/O, and the networking that needs them.
      Before v0.5: `std/io` goes through Rust's `std` today, which cannot
      express non-blocking I/O — and a thread parked in a blocking read cannot
      answer a collector pause.

What is left, and where it plugs in, is in [ROADMAP.md](ROADMAP.md).
Conventions and the invariants worth not breaking are in
[CLAUDE.md](CLAUDE.md).
