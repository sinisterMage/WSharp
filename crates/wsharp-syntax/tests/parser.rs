//! Parser tests. Assertions compare against the s-expression dump, which keeps
//! them readable and makes precedence bugs obvious.

use wsharp_syntax::dump::dump_module;
use wsharp_syntax::parse;

/// Parse, asserting no diagnostics, and return the AST dump.
fn ast(src: &str) -> String {
    let (module, diags) = parse(src);
    assert!(
        diags.is_empty(),
        "unexpected diagnostics: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    dump_module(&module)
}

/// Parse the body of `main`, returning the dump of its statements only.
///
/// The dump closes nested forms by appending `)` to the last content line, so
/// the two parens belonging to `(fn main` and `(module` land on the final
/// statement and have to be peeled off again.
fn body(src: &str) -> String {
    let dump = ast(&format!("fn main() void {{ {src} }}"));
    let mut lines: Vec<String> = dump
        .lines()
        .skip(2) // "(module" and "  (fn main (params) (ret void)"
        .map(|l| l.strip_prefix("    ").unwrap_or(l).to_string())
        .collect();
    let last = lines.last_mut().expect("main had no statements");
    for _ in 0..2 {
        assert!(last.ends_with(')'), "unbalanced dump: {last}");
        last.pop();
    }
    lines.join("\n")
}

fn errors(src: &str) -> Vec<String> {
    let (_, diags) = parse(src);
    diags.into_iter().map(|d| d.message).collect()
}

#[test]
fn function_with_and_without_annotations() {
    let dump = ast("fn add(a: i64, b) i64 { return a + b; }");
    assert!(
        dump.contains("(fn add (params (a i64) (b)) (ret i64)"),
        "{dump}"
    );
    assert!(dump.contains("(return (+ a b))"), "{dump}");

    // No return type and no parameter types at all.
    let dump = ast("fn id(x) { return x; }");
    assert!(dump.contains("(fn id (params (x))"), "{dump}");
    assert!(!dump.contains("(ret "), "{dump}");
}

#[test]
fn pub_marks_a_declaration_visible() {
    let dump = ast("pub fn f() void { }\nfn g() void { }");
    assert!(dump.contains("(pub fn f "), "{dump}");
    // Absent by default, so every dump written before visibility existed still
    // reads the same.
    assert!(dump.contains("(fn g "), "{dump}");
    assert!(!dump.contains("(pub fn g "), "{dump}");

    let dump = ast("pub const P = struct { x: i64 };\npub const N = 1;");
    assert!(dump.contains("(pub struct P (x i64))"), "{dump}");
    assert!(dump.contains("(pub const N 1)"), "{dump}");
}

#[test]
fn pub_needs_a_declaration_after_it() {
    let errs = errors("pub var x = 1;");
    assert!(
        errs.iter().any(|e| e.contains("after `pub`")),
        "unexpected errors: {errs:?}"
    );
}

#[test]
fn arithmetic_precedence() {
    assert_eq!(body("const x = 1 + 2 * 3;"), "(const x (+ 1 (* 2 3)))");
    assert_eq!(body("const x = (1 + 2) * 3;"), "(const x (* (+ 1 2) 3))");
    assert_eq!(body("const x = 1 - 2 - 3;"), "(const x (- (- 1 2) 3))");
    assert_eq!(body("const x = -a * b;"), "(const x (* (- a) b))");
}

#[test]
fn logical_operators_are_looser_than_comparison() {
    assert_eq!(
        body("const x = a < b and c > d;"),
        "(const x (and (< a b) (> c d)))"
    );
    assert_eq!(
        body("const x = a and b or c;"),
        "(const x (or (and a b) c))"
    );
}

#[test]
fn orelse_binds_tighter_than_comparison() {
    // Zig's precedence: `(a orelse 0) == 5`, not `a orelse (0 == 5)`.
    assert_eq!(
        body("const x = a orelse 0 == 5;"),
        "(const x (== (orelse a 0) 5))"
    );
}

#[test]
fn catch_binds_tighter_than_comparison_and_takes_a_capture() {
    assert_eq!(
        body("const x = f() catch 0;"),
        "(const x (catch (call f) 0))"
    );
    assert_eq!(
        body("const x = f() catch |e| g(e);"),
        "(const x (catch |e| (call f) (call g e)))"
    );
}

#[test]
fn chained_comparison_is_rejected_with_a_hint() {
    let (_, diags) = parse("fn main() void { const x = a < b < c; }");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].message.contains("cannot be chained"));
    assert!(diags[0].help.as_deref().unwrap().contains("and"));
}

