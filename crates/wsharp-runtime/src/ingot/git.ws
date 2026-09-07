// Git's smart HTTP transport, version 2, spoken rather than shelled out to.
//
// Three exchanges, each an ordinary HTTP request with pkt-lines inside it:
//
//   1. `GET {url}/info/refs?service=git-upload-pack`, with a `Git-Protocol`
//      header asking for version 2. The answer says which version the server
//      is willing to speak and what it can do.
//   2. `POST {url}/git-upload-pack` with `command=ls-refs`, which answers with
//      an object id and a name per reference.
//   3. The same again with `command=fetch`, which answers with a packfile,
//      wrapped in side bands so progress can be interleaved with it.
//
// **The byte-level half is separated from the transport on purpose.** Every
// function that reads or writes protocol bytes takes and returns them, and
// only `discover` and `fetch` open a socket. That is what lets the whole
// conversation be tested against recorded bytes -- and the bytes this client
// *sends* be compared against what a real `git upload-pack` accepts -- on a
// machine with no network, which is the same split `std/tls` made for the same
// reason.
const array = @import("std/array");
const bytes = @import("std/bytes");
const fault = @import("ingot/fault");
const http = @import("std/http");
const list = @import("std/list");
const packfile = @import("ingot/packfile");
const pkt = @import("ingot/pktline");
const text = @import("std/str");
const tls = @import("std/tls");

/// What this client calls itself. Servers log it, and one that refuses an
/// unknown agent is a server that has decided who may talk to it.
pub const AGENT = "ingot/0.1.0";

/// One reference a remote advertises.
pub const Ref = struct { id: str, name: str };

// ---------------------------------------------------------------------------
// The bytes
// ---------------------------------------------------------------------------

/// Whether a server's advertisement says it speaks version 2.
///
/// The body begins with a `# service=git-upload-pack` line and a flush -- which
/// is HTTP's own preamble and not part of the protocol -- and then the version
/// and the capabilities. A server that answers version 1 is refused rather
/// than fallen back to: this client has one protocol, and pretending otherwise
/// would mean writing the other one.
pub fn speaks_v2(f: fault.Fault, body: str) bool {
    const b = bytes.of(body);
    var at = 0;
    var seen = false;
    var going = true;
    while (going) {
        const line = pkt.read(b, at);
        if (line.kind == pkt.BROKEN) {
            fault.fail(f, "the server's advertisement is not pkt-line");
            return false;
        }
        at = line.next;
        if (line.kind != pkt.DATA) {
            if (at >= array.len(b)) { going = false; }
            continue;
        }
        const value = pkt.trimmed(b, line);
        if (text.eq(value, "version 2")) { seen = true; }
        if (at >= array.len(b)) { going = false; }
    }
    if (!seen) {
        fault.fail(f, "the server does not speak git protocol version 2");
    }
    return seen;
}

/// The body of an `ls-refs` request.
///
/// `peel` asks for what an annotated tag points at, and the two prefixes keep
/// a repository with thousands of references from sending all of them.
pub fn ls_refs_request() str {
    const out = bytes.buf(128);
    pkt.put(out, "command=ls-refs");
    pkt.put(out, text.concat("agent=", AGENT));
    pkt.put_delim(out);
    pkt.put(out, "peel");
    pkt.put(out, "ref-prefix refs/heads/");
    pkt.put(out, "ref-prefix refs/tags/");
    pkt.put(out, "ref-prefix HEAD");
    pkt.put_flush(out);
    return bytes.to_str(bytes.taken(out));
}

