// The content-addressed store, under `~/.wsharp`.
//
// ```
// store/sha256/<hex>/   a package's files, named by the hash of the tree
// tmp/<hex>/            in flight; anything here is rubbish from a fetch that
//                       did not finish, and `gc` removes it
// env/<hex>.lock        a lockfile some project resolved to, which is what
//                       `gc` reaches store entries through
// ```
//
// **An install is a `rename`.** A fetch builds the tree under `tmp/`, hashes
// it, and moves it into place in one step, so an interrupted install leaves
// rubbish rather than half a package -- and a package that is *there* is
// complete by construction. W# has no `defer`, so this is not a tidiness
// argument: it is the only structure that survives the process being killed
// between two syscalls.
//
// **The tree hash is defined here rather than borrowed**, because a key that
// two versions of ingot compute differently is a store that silently splits in
// half. The definition is:
//
// ```
// H(dir) = SHA-256 over each entry, sorted by name as bytes:
//            "f" name 0x00 <decimal size> 0x00 <contents>     for a file
//            "d" name 0x00 <hex H(sub)>   0x00                for a directory
// ```
//
// Names sorted by byte, because a filesystem's own order is not sorted and is
// not the same on two machines; the size written out, so that two files cannot
// be run together into one; and a subtree folded to its own hash, so that the
// whole thing is one pass and a deep tree costs no more memory than a shallow
// one. Permissions and timestamps are *not* in it, deliberately: a package is
// its text.
const array = @import("std/array");
const bytes = @import("std/bytes");
const crypto = @import("std/crypto");
const fault = @import("ingot/fault");
const fs = @import("std/fs");
const hash = @import("std/hash");
const io = @import("std/io");
const list = @import("std/list");
const os = @import("std/os");
const path = @import("std/path");
const text = @import("std/str");

/// Where the store is.
///
/// `WSHARP_HOME` first, so a test can point at a directory of its own and so a
/// machine with a shared cache can say where it is; the user's home otherwise.
pub fn home() !str {
    if (os.get("WSHARP_HOME")) |h| {
        if (text.len(h) > 0) { return h; }
    }
    return path.join(try os.home(), ".wsharp");
}

pub fn objects(h: str) str { return path.join(path.join(h, "store"), "sha256"); }
pub fn scratch(h: str) str { return path.join(h, "tmp"); }
pub fn environments(h: str) str { return path.join(h, "env"); }

/// Where the tree with this digest lives, whether or not it is there.
pub fn entry(h: str, digest: str) str { return path.join(objects(h), digest); }

/// Make the three directories the store is made of.
pub fn prepare(f: fault.Fault, h: str) void {
    make(f, objects(h));
    make(f, scratch(h));
    make(f, environments(h));
    make(f, fetches(h));
    return;
}

fn make(f: fault.Fault, dir: str) void {
    fs.mkdir_all(dir) catch fault.fail_at(f, dir, "cannot be created");
    return;
}

// ---------------------------------------------------------------------------
// The tree hash
// ---------------------------------------------------------------------------

/// The digest of a directory tree, in hex.
pub fn tree_hash(f: fault.Fault, dir: str) str {
    const s = hash.sha256_init();
    fold(f, s, dir);
    if (!f.ok) { return ""; }
    return bytes.to_hex(hash.sha256_final(s));
}

fn fold(f: fault.Fault, s: hash.Sha256, dir: str) void {
    const names = fs.read_dir(dir) catch {
        fault.fail_at(f, dir, "cannot be listed");
        return;
    };
    for (sorted(names)) |name| {
        const full = path.join(dir, name);
        if (fs.is_dir(full)) {
            feed(s, "d");
            feed(s, name);
            byte(s, 0);
            // A subtree folds to its own digest, so the walk is one pass and a
            // deep tree costs no more than a shallow one.
            feed(s, tree_hash(f, full));
            byte(s, 0);
            if (!f.ok) { return; }
            continue;
        }
        const body = io.read_file(full) catch {
            fault.fail_at(f, full, "cannot be read");
            return;
        };
        const data = bytes.of(body);
        const n = array.len(data);
        feed(s, "f");
        feed(s, name);
        byte(s, 0);
        // The length is written out so that two files cannot run together into
        // one: without it, `ab` + `` and `a` + `b` hash the same.
        feed(s, text.from_int(n));
        byte(s, 0);
        hash.sha256_update(s, data, 0, n);
    }
    return;
}