#[test]
fn postfix_chains() {
    assert_eq!(body("const x = f(1)(2);"), "(const x (call (call f 1) 2))");
    assert_eq!(body("const x = p.a.b;"), "(const x (. (. p a) b))");
    assert_eq!(body("const x = f().?;"), "(const x (unwrap (call f)))");
    assert_eq!(body("const x = try f();"), "(const x (try (call f)))");
}

#[test]
fn struct_declaration_and_literal() {
    let dump = ast("const Point = struct { x: i64, y: i64 };");
    assert!(dump.contains("(struct Point (x i64) (y i64))"), "{dump}");

    assert_eq!(
        body("const p = Point{ .x = 1, .y = 2 };"),
        "(const p (lit Point (x 1) (y 2)))"
    );
}

#[test]
fn a_struct_can_declare_a_supertype() {
    let dump = ast("const Status4xx = struct : Status { };");
    assert!(
        dump.contains("(struct Status4xx (parent Status))"),
        "{dump}"
    );

    // A subtype may still add fields of its own.
    let dump = ast("const Err = struct : Status4xx { code: i64 };");
    assert!(
        dump.contains("(struct Err (parent Status4xx) (code i64))"),
        "{dump}"
    );

    // The annotation slot before `=` is still rejected on the struct path, so
    // the two colons cannot be confused.
    let diags = errors("const Status4xx : Status = struct { };");
    assert!(
        diags[0].contains("cannot have a type annotation"),
        "{diags:?}"
    );
}

#[test]
fn if_statement_versus_if_expression() {
    // Braces after the condition mean the statement form.
    let out = body("if (c) { return; } else { return; }");
    assert!(out.starts_with("(if c"), "{out}");

    // No braces means the expression form, which requires an `else`.
    assert_eq!(body("const x = if (c) 1 else 2;"), "(const x (if c 1 2))");

    let diags = errors("fn main() void { const x = if (c) 1; }");
    assert!(diags[0].contains("expected `else`"), "{diags:?}");
}

#[test]
fn else_if_chain() {
    let out = body("if (a) { return; } else if (b) { return; } else { return; }");
    assert!(out.contains("(else"), "{out}");
    assert!(out.matches("(if ").count() == 2, "{out}");
}

#[test]
fn while_with_continue_expression_and_capture() {
    let out = body("while (i < 10) : (i += 1) { sum = sum + i; }");
    assert!(out.contains("(while (< i 10)"), "{out}");
    assert!(out.contains("(continue-expr"), "{out}");
    assert!(out.contains("(assign+ i 1)"), "{out}");

    let out = body("while (next()) |v| { print(v); }");
    assert!(out.contains("|v|"), "{out}");
}

#[test]
fn optional_capture_on_if() {
    let out = body("if (maybe) |v| { print(v); }");
    assert!(out.contains("(if maybe |v|"), "{out}");
}

#[test]
fn fn_literal_is_an_expression() {
    let out = body("const add = fn (a, b) { return a + b; };");
    assert!(
        out.contains("(const add (fn (params (a) (b)) ...))"),
        "{out}"
    );
}

#[test]
fn fn_literal_may_name_type_parameters() {
    // `fn` is followed by `[` or `(` and never a value, so the bracket can
    // only start a type parameter list -- the same argument that makes it
    // unambiguous after a declaration's name.
    let out = body("const first = fn [T](a: []T) T { return a[0]; };");
    assert!(
        out.contains("(const first (fn [T] (params (a []T)) (ret T) ...))"),
        "{out}"
    );
}

#[test]
fn error_literals_and_error_union_types() {
    let dump = ast("fn f(n) !i64 { return error.Negative; }");
    assert!(dump.contains("(ret !i64)"), "{dump}");
    assert!(dump.contains("(return error.Negative)"), "{dump}");
}

#[test]
fn optional_and_function_types() {
    let dump = ast("fn f(g: fn(i64) ?i64) ?i64 { return g(1); }");
    assert!(dump.contains("(g fn(i64) ?i64)"), "{dump}");
    assert!(dump.contains("(ret ?i64)"), "{dump}");
}

#[test]
fn compound_assignment_and_field_assignment() {
    assert_eq!(body("x += 1;"), "(assign+ x 1)");
    assert_eq!(body("p.x = 3;"), "(assign (. p x) 3)");
}

