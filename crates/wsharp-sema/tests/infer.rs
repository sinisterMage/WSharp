//! Type inference tests.
//!
//! Most assertions are on the rendered signature of a function, which exercises
//! inference, generalisation and printing together.

use wsharp_sema::analyze;
use wsharp_syntax::parse;

fn analysis(src: &str) -> wsharp_sema::Analysis {
    let (module, diags) = parse(src);
    assert!(
        diags.is_empty(),
        "parse errors: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    analyze(&module)
}

/// Infer `src`, asserting it type-checks, and return `name`'s signature.
fn sig(src: &str, name: &str) -> String {
    let a = analysis(src);
    assert!(
        a.diags.is_empty(),
        "unexpected type errors: {:?}",
        a.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    a.signatures
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| panic!("no signature for `{name}` in {:?}", a.signatures))
}

fn errors(src: &str) -> Vec<String> {
    analysis(src).diags.into_iter().map(|d| d.message).collect()
}

/// Assert some diagnostic mentions `needle`.
fn assert_error(src: &str, needle: &str) {
    let errs = errors(src);
    assert!(
        errs.iter().any(|e| e.contains(needle)),
        "expected an error containing {needle:?}, got {errs:?}"
    );
}

// ---- inference ----------------------------------------------------------

#[test]
fn infers_parameter_and_return_types_with_no_annotations() {
    assert_eq!(
        sig("fn add(a, b) { return a + b; }", "add"),
        "fn(i64, i64) i64"
    );
}

#[test]
fn annotations_are_checked_when_present() {
    assert_eq!(
        sig("fn f(a: f64) f64 { return a * 2.0; }", "f"),
        "fn(f64) f64"
    );
    // A partially annotated signature: the rest is inferred from it.
    assert_eq!(
        sig("fn f(a: f64, b) { return a + b; }", "f"),
        "fn(f64, f64) f64"
    );
}

#[test]
fn generalises_an_unconstrained_function() {
    assert_eq!(sig("fn id(x) { return x; }", "id"), "fn(T) T");
    assert_eq!(sig("fn first(a, b) { return a; }", "first"), "fn(T, U) T");
}

#[test]
fn a_generic_function_can_be_used_at_two_types() {
    let src = r#"
        fn id(x) { return x; }
        fn main() i64 {
            print_int(id(1));
            print_bool(id(true));
            return 0;
        }
    "#;
    // `id` stays generic even though main uses it at i64 and bool.
    assert_eq!(sig(src, "id"), "fn(T) T");
}

#[test]
fn recursion_typechecks() {
    let src = "fn fib(n: i64) i64 { if (n < 2) { return n; } return fib(n - 1) + fib(n - 2); }";
    assert_eq!(sig(src, "fib"), "fn(i64) i64");
}

#[test]
fn recursion_infers_without_annotations() {
    let src = "fn fact(n) { if (n < 1) { return 1; } return n * fact(n - 1); }";
    assert_eq!(sig(src, "fact"), "fn(i64) i64");
}

#[test]
fn mutual_recursion_typechecks_as_one_group() {
    let src = r#"
        fn is_even(n) { if (n == 0) { return true; } return is_odd(n - 1); }
        fn is_odd(n) { if (n == 0) { return false; } return is_even(n - 1); }
    "#;
    assert_eq!(sig(src, "is_even"), "fn(i64) bool");
    assert_eq!(sig(src, "is_odd"), "fn(i64) bool");
}

#[test]
fn float_arithmetic_stays_float() {
    assert_eq!(
        sig("fn f(x: f64) { return x * x + 1.0; }", "f"),
        "fn(f64) f64"
    );
}

#[test]
fn an_unconstrained_numeric_operand_defaults_to_i64() {
    assert_eq!(
        sig("fn double(x) { return x + x; }", "double"),
        "fn(i64) i64"
    );
}

#[test]
fn comparisons_produce_bool() {
    assert_eq!(
        sig("fn lt(a, b) { return a < b; }", "lt"),
        "fn(i64, i64) bool"
    );
    assert_eq!(
        sig("fn both(a, b) { return a and b; }", "both"),
        "fn(bool, bool) bool"
    );
}

#[test]
fn locals_and_loops_infer() {
    let src = r#"
        fn sum_to(n: i64) i64 {
            var total = 0;
            var i: i64 = 0;
            while (i < n) : (i += 1) { total = total + i; }
            return total;
        }
    "#;
    assert_eq!(sig(src, "sum_to"), "fn(i64) i64");
}

#[test]
fn if_as_an_expression_unifies_both_branches() {
    assert_eq!(
        sig("fn f(c: bool) { return if (c) 1 else 2; }", "f"),
        "fn(bool) i64"
    );
}

// ---- structs ------------------------------------------------------------

#[test]
fn struct_literals_and_field_access() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };
        fn dist2(p: Point) i64 { return p.x * p.x + p.y * p.y; }
        fn make() Point { return Point{ .x = 3, .y = 4 }; }
    "#;
    assert_eq!(sig(src, "dist2"), "fn(Point) i64");
    assert_eq!(sig(src, "make"), "fn() Point");
}

