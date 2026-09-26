# What W# 1.0 promises

**Status: in force for the v1.0 campaign.** The terms — which surfaces are
frozen, and for how long — are fixed in
[Part five of RELEASE-CRITERIA-1.0.md](RELEASE-CRITERIA-1.0.md#part-five-the-api-and-stability-freeze).
This file states the same promise to users, surface by surface. If the two ever
disagree, Part five is the one that is right and this file gets fixed.

This document states the promise. [RELEASE-CRITERIA-1.0.md](RELEASE-CRITERIA-1.0.md)
defines the bar the release has to clear, including — in Part one — the
definition of a **breaking change** and the **severity scale**. This file does
not restate either. Two copies of a definition drift, and that one is a gate on
the freeze, so its drift is expensive. Where this document says *breaking*, it
means what Part one says it means.

## The shape of the promise

> **1.0 promises that a program which compiles and behaves correctly against
> `v1.0.0` still compiles and behaves correctly against every `1.x` release,
> for the surfaces listed below.**

Two things follow from that sentence, and they are the whole reason this file is
short.

**Anything not listed below is not promised.** Not "probably fine", not "we will
try" — not promised. That is deliberate. A promise is a thing that cannot be
withdrawn without a major version, so the set of them is chosen rather than
inherited from whatever happened to be observable. If something you depend on is
not here and you think it should be, that is a conversation worth having before
the tag rather than after it.

**The limitations in [LIMITATIONS.md](LIMITATIONS.md) are not promises either.**
A limitation may be fixed in a `1.x` release. Fixing one is additive when it
accepts a program that was rejected before, and that is the usual shape.

## For how long

- **The whole `1.x` line.** A program that compiles and behaves correctly on
  `v1.0.0` does so on every later `1.x`, for every surface listed below.
  Breaking changes wait for `2.0`.
- **`1.x` receives P1 fixes for at least 12 months after the `v1.0.0` tag, and
  until `2.0` is tagged, whichever is later.** A P1 in `std/tls`, `std/x509` or
  the download path, or in the install and upgrade path, is fixed on the `1.x`
  line regardless of what is happening on `main`.
- **Deprecation before removal.** A promised name that is to go in `2.0` is
  deprecated in a `1.x` release first, with its replacement named. `2.0` does not
  remove something a `1.x` release never warned about.
- **`2.0` states what it breaks**, surface by surface, against this file.

---

# Surface by surface

## Language syntax

**Promised.** Every syntactic form accepted by `v1.0.0` keeps its meaning.
`const`, `var`, `fn`, `struct`, the generic parameter list, `?T`, `!T`, the
`!{A, B}T` error-set spelling, `try`, `catch |e|`, `orelse`, `for`, `while`
with its `: (i += 1)` continuation, the `@import`/`@spawn`/`@join` builtins, the
`[]T{ … }` array literal, the `Struct{ .field = … }` literal, the `struct : Base`
subtype spelling, and the operator set with its precedence.

**Not promised.**

- Whether a program that `v1.0.0` **rejects** is still rejected. Accepting more
  is additive.
- Diagnostic text, spans, help lines and their wording. A program that greps a
  diagnostic is depending on something no release promises.
- `--emit=ast`, which is a debugging aid shared with the parser tests and says
  so in its own help text. `--emit=api` is the one with a promise attached.

## Inference

**Promised.** A program that compiles under `v1.0.0` compiles under every `1.x`
and its top-level functions keep the signatures `--emit=types` reported. That
covers the rules a user actually builds on: an integer literal is a
`comptime_int` that settles at the three documented points, `f64` is one of the
types it may become when the value is exact, generalisation happens per binding
group, and a `fn` literal bound to a `const` is a definition rather than a value.

**Not promised.**

- The *internal* types of intermediate expressions, which nothing observes.
- The order in which diagnostics are reported when a program has several errors.
- That a program rejected by `v1.0.0` stays rejected.

## `std/*` and `ingot/*`

**Promised.** For every `pub` name in the modules listed below: the name, the
module it lives in, its parameter list and order, its return type, and — for a
fallible function — its **declared error set**. An error set is part of a
function's type here, so widening one is as breaking as narrowing one.

The modules: `std/array`, `std/bignum`, `std/broker`, `std/bytes`,
`std/cipher`, `std/crypto`, `std/curve25519`, `std/der`, `std/ffi`, `std/fs`,
`std/hash`, `std/http`, `std/inflate`, `std/json`, `std/list`, `std/map`,
`std/math`, `std/net`, `std/nistec`, `std/os`, `std/path`, `std/process`,
`std/rsa`, `std/str`, `std/tls`, `std/toml`, `std/x509`; and `ingot/fault`,
`ingot/git`, `ingot/manifest`, `ingot/packfile`, `ingot/pktline`, `ingot/plan`,
`ingot/progress`, `ingot/pubgrub`, `ingot/registry`, `ingot/semver`,
`ingot/store`.

Also promised: the **struct types** those modules export and their field names,
field types and place in the dispatch lattice. A struct's field order is part of
its layout and a subtype's fields are its supertype's followed by its own, so
reordering a field or moving a type in the lattice is breaking.

**Not promised.**

- A name that is not `pub`. `pub` is the surface; everything else is the
  implementation, and W#'s module system enforces that rather than asking.
- A **test hook** — a `pub` function that exists so a test can reach a primitive
  a whole operation would hide, and says so in its doc comment.
  `curve25519.field_mul`, `cipher.aes_sub_byte` and `tls.client_replay` are the
  three. They are `pub` for the test harness, not for callers.
- The `raw_*` builtin rows and the `ws_*` C symbols behind them. They are the
  boundary between the compiler and its own runtime archive; the W# wrapper is
  the interface.
- Performance. `RELEASE-CRITERIA-1.0.md` treats a 2x regression on a case as a
  P2 defect; that is a guard against a cliff, not a published figure.
- The output of `std/map` iteration order, or any other ordering a module does
  not document.

**Adding an overload to a `std/*` set is not automatically additive**, because
this language dispatches on every argument: an overload that makes an existing
call ambiguous, or that wins where another used to, is breaking. Part one says
so; it is repeated here only because it is the rule most likely to be missed
when a `1.x` adds a convenience.

## The CLI

### `wsharp`

**Promised.** The verbs `check`, `run`, `build` and `help`; their arguments; the
flags `--emit`, `--gc-stress`, `-o`/`--out`, `--module`, `-h`/`--help`,
`-V`/`--version`; and the `--emit` value names `tokens`, `ast`, `api`, `types`,
`hir`, `clif`, `obj`. `wsharp run` exits with `main`'s return value, as a C
program does. Everything after the file — or after `--` — is the program's
`os.args()` and nothing is interpreted on the way through.

The environment variables `WSHARP_GC_STATS`, `WSHARP_GC_TRACE`,
`WSHARP_GC_STRESS` and `WSHARP_RUNTIME_LIB` keep their names and meanings.

**Not promised.** The *content* of `--emit=tokens`, `--emit=ast`,
`--emit=types`, `--emit=hir` and `--emit=clif`. Four of those are debugging
aids; `clif` is Cranelift's own IR and changes when Cranelift does.
`--emit=api` is the exception and has its own section below.

### `ingot`

**Promised.** The verbs `init`, `add`, `remove`, `update`, `search`, `resolve`,
`install`, `verify`, `list`, `why`, `gc`, `store`, `run`, `help`; the `-C <dir>`
prefix; `--quiet`/`-q`; `--path <dir>` on `add`; and the environment variables
`WSHARP_HOME` and `INGOT_REGISTRY`, including that `INGOT_REGISTRY` naming an
existing **directory** uses it where it lies and never fetches it.

The exit statuses, which are the interface a script uses
(`crates/wsharp-runtime/src/ingot/main.ws:42-48`):

| Status | Means |
|---|---|
| `0` | `OK` — and, from `verify`, "ready" |
| `1` | `NEEDS_INSTALLING` |
| `2` | `NEEDS_RESOLVING` |
| `3` | `BROKEN` |
| `4` | `FAILED` — the verb could not be carried out at all, which is a mistake rather than an answer about a project |

The output discipline: **stdout carries what a verb achieved, stderr carries
why it got none**, progress goes to stderr, and a verb's answer is
tab-separated.

**Promised about TSV output**: the columns a verb prints today keep their
meaning and their order, and a column is never removed or reordered. **A column
may be appended.** A reader that splits on tab and indexes the columns it knows
keeps working; one that asserts on the field *count* does not, and that is
stated here so nobody writes the second kind.

**Not promised.** The prose of `ingot help`; the wording of a progress line; the
order of rows where a verb does not document one.

### `sharpie`

**Promised.** The verbs `show`, `default`, `toolchain`, `init`, `install`,
`uninstall`, `update`, `override`; the `+<toolchain>` prefix on a proxied
command; `SHARPIE_TOOLCHAIN`; and the `wsharp-toolchain.toml` file name and the
upward walk that finds it. `sharpie show` names the rung that answered, because
that is the first question when the answer surprises somebody.

**Not promised.** That the rung *set* stays closed — a `1.x` may add a
resolution rung, which is additive as long as it sits below every existing one
in precedence. The precedence order of the existing rungs is promised.

## On-disk formats

**Promised: that each of these stays readable by the version that wrote it, and
that a change of layout is a version bump rather than a silent reinterpretation.**

| Format | Where it is defined | The guard |
|---|---|---|
| The three emitted tables — type registry, stack maps, services | `crates/wsharp-runtime/src/aot.rs`, written by `crates/wsharp-codegen/src/tables.rs` | `MAGIC` (`WS#T`) and `VERSION`, checked at startup. A program built by one compiler and linked against another's runtime is refused with a message, not miscompiled. |
| `ingot.env` | written by `ingot install`, read by `Loader::follow` | Derived, absolute, and never committed. Unreadable and malformed get the same answer — `run ingot install` — because writing it again is the fix for both. |
| `ingot.lock` | `ingot resolve` | Promised readable across `1.x`. A lockfile written on one machine is readable on another: the path separator is `/` on every platform, Windows included. |
| The store tree hash | defined in `ingot/store.ws` and nowhere else | SHA-256 over each entry sorted by name: `"f" name 0 <decimal size> 0 <contents>` for a file, `"d" name 0 <hex of the subtree's hash> 0` for a directory. Permissions and timestamps are deliberately not in it. **Changing this is breaking**, because a key two versions compute differently is a store that silently splits in half. |

**Promised about the store.** A published version is never edited: a source
string maps to one tree digest for ever (`store.remembered`). A withdrawal is
`yanked = true`, which is filtered out of *selection* and stays installable,
because a lockfile that already names it must still work.

**Not promised.** The *directory layout* inside `WSHARP_HOME`, beyond the tree
hash being the key. A `1.x` may reorganise what sits around the store entries.

## `--emit=api`

**Promised.** This is the only emit with a promise attached, and it has three
parts:

1. **1.0 emits `api` format version 1** (`crates/wsharp-cli/src/api.rs:52`). The
   output opens with `(api 1)`.
2. Every name a program defines is **absolute** in the output.
3. It is printed after type checking, so it only ever describes a program the
   compiler accepted.

A change that could make an existing reader wrong is a bump of `api::VERSION` as
well as a breaking change under Part one. A golden test
(`crates/wsharp-cli/tests/api.rs`) pins the whole output for a two-module
program, so a format change fails there rather than quietly in somebody's
generator.

**Not promised.** That version 1 is the last one. A `1.x` may emit version 2;
what it may not do is emit version 1 with a different meaning.

## Release artefacts and the feed

**Promised: the naming scheme, because both installers spell URLs from it
rather than reading an index.**

- One archive per release per triple: `wsharp-<version>-<triple>.tar.gz`, where
  `<version>` has no `v` prefix.
- Beside it, a per-target sidecar `wsharp-<version>-<triple>.tar.gz.sha256`.
- Inside, a single directory `wsharp-<version>-<triple>/` containing `wsharp`,
  `ingot`, and `lib/` with the runtime archive.
- The four triples: `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
  `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- **The feed is the repository's git tags.** sharpie asks
  `https://github.com/sinisterMage/WSharp` for its refs over git's smart HTTP
  transport and reads `refs/tags/v<semver>`; `^{}` peeled entries are skipped
  and a tag that is not a version is skipped rather than raising. Every release
  tag is annotated. There is no JSON index to break.
- A release tag, once published, is never moved or deleted.

**Promised about integrity: what is promised is that the published digest
matches the bytes served, and that both installers verify the digest before
extracting anything.**

**Not promised:** that the digest is *signed*. A sidecar protects against a
corrupted or truncated transfer and not against whoever can serve the tarball,
because they can serve the sidecar too. `RELEASE-CRITERIA-1.0.md` criterion 5
puts the signing question to Johnny before the tag; **whichever way it is
decided, this section states the answer plainly rather than leaving it to be
inferred.** Until it is decided, the honest reading of this paragraph is the
weaker one.

**Not promised.** Reproducible builds. A third party cannot rebuild an artefact
and get the same bytes; see `LIMITATIONS.md`.

---

# What is outside the promise entirely

Collected so that the answer is one place rather than seven.

- **The runtime's C symbols.** `ws_*` in the runtime archive is an internal
  boundary between the compiler and its own runtime.
- **Anything reached by FFI.** A wrong native signature is undefined behaviour;
  W# cannot promise across a boundary it does not typecheck.
- **Diagnostic text**, everywhere.
- **`--emit` output other than `api`.**
- **Timing, pause lengths, allocation counts, and the collector's schedule.**
  `gc_live_objects()` and `WSHARP_GC_STATS` are instruments, not contracts.
- **Which documentation the promise covers.** 1.0's documentation promise covers
  what is in this repository — `README.md`, `docs/*.md`, `LIMITATIONS.md`, this
  file. The language reference and standard-library pages at
  [wsharp.io](https://wsharp.io), whose source is the public repository
  `sinisterMage/wsharp.io`, are **outside** it for 1.0, because nothing checks
  their examples against the compiler. See the note below.

## A note on wsharp.io

The documentation site's source lives in `sinisterMage/wsharp.io`, a public
repository with one workflow (`deploy.yml`) that publishes and does not check.
Gate 6a of `RELEASE-CRITERIA-1.0.md` — every documented example is a real file
executed in CI — therefore does not reach it as things stand.

**This is fixable rather than a reason to narrow the promise**, and the proposal
is a check workflow in that repository which clones `sinisterMage/WSharp` at the
tag and runs `scripts/check-doc-examples.sh` over `content/docs/**`, using the
same `<!-- from: examples/fib.ws -->` marker convention. That makes the site's
examples the same files the case suite runs.

Until that job exists and is green, **the 1.0 documentation promise covers the
in-repository documentation**, and the release notes say so. This sentence is
the one to delete once the job lands.
