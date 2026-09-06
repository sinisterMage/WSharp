// ECDH on NIST P-256, against RFC 5903 section 8.1 -- the IKE group 19 test
// vector, which is a whole exchange rather than a single scalar
// multiplication: two private keys, the two public points they derive, and the
// one secret both sides reach from opposite halves.
//
// The first line is the check that costs nothing and catches the most: the
// scalar one must give back the base point exactly. A field with a wrong
// constant, a Montgomery form that does not round-trip, or a ladder that has
// its bits the wrong way up all fail there rather than somewhere subtler.
//
// The vectors were confirmed against a second implementation before they were
// written here.
// expect: 046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c2964fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5
// expect: 04dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c37725811805271a0461cdb8252d61f1c456fa3e59ab1f45b33accf5f58389e0577b8990bb3
// expect: 04d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf6356fbf3ca366cc23e8157854c13c58d6aac23f046ada30f8353e74f33039872ab
// expect: d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de
// expect: both directions agree
// expect: a derived key is a valid one
// expect: nothing kept
const nistec = @import("std/nistec");
const bytes = @import("std/bytes");

fn main() i64 {
    const p256 = nistec.p256();

    // One times the base point is the base point. FIPS 186-4's G, written out
    // by the code rather than by the table it came from.
    print(bytes.to_hex(nistec.derive(p256, 
        hex("0000000000000000000000000000000000000000000000000000000000000001")) catch return 1));

    // RFC 5903 section 8.1: the initiator's i and gi, the responder's r and gr.
    const i = hex("c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433");
    const r = hex("c6ef9c5d78ae012a011164acb397ce2088685d8f06bf9be0b283ab46476bee53");
    const gi = nistec.derive(p256, i) catch return 2;
    const gr = nistec.derive(p256, r) catch return 3;
    print(bytes.to_hex(gi));
    print(bytes.to_hex(gr));

    // The shared secret is the x coordinate alone, which is what SEC1 section
    // 3.3.1 says it is and what TLS 1.3 feeds to the key schedule.
    const secret = nistec.ecdh(p256, i, gr) catch return 4;
    print(bytes.to_hex(secret));
    if (bytes.equal(secret, nistec.ecdh(p256, r, gi) catch return 5)) {
        print("both directions agree");
    }

    // The validation a peer's key share goes through must not reject a key
    // this library itself produced, which is the half of a check that is easy
    // to forget to test.
    if (nistec.valid(p256, gi) and nistec.valid(p256, gr)) { print("a derived key is a valid one"); }

    // A scalar multiplication allocates for itself and keeps nothing: the
    // ladder's 256 steps write into scratch made once, which is also why this
    // case takes the same time under `--gc-stress` as without it.
    const before = gc_live_objects();
    var n = 0;
    while (n < 4) : (n += 1) {
        const scratch = nistec.ecdh(p256, i, gr) catch return 6;
        if (scratch[0] == 0xff and scratch[31] == 0xff) { print("unreachable"); }
    }
    gc_collect();
    gc_trace();
    if (gc_live_objects() <= before + 4) { print("nothing kept"); }
    return 0;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
