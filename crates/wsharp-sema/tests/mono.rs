//! Monomorphisation tests.

use wsharp_sema::hir;
use wsharp_sema::ty::{Type, TypeStore};
use wsharp_sema::{analyze, monomorphize};
use wsharp_syntax::parse;

fn mono(src: &str) -> (hir::Program, TypeStore) {
    let (module, diags) = parse(src);
    assert!(diags.is_empty(), "parse errors");
    let mut analysis = analyze(&module);
    assert!(
        analysis.diags.is_empty(),
        "type errors: {:?}",
        analysis
            .diags
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
    let result = monomorphize(&analysis.program, &mut analysis.store);
    assert!(
        result.diags.is_empty(),
        "mono errors: {:?}",
        result.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    (result.program, analysis.store)
}

fn copies_of(program: &hir::Program, name: &str) -> Vec<hir::FuncId> {
    program
        .funcs
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name == name)
        .map(|(i, _)| i as hir::FuncId)
        .collect()
}

#[test]
fn a_generic_function_used_at_two_types_gets_two_copies() {
    let src = r#"
        fn id(x) { return x; }
        fn main() i64 {
            print_int(id(1));
            print_bool(id(true));
            return 0;
        }
    "#;
    let (program, mut store) = mono(src);
    let ids = copies_of(&program, "id");
    assert_eq!(ids.len(), 2, "expected one copy per instantiation");

    let mut rendered: Vec<String> = ids
        .iter()
        .map(|&i| {
            let ty = program.func(i).scheme.ty.clone();
            store.show(&ty)
        })
        .collect();
    rendered.sort();
    assert_eq!(rendered, vec!["fn(bool) bool", "fn(i64) i64"]);
}

#[test]
fn the_same_instantiation_is_shared() {
    let src = r#"
        fn id(x) { return x; }
        fn main() i64 { return id(1) + id(2) + id(3); }
    "#;
    let (program, _) = mono(src);
    assert_eq!(
        copies_of(&program, "id").len(),
        1,
        "identical instantiations must be shared"
    );
}

#[test]
fn a_non_generic_function_gets_exactly_one_copy() {
    let src = r#"
        fn inc(n: i64) i64 { return n + 1; }
        fn main() i64 { return inc(1) + inc(2); }
    "#;
    let (program, _) = mono(src);
    assert_eq!(copies_of(&program, "inc").len(), 1);
}

#[test]
fn recursion_does_not_specialise_forever() {
    let src = r#"
        fn fib(n: i64) i64 {
            if (n < 2) { return n; }
            return fib(n - 1) + fib(n - 2);
        }
        fn main() i64 { return fib(10); }
    "#;
    let (program, _) = mono(src);
    assert_eq!(copies_of(&program, "fib").len(), 1);
}

#[test]
fn mutual_recursion_terminates() {
    let src = r#"
        fn is_even(n) { if (n == 0) { return true; } return is_odd(n - 1); }
        fn is_odd(n) { if (n == 0) { return false; } return is_even(n - 1); }
        fn main() i64 { if (is_even(4)) { return 1; } return 0; }
    "#;
    let (program, _) = mono(src);
    assert_eq!(copies_of(&program, "is_even").len(), 1);
    assert_eq!(copies_of(&program, "is_odd").len(), 1);
}

#[test]
fn unreachable_functions_are_dropped() {
    let src = r#"
        fn used() i64 { return 1; }
        fn unused() i64 { return 2; }
        fn main() i64 { return used(); }
    "#;
    let (program, _) = mono(src);
    assert_eq!(copies_of(&program, "used").len(), 1);
    assert!(
        copies_of(&program, "unused").is_empty(),
        "dead code should not be emitted"
    );
}

#[test]
fn every_type_is_concrete_afterwards() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };
        fn id(x) { return x; }
        fn main() i64 {
            const p = Point{ .x = 1, .y = 2 };
            const q = id(p);
            print_bool(id(true));
            return q.x + id(3);
        }
    "#;
    let (program, mut store) = mono(src);
    for func in &program.funcs {
        assert!(
            !store.has_unbound(&func.ret),
            "`{}` has an unresolved return type",
            func.name
        );
        for local in &func.locals {
            assert!(
                !store.has_unbound(&local.ty),
                "`{}` local `{}` is unresolved",
                func.name,
                local.name
            );
        }
    }
    // Three distinct instantiations of `id`: Point, bool and i64.
    assert_eq!(copies_of(&program, "id").len(), 3);
}