fn feed(s: hash.Sha256, t: str) void {
    const b = bytes.of(t);
    hash.sha256_update(s, b, 0, array.len(b));
    return;
}

fn byte(s: hash.Sha256, v: i64) void {
    const b = bytes.new(1);
    b[0] = u8(v);
    hash.sha256_update(s, b, 0, 1);
    return;
}

/// Names in byte order.
///
/// Defined here beside the hash rather than in `std/array`, because it is not a
/// convenience: it is half of the hash's definition, and a sort that drifted
/// would change every key in the store. An insertion sort, because a directory
/// holds tens of names.
pub fn sorted(names: []str) []str {
    var out = array.slice(names, 0, array.len(names));
    const n = array.len(out);
    var i = 1;
    while (i < n) : (i += 1) {
        const v = out[i];
        var j = i;
        while (j > 0 and before(v, out[j - 1])) : (j -= 1) {
            out[j] = out[j - 1];
        }
        out[j] = v;
    }
    return out;
}

/// Byte order, and a shorter string that is a prefix of a longer one first.
fn before(a: str, b: str) bool {
    const na = text.len(a);
    const nb = text.len(b);
    var n = na;
    if (nb < n) { n = nb; }
    var i = 0;
    while (i < n) : (i += 1) {
        const x = text.byte_at(a, i);
        const y = text.byte_at(b, i);
        if (x != y) { return x < y; }
    }
    return na < nb;
}

// ---------------------------------------------------------------------------
// Putting something in, and taking it out again
// ---------------------------------------------------------------------------

/// Copy a tree into the store and answer with its digest.
///
/// Already there and *correct* is success and does nothing: the digest is the
/// contents, so a healthy entry is the one that would have been written. An
/// entry that is there and is not what its name says is a different matter --
/// it is removed and replaced, because leaving it would mean `install` had
/// reported success over a tree nothing put there.
///
/// Testing the entry rather than testing that the directory exists is the
/// whole distinction `verify` is built on, and getting it wrong here is what
/// made `install` a no-op on a damaged store.
pub fn install(f: fault.Fault, h: str, from: str) str {
    const digest = tree_hash(f, from);
    if (!f.ok) { return ""; }
    const target = entry(h, digest);
    const state = check(h, digest);
    if (state == READY) { return digest; }

    prepare(f, h);
    if (!f.ok) { return ""; }
    const staging = path.join(scratch(h), temporary());
    copy_tree(f, from, staging);
    if (!f.ok) {
        // Leave nothing behind that a later `gc` would have to reason about.
        fs.remove_tree(staging) catch ignore();
        return "";
    }
    // A damaged entry goes before the good copy takes its place: `rename` will
    // not replace a directory that is there, and what is there is wrong.
    if (state == DAMAGED) {
        fs.remove_tree(target) catch {
            fault.fail_at(f, target, "is damaged and cannot be removed");
            fs.remove_tree(staging) catch ignore();
            return "";
        };
    }
    // Another process may have published the same digest while this one was
    // copying. Both are right -- the contents are what the name says -- so the
    // loser drops its copy and carries on.
    fs.rename(staging, target) catch settle(f, staging, target);
    if (!f.ok) { return ""; }
    return digest;
}

/// What to do when the publishing `rename` did not happen.
fn settle(f: fault.Fault, staging: str, target: str) void {
    fs.remove_tree(staging) catch ignore();
    if (!fs.is_dir(target)) {
        fault.fail_at(f, target, "cannot be published into the store");
    }
    return;
}

/// Write a list of files into the store, and answer with the digest.
///
/// What a fetch produces is a list of paths and contents rather than a
/// directory, so this is the other door into `install`: build the tree under
/// `tmp/`, then hand it over. The two share the atomic `rename` and the
/// hashing, which is the point -- a git package and a path package are the
/// same thing in the store and nothing downstream can tell them apart.
pub fn install_files(f: fault.Fault, h: str, files: list.List[File]) str {
    prepare(f, h);
    if (!f.ok) { return ""; }
    const staging = path.join(scratch(h), temporary());
    fs.mkdir_all(staging) catch {
        fault.fail_at(f, staging, "cannot be created");
        return "";
    };
    for (list.to_array(files)) |file| {
        const where = path.join(staging, file.path);
        fs.mkdir_all(path.dirname(where)) catch {
            fault.fail_at(f, path.dirname(where), "cannot be created");
            fs.remove_tree(staging) catch ignore();
            return "";
        };
        io.write_file(where, bytes.to_str(file.data)) catch {
            fault.fail_at(f, where, why_unwritable(path.dirname(where)));
            fs.remove_tree(staging) catch ignore();
            return "";
        };
    }
    const digest = install(f, h, staging);
    fs.remove_tree(staging) catch ignore();
    return digest;
}

