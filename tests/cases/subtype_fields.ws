// expect: 404
// expect: 404
// expect: 1
// expect: 2
// A subtype's layout starts with a byte-identical copy of its supertype's, so
// a function written against the supertype reads an inherited field correctly
// off any instance of a subtype.
const Response = struct { code: i64 };
const ClientError = struct : Response { };
const Missing = struct : ClientError { detail: str };

fn code_of(r: Response) i64 { return r.code; }

fn kind(r: Response) i64 { return 1; }
fn kind(r: Missing) i64 { return 2; }

fn main() i64 {
    const m = Missing{ .code = 404, .detail = "gone" };
    print_int(m.code);
    print_int(code_of(m));

    const plain = Response{ .code = 200 };
    print_int(kind(plain));
    print_int(kind(m));
    return 0;
}