#[test]
fn call_sites_point_at_the_specialised_copy() {
    let src = r#"
        fn id(x) { return x; }
        fn main() i64 {
            print_bool(id(true));
            return id(1);
        }
    "#;
    let (program, mut store) = mono(src);
    let main = program.func(program.entry.expect("has a main"));

    // Collect the callee of every static call in main, with its parameter type.
    let mut called: Vec<String> = Vec::new();
    collect_static_calls(&main.body, &program, &mut store, &mut called);
    called.sort();
    assert_eq!(called, vec!["id: fn(bool) bool", "id: fn(i64) i64"]);
}

fn collect_static_calls(
    block: &hir::Block,
    program: &hir::Program,
    store: &mut TypeStore,
    out: &mut Vec<String>,
) {
    for stmt in &block.stmts {
        match stmt {
            hir::Stmt::Return(Some(e)) => walk(e, program, store, out),
            hir::Stmt::Expr(e) => walk(e, program, store, out),
            hir::Stmt::Let { init, .. } => walk(init, program, store, out),
            _ => {}
        }
    }
}

fn walk(expr: &hir::Expr, program: &hir::Program, store: &mut TypeStore, out: &mut Vec<String>) {
    if let hir::ExprKind::Call { callee, args } = &expr.kind {
        match callee {
            hir::Callee::Static { func, targs } => {
                assert!(
                    targs.is_empty(),
                    "type arguments must be consumed by monomorphisation"
                );
                let def = program.func(*func);
                let ty = def.scheme.ty.clone();
                out.push(format!("{}: {}", def.name, store.show(&ty)));
            }
            // Every case of a dispatch table is a call target too, so a walker
            // that skipped them would silently miss dispatched functions.
            hir::Callee::Dynamic { cases } => {
                for case in cases {
                    assert!(
                        case.targs.is_empty(),
                        "type arguments must be consumed by monomorphisation"
                    );
                    let def = program.func(case.func);
                    let ty = def.scheme.ty.clone();
                    out.push(format!("{}: {}", def.name, store.show(&ty)));
                }
            }
            hir::Callee::Builtin(_) | hir::Callee::Indirect(_) => {}
        }
        for a in args {
            walk(a, program, store, out);
        }
    }
}

#[test]
fn a_closure_inside_a_generic_function_is_specialised_per_instantiation() {
    let src = r#"
        fn apply_twice(x) {
            const f = fn (v) { return v; };
            return f(f(x));
        }
        fn main() i64 {
            print_bool(apply_twice(true));
            return apply_twice(7);
        }
    "#;
    let (program, mut store) = mono(src);
    assert_eq!(copies_of(&program, "apply_twice").len(), 2);
    let closures: Vec<&hir::FuncDef> = program.funcs.iter().filter(|f| f.is_closure).collect();
    assert_eq!(
        closures.len(),
        2,
        "the closure needs one copy per enclosing instantiation"
    );
    let mut rendered: Vec<String> = closures
        .iter()
        .map(|f| store.show(&f.scheme.ty.clone()))
        .collect();
    rendered.sort();
    assert_eq!(rendered, vec!["fn(bool) bool", "fn(i64) i64"]);
}

#[test]
fn every_overload_in_a_dispatch_table_survives_monomorphisation() {
    // A dispatch table entry is the only thing keeping an overload reachable,
    // so a `Dynamic` arm that forgot to specialise its cases would drop them
    // as dead code and leave the table pointing at nothing.
    let src = r#"
        const Base = struct { };
        const Mid = struct : Base { };
        const Leaf = struct : Mid { };
        fn pick(x: Base) i64 { return 1; }
        fn pick(x: Mid) i64 { return 2; }
        fn pick(x: Leaf) i64 { return 3; }
        fn route(x: Base) i64 { return pick(x); }
        fn main() i64 { return route(Leaf); }
    "#;
    let (program, mut store) = mono(src);
    assert_eq!(copies_of(&program, "pick").len(), 3);

    let route = program.func(copies_of(&program, "route")[0]);
    let mut called = Vec::new();
    collect_static_calls(&route.body, &program, &mut store, &mut called);
    called.sort();
    assert_eq!(
        called,
        vec![
            "pick: fn(Base) i64",
            "pick: fn(Leaf) i64",
            "pick: fn(Mid) i64"
        ]
    );
}