#[test]
fn a_struct_can_refer_to_itself() {
    let src = r#"
        const Node = struct { value: i64, next: Node };
        fn head(n: Node) i64 { return n.value; }
    "#;
    assert_eq!(sig(src, "head"), "fn(Node) i64");
}

#[test]
fn field_of_an_unknown_type_asks_for_an_annotation() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };
        fn getx(p) { return p.x; }
    "#;
    assert_error(src, "cannot tell which type the field `x` belongs to");
}

#[test]
fn unknown_field_is_reported() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };
        fn f(p: Point) { return p.z; }
    "#;
    assert_error(src, "`Point` has no field `z`");
}

#[test]
fn missing_field_in_a_literal_is_reported() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };
        fn f() Point { return Point{ .x = 1 }; }
    "#;
    assert_error(src, "missing field `y`");
}

// ---- optionals and error unions -----------------------------------------

#[test]
fn a_plain_value_coerces_into_an_optional_return() {
    let src = r#"
        fn lookup(k: i64) ?i64 {
            if (k == 0) { return null; }
            return k * 2;
        }
    "#;
    assert_eq!(sig(src, "lookup"), "fn(i64) ?i64");
}

#[test]
fn orelse_and_unwrap_produce_the_payload() {
    let src = r#"
        fn lookup(k: i64) ?i64 { return k; }
        fn f() i64 { return lookup(1) orelse 0; }
        fn g() i64 { return lookup(1).?; }
    "#;
    assert_eq!(sig(src, "f"), "fn() i64");
    assert_eq!(sig(src, "g"), "fn() i64");
}

#[test]
fn a_plain_value_coerces_into_an_error_union_return() {
    let src = r#"
        fn risky(n) !i64 {
            if (n < 0) { return error.Negative; }
            return n;
        }
    "#;
    assert_eq!(sig(src, "risky"), "fn(i64) !i64");
}

#[test]
fn catch_produces_the_payload_and_binds_the_error() {
    let src = r#"
        fn risky(n: i64) !i64 { return n; }
        fn f() i64 { return risky(1) catch 0; }
        fn g() i64 { return risky(1) catch |e| 0; }
    "#;
    assert_eq!(sig(src, "f"), "fn() i64");
    assert_eq!(sig(src, "g"), "fn() i64");
}

#[test]
fn try_requires_a_fallible_enclosing_function() {
    let src = r#"
        fn risky(n: i64) !i64 { return n; }
        fn ok(n: i64) !i64 { return try risky(n); }
    "#;
    assert_eq!(sig(src, "ok"), "fn(i64) !i64");

    let bad = r#"
        fn risky(n: i64) !i64 { return n; }
        fn nope(n: i64) i64 { return try risky(n); }
    "#;
    assert_error(bad, "`try` in a function that cannot fail");
}

#[test]
fn optional_capture_binds_the_payload() {
    let src = r#"
        fn lookup(k: i64) ?i64 { return k; }
        fn f() i64 {
            if (lookup(1)) |v| { return v; }
            return 0;
        }
    "#;
    assert_eq!(sig(src, "f"), "fn() i64");
}

// ---- closures -----------------------------------------------------------

#[test]
fn a_closure_infers_and_is_callable() {
    let src = r#"
        fn main() i64 {
            const add = fn (a, b) { return a + b; };
            return add(1, 2);
        }
    "#;
    assert_eq!(sig(src, "main"), "fn() i64");
}

#[test]
fn a_closure_captures_an_enclosing_local() {
    let src = r#"
        fn main() i64 {
            const base = 10;
            const bump = fn (x) { return x + base; };
            return bump(5);
        }
    "#;
    assert_eq!(sig(src, "main"), "fn() i64");
    // The closure body records `base` as a capture.
    let a = analysis(src);
    let closure = a
        .program
        .funcs
        .iter()
        .find(|f| f.is_closure)
        .expect("a closure was created");
    assert_eq!(closure.captures.len(), 1, "expected exactly one capture");
}

#[test]
fn assigning_to_a_captured_binding_is_rejected() {
    let src = r#"
        fn main() i64 {
            var count = 0;
            const bump = fn () { count = count + 1; };
            bump();
            return count;
        }
    "#;
    assert_error(src, "which this closure captured");
}

