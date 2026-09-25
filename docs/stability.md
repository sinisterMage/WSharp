# Stability audit and reproduction

This is implementation evidence for independent QA, not a language stability or
release verdict. The audit starts at W# 0.2.3 commit
`716e852e9199ef8eb9ac3f713771429154ef8a5a`. The review branch is
`ada/wla-2-stabilization`; record its exact commit when reproducing it.

## Environment and commands

The local audit uses Debian 13.7, x86_64, Rust 1.95.0
(`59807616e 2026-04-14`), Cargo 1.95.0, and Zig 0.15.2's clang 20.1.2.
Rust components were checksum-verified and installed in the workspace, as was
the C toolchain. No system toolchain or infrastructure was changed. The local
`cc` wrapper invokes `zig cc -target x86_64-linux-gnu "$@" -lunwind`; the final
flag resolves the Rust archive's unwind symbols with this linker. Standard CI
uses its platform C compiler. Cargo.lock pins dependencies, including Cranelift
0.134.3. Preserve `.cargo/config.toml`'s frame-pointer setting.

From a fresh checkout with Rust 1.95.0 and a working C linker:

```sh
git rev-parse HEAD
rustc --version
cargo --version
cc --version
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Build **before** tests: Cargo does not build the `wsharp-start` static archive
when running tests, and AOT tests must link the runtime from this source tree.
Set `CC` for W#'s native linker if it is not named `cc`. The local provisioning
script is outside the repository; no machine-specific toolchain paths are
committed. Network and process tests use local fixtures and loopback sockets.

Focused commands:

```sh
cargo test -p wsharp-cli --test cases a_compiler_panic_is_not_an_expected_compile_error --locked
cargo test -p wsharp-runtime aliased_roots_resolve_to_the_same_evacuation_copy --locked
cargo test -p wsharp-syntax --test generated --locked
./target/debug/wsharp run tests/cases/gc_map_model.ws
./target/debug/wsharp run --gc-stress tests/cases/gc_map_model.ws
./target/debug/wsharp build tests/cases/gc_map_model.ws -o target/map-model
./target/map-model
WSHARP_GC_STRESS=1 ./target/map-model
```

## Feature and assertion matrix

Case names below mean `tests/cases/<name>.ws`; Rust tests live beside their
implementation or under a crate's `tests/`. Headers assert ordered stdout,
exit status, compiler diagnostics or language panics. Compiler-error cases now
require exit 1, so a Rust panic quoting the expected diagnostic cannot pass.
The full suite runs all cases in JIT, stress JIT and AOT modes, and GC/worker
cases also in stress AOT. This maps representative assertions, not branch
coverage or proof of correctness of every combination.

| Feature | Normal assertions | Boundary assertions | Invalid-input assertions | Interactions |
|---|---|---|---|---|
| Lexer/parser/diagnostics | `arithmetic`, parser precedence/dump tests | `err_too_deeply_nested`, empty and UTF-8 generated inputs | `err_bad_escape`, `err_bad_radix_digit`, malformed generated fragments | source offsets in a second module; diagnostic rendering |
| Integers/floats/conversion | `ints_sized`, `floats`, `int_convert` | wrap at signed/unsigned widths; `shift_signedness`, `bits_rotate` | `err_int_literal_range`, `err_mixed_int_widths`, `panic_div_overflow` | packed fields, worker arguments, generic conversions |
| Control flow/scope | `loops`, `if_expression`, `recursion` | `while_capture` reaches null; `mutual_recursion` terminates | `err_break_outside_loop`, `err_missing_return` | returns inside branches, captures, module shadowing |
| Inference/generics/dispatch | `inference`, `generics`, `dispatch` | `generic_recursion`, `abstract_convert_widths` | `err_unpinned_generic`, `err_ambiguous_dispatch` | `closures_generic`, `reexport_merge` |
| Arrays/structs/subtypes | `arrays`, `structs`, `subtype_fields` | `array_ops` slices/concatenation; `struct_eq` unequal fields/subtypes | `panic_index_negative`, `panic_index_out_of_bounds`, `err_missing_field` | `ints_packed`, `gc_compound_index`, seeded map oracle |
| Closures/function values | `closures`, `overload_value` | `closures_recursive`, generic captures at distinct types | `err_assign_generic_closure`, `err_overload_annotation` | `gc_function_value`, `closures_generic` |
| Optionals/error unions | `optionals`, `errors`, `catch_block` | null branch, success/error payloads, `error_void` | `panic_unwrap_null`, `err_error_not_in_set`, `err_reraise_too_wide` | `reraise`, `wide_return`, map optional results |
| Modules/visibility | `module_import`, `module_reexport` | alias/re-export merging and local shadowing | `err_import_missing_file`, `err_private_name`, `err_reexport_clash` | module fixtures, package facade loading |
| GC/reference lifetimes | `gc_roots`, `gc_concurrent` | fresh-object grace tests; repeated evacuation of aliased roots | runtime rejects implausible root addresses; raw malformed pointers are not a source-language input | `gc_barrier_concurrent`, `gc_map_replacement`, `gc_map_model` |
| Workers/RPC/broker | `worker_rpc`, `broker` | `worker_daemon_exit`, `worker_blocking_init` | `err_not_transferable`, `err_spawn_not_a_service` | `gc_transfer`, `worker_heaps`, `net_gc` |
| Containers/text | `list`, `map`, `array_ops` | `map_growth`: 2000 keys, tombstones, removal/reinsert; empty-key/model traces | bounds panic cases; missing map keys/removals return absence (valid API input) | references through generics, iterators and concurrent collection |
| JSON/TOML | `json_values`, `toml_values` | nested values, round trips and JSON depth limits in existing cases | `json_reject` and parser rejection assertions in TOML cases | `json_write`, `toml_tables`, package manifests |
| OS/files/process/network/FFI | `io`, `fs_ops`, `net_tcp`, process/FFI integration tests | partial I/O, readiness retries, byte buffers, child exit status | `net_refused`, `process_invalid`, `err_ffi_heap_argument`, `ffi_buffers` missing library | `net_gc`, `net_poller`, native linking |
| Crypto/TLS/X509 | `hash_sha2`, `tls_rfc8448` and published-vector cases | block sizes, handshake transcripts, certificate parsing | `tls_reject`, `x509_reject`, `ed25519_reject`, AEAD altered inputs | `https_loopback`, `tls_schedule`, `tls_loopback` |
| Package resolution/store/verbs | `ingot_store`, `pubgrub_solving`, `verbs.rs` | version ranges, tombstones/yanks, damaged store repair in existing cases | manifest/lock/fetch diagnostic fixtures and invalid commands | filesystem, hashing, facade imports, subprocess execution |

The baseline has 257 top-level W# cases (64 compiler-error, 10 panic and 190
stdout-bearing cases; categories overlap). This branch adds `gc_map_model`, one
runtime regression, one harness regression and one 512-input syntax test.
Counts alone are not a coverage claim. The task evidence includes the complete
case/header inventory so QA can inspect the expectations rather than infer
coverage from filenames.

## New defects and generated tests

1. The case harness accepted any unsuccessful process with a matching error
   substring, including a compiler panic (101). Its regression fails before the
   change and passes after requiring diagnostic exit 1; status 0, 2 and 101 and
   an unrelated diagnostic are checked separately.
2. A second root alias could enter `evacuate_one` after the first replaced the
   source header with a forwarding address. Reading that address as a type/size
   could return the original pointer or interpret an unrelated layout. The
   deterministic runtime regression fails before the forwarding check and
   passes afterwards. Language syntax and object layout are unchanged.

The syntax generator uses xorshift seed `0x575348415250`, 512 inputs and at most
64 fragments per input. It checks panic freedom, monotone token spans, valid
UTF-8 boundaries, diagnostic spans and rendering with a nonzero module base.
Failures retain the seed, case number and full input. It is bounded robustness
coverage, not a specification for acceptance of malformed programs.

The map model uses LCG seeds `1`, `0x57534841`, `0xdeadbeef`, 128 operations per
seed and 24 keys. An indexed array is the independent oracle. Every operation
checks presence, values, count, keys and unique iterator entries. It covers
empty keys, missing removal, replacement, growth, clear and tombstone reuse
with heap references. Seeds and operation indexes survive in failures.
The larger investigation repeated all three traces 32 times; three normal runs
passed after the fix (288 traces), as did three ordinary-size stress runs
(nine traces). A 32-repeat stress run hit a 60-second investigation limit;
that run is not a passing result or an established hang (one repetition takes
about five seconds in this environment). The deterministic runtime test,
not a probabilistic workload result, establishes the forwarding regression.

## CI and QA limits

Existing GitHub CI already discovers the added Rust tests and W# case; no
workflow expansion is needed. It builds before testing on Ubuntu, macOS ARM,
macOS Intel, Windows and Debian 13/glibc 2.41, with fmt and clippy jobs.
The Forgejo workflow is retained upstream but documented as disabled.
Local results prove only Debian/x86_64. Hosted CI results must be recorded
against the exact review commit before a cross-platform handoff is approved.

Known documented limitations remain: cyclic structural equality is recursive
without cycle detection; removed map/list values can remain reachable in spare
storage; mutation during map iteration is unsupported; test headers cannot
express leading/trailing whitespace or arguments containing spaces; only JSON,
not author-controlled TOML, has a nesting bound. None is silently redefined by
this patch. Fuzzing is bounded and cannot establish all concurrent schedules,
resource exhaustion, external network interoperability or cryptographic
security. BSD platform behavior is not validated here.

Quinn owns independent application validation and the stability verdict. Record
clean-checkout full-suite results and exact commit in the task evidence, then
validate three useful applications with normal/failure fixtures and repeated
runs. Merging, tagging and release publication are outside this change.