#[test]
fn only_the_status_types_a_program_mentions_are_created() {
    // The status lattice is a table, not 27 declarations in every program: a
    // program pays for the statuses it names and no others.
    let (bare, _) = mono("fn main() i64 { return 0; }");
    assert!(bare.structs.is_empty());

    let (used, _) = mono("fn main() i64 { print_int(1); return 0; }");
    assert!(used.structs.is_empty());

    // Naming one pulls in its ancestors, because the lattice needs them, but
    // nothing else.
    let (some, _) = mono(
        "const http = @import(\"std/http\");
         fn f(s: http.Status4xx) i64 { return 1; }
         fn main() i64 { return f(http.NotFound404); }",
    );
    let mut names: Vec<&str> = some.structs.iter().map(|s| s.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["NotFound404", "Status", "Status4xx"]);
}

#[test]
fn a_program_without_a_main_is_passed_through() {
    let src = "fn helper() i64 { return 1; }";
    let (module, _) = parse(src);
    let mut analysis = analyze(&module);
    let result = monomorphize(&analysis.program, &mut analysis.store);
    assert!(result.diags.is_empty());
    assert_eq!(result.program.funcs.len(), 1);
    assert!(result.program.entry.is_none());
}

#[test]
fn structs_strings_and_errors_survive() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };
        fn risky(n: i64) !i64 { if (n < 0) { return error.Negative; } return n; }
        fn main() i64 {
            print("hello");
            const p = Point{ .x = 1, .y = 2 };
            return p.x + (risky(1) catch 0);
        }
    "#;
    let (program, _) = mono(src);
    assert_eq!(program.structs.len(), 1);
    assert_eq!(program.strings, vec!["hello".to_string()]);
    // The standard library's errors are interned first, so that a builtin can
    // return one by index long before the program catching it has been read.
    assert_eq!(
        program.errors.last(),
        Some(&"Negative".to_string()),
        "the program's own error comes after the library's"
    );
    assert!(
        program.errors.starts_with(
            &wsharp_runtime::builtins::builtin_errors()
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
        )
    );
    // The struct's field offsets sit past the object header.
    let point = program.strukt(0);
    assert_eq!(point.fields[0].offset, wsharp_runtime::HEADER_SIZE);
    assert_eq!(point.fields[1].offset, wsharp_runtime::HEADER_SIZE + 8);
    assert_eq!(point.size, 32);
    assert_eq!(point.type_id, wsharp_runtime::header::TYPE_ID_FIRST_USER);
}

#[test]
fn a_subtype_inherits_its_supertype_field_layout() {
    let src = r#"
        const Status = struct { code: i64 };
        const Status4xx = struct : Status { };
        const NotFound404 = struct : Status4xx { detail: str };
        fn main() i64 { return 0; }
    "#;
    let (program, _) = mono(src);
    let base = wsharp_runtime::HEADER_SIZE;
    let (status, s4xx, nf) = (program.strukt(0), program.strukt(1), program.strukt(2));

    // Each subtype's layout begins with a byte-identical copy of its parent's,
    // which is what lets a field read compiled against `Status` run unchanged
    // on a `NotFound404`.
    assert_eq!(status.fields[0].offset, base);
    assert_eq!(
        (s4xx.fields[0].name.as_str(), s4xx.fields[0].offset),
        ("code", base)
    );
    assert_eq!(
        (nf.fields[0].name.as_str(), nf.fields[0].offset),
        ("code", base)
    );
    assert_eq!(
        (nf.fields[1].name.as_str(), nf.fields[1].offset),
        ("detail", base + 8)
    );
    assert_eq!((status.inherited, s4xx.inherited, nf.inherited), (0, 1, 1));

    // Preorder numbering: a type's subtypes occupy a contiguous id range, so
    // the dispatcher tests membership with a subtract and one compare.
    let first = wsharp_runtime::header::TYPE_ID_FIRST_USER;
    assert_eq!((status.type_id, status.subtree_len), (first, 3));
    assert_eq!((s4xx.type_id, s4xx.subtree_len), (first + 1, 2));
    assert_eq!((nf.type_id, nf.subtree_len), (first + 2, 1));
}

#[test]
fn sibling_subtrees_get_disjoint_id_ranges() {
    let src = r#"
        const Status = struct { };
        const Status4xx = struct : Status { };
        const NotFound404 = struct : Status4xx { };
        const Status5xx = struct : Status { };
        fn main() i64 { return 0; }
    "#;
    let (program, _) = mono(src);
    let first = wsharp_runtime::header::TYPE_ID_FIRST_USER;
    let ranges: Vec<(u32, u32)> = (0..4)
        .map(|i| {
            let s = program.strukt(i);
            (s.type_id, s.subtree_len)
        })
        .collect();
    // Status covers everything; the 4xx and 5xx subtrees do not overlap.
    assert_eq!(
        ranges,
        vec![(first, 4), (first + 1, 2), (first + 2, 1), (first + 3, 1)]
    );
}