/// One file of a fetched tree.
///
/// Declared here rather than taken from `ingot/git`, so that the store does not
/// depend on the transport: what it needs is a path and some bytes, and where
/// they came from is not its business.
pub const File = struct { path: str, data: []u8 };

// ---------------------------------------------------------------------------
// Remembering a fetch
// ---------------------------------------------------------------------------
//
// A git revision names one tree for ever, so fetching one twice is wasted
// work. `git/<hash of the source>` holds the digest that source resolved to,
// which turns the second `resolve` of a project into no network at all.

pub fn fetches(h: str) str { return path.join(h, "git"); }

pub fn remembered(h: str, source: str) ?str {
    const file = path.join(fetches(h), bytes.to_hex(hash.sha256(bytes.of(source))));
    if (!io.exists(file)) { return null; }
    const digest = text.trim(io.read_file(file) catch return null);
    if (text.len(digest) != 64) { return null; }
    return digest;
}

pub fn remember(f: fault.Fault, h: str, source: str, digest: str) void {
    make(f, fetches(h));
    if (!f.ok) { return; }
    const file = path.join(fetches(h), bytes.to_hex(hash.sha256(bytes.of(source))));
    io.write_file(file, digest) catch fault.fail_at(f, file, "cannot be written");
    return;
}

/// What `verify` answers about one entry.
pub const READY = 0;
pub const MISSING = 1;
pub const DAMAGED = 3;

/// Whether the entry for `digest` is there and is what it claims to be.
///
/// Rehashing is the whole point: "installed" and "installed and then edited"
/// are different answers, and a store that cannot tell them apart is a store
/// whose guarantee is a comment.
pub fn check(h: str, digest: str) i64 {
    const target = entry(h, digest);
    if (!fs.is_dir(target)) { return MISSING; }
    const f = fault.none();
    const found = tree_hash(f, target);
    if (!f.ok) { return DAMAGED; }
    if (text.eq(found, digest)) { return READY; }
    return DAMAGED;
}

/// Copy every file under `from` to `to`, making directories as needed.
pub fn copy_tree(f: fault.Fault, from: str, to: str) void {
    fs.mkdir_all(to) catch {
        fault.fail_at(f, to, "cannot be created");
        return;
    };
    // A postcondition, because on Windows this has been reporting success
    // without creating anything and the lie surfaces two calls later as a
    // write that cannot explain itself. Checked here rather than inside
    // `mkdir_all` so the message can say how far up the tree anything actually
    // exists, which is the fact that says *where* the creation stopped.
    // A postcondition, because a `mkdir_all` that answers yes and makes nothing
    // is the worst shape a failure can take: the caller carries on and fails
    // somewhere else entirely, describing a symptom rather than the fault. This
    // is where it happened.
    if (!fs.is_dir(to)) {
        fault.fail_at(f, to, "mkdir_all reported success and made nothing");
        return;
    }
    const names = fs.read_dir(from) catch {
        fault.fail_at(f, from, "cannot be listed");
        return;
    };
    for (names) |name| {
        const src = path.join(from, name);
        const dst = path.join(to, name);
        if (fs.is_dir(src)) {
            copy_tree(f, src, dst);
            if (!f.ok) { return; }
            continue;
        }
        const body = io.read_file(src) catch {
            fault.fail_at(f, src, "cannot be read");
            return;
        };
        io.write_file(dst, body) catch {
            fault.fail_at(f, dst, why_unwritable(path.dirname(dst)));
            return;
        };
    }
    return;
}

/// A name nothing else is using.
///
/// From the system's generator rather than from a counter, because two ingots
/// may be installing at the same moment and there is no lock between them.
fn temporary() str {
    return bytes.to_hex(crypto.random(8) catch bytes.new(8));
}

