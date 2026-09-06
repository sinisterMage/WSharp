// The points X25519 must refuse, and why refusing them is this function's job
// rather than its caller's.
//
// Curve25519 has a subgroup of order eight sitting beside the prime-order one
// the protocol uses. A peer that sends a point from it forces the shared
// secret to zero whatever the local private key is -- so an implementation
// that handed the zero back would let anyone who could reach the wire choose
// the key both sides then derived from. RFC 8446 section 7.4.2 requires a TLS
// client to abort on it, which is why `x25519` answers with `error.WeakPoint`
// rather than with a value a caller could forget to test.
//
// The seven below are the whole published set: zero and one, the two points of
// order eight, and the three representatives of them that are above the prime
// and so only become small once they are reduced. That last group is the one
// an implementation misses -- it checks the input against a list of canonical
// encodings and never notices that `p + 1` is `1`. Checking the *output*
// cannot miss any of them, which is the argument for doing it there.
//
// Confirmed against a second implementation, which rejects exactly these seven
// and nothing else.
// expect: zero refused
// expect: one refused
// expect: order eight refused
// expect: the other order eight refused
// expect: p-1 refused
// expect: p refused
// expect: p+1 refused
// expect: a real point is not refused
const curve = @import("std/curve25519");
const bytes = @import("std/bytes");

// An ordinary private key, so that what is being tested is the point and not
// the scalar beside it.
const KEY = "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4";

fn main() i64 {
    refuse("0000000000000000000000000000000000000000000000000000000000000000", "zero refused");
    refuse("0100000000000000000000000000000000000000000000000000000000000000", "one refused");
    refuse("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800", "order eight refused");
    refuse("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157", "the other order eight refused");
    refuse("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f", "p-1 refused");
    refuse("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f", "p refused");
    refuse("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f", "p+1 refused");

    // And the other half of the claim: a point from the prime-order subgroup
    // goes through, so this is a check and not a refusal of everything.
    const good = curve.x25519_base(hex(KEY));
    const shared = curve.x25519(hex(KEY), good) catch return 1;
    if (!bytes.equal(shared, bytes.new(32))) { print("a real point is not refused"); }
    return 0;
}

fn refuse(u: str, note: str) void {
    const out = curve.x25519(hex(KEY), hex(u)) catch {
        print(note);
        bytes.new(0)
    };
    return;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
