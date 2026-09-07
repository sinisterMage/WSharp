// A git packfile, read: both delta kinds, the checksum, and the walk from a
// commit down to the files it names.
//
// The fixtures are packs `git repack` actually wrote, which is the rule the
// cryptographic cases follow for the same reason -- a packfile written by the
// code that reads it proves only that the code agrees with itself. What comes
// out is checked against the object ids git itself assigned, which is a
// stronger statement than "it did not crash": an id is a SHA-1 over the
// object's whole contents, so a delta applied one byte wrongly is a different
// id.
// expect: offset deltas: 10
// expect: - the tree is the one git named
// expect: - ingot.toml 42
// expect: - src/demo.ws 10085
// expect: - src/other.ws 10054
// expect: reference deltas: 10
// expect: - the tree is the one git named
// expect: - ingot.toml 42
// expect: - src/demo.ws 10085
// expect: - src/other.ws 10054
// expect: truncated: the packfile's checksum does not match its contents
// expect: damaged: the packfile's checksum does not match its contents
// expect: not a packfile: the packfile is too short to be one
// expect: empty: the packfile is too short to be one
const array = @import("std/array");
const bytes = @import("std/bytes");
const fault = @import("ingot/fault");
const fixture = @import("./modules/packfixture.ws");
const list = @import("std/list");
const packfile = @import("ingot/packfile");
const text = @import("std/str");

fn examine(name: str, hex: str) void {
    const f = fault.none();
    const pack = bytes.from_hex(hex) catch {
        print(text.concat(name, ": the fixture is not hex"));
        return;
    };
    const objects = packfile.read(f, pack) orelse {
        print(text.concat(name, text.concat(": ", f.message)));
        return;
    };
    print(text.concat(name, text.concat(": ", text.from_int(list.len(objects)))));

    // The commit git said was at the top must be here, and must name the tree
    // git said it does.
    const commit = packfile.find(objects, fixture.HEAD) orelse {
        print(text.concat(name, ": the commit is missing"));
        return;
    };
    const tree_id = packfile.commit_tree(f, commit.data) orelse {
        print(text.concat(name, text.concat(": ", f.message)));
        return;
    };
    print(if (text.eq(tree_id, fixture.TREE)) "- the tree is the one git named" else "- WRONG TREE");

    // Walk it, which is what installing a package does.
    var files: list.List[str] = list.new();
    walk(f, objects, tree_id, "", files);
    if (!f.ok) { print(text.concat("- ", f.message)); return; }
    for (list.to_array(files)) |line| { print(text.concat("- ", line)); }
    return;
}

/// Every file under a tree, as `path bytes`, deepest last.
fn walk(f: fault.Fault, objects: list.List[packfile.Object], id: str, prefix: str,
        out: list.List[str]) void {
    const tree = packfile.find(objects, id) orelse {
        fault.fail(f, text.concat("a tree is missing: ", id));
        return;
    };
    const entries = packfile.parse_tree(f, tree.data) orelse return;
    for (list.to_array(entries)) |e| {
        const path = text.concat(prefix, e.name);
        if (e.is_dir) {
            walk(f, objects, e.id, text.concat(path, "/"), out);
            if (!f.ok) { return; }
            continue;
        }
        const blob = packfile.find(objects, e.id) orelse {
            fault.fail(f, text.concat("a blob is missing: ", e.id));
            return;
        };
        list.push(out, text.concat(path, text.concat(" ", text.from_int(array.len(blob.data)))));
    }
    return;
}

/// A pack that should not be read, and the reason it is not.
fn refuses(name: str, hex: str) void {
    const f = fault.none();
    const pack = bytes.from_hex(hex) catch {
        print(text.concat(name, ": the fixture is not hex"));
        return;
    };
    const objects = packfile.read(f, pack) orelse {
        print(text.concat(name, text.concat(": ", f.message)));
        return;
    };
    print(text.concat(name, ": ACCEPTED"));
    return;
}

fn main() i64 {
    examine("offset deltas", fixture.offset_pack());
    examine("reference deltas", fixture.reference_pack());

    // The failures a download meets, each told apart from the others.
    const whole = fixture.offset_pack();
    refuses("truncated", text.substr(whole, 0, text.len(whole) / 2));
    // One byte of the body changed, which the trailer is a hash over.
    const damaged = text.concat(text.substr(whole, 0, 200),
        text.concat("ff", text.substr(whole, 202, text.len(whole))));
    refuses("damaged", damaged);
    refuses("not a packfile", "00000000000000000000000000000000");
    refuses("empty", "");
    return 0;
}