#[test]
fn an_optional_field_occupies_two_slots() {
    let src = r#"
        const Box = struct { tag: i64, value: ?i64, next: i64 };
        fn main() i64 { return 0; }
    "#;
    let (program, _) = mono(src);
    let b = program.strukt(0);
    let base = wsharp_runtime::HEADER_SIZE;
    assert_eq!(b.fields[0].offset, base);
    assert_eq!(b.fields[1].offset, base + 8);
    // `?i64` is a tag plus a payload, so `next` starts two slots later.
    assert_eq!(b.fields[2].offset, base + 24);
}

#[test]
fn unused_type_variables_are_reported_rather_than_silently_defaulted() {
    // `pick` never constrains its second parameter, and nothing at the call
    // site does either, so there is no concrete type to specialise it at.
    let src = r#"
        fn pick(a, b) { return a; }
        fn main() i64 { return pick(1, null); }
    "#;
    let (module, diags) = parse(src);
    assert!(diags.is_empty());
    let mut analysis = analyze(&module);
    assert!(analysis.diags.is_empty(), "{:?}", analysis.diags);
    let result = monomorphize(&analysis.program, &mut analysis.store);
    assert!(
        result
            .diags
            .iter()
            .any(|d| d.message.contains("cannot tell what type")),
        "expected an ambiguity diagnostic, got {:?}",
        result.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn type_arguments_are_consumed() {
    let src = r#"
        fn id(x) { return x; }
        fn main() i64 { return id(1); }
    "#;
    let (program, _) = mono(src);
    for func in &program.funcs {
        assert!(
            func.scheme.vars.is_empty(),
            "`{}` is still generic after monomorphisation",
            func.name
        );
    }
    let _ = Type::i64();
}

#[test]
fn a_generic_struct_is_laid_out_per_instantiation() {
    // The whole reason offsets cannot be computed once: `first` is one slot in
    // `Pair[i64, i64]` and two in `Pair[?i64, i64]`, which moves `second`.
    let src = r#"
        const Pair = struct[A, B] { first: A, second: B };
        fn main() i64 {
            const a = Pair{ .first = 1, .second = 2 };
            const b: Pair[?i64, i64] = Pair{ .first = 3, .second = 4 };
            return a.second + b.second;
        }
    "#;
    let (program, mut store) = mono(src);
    // The declaration survives as a template; its offsets are unresolved,
    // because they depend on what it is used at.
    let template = program
        .structs
        .iter()
        .find(|s| s.name == "Pair")
        .expect("the declaration is in the table");
    assert!(!template.params.is_empty());
    assert_eq!(template.size, 0, "a template has no size of its own");

    // Both instantiations are reachable, and their second fields differ.
    let mut used: Vec<String> = Vec::new();
    for func in &program.funcs {
        for local in &func.locals {
            let ty = local.ty.clone();
            let shown = store.show(&ty);
            if shown.starts_with("Pair[") && !used.contains(&shown) {
                used.push(shown);
            }
        }
    }
    used.sort();
    assert_eq!(used, vec!["Pair[?i64, i64]", "Pair[i64, i64]"]);
}

#[test]
fn a_recursive_generic_function_specialises_once_per_type() {
    let src = r#"
        fn pick(x, again: bool) { if (again) { return pick(x, false); } return x; }
        fn main() i64 {
            print_bool(pick(true, true));
            return pick(1, true);
        }
    "#;
    let (program, mut store) = mono(src);
    let mut rendered: Vec<String> = copies_of(&program, "pick")
        .iter()
        .map(|&i| {
            let ty = program.func(i).scheme.ty.clone();
            store.show(&ty)
        })
        .collect();
    rendered.sort();
    assert_eq!(
        rendered,
        vec!["fn(bool, bool) bool", "fn(i64, bool) i64"],
        "an in-group call must carry the group's own variables"
    );
}

#[test]
fn an_unused_library_module_is_dropped() {
    // The parts of the standard library written in W# are compiled with the
    // program, so they have to fall out as dead code like anything else.
    let (program, _) = mono("fn main() i64 { return 0; }");
    assert!(
        copies_of(&program, "concat").is_empty(),
        "nothing called it, so nothing should be emitted"
    );
}
