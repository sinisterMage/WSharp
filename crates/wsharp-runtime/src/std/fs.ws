// The parts of a filesystem that build arrays, and the walks above them.
//
// `mkdir`, `rmdir`, `remove`, `rename`, `is_dir` and `size` are builtins;
// `raw_read_dir` is one too, and answers with a blob rather than a `[]str`,
// because a builtin may not allocate an array. What is here is the cutting up,
// and the two recursive walks that a store cannot do without -- both of which
// move references between objects and so have to be W#.
const array = @import("std/array");
const io = @import("std/io");
const os = @import("std/os");
const path = @import("std/path");
const text = @import("std/str");

/// What a directory holds, without `.` and `..`.
///
/// The order is the filesystem's, which is neither sorted nor the same on two
/// machines. Anything hashing a tree has to sort for itself, and the store
/// does.
pub fn read_dir(dir: str) ![]str {
    return os.unpack(try raw_read_dir(dir));
}

/// Create `dir` and every directory above it that is not there yet.
///
/// The existence check before each `mkdir` is what makes this idempotent, and
/// it is a check rather than a caught `AlreadyExists` because W# has no way to
/// re-raise a caught error. Two processes racing here can still collide, and
/// the loser is told `AlreadyExists` -- which is the honest answer, and the
/// reason the store publishes by `rename` rather than by building in place.
pub fn mkdir_all(dir: str) !void {
    const full = path.normalise(dir);
    const parts = text.split(full, "/");
    const n = array.len(parts);
    var so_far = "";
    var i = 0;
    if (path.is_absolute(full)) {
        // **A Windows path begins at its drive, not at `/`.** `C:/Users/x`
        // splits into `C:`, `Users`, `x`; seeding with `/` builds `/C:` and
        // fails on the first `mkdir`. That is what it did, and the message said
        // so with the seam visible in it -- `...\home/store/sha256: cannot be
        // created`, backslashes on one side and slashes on the other.
        //
        // The drive is `parts[0]`, so starting there means starting at 1.
        so_far = path.drive(full);
        if (text.len(so_far) > 0) { i = 1; } else { so_far = "/"; }
    }
    while (i < n) : (i += 1) {
        const part = parts[i];
        if (text.len(part) == 0) { continue; }
        so_far = path.join(so_far, part);
        if (!is_dir(so_far)) { try mkdir(so_far); }
    }
    return;
}

/// Remove `p`, and everything under it if it is a directory.
///
/// Depth first, because `rmdir` refuses a directory that still holds
/// something. A path that is not there at all is not an error: this is what
/// undoes a half-written install, and an install that got no further than
/// choosing a name has nothing to undo.
pub fn remove_tree(p: str) !void {
    if (!is_dir(p)) {
        if (io.exists(p)) { try remove(p); }
        return;
    }
    const names = try read_dir(p);
    const n = array.len(names);
    var i = 0;
    while (i < n) : (i += 1) {
        try remove_tree(path.join(p, names[i]));
    }
    try rmdir(p);
    return;
}
