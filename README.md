# W# (WSharp)

[![CI](https://forgejo-hagc.srv1954822.hstgr.cloud/ofekbickel/WSharp/actions/workflows/ci.yml/badge.svg)](https://forgejo-hagc.srv1954822.hstgr.cloud/ofekbickel/WSharp/actions?workflow=ci.yml)

**A compiled language built on multiple dispatch, for services, tools and
long-running processes.**

Multiple dispatch is a better way to organise a program than either classes or
a growing `switch`: a special case is a function you add, not a branch you
insert into something that already works. W# takes that idea and compiles it.
Dispatch is resolved at compile time wherever the types allow. Types are
inferred rather than declared, and checked across the whole program before it
builds. Values are unboxed. The collector is built for low pause times.

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

The documentation is at **[wsharp.io](https://wsharp.io)**: installing, a tour
through the examples, the language reference, the standard library, and how the
dispatcher and the collector actually work.

## What it's good at

- **Dispatch that mostly isn't there at run time.** When inference pins the
  arguments, the call lowers to an ordinary direct call — no dispatch code at
  all. When it can't, the test is one subtract and one unsigned compare against
  a contiguous range of type ids: no vtable, no inline cache, no method-table
  lookup. Type ids are assigned in a preorder walk of the subtype lattice,
  which is what makes that range contiguous.
- **Types you never write and can still rely on.** Hindley-Milner inference
  over the whole program, so `fn add(a, b) { return a + b; }` has a signature
  rather than a hope. Annotations are optional everywhere and checked where
  written. An ambiguous pair of overloads, an error raised outside a declared
  `!{…}` set, and a function that can reach the end of its body without
  returning a value are all compile errors.
- **Failure in the type, and it says which.** `!T` carries the error *set* —
  inferred from what a function raises and propagates, or written down as
  `!{NotFound, IoFailed}str` and checked — so the `e` bound by `catch |e|` is
  worth testing against. Nothing caps how many errors a set may name.
- **Values that cost what they say.** A `u8` is a byte and `[]u8` is a byte
  array. `?T` and `!T` are a tag and a payload in registers: no boxing, no
  allocation for an optional. Generics are monomorphised, so `fn(T) T` becomes
  one copy per instantiation and the unreachable ones are never emitted.
- **Pauses that don't grow with the heap.** Reference counting with a
  coalescing write barrier, a concurrent mark trace for the cycles counting
  can't reclaim, and compaction that runs while the program does. The longest
  pause measured 40 microseconds on 120,000 live objects — and the same on
  15,000, because a pause visits what the program changed rather than what it
  holds.
- **Threads that share no heap.** A worker is an ordinary module: `init` makes
  the state, and any function taking that state first is something the worker
  can be asked to do. Values cross as bytes, so there is no shared collector,
  no lock on the fast path, and no data race to write. `std/broker` is the same
  idea at the other end — topics, partitions, consumer groups with their own
  offsets, and replay.
- **A standard library written in the language.** SHA-2, ChaCha20-Poly1305,
  AES-GCM, X25519, P-256, P-384, RSA, X.509 and TLS 1.3 are all `.ws` files
  compiled with your program, so `http.get("https://…")` is W# the whole way
  down. They are monomorphised per use and dropped when nothing calls them.
- **Nothing you didn't ask for.** A library module is read only if something
  imports it, so `wsharp check` on a ten-line file takes about five
  milliseconds however far the library grows. The runtime crate has no
  dependencies at all; the operating system is declared by hand.

W# is young and its standard library is small. What is here is tested end to
end, and the whole case suite runs a second time under a collector that
collects at every allocation and validates every root.

## The language

Zig-flavoured, with one deliberate departure: **type annotations are optional
everywhere.** They are checked when written and inferred when not.

| | |
|---|---|
| Bindings | `const x = 1;` immutable, `var y: i64 = 2;` mutable |
| Types | `i8` `i16` `i32` `i64` `u8` `u16` `u32` `u64` `f64` `bool` `void` `str`, `[]T` array, `?T` optional, `!T` error union, `fn(A) B` |
| Functions | `fn add(a, b) { return a + b; }`, `fn add(a: i64, b: i64) i64 { ... }` |
| Overloads | several `fn`s may share a name; the call picks the most specific |
| Abstract types | `Number` stands for every numeric type, `Integer` for the eight integer ones and `Signed` for the four signed ones, so an overload can claim "any number" while another claims `i64` |
| Control flow | `if (c) { } else { }`, `while (c) : (i += 1) { }`, `for (xs) \|x\| { }`, `break`, `continue` |
| Expressions | `if (c) a else b`, `fn (a, b) { ... }` closures |
| Closures | `const id = fn (x) { return x; };` generalises, may name itself, and `fn [T](a: []T) T` writes the parameters out |
| Literals | `42`, `0xff`, `0b1010`, `0o17`, `1_000_000`, `2.5`, `"text"` with `\n \t \r \0 \\ \"`; an integer literal takes the type it is used at -- including `f64`, where the value is exact -- and defaults to `i64` |
| Arrays | `[]i64{ 1, 2, 3 }`, `a[i]` at any integer type, `g[i][j] = v`, `for (a) \|v, i\| { }`; an index out of range panics |
| Growable | `std/list` — a backing array plus a count, so `push` is amortised constant time |
| Iterating | `for (xs) \|x\|` over an array walks it by index; over anything else it calls `iter` and `next` from the module that declares its type |
| Generics | `fn first[T](a: []T) T`, `const Box = struct[T] { value: T };`, `fn [T](x: T) T` — inferred when not written |
| Modules | `const http = @import("std/http");`, then `http.NotFound404`; `pub` is what another module may name, and `pub const parse = inner.parse;` renames one so a package of several files can present one |
| Packages | `ingot` resolves and installs; `@import("acme/json")` then names a package's facade, exactly as a library path names a module |
| Workers | `@spawn(counter, 0)` starts a thread with a heap of its own, `w.add(5)` calls into it, `@join(w)` waits for it |
| Messages | `std/broker` — named topics, partitioned logs, consumer groups with their own offsets, and replay |
| Bytes | `std/bytes` — `[]u8` as a buffer, the bridge to and from `str`, word accessors and hex |
| Crypto | `std/hash` — SHA-2, HMAC, HKDF; `std/cipher` — ChaCha20-Poly1305 and AES-GCM; `std/crypto` — the system's generator |
| Key agreement | `std/curve25519` — X25519; `std/nistec` — ECDH on NIST P-256 and P-384, with the key-share validation RFC 8446 requires |
| Signatures | `std/rsa` — PKCS#1 v1.5 and PSS verification; `std/curve25519` — Ed25519, signing and verification; `std/nistec` — ECDSA verification on P-256 and P-384 |
| TLS | `std/tls` — TLS 1.3, client and server; `std/x509` — certificates and chains, so `http.get("https://…")` works |
| Structs | `const P = struct { x: i64 };`, `P{ .x = 1 }`, `p.x` |
| Subtyping | `const Sub = struct : Base { };`, or `struct : pkg.Base` — a subtype widens implicitly |
| Singletons | a struct with no fields is also a value: its sole instance |
| Optionals | `null`, `a orelse b`, `a.?`, `if (a) \|v\| { }`, `while (a) \|v\| { }` |
| Errors | `error.Name`, `try f()`, `f() catch 0`, `f() catch \|e\| ...`, `f() catch return e`, `f() catch return false`, `f() catch { log(); 0 }` |
| Error sets | `!i64` infers which errors; `!{NotFound, IoFailed}str` writes them down and is checked |
| Operators | `+ - * / %`, `& \| ^ << >> ~` (integers only), `== != < <= > >=` (non-chaining), `and or !`; `u32(x)` converts |

`==` compares `str` by contents, so a string built at run time equals a literal.

`!T` says *which* errors: the set is inferred from what a function raises and
propagates, or written down and checked. So the `e` bound by `catch |e|` is
something worth testing.

```zig
fn risky(n: i64) !i64 {            // fn(i64) !{Negative, Zero}i64
    if (n < 0) { return error.Negative; }
    if (n == 0) { return error.Zero; }
    return n;
}
const v = risky(n) catch |e| if (e == error.Negative) 0 else -1;
```

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

The steps compose, so `return Sub{ .. };` is legal in a function declared
`!Base` and `!?T` is a type worth writing -- a value, nothing, or a failure, in
one return.

### Multiple dispatch

An overload set is several top-level functions sharing a name. Every parameter
of an overloaded function must be annotated — dispatch chooses *by* parameter
type, so those types cannot themselves be inferred from the calls being
resolved.

Overloading is not limited to struct types. An **abstract type** stands for a
set of concrete ones -- `Number` for every numeric type, `Integer` for the
eight integer ones, `Signed` for the four signed ones -- so a general case can
be written alongside a specific one:

```zig
fn show(x: i64)     str { return "an integer"; }
fn show(x: Integer) str { return "some width of integer"; }
fn show(x: Number)  str { return "a number"; }   // catches f64
```

Abstract types are ordered by their member sets, so `Integer` is more specific
than `Number` and wins wherever both apply. A body annotated `Number` must work
for *every* type it lists, which is why it may not use a bit operator (`f64` has
no bit pattern to ask for) or negate (no unsigned negatives) -- `Integer` and
`Signed` are what such bodies claim. `std/math`'s `abs` and `sign` are one
definition over `Signed` for exactly that reason.

An abstract type classifies values for dispatch and is never one itself: a
parameter annotated with it is a *generic* parameter constrained to the
members, compiled once per type it is used at, exactly as an unannotated
parameter is. Nothing is tested at run time, because a scalar's type is always
known at compile time.

An overload set can also be named. `const g: fn(i64) i64 = f;` picks the member
with that signature and gives you an ordinary function value; `const g = f;`
binds an alias that dispatches just as `f` does.

Selection is by specificity: an overload wins if it is at least as specific as
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

## Installing

[**sharpie**](https://github.com/sinisterMage/sharpie) is the version manager,
in the mould of `rustup` and `juliaup`: it installs toolchains, keeps several
side by side, puts proxies on `PATH`, and lets a directory pin the version it
wants.

```sh
curl -fsSL https://raw.githubusercontent.com/sinisterMage/sharpie/main/install.sh | sh
export PATH="$HOME/.sharpie/bin:$PATH"
sharpie install stable
```

It is written in W#, which was the point rather than a constraint for the same
reason `ingot` was: a program that has to speak HTTP, verify a digest, read TOML
and unpack an archive is a broad enough one to find out where the library bends.

Otherwise take a release tarball from
[Releases](https://github.com/sinisterMage/WSharp/releases) and put its
directory on `PATH`. An installation is a directory rather than a single file,
because `wsharp` looks for its runtime archive beside itself and under `../lib`,
and `ingot` looks for `wsharp` beside itself before `PATH`.

Builds exist for `x86_64-unknown-linux-gnu` and both Darwins.
[wsharp.io/docs/install](https://wsharp.io/docs/install/) has the rest,
including why there is no Windows one.

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

### Fetching something over TLS

Every part of this is a `.ws` file — the hash, the cipher, the curve, the
signature, the certificate parser and the handshake — and the trust anchors are
the ones the machine already has.

```zig
const http = @import("std/http");
const text = @import("std/str");

fn main() i64 {
    const answer = http.get("https://www.google.com/") catch return 1;
    print_int(answer.code);
    print_int(text.len(answer.body));
    return 0;
}
```

It is not in `examples/`, because the test suite runs every example and a
network-dependent one would make CI depend on the weather. Save it and run it:

```sh
nix-shell --run "cargo run -p wsharp-cli -- run /tmp/fetch.ws"
```

`example.com`, `github.com`, `nixos.org`, `www.cloudflare.com` and
`crates.io` all work, which between them cover RSA, P-256 and P-384 chains.
What does not is a chain through a P-521 key — there is one such root in a
typical store, and it is the last line under what is left in
[ROADMAP.md](ROADMAP.md).

## The compiler

```sh
wsharp run   <file.ws>            # compile and run main; exits with main's return value
wsharp check <file.ws>            # everything but the code generation
wsharp build <file.ws> -o <prog>  # compile to a native executable
```

`check` accepts exactly what `run` accepts: it goes as far as monomorphisation,
which is where a generic call nothing pinned is reported, and stops before code
generation. A file with no `main` is a library and is fine to check.

`run` compiles into memory and runs there, which is what you want while writing
something. `build` writes a real program: the collector, the workers, TLS and
the rest of the runtime are linked into it, and it needs no compiler on the
machine that runs it. Linking is done by `$CC`, or `cc`, against a runtime
archive that `wsharp` looks for beside itself and under `../lib` — so an
installation is a directory rather than a single file. The archive is
`libwsharp_start.a`, or `wsharp_start.lib` where MSVC named it: cargo names a
staticlib after the platform rather than after the crate, and both spellings are
looked for. `--emit=obj` stops at the relocatable object, which is the half that
needs no C compiler.

Anything after the file is the program's, not the compiler's, and reaches it
through `std/os`:

```sh
wsharp run prog.ws -- one two    # os.args() is ["one", "two"]
```

The process exits with the low byte of `main`'s return value, as a C program
does, so `return 256;` exits 0. A compile error exits 1. A failure the type
system allows but the program must not perform — `.?` on a null optional, a
failed `assert`, integer division by zero, a signed `MIN / -1`, an index
outside an array, a call no overload matches — prints `W# panic: <reason>` to
stderr and exits with status 101. Ordinary overflow is not one of them:
`+`, `-` and `*` wrap, which for an unsigned type is the definition rather than
a concession.

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
name. A path is a file next to the importing one, one of the library's, or a
package this project depends on. A module's names are private to it unless it
writes `pub`.

A `const` may also be a second name for something another module declares,
which is what lets a package of several files present one of them:

```zig
const inner = @import("./inside.ws");

pub const Pair  = inner.Pair;      // a type
pub const twice = inner.twice;     // a function, or a whole overload set
```

The alias and the original are the same type and the same function set rather
than copies of them, so a value made through one is usable through the other and
an overload set renamed once still dispatches on every member.

`std/io` reads and writes whole files; `std/fs` is the tree they sit in --
`mkdir`, `read_dir`, `rename`, `remove`, and enough of a stat to tell a
directory from a file and say how big one is. It also has `chmod` and
`is_executable`, which are what a program that *writes another program* needs:
a file written by `io.write_file` is 0o644, and 0o644 is not a thing that can be
run. There is no mode *reader* -- that would mean a `struct stat`, whose layout
differs on every system in the BSD family, and the question worth asking is
"will this start" rather than "which bits are set". Windows has no permission
bits, so `chmod` succeeds there without doing anything and `is_executable` is
`exists`. `std/path` is the arithmetic above both, and makes no syscall at all.
`std/os` is what the process knows about itself -- `args`, `get`, `home`,
`temp_dir`, `cwd`, and `target`, the triple this binary was built for.
`std/toml` is TOML 1.0, read and written.

A library module is only read if something imports it, so a program that
mentions nothing pays for nothing: `wsharp check` on a ten-line file takes
about five milliseconds whatever the library grows to.

```zig
const str  = @import("std/str");
const http = @import("std/http");

fn main() i64 {
    for (str.split("a,b,c", ",")) |part| { print(part); }
    return 0;
}
```

```zig
const net  = @import("std/net");
const http = @import("std/http");

fn main() i64 {
    const l = net.listen("127.0.0.1", 8080, 16) catch return 1;
    const c = http.connection(net.accept(l) catch return 2);
    const request = http.read_request(c) catch return 3;
    http.respond(c, 200, "text/plain", request.path) catch return 4;
    return 0;
}
```

A blocking read is safe here rather than merely tolerated: a builtin that blocks
does so inside a *safe region*, so the collector can walk this worker's stack and
run its pauses while the thread waits on the network. `std/net.poller` is for
serving many connections from one worker, not for keeping the collector alive.

| Module | |
|---|---|
| `std/str` | `len` `concat` `eq` `substr` `find` `split` `join` `repeat` `starts_with` `from_int` `from_float` `byte_at` `from_byte` `parse_int` `to_lower` `trim` |
| `std/array` | `len` `new` `concat` `push` `slice` `repeat` |
| `std/list` | `List[T]`, a growable array: `new` `with_capacity` `from` `len` `capacity` `get` `set` `push` `pop` `insert` `remove` `extend` `clear` `iter` `next` `to_array` |
| `std/math` | `abs` `min` `max` `sign` `rem` `sqrt` `pow` `floor` `ceil` `round` `trunc` `ipow` |
| `std/bits` | `rotl` `rotr` — rotation, generic over `Integer`, one instruction on both targets; `f64_bits` `f64_from_bits` — an `f64`'s representation, which is what a wire format carries |
| `std/io` | `read_file` `read_line` `write_file` `exists` — the fallible ones name their errors, e.g. `!{NotFound, PermissionDenied, IoFailed}str` |
| `std/net` | TCP: `Socket` `Listener` and `connect` `listen` `accept` `read` `write` `write_all` `read_exactly` `read_all` `set_nonblocking` `close`. UDP: `Datagrams` `Peer` `Datagram` and `udp` `send_to` `receive` `reply`. Readiness: `Poller` `Event` and `poller` `watch` `wait`. IPv4 or IPv6, with the family the resolver's choice |
| `std/http` | the 27 HTTP status types, materialised on first mention, plus an HTTP/1.1 client and server: `get` `post` `request` `read_request` `respond` `header` `status_of`; and since item 10, `https://` over `std/tls` |
| `std/broker` | `Topic[M]` `Consumer[M]` and `topic` `publish` `subscribe` `next` `commit` `seek` `len` |
| `std/bytes` | `[]u8` as a buffer, and the bridge to and from `str`: `new` `of` `to_str` `slice` `concat` `copy` `fill` `xor` `equal`, the big- and little-endian word accessors, `to_hex` `from_hex` |
| `std/hash` | SHA-256, SHA-384 and SHA-512, one-shot and incremental, plus `hmac` `hkdf_extract` `hkdf_expand` — written once over a `Hash` value that says a block size, a digest size and how to hash |
| `std/cipher` | ChaCha20, Poly1305, ChaCha20-Poly1305; AES-128/256, GHASH, AES-GCM. Constant-time by construction: no table is indexed by a secret byte, so AES's S-box is computed in GF(2^8) and GHASH is 128 shifts |
| `std/crypto` | `random` — the system's generator, which is the kernel's |
| `std/time` | `now` — seconds since the Unix epoch |
| `std/bignum` | fixed-width unsigned limbs and Montgomery arithmetic: `from_be` `to_be` `cmp` `add` `sub` `mont` `mont_mul` `mont_add` `mont_sub` `to_mont` `from_mont` `modexp`. A limb is 32 bits, which is what makes a 64x64 → 128 product unnecessary |
| `std/curve25519` | `x25519` `x25519_base` — and the small-order check on the *output*, which is the one a list of bad encodings misses |
| `std/nistec` | `p256` `p384` `derive` `ecdh` `valid` `ecdsa_verify` — the NIST prime curves, one implementation over `std/bignum` |
| `std/rsa` | `public_key` `verify_pkcs1` `verify_pss` — verification only, since TLS 1.3 does no RSA key exchange. The encoded message is built and compared, never parsed |
| `std/der` | a strict DER reader: `read_value`, `read_seq`, `read_uint`, `read_oid`, `read_bitstring`, `read_time` (item 10) |
| `std/x509` | `SigKey` and its three subtypes, `parse_spki`, `verify_signature`; certificates, `matches_host`, `verify_chain`, `pem_certificates`, `system_roots` (item 10) |
| `std/tls` | TLS 1.3, both ends: `client`, `server`, `feed`, `pending`, and a blocking `Session` over a socket (item 10) |

A **prelude** needs no import, because every module has it:

| | |
|---|---|
| `print(s: str)`, `print_int(i64)`, `print_uint(u64)`, `print_float(f64)`, `print_bool(bool)` | write a line to stdout; a narrower value is written `print_int(i64(x))`, because conversions are written rather than inferred |
| `assert(c: bool)` | panic if `c` is false |
| `panic_index(i: i64, len: i64)` | the out-of-bounds panic, so a container written in W# reports a bad index exactly as `a[i]` does |
| `gc_collect()` | one reference-counting collection |
| `gc_trace()` | a whole mark trace, synchronously: cycles are reclaimed when it returns |
| `gc_trace_start()`, `gc_trace_finish()` | the two halves of a trace, so a program can mutate the heap while the collector thread marks it |
| `gc_live_objects()`, `gc_live_bytes()`, `gc_collections()`, `gc_traces()` | the collector's counters, for asserting on it |

Most of the library is written in W# rather than Rust — `std/array`, `std/list`,
`std/math`, `std/net`, `std/http`, all of the cryptography and `str.split` are `.ws` files compiled with your program,
monomorphised per element type and dropped when nothing calls them. The rule that draws the line
is worth knowing if you add to it: **a builtin may read and write bytes, and
anything that moves a *reference* from one object into another is written in
W#**, where the write barrier, the load barrier and the stack maps all apply by
construction.

## Packages

`ingot` is the package manager. `wsharp` stays the compiler, and the split is on
purpose: one of them has to work on a machine with no network and no store, and
the other is the thing that fills the store.

```sh
ingot init myapp                    # write an ingot.toml here
ingot add acme/json                 # record a dependency, from the registry
ingot add util --path ../util       # or on a directory
ingot resolve                       # choose versions and write ingot.lock
ingot install                       # make the store satisfy it
ingot verify                        # 0 ready, 1 install, 2 resolve, 3 broken
ingot why core                      # the paths that pulled it in
```

Resolving, installing and building are separate verbs: nothing compiles because
something else was fetched. Output is tab-separated, `verify` answers with its
exit status, and a conflict comes back as the derivation that caused it:

```
Because no versions of core match >=2.0.0 <3.0.0 and util 0.3.0 depends on
core >=2.0.0 <3.0.0, util 0.3.0 cannot be used.
```

A dependency is a version, a directory or a git revision. A version comes from
the registry — [Foundry](https://github.com/sinisterMage/Foundry), an index of
plain TOML in a git repository, in the shape of Julia's General:

```sh
ingot add acme/json          # the newest published version, as a caret
ingot search json            # what is published
ingot update                 # fetch the index again
```

A registry release records the hash of its own tree, which is the store key —
so `resolve` chooses versions and writes a lockfile having fetched no package at
all, and the fetch `install` does afterwards is *checked against that hash*. The
client trusts a hash rather than a host, and the registry's own CI is what makes
the hash a promise: every entry is fetched and hashed before it is merged.
Packages are W# source; nothing is precompiled, because `wsharp build` is
ahead-of-time and the machine that installs is the machine that compiles.

A registry is a **directory**, and cloning one over git is only how the
directory arrives — `INGOT_REGISTRY` naming a directory is used where it lies
and never fetched, which is what a private registry is, an offline one, and how
this project tests the whole path with no server. A path or git dependency
overrides the registry for the name it supplies.

Packages live in a content-addressed store under `~/.wsharp` (`WSHARP_HOME`
moves it), named by the hash of their tree, so two projects that want the same
tree share one copy and `ingot gc` drops what no lockfile reaches.

After `ingot install`, a package is just a module path:

```zig
const util = @import("util");

fn main() i64 { return util.twice(21); }
```

which works under plain `wsharp run` as well as `ingot run` — `install` writes
an `ingot.env` beside the lockfile saying where each package's files ended up,
and the compiler reads that. A package presents exactly one file, the `root` in
its manifest; what else it shows is what that file re-exports. And it may import
only what its own `ingot.toml` asked for, even though the lockfile holds the
whole graph.

`ingot` is written in W#, which was the point rather than a flourish: a
resolver, a hash, a protocol and a file format is a broad enough program to find
out what the language is actually missing. It is now written in W# *all the way
out* — there is no Rust driver behind it, and `cargo build` does not produce it.
`wsharp build --module ingot/main -o ingot` does, which makes the package
manager the first real user of the compiler's own `build`. The three things its
driver used to do are W#'s now: `-C` is `os.chdir`, its diagnostics go to
stderr, and `ingot run` becomes `wsharp run` through `os.exec` rather than
embedding a compiler a W# program cannot have.

## How it works

```
source ──► wsharp-syntax ──► wsharp-sema ──► wsharp-codegen ──► native code
           lex, parse         infer, mono      Cranelift: JIT
                                               or object file
                                  │
                            wsharp-runtime
                    heap, collector, header, builtins
```

| Crate | Job |
|---|---|
| `wsharp-syntax` | Lexer, recursive-descent parser with Pratt-style precedence, spans, diagnostic rendering |
| `wsharp-sema` | Name resolution, Hindley-Milner inference, the subtype lattice, overload selection, typed HIR, monomorphisation, value layout |
| `wsharp-codegen` | HIR to Cranelift IR, the dispatcher, the write barrier, stack-map harvesting, and both backends — the JIT and the object writer, over one lowering |
| `wsharp-runtime` | Object header, block/line heap, reference counting, the mark trace and its thread, evacuation, stack walker, type registry, builtins — a leaf crate with no dependencies at all |
| `wsharp-cli` | The `wsharp` binary, the module loader, the linker driver, and the end-to-end test suites |
| `wsharp-start` | The `main` a compiled program starts in, and the archive it links against — `wsharp-runtime` bundled with a startup that installs the emitted tables |

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
  prelude's, and both are always visible. Re-export falls out of that shape:
  `pub const T = other.T;` is one more key in the same table, holding the same
  type or the same function set, so nothing below the type checker knows it
  happened. Files are laid
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
- [x] **7.** Multithreading — workers with their own heaps, talking by typed
      RPC or through a Kafka-shaped message broker
- [x] **8.** The operating system declared by hand — files and sockets on
      Linux, macOS/BSD and Windows arms, a readiness API, and a *safe region*
      that lets a thread block in a syscall while its collector walks the
      stack it left behind. `std/net` and `std/http` are on top of it.
- [x] **9.** Sized and unsigned integers, and bitwise operators — `i8` through
      `u64`, `& | ^ << >> ~`, a rotate, and integer literals that take the type
      they are used at. `i64` was the right default and the wrong only choice
      the moment a program computed on bytes; ChaCha20's quarter round is now a
      test case rather than a thing the language could not say.
- [x] **10.** TLS 1.3, written in W#, with certificate chains verified against
      the platform's own root store — which is what turns `https://` from
      `error.NotSupported` into a connection. Every hash, cipher, curve,
      signature scheme and X.509 structure is a `.ws` file; the whole client
      and server are replayed against RFC 8448's published traces byte for
      byte. `http.get("https://www.google.com/")` returns a page.
- [x] **11.** Package management, in a tool called **ingot**: git spoken rather
      than shelled out to, a content-addressed store, and a resolver that says
      *why* a version was ruled out rather than that it was. A program can read
      its own command line and walk a directory, and `struct stat` turned out
      not to be needed to do it. TOML 1.0 is read and written in W#; the store
      can tell "not installed" from "damaged"; the tool itself is a W# program
      with two hundred lines of Rust under it. The resolver is PubGrub over
      version *sets* as unions of intervals, which answers a conflict with the
      derivation that caused it and found a real bug in the garbage collector
      on its way in. Git is SHA-1, zlib inflate, pkt-line framing and a
      packfile with both kinds of delta resolved, all of it W#, replayed
      against a conversation a real `git upload-pack` took part in. And the
      last stage is the point of the other four: `@import("acme/json")`
      compiles — one more branch in the loader, and a `pub const x = other.x;`
      that lets a package of several files present one of them. And there is a
      registry, [Foundry](https://github.com/sinisterMage/Foundry): an index of
      TOML in a git repository, recording per version the commit to fetch and
      the tree it must hash to, so resolving is arithmetic over a file and
      installing is checked against a hash somebody's CI already verified.

Since that list ran out:
[**sharpie**](https://github.com/sinisterMage/sharpie), a version manager, and
the second real program written in W#. It finds releases by reading this
repository's tags over git's smart HTTP rather than through a forge's REST API,
because that answers in JSON, W# has no JSON reader, and writing one would have
stood between sharpie and its first useful act. Two plain HTTP conversations, no
new parser.

What is left, and where it plugs in, is in [ROADMAP.md](ROADMAP.md).
Conventions and the invariants worth not breaking are in
[CLAUDE.md](CLAUDE.md). The documentation is at
[wsharp.io](https://wsharp.io).