#[test]
fn assigning_to_a_non_place_is_an_error() {
    let diags = errors("fn main() void { f() = 1; }");
    assert!(
        diags.iter().any(|m| m.contains("cannot assign")),
        "{diags:?}"
    );
}

#[test]
fn top_level_var_is_rejected_with_a_hint() {
    let (_, diags) = parse("var x = 1;");
    assert!(
        diags[0]
            .message
            .contains("`var` is not allowed at the top level")
    );
    assert!(diags[0].help.as_deref().unwrap().contains("`const`"));
}

#[test]
fn recovers_and_keeps_parsing_later_items() {
    // The first function is broken; the second must still be parsed.
    let (module, diags) = parse("fn bad( { }\nfn good() i64 { return 1; }");
    assert!(!diags.is_empty());
    assert!(
        module.items.iter().any(|i| i.name().as_str() == "good"),
        "recovery lost the following item"
    );
}

#[test]
fn spans_cover_the_written_text() {
    let src = "fn main() i64 { return 1 + 2; }";
    let (module, _) = parse(src);
    let item = &module.items[0];
    assert_eq!(&src[item.span().range()], src);
}

#[test]
fn a_whole_program_parses() {
    let src = r#"
        const Point = struct { x: i64, y: i64 };

        fn fib(n: i64) i64 {
            if (n < 2) { return n; }
            return fib(n - 1) + fib(n - 2);
        }

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

            print_int(sum + p.x + add(1, 2) + v + r);
            return 0;
        }
    "#;
    let (module, diags) = parse(src);
    assert!(
        diags.is_empty(),
        "{:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert_eq!(module.items.len(), 5);
}

// ---- arrays, generics and modules ---------------------------------------

#[test]
fn array_types_literals_and_indexing() {
    assert_eq!(
        body("const a = []i64{ 1, 2 };"),
        "(const a (array i64 1 2))"
    );
    assert_eq!(body("const x = a[0];"), "(const x (index a 0))");
    // Indexing is postfix, so it chains with calls and fields.
    assert_eq!(
        body("const x = f()[0].y;"),
        "(const x (. (index (call f) 0) y))"
    );
    // `a[i] = v` is a place, and so is a compound assignment to one.
    assert_eq!(body("a[1] = 2;"), "(assign (index a 1) 2)");
    assert_eq!(body("a[1] += 2;"), "(assign+ (index a 1) 2)");
    // `[]T` nests, and a return type may be one: type syntax never starts
    // with `{`, so the body is still unambiguous.
    let dump = ast("fn f(a: [][]str) []i64 { return a2; }");
    assert!(dump.contains("(a [][]str)"), "{dump}");
    assert!(dump.contains("(ret []i64)"), "{dump}");
}

#[test]
fn for_loops_bind_a_value_and_an_index() {
    assert_eq!(
        body("for (xs) |x| { print(x); }"),
        "(for xs |x|\n  (block\n    (call print x)))"
    );
    assert_eq!(
        body("for (xs) |x, i| { print(i); }"),
        "(for xs |x i|\n  (block\n    (call print i)))"
    );
}

#[test]
fn type_parameters_are_declared_in_brackets() {
    let dump = ast("fn first[T, U](a: T, b: U) T { return a; }");
    assert!(dump.contains("(fn first [T U]"), "{dump}");

    let dump = ast("const Box = struct[T] { value: T };");
    assert!(dump.contains("(struct Box [T] (value T))"), "{dump}");

    // In a type, arguments follow the name; in an expression they do not, so
    // `Box[i64]` cannot be confused with indexing `Box`.
    let dump = ast("fn f(b: Box[i64]) void { }");
    assert!(dump.contains("(b Box[i64])"), "{dump}");
}

#[test]
fn imports_and_qualified_names() {
    let dump = ast("const http = @import(\"std/http\");");
    assert!(
        dump.contains("(const http (import \"std/http\"))"),
        "{dump}"
    );

    // A path in type position, and a struct literal reached through a module.
    let dump = ast("fn f(s: http.Status4xx) void { const p = util.Point{ .x = 1 }; }");
    assert!(dump.contains("(s http.Status4xx)"), "{dump}");
    assert!(dump.contains("(lit util.Point (x 1))"), "{dump}");
}

#[test]
fn a_type_parameter_list_cannot_be_empty() {
    assert!(
        errors("fn f[]() void { }")
            .iter()
            .any(|e| e.contains("type parameter list cannot be empty")),
        "{:?}",
        errors("fn f[]() void { }")
    );
}