#[test]
fn a_named_function_can_be_passed_as_a_value() {
    let src = r#"
        fn twice(f: fn(i64) i64, x: i64) i64 { return f(f(x)); }
        fn inc(n: i64) i64 { return n + 1; }
        fn main() i64 { return twice(inc, 1); }
    "#;
    assert_eq!(sig(src, "twice"), "fn(fn(i64) i64, i64) i64");
    assert_eq!(sig(src, "main"), "fn() i64");
}

// ---- errors -------------------------------------------------------------

#[test]
fn type_mismatch_is_reported() {
    assert_error("fn f() i64 { return true; }", "type mismatch");
    assert_error("fn f(a: i64) { return a + true; }", "type mismatch");
}

#[test]
fn arithmetic_on_a_non_number_is_reported() {
    assert_error("fn f(a: bool, b: bool) { return a * b; }", "needs a number");
}

#[test]
fn remainder_needs_integers() {
    assert_error("fn f() f64 { return 1.5 % 0.5; }", "`%` needs an integer");
    assert_error(
        "fn f(a: f64) f64 { var x = a; x %= 2.0; return x; }",
        "`%` needs an integer",
    );
    // Left unconstrained it defaults to i64, like every other operator.
    assert_eq!(sig("fn f(a, b) { return a % b; }", "f"), "fn(i64, i64) i64");
}

#[test]
fn an_infinite_type_is_named_as_such() {
    // `f` returned from itself: its return type would have to be `fn(_) R`
    // with `R` that same type. The occurs check catches it in `coerce`.
    let errs = errors("fn f(x) { return f; }");
    assert!(
        errs.iter().any(|e| e.contains("would be infinite")),
        "{errs:?}"
    );
    // And through `expect`, where an operand must match its partner.
    let errs = errors("fn f(x) { return x + f; }");
    assert!(
        errs.iter().any(|e| e.contains("would be infinite")),
        "{errs:?}"
    );
    assert!(
        !errs.iter().any(|e| e.contains("type mismatch")),
        "{errs:?}"
    );
}

#[test]
fn unsolved_type_variables_render_as_underscores() {
    // The message shows the still-unknown parts as `_`, never as `?0`, which
    // would read as an optional.
    let errs = errors("fn f(g) { return g(g); }");
    let infinite = errs
        .iter()
        .find(|e| e.contains("would be infinite"))
        .unwrap_or_else(|| panic!("{errs:?}"));
    assert!(!infinite.contains('?'), "{infinite}");
}

#[test]
fn redeclaring_a_builtin_is_its_own_error() {
    let errs = errors("fn print(s: str) void { }");
    assert_eq!(
        errs,
        vec!["`print` is a builtin and cannot be redeclared".to_string()]
    );
    let errs = errors("const assert = 1;");
    assert!(errs.iter().any(|e| e.contains("is a builtin")), "{errs:?}");
}

#[test]
fn a_struct_literal_of_a_non_struct_says_so() {
    assert_error(
        "const Point = 1; fn f() i64 { return Point{}.x; }",
        "`Point` is not a struct",
    );
    assert_error("fn f(p: i64) i64 { return p{}.x; }", "`p` is not a struct");
    assert_error("fn f() i64 { return Nope{}.x; }", "unknown struct `Nope`");
}

#[test]
fn duplicates_point_back_at_the_first_declaration() {
    let a = analysis("fn f() void {} const f = 1;");
    let dup = a
        .diags
        .iter()
        .find(|d| d.message.contains("declared more than once"))
        .unwrap_or_else(|| panic!("{:?}", a.diags));
    assert_eq!(dup.secondary.len(), 1);
    assert_eq!(dup.secondary[0].message, "first declared here");
    assert_eq!(dup.secondary[0].span, wsharp_syntax::Span::new(3, 4));

    let a = analysis("const P = struct { x: i64, x: i64 };");
    let dup = a
        .diags
        .iter()
        .find(|d| d.message.contains("declared more than once"))
        .unwrap_or_else(|| panic!("{:?}", a.diags));
    assert_eq!(dup.secondary[0].message, "first declared here");
}

#[test]
fn wrong_argument_count_is_reported() {
    let src = "fn f(a, b) { return a + b; } fn main() i64 { return f(1); }";
    assert_error(src, "takes 2 arguments but 1 was given");
}

#[test]
fn unknown_names_and_types_are_reported() {
    assert_error(
        "fn f() i64 { return nope; }",
        "cannot find `nope` in this scope",
    );
    assert_error("fn f(x: Nope) i64 { return 0; }", "unknown type `Nope`");
}

#[test]
fn a_missing_return_is_reported() {
    assert_error(
        "fn f() i64 { const x = 1; }",
        "not every path returns a value",
    );
    // An `if` without an `else` cannot guarantee a return.
    assert_error(
        "fn f(c: bool) i64 { if (c) { return 1; } }",
        "not every path returns",
    );
    // With both branches returning, it is fine.
    assert!(errors("fn f(c: bool) i64 { if (c) { return 1; } else { return 2; } }").is_empty());
}

