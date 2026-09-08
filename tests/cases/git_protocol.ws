// Git's smart HTTP transport, version 2, against a conversation a real
// `git upload-pack` took part in.
//
// Recorded rather than written: the requests are the bytes this client
// generates, and the answers are what `git upload-pack --stateless-rpc` said
// when it was handed them -- the same program `git http-backend` puts behind
// the HTTP endpoint. That checks the half a recorded transcript alone cannot,
// which is whether a real server *accepts* what this client sends.
//
// `tls_rfc8448.ws` is the shape and the reason: a recorded request is an input,
// so comparing against it says nothing about what the client would have
// written. Here the recording was made *from* the client's own output, and the
// server's willingness to answer is the assertion.
// expect: the ls-refs request is the one the server accepted
// expect: the fetch request is the one the server accepted
// expect: version 2, bare
// expect: version 2, over HTTP
// expect: refused: the server does not speak git protocol version 2
// expect: HEAD 63e2817089c3d11d6f227abe4ac80847cc195167
// expect: refs/heads/main 63e2817089c3d11d6f227abe4ac80847cc195167
// expect: main is 63e2817089c3d11d6f227abe4ac80847cc195167
// expect: trunk is nowhere
// expect: packfile bytes: 2591
// expect: ingot.toml 42
// expect: src/demo.ws 10085
// expect: src/other.ws 10054
// expect: the server said: upload-pack: not our ref 0000000000000000000000000000000000000001
const array = @import("std/array");
const bytes = @import("std/bytes");
const fault = @import("ingot/fault");
const fixture = @import("./modules/gitfixture.ws");
const git = @import("ingot/git");
const list = @import("std/list");
const text = @import("std/str");

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
fn plain(s: str) str { return bytes.to_str(hex(s)); }

fn main() i64 {
    // What this client writes must still be what the server was given.
    print(if (text.eq(git.ls_refs_request(), plain(fixture.ls_refs_request())))
        "the ls-refs request is the one the server accepted"
        else "THE LS-REFS REQUEST HAS CHANGED");
    print(if (text.eq(git.fetch_request(fixture.HEAD), plain(fixture.fetch_request())))
        "the fetch request is the one the server accepted"
        else "THE FETCH REQUEST HAS CHANGED");

    // Version 2, in both the shapes it arrives in.
    const f = fault.none();
    print(if (git.speaks_v2(f, plain(fixture.advertisement()))) "version 2, bare" else "NOT V2");
    print(if (git.speaks_v2(f, plain(fixture.http_advertisement()))) "version 2, over HTTP" else "NOT V2");
    // And a server that only speaks version 1 is refused rather than guessed at.
    const g = fault.none();
    print(if (!git.speaks_v2(g, plain(fixture.refs()))) text.concat("refused: ", g.message) else "ACCEPTED V1");

    const refs = git.parse_refs(f, plain(fixture.refs())) orelse {
        print(f.message);
        return 1;
    };
    for (list.to_array(refs)) |r| {
        print(text.concat(r.name, text.concat(" ", r.id)));
    }

    // Picking a branch out of that list is what makes a *registry* fetchable:
    // a package names a revision that never moves, and an index is named by a
    // branch that must, so `fetch` gets its object id from here.
    print(text.concat("main is ", git.ref_id(refs, "refs/heads/main") orelse "nowhere"));
    print(text.concat("trunk is ", git.ref_id(refs, "refs/heads/trunk") orelse "nowhere"));

    // The packfile, out of the sections and side bands it arrives wrapped in.
    const pack = git.parse_packfile(f, plain(fixture.fetched())) orelse {
        print(f.message);
        return 1;
    };
    print(text.concat("packfile bytes: ", text.from_int(array.len(pack))));

    const files = git.files(f, pack, fixture.HEAD) orelse {
        print(f.message);
        return 1;
    };
    for (list.to_array(files)) |file| {
        print(text.concat(file.path, text.concat(" ", text.from_int(array.len(file.data)))));
    }

    // A refusal arrives as a plain `ERR` line rather than in a side band,
    // because the side bands do not exist until the packfile section does.
    const h = fault.none();
    const refused = git.parse_packfile(h, plain(fixture.refusal()));
    if (refused) |p| { print("A REFUSAL WAS ACCEPTED"); } else { print(h.message); }
    return 0;
}