/// The references an `ls-refs` answer holds: `<id> <name>` a line.
pub fn parse_refs(f: fault.Fault, body: str) ?list.List[Ref] {
    const b = bytes.of(body);
    var out: list.List[Ref] = list.new();
    var at = 0;
    while (at < array.len(b)) {
        const line = pkt.read(b, at);
        if (line.kind == pkt.BROKEN) {
            fault.fail(f, "the server's reference list is not pkt-line");
            return null;
        }
        at = line.next;
        if (line.kind != pkt.DATA) { continue; }
        const value = pkt.trimmed(b, line);
        if (refused(f, value)) { return null; }
        const space = text.find(value, " ");
        if (space != 40) { continue; }
        // A peeled tag arrives as a third field, which nothing here wants.
        var name = text.substr(value, 41, text.len(value));
        const tab = text.find(name, " ");
        if (tab >= 0) { name = text.substr(name, 0, tab); }
        list.push(out, Ref{ .id = text.substr(value, 0, 40), .name = name });
    }
    return out;
}

/// The body of a `fetch` request for one commit.
///
/// `deepen 1` makes it a shallow fetch: one commit and the tree under it, which
/// is all a package is. Without it a server sends the whole history, which for
/// a dependency is a great deal of somebody else's past.
pub fn fetch_request(want: str) str {
    const out = bytes.buf(256);
    pkt.put(out, "command=fetch");
    pkt.put(out, text.concat("agent=", AGENT));
    pkt.put_delim(out);
    pkt.put(out, "no-progress");
    pkt.put(out, text.concat("want ", want));
    pkt.put(out, "deepen 1");
    pkt.put(out, "done");
    pkt.put_flush(out);
    return bytes.to_str(bytes.taken(out));
}

/// The packfile out of a `fetch` answer.
///
/// The answer is a run of *sections*, each a name and then its lines, and only
/// one of them is wanted: `shallow-info` and `acknowledgments` are told what
/// they are and skipped. Inside `packfile` every line carries a band -- 1 the
/// pack, 2 progress, 3 an error the server wants read -- and band 3 is the one
/// that turns a silent empty answer into a message.
pub fn parse_packfile(f: fault.Fault, body: str) ?[]u8 {
    const b = bytes.of(body);
    const out = bytes.buf(4096);
    var at = 0;
    var in_pack = false;
    while (at < array.len(b)) {
        const line = pkt.read(b, at);
        if (line.kind == pkt.BROKEN) {
            fault.fail(f, "the server's answer is not pkt-line");
            return null;
        }
        at = line.next;
        if (line.kind == pkt.DELIM) { continue; }
        if (line.kind == pkt.FLUSH or line.kind == pkt.RESPONSE_END) { continue; }
        if (!in_pack) {
            const value = pkt.trimmed(b, line);
            // A refusal arrives as a plain line rather than in a side band,
            // because the side bands do not start until the packfile does.
            if (refused(f, value)) { return null; }
            if (text.eq(value, "packfile")) { in_pack = true; }
            continue;
        }
        const which = pkt.band(b, line);
        if (which == pkt.BAND_PACK) {
            bytes.put_bytes(out, b, pkt.band_from(line), line.to - pkt.band_from(line));
            continue;
        }
        if (which == pkt.BAND_ERROR) {
            fault.fail(f, text.concat("the server said: ",
                text.trim(bytes.slice_str(b, pkt.band_from(line), line.to))));
            return null;
        }
        // Band 2 is progress meant for a terminal, and there is none here.
    }
    if (!in_pack) {
        fault.fail(f, "the server's answer holds no packfile");
        return null;
    }
    return bytes.taken(out);
}

// ---------------------------------------------------------------------------
// The transport
// ---------------------------------------------------------------------------

/// Whether a line is the server refusing, and if so what it said.
///
/// `ERR <message>` is git's out-of-band no: it can appear anywhere a line can,
/// including before the side bands exist to carry one. A client that only
/// looked at band 3 would read this as an unknown section and report an empty
/// answer.
pub fn refused(f: fault.Fault, line: str) bool {
    if (!text.starts_with(line, "ERR ")) { return false; }
    fault.fail(f, text.concat("the server said: ",
        text.trim(text.substr(line, 4, text.len(line)))));
    return true;
}

/// A remote, and the trust store to reach it through.
pub const Remote = struct { url: str, cfg: tls.Config };

