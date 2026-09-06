// HMAC-SHA-256 and HMAC-SHA-384, against RFC 4231's test cases. HKDF, which
// is HMAC twice over, has a case of its own next door.
//
// All seven, because each is there for a different reason: a key shorter than
// the block, a key exactly the block, keys longer than the block (which are
// hashed first), and a message longer than the block. Case 5 truncates its
// output, which HMAC itself does not do -- so it is checked by comparing a
// prefix rather than by asking the library for a short tag it should not offer.
// expect: b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
// expect: afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6
// expect: 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
// expect: 773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe
// expect: 82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b
// expect: 60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54
// expect: 9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2
// expect: truncated
const hash = @import("std/hash");
const bytes = @import("std/bytes");

fn main() i64 {
    const sha256 = hash.sha256_hash();
    const sha384 = hash.sha384_hash();

    // Case 1: a 20-byte key of 0x0b, "Hi There".
    const key1 = filled(20, 0x0b);
    const data1 = bytes.of("Hi There");
    print(bytes.to_hex(hash.hmac(sha256, key1, data1)));
    print(bytes.to_hex(hash.hmac(sha384, key1, data1)));

    // Case 2: a short key, and a message shorter still.
    print(bytes.to_hex(hash.hmac(sha256, bytes.of("Jefe"),
        bytes.of("what do ya want for nothing?"))));

    // Case 3: a 20-byte key of 0xaa and fifty 0xdd bytes.
    print(bytes.to_hex(hash.hmac(sha256, filled(20, 0xaa), filled(50, 0xdd))));

    // Case 4: a 25-byte key counting up, and fifty 0xcd bytes.
    const key4 = bytes.from_hex("0102030405060708090a0b0c0d0e0f10111213141516171819")
        catch return 1;
    print(bytes.to_hex(hash.hmac(sha256, key4, filled(50, 0xcd))));

    // Case 6: a 131-byte key, longer than SHA-256's block, so it is hashed
    // down first. Case 7 is the same key with a much longer message.
    const key6 = filled(131, 0xaa);
    print(bytes.to_hex(hash.hmac(sha256, key6,
        bytes.of("Test Using Larger Than Block-Size Key - Hash Key First"))));
    print(bytes.to_hex(hash.hmac(sha256, key6, bytes.of(
        "This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm."))));

    // Case 5 publishes only the first 16 bytes of its tag.
    const tag5 = hash.hmac(sha256, filled(20, 0x0c), bytes.of("Test With Truncation"));
    const want5 = bytes.from_hex("a3b61674") catch return 2;
    if (bytes.equal(bytes.slice(tag5, 0, 4), want5)) { print("truncated"); }
    return 0;
}

fn filled(n: i64, v: u8) []u8 {
    const out = bytes.new(n);
    bytes.fill(out, 0, n, v);
    return out;
}