#[test]
fn void_functions_need_no_return() {
    assert!(errors("fn f() void { const x = 1; }").is_empty());
    assert_eq!(sig("fn f() void { print_int(1); }", "f"), "fn() void");
}

#[test]
fn assigning_to_a_const_is_rejected() {
    assert_error("fn f() void { const x = 1; x = 2; }", "which is `const`");
    assert!(errors("fn f() void { var x = 1; x = 2; }").is_empty());
}

#[test]
fn break_outside_a_loop_is_rejected() {
    assert_error("fn f() void { break; }", "`break` outside of a loop");
    assert!(errors("fn f() void { while (true) { break; } }").is_empty());
}

#[test]
fn duplicate_declarations_are_reported() {
    assert_error("fn f() void {} fn f() void {}", "declared more than once");
}

#[test]
fn functions_may_share_a_name_as_an_overload_set() {
    let src = r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn size(x: Base) i64 { return 1; }
        fn size(x: Sub) i64 { return 2; }
        fn main() i64 { return size(Sub); }
    "#;
    let a = analysis(src);
    assert!(
        a.diags.is_empty(),
        "unexpected errors: {:?}",
        a.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    let mut sizes: Vec<&str> = a
        .signatures
        .iter()
        .filter(|(n, _)| n == "size")
        .map(|(_, t)| t.as_str())
        .collect();
    sizes.sort();
    assert_eq!(sizes, vec!["fn(Base) i64", "fn(Sub) i64"]);
}

#[test]
fn a_subtype_widens_implicitly() {
    // Passing a subtype where a supertype is wanted needs no coercion: both
    // are one pointer, and the layouts agree on every inherited field.
    let src = r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn take(x: Base) i64 { return 1; }
        fn main() i64 { return take(Sub); }
    "#;
    assert!(errors(src).is_empty());

    // The other direction is still a mismatch.
    assert_error(
        r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn take(x: Sub) i64 { return 1; }
        fn give(b: Base) i64 { return take(b); }
        fn main() i64 { return give(Sub); }
        "#,
        "type mismatch",
    );
}

#[test]
fn overload_sets_have_rules_of_their_own() {
    let common = "const Base = struct { };\nconst Sub = struct : Base { };\n";

    // Every parameter must be annotated: dispatch chooses by parameter type.
    assert_error(
        &format!(
            "{common} fn f(x: Base) i64 {{ return 1; }} fn f(x) i64 {{ return 2; }} fn main() i64 {{ return f(Sub); }}"
        ),
        "is an overload set",
    );

    // Two overloads that accept the same arguments are a duplicate.
    assert_error(
        &format!(
            "{common} fn f(x: Base) i64 {{ return 1; }} fn f(x: Base) i64 {{ return 2; }} fn main() i64 {{ return f(Sub); }}"
        ),
        "declared more than once",
    );

    // An overload set is still not a value: there is no single code pointer.
    // `const g = f;` alone no longer says so, because it now *binds* an alias
    // for the set, so the mistake has to be made by using one -- here by
    // passing it where a function value is wanted.
    let take = "fn take(h: fn(Base) i64) i64 { return h(Base); }";
    assert_error(
        &format!(
            "{common} fn f(x: Base) i64 {{ return 1; }} fn f(x: Sub) i64 {{ return 2; }} {take} fn main() i64 {{ return take(f); }}"
        ),
        "names 2 functions",
    );
    // And through an alias, which says which kind of name it is.
    assert_error(
        &format!(
            "{common} fn f(x: Base) i64 {{ return 1; }} fn f(x: Sub) i64 {{ return 2; }} {take} fn main() i64 {{ const g = f; return take(g); }}"
        ),
        "`g` is an alias for an overload set",
    );

    // `main` is the entry point, not a dispatch target.
    assert_error(
        "fn main() i64 { return 0; } fn main() i64 { return 1; }",
        "`main` cannot be overloaded",
    );

    // Overloads must agree about what the call produces.
    assert_error(
        &format!(
            "{common} fn f(x: Base) i64 {{ return 1; }} fn f(x: Sub) bool {{ return true; }} fn g(x: Base) i64 {{ return f(x); }} fn main() i64 {{ return g(Sub); }}"
        ),
        "disagree about the return type",
    );
}

// ---- abstract types -----------------------------------------------------

/// The overload a static call in `caller`'s body resolved to, as the callee's
/// rendered scheme. Dispatch is decided at compile time, so this is the choice.
fn chosen_overload(src: &str, caller: &str) -> String {
    let mut a = analysis(src);
    assert!(
        a.diags.is_empty(),
        "unexpected type errors: {:?}",
        a.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    let body = a
        .program
        .funcs
        .iter()
        .find(|f| f.name == caller)
        .unwrap_or_else(|| panic!("no function `{caller}`"))
        .body
        .clone();
    let func = static_callee(&body).unwrap_or_else(|| panic!("no static call in `{caller}`"));
    let scheme = a.program.funcs[func as usize].scheme.clone();
    a.store.show_scheme(&scheme)
}

/// The callee of the first `return <call>;` in a block.
fn static_callee(block: &wsharp_sema::hir::Block) -> Option<wsharp_sema::hir::FuncId> {
    use wsharp_sema::hir::{Callee, ExprKind, Stmt};
    for stmt in &block.stmts {
        let Stmt::Return(Some(expr)) = stmt else {
            continue;
        };
        if let ExprKind::Call {
            callee: Callee::Static { func, .. },
            ..
        } = &expr.kind
        {
            return Some(*func);
        }
    }
    None
}

#[test]
fn an_abstract_type_makes_a_scalar_dispatchable() {
    let src = r#"
        fn f(x: i64) str { return "int"; }
        fn f(x: Number) str { return "number"; }
        fn pick_int() str { return f(1); }
        fn pick_float() str { return f(1.5); }
    "#;
    // `i64` is the more specific of the two, so it claims the integer.
    assert_eq!(chosen_overload(src, "pick_int"), "fn(i64) str");
    // The `Number` overload is the generic one -- an abstract annotation is a
    // constrained generic parameter -- so its scheme still shows a variable.
    assert_eq!(chosen_overload(src, "pick_float"), "fn(T) str");

    // But what the program wrote is what it is shown as.
    let a = analysis(src);
    let mut sigs: Vec<&str> = a
        .signatures
        .iter()
        .filter(|(n, _)| n == "f")
        .map(|(_, t)| t.as_str())
        .collect();
    sigs.sort();
    assert_eq!(sigs, vec!["fn(Number) str", "fn(i64) str"]);
}

#[test]
fn an_abstract_parameter_accepts_only_its_members() {
    // `str` is not one of the types `Number` lists, so nothing applies.
    assert_error(
        r#"
        fn f(x: i64) str { return "int"; }
        fn f(x: Number) str { return "number"; }
        fn main() i64 { print(f("nope")); return 0; }
        "#,
        "no overload of `f` accepts",
    );
    // And on its own, where the constraint is checked at the use site.
    assert_error(
        "fn f(x: Number) str { return \"n\"; } fn main() i64 { print(f(\"nope\")); return 0; }",
        "`Number` accepts `i64` and `f64`, but this is `str`",
    );
}

#[test]
fn an_abstract_parameter_is_specialised_per_argument_type() {
    let src = r#"
        fn bigger(a: Number, b: Number) { if (a > b) { return a; } return b; }
        fn main() i64 {
            print_int(bigger(3, 7));
            print_float(bigger(1.5, 0.5));
            return 0;
        }
    "#;
    // The annotation keeps `>` from defaulting the parameter to `i64`: the
    // members are all numeric, so the operator is answered without deciding.
    assert_eq!(sig(src, "bigger"), "fn(Number, Number) Number");

    let mut a = analysis(src);
    assert!(a.diags.is_empty(), "{:?}", a.diags);
    let m = wsharp_sema::monomorphize(&a.program, &mut a.store);
    assert!(m.diags.is_empty(), "{:?}", m.diags);
    let mut copies: Vec<String> = m
        .program
        .funcs
        .iter()
        .filter(|f| f.name == "bigger")
        .map(|f| {
            let ty = f.scheme.ty.clone();
            a.store.show(&ty)
        })
        .collect();
    copies.sort();
    assert_eq!(copies, vec!["fn(f64, f64) f64", "fn(i64, i64) i64"]);
}

#[test]
fn an_abstract_type_is_not_a_type_of_a_value() {
    let rule = "`Number` is an abstract type";
    // As a return type, a local's annotation, and a field's.
    assert_error("fn f(x: i64) Number { return x; }", rule);
    assert_error("fn f() i64 { const n: Number = 1; return n; }", rule);
    assert_error("const P = struct { n: Number };", rule);
    // As a supertype, as a value, and instantiated.
    assert_error("const P = struct : Number { };", rule);
    assert_error("fn f() i64 { const n = Number; return 0; }", rule);
    assert_error("fn f() i64 { return Number{}.x; }", rule);
    // Nested inside a parameter's type is not "a parameter's type" either:
    // there is nothing for the variable it becomes to be a variable of.
    assert_error("fn f(x: ?Number) i64 { return 0; }", rule);
}

#[test]
fn an_operator_no_member_supports_is_rejected_at_the_declaration() {
    // Every member has to work, because the caller is the one who picks: `%`
    // has no float form and `Number` includes `f64`.
    assert_error(
        "fn f(a: Number, b: Number) { return a % b; }",
        "`%` needs an integer, but `Number` includes `f64`",
    );
}

// ---- overload sets as values --------------------------------------------

#[test]
fn an_annotation_selects_one_member_of_an_overload_set() {
    let src = r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn size(x: Base) i64 { return 1; }
        fn size(x: Sub) i64 { return 2; }
        const wide: fn(Base) i64 = size;
        fn apply(f: fn(Sub) i64) i64 { return f(Sub); }
        fn main() i64 {
            const narrow: fn(Sub) i64 = size;
            return wide(Sub) + apply(narrow);
        }
    "#;
    assert!(errors(src).is_empty(), "{:?}", errors(src));

    // An annotation matching no member, and one matching none because the
    // return type is wrong rather than the parameter.
    let common = r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn size(x: Base) i64 { return 1; }
        fn size(x: Sub) i64 { return 2; }
    "#;
    assert_error(
        &format!("{common} const bad: fn(Base) str = size; fn main() i64 {{ return 0; }}"),
        "no overload of `size` has type `fn(Base) str`",
    );
    assert_error(
        &format!("{common} fn main() i64 {{ const bad: fn(i64) i64 = size; return 0; }}"),
        "no overload of `size` has type `fn(i64) i64`",
    );
}

#[test]
fn an_unannotated_const_aliases_the_whole_overload_set() {
    // `measure` dispatches exactly as `size` does, at top level and locally.
    let src = r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn size(x: Base) i64 { return 1; }
        fn size(x: Sub) i64 { return 2; }
        const measure = size;
        fn main() i64 {
            const m = size;
            return measure(Base) + m(Sub);
        }
    "#;
    assert!(errors(src).is_empty(), "{:?}", errors(src));

    // The alias is not a second overload set to be checked over again.
    let a = analysis(src);
    assert!(
        !a.diags.iter().any(|d| d.message.contains("measure")),
        "{:?}",
        a.diags
    );
}

#[test]
fn ambiguous_overloads_are_rejected_rather_than_guessed() {
    // Neither is more specific: the first wins on argument one, the second on
    // argument two, and `(Sub, Sub)` matches both.
    assert_error(
        r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn pick(a: Sub, b: Base) i64 { return 1; }
        fn pick(a: Base, b: Sub) i64 { return 2; }
        fn call(a: Base, b: Base) i64 { return pick(a, b); }
        fn main() i64 { return call(Sub, Sub); }
        "#,
        "is ambiguous",
    );

    // Adding the overload that resolves it makes the program legal.
    assert!(
        errors(
            r#"
        const Base = struct { };
        const Sub = struct : Base { };
        fn pick(a: Sub, b: Base) i64 { return 1; }
        fn pick(a: Base, b: Sub) i64 { return 2; }
        fn pick(a: Sub, b: Sub) i64 { return 3; }
        fn call(a: Base, b: Base) i64 { return pick(a, b); }
        fn main() i64 { return call(Sub, Sub); }
        "#
        )
        .is_empty()
    );
}

#[test]
fn incomparable_overloads_with_disjoint_types_are_not_ambiguous() {
    // `Left` and `Right` are siblings, so no value is both. There is nothing
    // to be ambiguous about, and requiring an ordering between them would be
    // noise.
    assert!(
        errors(
            r#"
        const Base = struct { };
        const Left = struct : Base { };
        const Right = struct : Base { };
        fn f(x: Left) i64 { return 1; }
        fn f(x: Right) i64 { return 2; }
        fn main() i64 { return f(Left) + f(Right); }
        "#
        )
        .is_empty()
    );
}

#[test]
fn a_status_type_is_a_value_as_well_as_a_type() {
    // Zero-field structs get a singleton, so the type name is usable directly.
    // The lattice lives in `std/http`, so it has to be imported first.
    assert_eq!(
        sig(
            "const http = @import(\"std/http\");
             fn kind(s: http.Status4xx) i64 { return 4; }
             fn main() i64 { return kind(http.NotFound404); }",
            "kind"
        ),
        "fn(Status4xx) i64"
    );

    // A struct with fields has no singleton and must be constructed.
    assert_error(
        "const P = struct { x: i64 }; fn f(p: P) i64 { return p.x; } fn main() i64 { return f(P); }",
        "cannot find `P` in this scope",
    );
}

#[test]
fn a_bad_supertype_is_rejected() {
    assert_error(
        "const A = struct : Missing { }; fn main() i64 { return 0; }",
        "unknown supertype `Missing`",
    );
    assert_error(
        "const A = struct : A { }; fn main() i64 { return 0; }",
        "`A` inherits from itself",
    );
    assert_error(
        "const A = struct : B { }; const B = struct : A { }; fn main() i64 { return 0; }",
        "cycle of supertypes",
    );
    assert_error(
        "const A = struct { x: i64 }; const B = struct : A { x: i64 }; fn main() i64 { return 0; }",
        "already inherited from `A`",
    );
}

#[test]
fn a_supertype_may_be_declared_after_its_subtype() {
    // Struct names are all collected before any parent is resolved, so
    // declaration order does not matter -- same as for field types.
    assert!(
        errors("const B = struct : A { }; const A = struct { }; fn main() i64 { return 0; }")
            .is_empty()
    );
}

#[test]
fn main_must_have_the_right_shape() {
    assert_error(
        "fn main(x: i64) i64 { return x; }",
        "`main` must take no arguments",
    );
    assert_error(
        "fn main() bool { return true; }",
        "`main` must take no arguments",
    );
    assert!(errors("fn main() i64 { return 0; }").is_empty());
    assert!(errors("fn main() void { print_int(1); }").is_empty());
}

#[test]
fn a_computed_top_level_const_is_rejected_with_a_hint() {
    assert_error("const X = 1 + 2;", "must be a literal or a `fn`");
    assert!(errors("const X: i64 = 7; fn main() i64 { return X; }").is_empty());
}

#[test]
fn a_top_level_const_fn_is_just_a_function() {
    let src = "const add = fn (a, b) { return a + b; }; fn main() i64 { return add(1, 2); }";
    assert_eq!(sig(src, "add"), "fn(i64, i64) i64");
}

// ---- the whole sample program -------------------------------------------

#[test]
fn the_sample_program_typechecks() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };

        fn fib(n: i64) i64 {
            if (n < 2) { return n; }
            return fib(n - 1) + fib(n - 2);
        }

        fn id(x) { return x; }

        fn lookup(k: i64) ?i64 {
            if (k == 0) { return null; }
            return k * 2;
        }

        fn risky(n) !i64 {
            if (n < 0) { return error.Negative; }
            return n;
        }

        fn main() i64 {
            var sum = 0;
            var i: i64 = 0;
            while (i < 10) : (i += 1) { sum = sum + fib(i); }

            const p = Point{ .x = 3, .y = 4 };
            const add = fn (a, b) { return a + b; };
            const v = lookup(5) orelse 0;
            const r = risky(sum) catch 0;

            print(id("done"));
            print_int(sum + p.x + add(1, 2) + v + r);
            return 0;
        }
    "#;
    let a = analysis(src);
    assert!(
        a.diags.is_empty(),
        "unexpected errors: {:?}",
        a.diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert_eq!(sig(src, "fib"), "fn(i64) i64");
    assert_eq!(sig(src, "id"), "fn(T) T");
    assert_eq!(sig(src, "lookup"), "fn(i64) ?i64");
    assert_eq!(sig(src, "risky"), "fn(i64) !i64");
    assert!(a.program.entry.is_some());
}

// ---- arrays -------------------------------------------------------------

#[test]
fn array_types_and_indexing() {
    assert_eq!(
        sig("fn head(a: []i64) i64 { return a[0]; }", "head"),
        "fn([]i64) i64"
    );
    // The element type comes from the literal's written type, not its
    // elements, so an empty one still has it.
    assert_eq!(
        sig("fn empty() []str { return []str{}; }", "empty"),
        "fn() []str"
    );
    assert_eq!(
        sig(
            "fn nest() [][]i64 { return [][]i64{ []i64{ 1 } }; }",
            "nest"
        ),
        "fn() [][]i64"
    );
    // Indexing is generic over the element type when the annotation says so.
    // Without one it is not inferred, for the same reason a field access is
    // not: the group is solved before any caller could say what `a` was.
    assert_eq!(
        sig("fn head[T](a: []T) T { return a[0]; }", "head"),
        "fn([]T) T"
    );
}

#[test]
fn indexing_something_that_is_not_an_array_is_reported() {
    assert_error(
        "fn f() i64 { const n = 1; return n[0]; }",
        "cannot be indexed",
    );
    // With nothing to say what is being indexed, the question is deferred and
    // then asked -- there is no sensible default the way `i64` is for a
    // numeric operand.
    assert_error(
        "fn f(a) { return a[0]; } fn main() i64 { return 0; }",
        "cannot tell what is being indexed",
    );
    assert_error("fn f(a: []i64) i64 { return a[\"x\"]; }", "type mismatch");
}

#[test]
fn a_for_loop_binds_the_element_type() {
    let src = r#"
        fn total(xs: []i64) i64 {
            var sum = 0;
            for (xs) |x, i| { sum = sum + x + i; }
            return sum;
        }
    "#;
    assert_eq!(sig(src, "total"), "fn([]i64) i64");
    assert_error(
        "fn f() void { for (5) |x| { print_int(x); } }",
        "cannot be indexed",
    );
}

// ---- explicit generics --------------------------------------------------

#[test]
fn type_parameters_may_be_named() {
    assert_eq!(sig("fn id[T](x: T) T { return x; }", "id"), "fn(T) T");
    assert_eq!(
        sig("fn first[A, B](a: A, b: B) A { return a; }", "first"),
        "fn(T, U) T"
    );
    assert_eq!(
        sig("fn head[T](a: []T) T { return a[0]; }", "head"),
        "fn([]T) T"
    );
}

#[test]
fn a_type_parameter_may_not_hide_a_type() {
    assert_error(
        "fn f[i64](x: i64) i64 { return x; }",
        "`i64` is a primitive type",
    );
    assert_error(
        "const P = struct { x: i64 }; fn f[P](x: P) P { return x; }",
        "`P` is already a type",
    );
    assert_error(
        "fn f[Number](x: Number) Number { return x; }",
        "abstract type",
    );
    assert_error("fn f[T, T](x: T) T { return x; }", "declared twice");
}

#[test]
fn a_recursive_generic_function_resolves() {
    // The call to `pick` inside itself records no type arguments while the
    // group is being inferred; they are filled in once it generalises.
    let src = r#"
        fn pick(x, again: bool) { if (again) { return pick(x, false); } return x; }
    "#;
    assert_eq!(sig(src, "pick"), "fn(T, bool) T");
}

// ---- generic structs ----------------------------------------------------

#[test]
fn generic_structs_carry_their_arguments() {
    let src = r#"
        const Box = struct[T] { value: T };
        fn unwrap[T](b: Box[T]) T { return b.value; }
        fn make() Box[i64] { return Box{ .value = 1 }; }
    "#;
    assert_eq!(sig(src, "unwrap"), "fn(Box[T]) T");
    assert_eq!(sig(src, "make"), "fn() Box[i64]");
}

#[test]
fn instantiations_are_related_only_to_themselves() {
    // Struct subtyping ignores arguments only for the non-generic case; a
    // `Box[i64]` is not a `Box[str]`.
    assert_error(
        "const Box = struct[T] { value: T };
         fn f(b: Box[str]) str { return b.value; }
         fn main() i64 { const x: Box[str] = Box{ .value = 1 }; return 0; }",
        "type mismatch",
    );
}

#[test]
fn a_generic_struct_stands_outside_the_lattice() {
    // Rejected by the parser, which is where the two halves are both in hand.
    let (_, diags) = parse("const Base = struct { }; const Sub = struct[T] : Base { v: T };");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("cannot have a supertype")),
        "{:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    assert_error(
        "const Box = struct[T] { value: T }; fn f(b: Box) i64 { return 0; }",
        "takes 1 type argument",
    );
    assert_error("const Empty = struct[T] { };", "at least one field");
}

