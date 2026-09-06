// SHA-256, SHA-384 and SHA-512, against FIPS 180-4's own examples.
//
// The vectors are the published ones and were written here before the code
// under them worked, which is the discipline item 10 asks for by name: a
// collector bug shows up as a failing test, and a hash that is subtly wrong
// shows up as nothing at all until something signs with it.
//
// The three lengths chosen are the three cases the padding has: a message
// shorter than a block, one that needs a second block for its length field,
// and the empty one.
// expect: ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
// expect: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
// expect: 248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1
// expect: cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0
// expect: cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7
// expect: 38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b
// expect: 09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039
// expect: ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f
// expect: cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e
// expect: 8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909
// expect: incremental matches
// expect: multiblock
const hash = @import("std/hash");
const bytes = @import("std/bytes");
const array = @import("std/array");

fn main() i64 {
    // "abc", the empty string, and the 448-bit message -- the three the
    // standard prints, because they are the three the padding treats
    // differently.
    const abc = bytes.of("abc");
    const empty = bytes.new(0);
    const long = bytes.of("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
    // The 896-bit one, which SHA-384 and SHA-512 print instead.
    const longer = bytes.of(
        "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu");

    print(bytes.to_hex(hash.sha256(abc)));
    print(bytes.to_hex(hash.sha256(empty)));
    print(bytes.to_hex(hash.sha256(long)));
    // The standard's fourth vector: a million 'a's. It stays in the suite at
    // full size because the whole digest allocates once rather than once per
    // block, so it costs the same under `--gc-stress` as without it -- which
    // is the property that check is really testing.
    print(bytes.to_hex(hash.sha256(repeated(97, 1000000))));

    print(bytes.to_hex(hash.sha384(abc)));
    print(bytes.to_hex(hash.sha384(empty)));
    print(bytes.to_hex(hash.sha384(longer)));

    print(bytes.to_hex(hash.sha512(abc)));
    print(bytes.to_hex(hash.sha512(empty)));
    print(bytes.to_hex(hash.sha512(longer)));

    // The incremental API is what HMAC and the record layer use, so it has to
    // agree with the one-shot form byte for byte -- including when a block
    // boundary falls in the middle of an update.
    const s = hash.sha256_init();
    hash.sha256_update(s, long, 0, 1);
    hash.sha256_update(s, long, 1, 40);
    hash.sha256_update(s, long, 41, array.len(long) - 41);
    if (bytes.equal(hash.sha256_final(s), hash.sha256(long))) {
        print("incremental matches");
    }

    // And that a message spanning several blocks is fed the same way whether
    // it arrives in one piece or in awkward ones.
    const big = repeated(65, 200);
    const t = hash.sha512_init();
    var at = 0;
    while (at < array.len(big)) : (at += 7) {
        var n = 7;
        if (at + n > array.len(big)) { n = array.len(big) - at; }
        hash.sha512_update(t, big, at, n);
    }
    if (bytes.equal(hash.sha512_final(t), hash.sha512(big))) { print("multiblock"); }
    return 0;
}

fn repeated(b: u8, n: i64) []u8 {
    const out = bytes.new(n);
    bytes.fill(out, 0, n, b);
    return out;
}
