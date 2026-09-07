// A package registry held in memory, so a solver test needs no store and no
// network. `provider` hands the solver two closures over it, which is exactly
// what the tool hands over with a store behind them -- the solver cannot tell
// the difference, which is the point of the `Provider` being a struct of `fn`
// values rather than anything cleverer.
const array = @import("std/array");
const list = @import("std/list");
const pubgrub = @import("ingot/pubgrub");
const semver = @import("ingot/semver");
const text = @import("std/str");

pub const Entry = struct { package: str, version: semver.Version, needs: list.List[pubgrub.Need] };
pub const Registry = struct { entries: list.List[Entry] };

pub fn registry() Registry {
    var entries: list.List[Entry] = list.new();
    return Registry{ .entries = entries };
}

/// `add(r, "foo", "1.0.0", "bar ^1.0.0, baz ^2.0.0")`, and `""` for no needs.
pub fn add(r: Registry, package: str, version: str, needs: str) void {
    var ns: list.List[pubgrub.Need] = list.new();
    const trimmed = text.trim(needs);
    if (text.len(trimmed) > 0) {
        for (text.split(trimmed, ",")) |piece| {
            const one = text.trim(piece);
            const at = text.find(one, " ");
            list.push(ns, pubgrub.Need{
                .package = text.substr(one, 0, at),
                .range = semver.requirement(text.substr(one, at + 1, text.len(one))) orelse semver.any(),
            });
        }
    }
    list.push(r.entries, Entry{
        .package = package,
        .version = semver.parse(version) orelse semver.zero(),
        .needs = ns,
    });
    return;
}

pub fn provider(r: Registry) pubgrub.Provider {
    return pubgrub.Provider{
        .versions = fn (p: str) list.List[semver.Version] {
            var out: list.List[semver.Version] = list.new();
            for (list.to_array(r.entries)) |e| {
                if (text.eq(e.package, p)) { list.push(out, e.version); }
            }
            return out;
        },
        .dependencies = fn (p: str, v: semver.Version) list.List[pubgrub.Need] {
            for (list.to_array(r.entries)) |e| {
                if (text.eq(e.package, p) and semver.eq(e.version, v)) { return e.needs; }
            }
            var none: list.List[pubgrub.Need] = list.new();
            return none;
        },
    };
}

/// The solution as one line, in the order the solver decided things.
pub fn solution(a: pubgrub.Answer) str {
    var out = "";
    const n = list.len(a.names);
    var i = 0;
    while (i < n) : (i += 1) {
        if (i > 0) { out = text.concat(out, " "); }
        out = text.concat(out, text.concat(list.get(a.names, i),
            text.concat(" ", semver.render(list.get(a.versions, i)))));
    }
    return out;
}