// ---- modules ------------------------------------------------------------

#[test]
fn the_status_lattice_lives_behind_a_module() {
    // Unqualified, the status types are not in scope at all -- which is what
    // the module system was for.
    assert_error(
        "fn f(s: Status4xx) i64 { return 4; }",
        "unknown type `Status4xx`",
    );
    assert_eq!(
        sig(
            "const http = @import(\"std/http\");
             fn f(s: http.Status4xx) i64 { return 4; }",
            "f"
        ),
        "fn(Status4xx) i64"
    );
}

#[test]
fn an_import_is_a_module_rather_than_a_value() {
    assert_error(
        "const http = @import(\"std/http\");
         fn main() i64 { print_int(http); return 0; }",
        "is a module, not a value",
    );
    assert_error(
        "const nope = @import(\"std/nope\");",
        "cannot find module `std/nope`",
    );
    assert_error(
        "fn main() i64 { const http = @import(\"std/http\"); return 0; }",
        "only allowed at the top level",
    );
}

#[test]
fn a_local_shadows_an_imported_module() {
    // Adding an import must not break code that already used the name.
    assert_eq!(
        sig(
            "const http = @import(\"std/http\");
             fn f() i64 { const http = 5; return http; }",
            "f"
        ),
        "fn() i64"
    );
}

// ---- the standard library ------------------------------------------------

#[test]
fn strings_can_be_compared() {
    assert_eq!(
        sig("fn same(a: str, b: str) bool { return a == b; }", "same"),
        "fn(str, str) bool"
    );
    assert_error(
        "const P = struct { x: i64 }; fn f(a: P, b: P) bool { return a == b; }",
        "cannot be compared",
    );
}

#[test]
fn a_generic_builtin_is_instantiated_per_use() {
    // `std/array.len` reads the count out of the header whatever the elements
    // are, so one machine implementation serves both of these.
    let src = r#"
        const array = @import("std/array");
        fn f() i64 { return array.len([]i64{ 1 }) + array.len([]str{ "a" }); }
    "#;
    assert_eq!(sig(src, "f"), "fn() i64");
}