fn headers(content_type: str) list.List[http.Header] {
    var out: list.List[http.Header] = list.new();
    // The header that chooses version 2. Without it a server answers with the
    // old protocol, whose reference advertisement looks similar enough to be
    // confusing and is not the same thing.
    list.push(out, http.Header{ .name = "Git-Protocol", .value = "version=2" });
    if (text.len(content_type) > 0) {
        list.push(out, http.Header{ .name = "Content-Type", .value = content_type });
        list.push(out, http.Header{
            .name = "Accept",
            .value = "application/x-git-upload-pack-result",
        });
    }
    return out;
}

/// Ask a remote what it can do and what it holds.
pub fn discover(f: fault.Fault, r: Remote) ?list.List[Ref] {
    var none: list.List[http.Header] = list.new();
    const advert = http.request_headers(
        text.concat(r.url, "/info/refs?service=git-upload-pack"),
        "GET", "", r.cfg, headers("")) catch {
        fault.fail(f, text.concat("cannot reach ", r.url));
        return null;
    };
    if (advert.code != 200) {
        fault.fail(f, text.concat(text.concat(r.url, " answered "), text.from_int(advert.code)));
        return null;
    }
    if (!speaks_v2(f, advert.body)) { return null; }

    const listed = http.request_headers(
        text.concat(r.url, "/git-upload-pack"),
        "POST", ls_refs_request(), r.cfg,
        headers("application/x-git-upload-pack-request")) catch {
        fault.fail(f, text.concat("cannot list the references of ", r.url));
        return null;
    };
    if (listed.code != 200) {
        fault.fail(f, text.concat(text.concat(r.url, " answered "), text.from_int(listed.code)));
        return null;
    }
    return parse_refs(f, listed.body);
}

/// Fetch one commit and everything under it, as a packfile.
pub fn fetch(f: fault.Fault, r: Remote, want: str) ?[]u8 {
    const answer = http.request_headers(
        text.concat(r.url, "/git-upload-pack"),
        "POST", fetch_request(want), r.cfg,
        headers("application/x-git-upload-pack-request")) catch {
        fault.fail(f, text.concat("cannot fetch from ", r.url));
        return null;
    };
    if (answer.code != 200) {
        fault.fail(f, text.concat(text.concat(r.url, " answered "), text.from_int(answer.code)));
        return null;
    }
    return parse_packfile(f, answer.body);
}

/// Everything a commit's tree holds, as paths and contents.
///
/// The bridge between this module and `ingot/store`: a fetch answers with a
/// packfile, and a store wants a directory.
pub const File = struct { path: str, data: []u8 };

pub fn files(f: fault.Fault, pack: []u8, commit_id: str) ?list.List[File] {
    const objects = packfile.read(f, pack) orelse return null;
    const commit = packfile.find(objects, commit_id) orelse {
        fault.fail(f, text.concat("the packfile does not hold ", commit_id));
        return null;
    };
    const tree = packfile.commit_tree(f, commit.data) orelse return null;
    var out: list.List[File] = list.new();
    collect(f, objects, tree, "", out);
    if (!f.ok) { return null; }
    return out;
}

fn collect(f: fault.Fault, objects: list.List[packfile.Object], id: str, prefix: str,
           out: list.List[File]) void {
    const tree = packfile.find(objects, id) orelse {
        fault.fail(f, text.concat("the packfile does not hold the tree ", id));
        return;
    };
    const entries = packfile.parse_tree(f, tree.data) orelse return;
    for (list.to_array(entries)) |e| {
        const path = text.concat(prefix, e.name);
        if (e.is_dir) {
            collect(f, objects, e.id, text.concat(path, "/"), out);
            if (!f.ok) { return; }
            continue;
        }
        const blob = packfile.find(objects, e.id) orelse {
            fault.fail(f, text.concat("the packfile does not hold the blob ", e.id));
            return;
        };
        list.push(out, File{ .path = path, .data = blob.data });
    }
    return;
}