/// Nothing.
///
/// Somewhere to send a cleanup failure that is already a consequence of the
/// failure being reported: a `catch` must produce a value or leave, and there
/// is nothing here to say that the message already being carried does not.
fn ignore() void { return; }

/// Why a file could not be written, as far as this can tell from outside.
///
/// A W# error union carries a tag and no detail, so "cannot be written" is all
/// the `catch` itself knows -- and that is the least useful sentence a package
/// manager can end on. The one distinction worth drawing costs a `stat`: a
/// write into a directory that is not there is a *different* fault from a write
/// that was refused, and only the first points at the line above rather than at
/// the filesystem.
///
/// It earns its place: this is what the message said on Windows when
/// `mkdir_all` was starting absolute paths at `/` instead of at their drive,
/// and "cannot be written" was not enough to say so.
///
/// The failure is *asked about* rather than read off the error, because a W#
/// error carries a tag and this needs three separate facts -- whether the
/// directory is there, whether anything at all can be written into it, and
/// whether something is already sitting at the name. Between them they say
/// which of the three possible bugs it is, and each points at a different file.
fn why_unwritable(dir: str) str {
    if (!fs.is_dir(dir)) {
        return text.concat("cannot be written -- there is no directory ", dir);
    }
    if (!takes_a_file(dir)) {
        return text.concat("cannot be written -- nothing can be written into ", dir);
    }
    return "cannot be written";
}

/// Whether *some* file can be created in `dir`, whatever happened to the one
/// that failed.
///
/// The discriminator worth having: a directory that refuses everything is a
/// permissions or handle problem, and one that takes a probe but refused the
/// real name is a problem with the name or with what was being written.
fn takes_a_file(dir: str) bool {
    const probe = path.join(dir, "ingot-probe");
    io.write_file(probe, "probe") catch { return false; };
    fs.remove(probe) catch ignore();
    return true;
}

// ---------------------------------------------------------------------------
// Collecting
// ---------------------------------------------------------------------------

/// Remove every entry no registered environment reaches, and everything left
/// under `tmp/`.
///
/// Answers with how many entries went. The half of a store people forget until
/// a disk fills.
pub fn collect(f: fault.Fault, h: str, keep: list.List[str]) i64 {
    var removed = 0;
    const dir = objects(h);
    if (fs.is_dir(dir)) {
        const names = fs.read_dir(dir) catch {
            fault.fail_at(f, dir, "cannot be listed");
            return 0;
        };
        for (names) |name| {
            if (holds(keep, name)) { continue; }
            fs.remove_tree(path.join(dir, name)) catch {
                fault.fail_at(f, path.join(dir, name), "cannot be removed");
                return removed;
            };
            removed += 1;
        }
    }
    const staging = scratch(h);
    if (fs.is_dir(staging)) {
        const names = fs.read_dir(staging) catch []str{};
        for (names) |name| {
            fs.remove_tree(path.join(staging, name)) catch ignore();
        }
    }
    return removed;
}

fn holds(l: list.List[str], want: str) bool {
    for (list.to_array(l)) |v| {
        if (text.eq(v, want)) { return true; }
    }
    return false;
}

/// Register a lockfile as a thing the store must keep entries for.
///
/// The file is copied under `env/`, named by the hash of its own path, so a
/// project that moves registers again and the one it left behind ages out with
/// the next `gc` that cannot find it.
pub fn register(f: fault.Fault, h: str, lock_path: str) void {
    prepare(f, h);
    if (!f.ok) { return; }
    const body = io.read_file(lock_path) catch {
        fault.fail_at(f, lock_path, "cannot be read");
        return;
    };
    const key = bytes.to_hex(hash.sha256(bytes.of(lock_path)));
    io.write_file(path.join(environments(h), text.concat(key, ".lock")), body) catch {
        fault.fail_at(f, environments(h), "cannot be written to");
        return;
    };
    return;
}

/// The lockfiles the store is keeping entries for.
pub fn registered(h: str) []str {
    const dir = environments(h);
    if (!fs.is_dir(dir)) { return []str{}; }
    const names = fs.read_dir(dir) catch return []str{};
    var out = []str{};
    for (names) |name| {
        out = array.push(out, path.join(dir, name));
    }
    return out;
}
