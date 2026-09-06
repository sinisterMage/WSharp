// SHA-2, HMAC and HKDF.
//
// Written in W# rather than as builtins, and that is the point rather than a
// constraint: the rule that draws the boundary would *permit* a Rust
// implementation -- a hash is a pure byte-to-byte transform, which is exactly
// what a builtin may be -- so the reason is the other one item 10 gives. A
// language that cannot express SHA-256 has a hole in it, and the fastest way
// to find out where the hole is, is to try. Writing this is what asked the
// language for a top-level `const` array; nothing else was missing.
//
// Two things about how it is written are load-bearing rather than stylistic.
//
// **It allocates once per digest, not once per block.** The message schedule
// and the working words live in the state, made at `init` and reused by every
// block after it. The whole end-to-end suite runs a second time under
// `--gc-stress`, which collects at *every* allocation, so a temporary inside
// the block loop is the difference between a test and a timeout -- and the
// shape that avoids it is the shape a hash is written in anyway.
//
// **The constants are `const` arrays.** They live in the data section beside
// the string literals, immortal and holding no references, so naming one costs
// an address rather than an allocation.
const array = @import("std/array");
const bytes = @import("std/bytes");
const bits = @import("std/bits");

const K256 = []u32{
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

const H256 = []u32{
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
};

const K512 = []u64{
    0x428a2f98d728ae22, 0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019,
    0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe,
    0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1,
    0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210,
    0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725,
    0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001,
    0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910,
    0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53,
    0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60,
    0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9,
    0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6,
    0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
};

const H512 = []u64{
    0x6a09e667f3bcc908, 0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
    0x510e527fade682d1, 0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b, 0x5be0cd19137e2179,
};

const H384 = []u64{
    0xcbbb9d5dc1059ed8, 0x629a292a367cd507,
    0x9159015a3070dd17, 0x152fecd8f70e5939,
    0x67332667ffc00b31, 0x8eb44a8768581511,
    0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4,
};

// ---------------------------------------------------------------------------
// SHA-256
// ---------------------------------------------------------------------------

/// A SHA-256 in progress.
///
/// `w` is the message schedule, kept here rather than made per block for the
/// reason the header gives. `used` is how much of `block` is filled, and
/// `length` counts the whole message, because the padding ends with its bit
/// length and a hash of four gigabytes must still say so.
pub const Sha256 = struct {
    h: []u32,
    w: []u32,
    block: []u8,
    used: i64,
    length: u64,
};

pub fn sha256_init() Sha256 {
    const h: []u32 = array.new(8);
    var i = 0;
    while (i < 8) : (i += 1) { h[i] = H256[i]; }
    return Sha256{
        .h = h,
        .w = array.new(64),
        .block = bytes.new(64),
        .used = 0,
        .length = 0,
    };
}

/// One 64-byte block of `data` at `at`, compressed into `s.h`.
fn sha256_block(s: Sha256, data: []u8, at: i64) void {
    const w = s.w;
    var i = 0;
    while (i < 16) : (i += 1) { w[i] = bytes.be32(data, at + i * 4); }
    while (i < 64) : (i += 1) {
        const x = w[i - 15];
        const y = w[i - 2];
        const s0 = bits.rotr(x, 7) ^ bits.rotr(x, 18) ^ (x >> 3);
        const s1 = bits.rotr(y, 17) ^ bits.rotr(y, 19) ^ (y >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }

    var a = s.h[0];
    var b = s.h[1];
    var c = s.h[2];
    var d = s.h[3];
    var e = s.h[4];
    var f = s.h[5];
    var g = s.h[6];
    var hh = s.h[7];

    i = 0;
    while (i < 64) : (i += 1) {
        const big1 = bits.rotr(e, 6) ^ bits.rotr(e, 11) ^ bits.rotr(e, 25);
        const choice = (e & f) ^ (~e & g);
        const t1 = hh + big1 + choice + K256[i] + w[i];
        const big0 = bits.rotr(a, 2) ^ bits.rotr(a, 13) ^ bits.rotr(a, 22);
        const major = (a & b) ^ (a & c) ^ (b & c);
        const t2 = big0 + major;
        hh = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }

    s.h[0] += a;
    s.h[1] += b;
    s.h[2] += c;
    s.h[3] += d;
    s.h[4] += e;
    s.h[5] += f;
    s.h[6] += g;
    s.h[7] += hh;
    return;
}

pub fn sha256_update(s: Sha256, data: []u8, at: i64, n: i64) void {
    s.length += u64(n);
    var taken = 0;
    // Fill whatever the last call left part-filled, first.
    if (s.used > 0) {
        var room = 64 - s.used;
        if (room > n) { room = n; }
        bytes.copy(s.block, s.used, data, at, room);
        s.used += room;
        taken = room;
        if (s.used == 64) {
            sha256_block(s, s.block, 0);
            s.used = 0;
        }
    }
    // Then whole blocks straight out of the caller's buffer, copying nothing.
    while (n - taken >= 64) {
        sha256_block(s, data, at + taken);
        taken += 64;
    }
    if (n - taken > 0) {
        bytes.copy(s.block, 0, data, at + taken, n - taken);
        s.used = n - taken;
    }
    return;
}

/// The digest, having appended the padding the standard describes: a one bit,
/// then zeros, then the message's length in bits as a big-endian 64.
pub fn sha256_final(s: Sha256) []u8 {
    const bit_length = s.length * 8;
    const tail = bytes.new(72);
    tail[0] = 0x80;
    // Enough zeros that the length lands in the last eight bytes of a block.
    var pad = 64 - ((s.used + 9) % 64);
    if (pad == 64) { pad = 0; }
    bytes.put_be64(tail, 1 + pad, bit_length);
    sha256_update(s, tail, 0, 1 + pad + 8);

    const out = bytes.new(32);
    var i = 0;
    while (i < 8) : (i += 1) { bytes.put_be32(out, i * 4, s.h[i]); }
    return out;
}

pub fn sha256(data: []u8) []u8 {
    const s = sha256_init();
    sha256_update(s, data, 0, array.len(data));
    return sha256_final(s);
}

// ---------------------------------------------------------------------------
// SHA-512, and SHA-384 above it
// ---------------------------------------------------------------------------
//
// The same algorithm at twice the width: 128-byte blocks, 80 rounds, `u64`
// words and different rotation amounts. SHA-384 *is* SHA-512 with a different
// initial vector and the digest cut short, which is why one state type serves
// both and carries how much of itself to hand back.

pub const Sha512 = struct {
    h: []u64,
    w: []u64,
    block: []u8,
    used: i64,
    length: u64,
    out_len: i64,
};

fn sha512_start(iv: []u64, out_len: i64) Sha512 {
    const h: []u64 = array.new(8);
    var i = 0;
    while (i < 8) : (i += 1) { h[i] = iv[i]; }
    return Sha512{
        .h = h,
        .w = array.new(80),
        .block = bytes.new(128),
        .used = 0,
        .length = 0,
        .out_len = out_len,
    };
}

pub fn sha512_init() Sha512 { return sha512_start(H512, 64); }
pub fn sha384_init() Sha512 { return sha512_start(H384, 48); }

fn sha512_block(s: Sha512, data: []u8, at: i64) void {
    const w = s.w;
    var i = 0;
    while (i < 16) : (i += 1) { w[i] = bytes.be64(data, at + i * 8); }
    while (i < 80) : (i += 1) {
        const x = w[i - 15];
        const y = w[i - 2];
        const s0 = bits.rotr(x, 1) ^ bits.rotr(x, 8) ^ (x >> 7);
        const s1 = bits.rotr(y, 19) ^ bits.rotr(y, 61) ^ (y >> 6);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }

    var a = s.h[0];
    var b = s.h[1];
    var c = s.h[2];
    var d = s.h[3];
    var e = s.h[4];
    var f = s.h[5];
    var g = s.h[6];
    var hh = s.h[7];

    i = 0;
    while (i < 80) : (i += 1) {
        const big1 = bits.rotr(e, 14) ^ bits.rotr(e, 18) ^ bits.rotr(e, 41);
        const choice = (e & f) ^ (~e & g);
        const t1 = hh + big1 + choice + K512[i] + w[i];
        const big0 = bits.rotr(a, 28) ^ bits.rotr(a, 34) ^ bits.rotr(a, 39);
        const major = (a & b) ^ (a & c) ^ (b & c);
        const t2 = big0 + major;
        hh = g;
        g = f;
        f = e;
        e = d + t1;
        d = c;
        c = b;
        b = a;
        a = t1 + t2;
    }

    s.h[0] += a;
    s.h[1] += b;
    s.h[2] += c;
    s.h[3] += d;
    s.h[4] += e;
    s.h[5] += f;
    s.h[6] += g;
    s.h[7] += hh;
    return;
}

pub fn sha512_update(s: Sha512, data: []u8, at: i64, n: i64) void {
    s.length += u64(n);
    var taken = 0;
    if (s.used > 0) {
        var room = 128 - s.used;
        if (room > n) { room = n; }
        bytes.copy(s.block, s.used, data, at, room);
        s.used += room;
        taken = room;
        if (s.used == 128) {
            sha512_block(s, s.block, 0);
            s.used = 0;
        }
    }
    while (n - taken >= 128) {
        sha512_block(s, data, at + taken);
        taken += 128;
    }
    if (n - taken > 0) {
        bytes.copy(s.block, 0, data, at + taken, n - taken);
        s.used = n - taken;
    }
    return;
}

/// The digest. The length field is 128 bits here rather than 64, and its high
/// half is written as the zero it is: a message long enough to fill it would
/// be sixteen exabytes.
pub fn sha512_final(s: Sha512) []u8 {
    const bit_length = s.length * 8;
    const tail = bytes.new(144);
    tail[0] = 0x80;
    var pad = 128 - ((s.used + 17) % 128);
    if (pad == 128) { pad = 0; }
    bytes.put_be64(tail, 1 + pad, 0);
    bytes.put_be64(tail, 1 + pad + 8, bit_length);
    sha512_update(s, tail, 0, 1 + pad + 16);

    const out = bytes.new(s.out_len);
    var i = 0;
    while (i * 8 < s.out_len) : (i += 1) { bytes.put_be64(out, i * 8, s.h[i]); }
    return out;
}

pub fn sha512(data: []u8) []u8 {
    const s = sha512_init();
    sha512_update(s, data, 0, array.len(data));
    return sha512_final(s);
}

pub fn sha384(data: []u8) []u8 {
    const s = sha384_init();
    sha512_update(s, data, 0, array.len(data));
    return sha512_final(s);
}

// ---------------------------------------------------------------------------
// HMAC and HKDF
// ---------------------------------------------------------------------------
//
// Written once over a *value* describing the hash rather than once per hash.
// The three things HMAC needs to know -- the block size, the digest size, and
// how to hash -- are three fields, and the third is an ordinary function value.
//
// An overload set would have read better and does not work: which overload is
// meant is a question about a parameter's type, and inside a body generic over
// the algorithm there is no type yet to ask about. A value carries the answer
// to where it is needed instead, which is what a function value is for.

pub const Hash = struct {
    block_size: i64,
    digest_size: i64,
    digest: fn([]u8) []u8,
};

pub fn sha256_hash() Hash {
    return Hash{ .block_size = 64, .digest_size = 32, .digest = sha256 };
}

pub fn sha384_hash() Hash {
    return Hash{ .block_size = 128, .digest_size = 48, .digest = sha384 };
}

pub fn sha512_hash() Hash {
    return Hash{ .block_size = 128, .digest_size = 64, .digest = sha512 };
}

/// HMAC, RFC 2104: `H((K ^ opad) || H((K ^ ipad) || m))`.
///
/// A key longer than the block is hashed first and a shorter one is zero
/// padded, which is what makes every key length legal and is also the reason
/// two different keys can collide -- a property of the construction, not of
/// this writing of it.
pub fn hmac(h: Hash, key: []u8, msg: []u8) []u8 {
    const padded = bytes.new(h.block_size);
    if (array.len(key) > h.block_size) {
        const hashed = h.digest(key);
        bytes.copy(padded, 0, hashed, 0, array.len(hashed));
    } else {
        bytes.copy(padded, 0, key, 0, array.len(key));
    }

    const inner = bytes.new(h.block_size + array.len(msg));
    var i = 0;
    while (i < h.block_size) : (i += 1) { inner[i] = padded[i] ^ 0x36; }
    bytes.copy(inner, h.block_size, msg, 0, array.len(msg));
    const first = h.digest(inner);

    const outer = bytes.new(h.block_size + h.digest_size);
    i = 0;
    while (i < h.block_size) : (i += 1) { outer[i] = padded[i] ^ 0x5c; }
    bytes.copy(outer, h.block_size, first, 0, h.digest_size);
    return h.digest(outer);
}

/// HKDF-Extract, RFC 5869: the salt is the key and the input keying material
/// is the message, which is the way round that surprises everyone once.
pub fn hkdf_extract(h: Hash, salt: []u8, ikm: []u8) []u8 {
    return hmac(h, salt, ikm);
}

/// HKDF-Expand, RFC 5869: `T(n) = HMAC(prk, T(n-1) || info || n)`, taken until
/// `length` bytes have been produced.
///
/// The counter is one byte, so no more than 255 blocks can ever be asked for.
/// Exceeding that is a mistake in the caller rather than in its data -- there
/// is no input that makes it happen -- so it asserts rather than returning an
/// error every TLS key schedule would then have to thread through.
pub fn hkdf_expand(h: Hash, prk: []u8, info: []u8, length: i64) []u8 {
    assert(length <= 255 * h.digest_size);
    const out = bytes.new(length);
    var previous = bytes.new(0);
    var done = 0;
    var counter = 1;
    while (done < length) {
        const input = bytes.new(array.len(previous) + array.len(info) + 1);
        bytes.copy(input, 0, previous, 0, array.len(previous));
        bytes.copy(input, array.len(previous), info, 0, array.len(info));
        input[array.len(input) - 1] = u8(counter);
        previous = hmac(h, prk, input);

        var take = h.digest_size;
        if (done + take > length) { take = length - done; }
        bytes.copy(out, done, previous, 0, take);
        done += take;
        counter += 1;
    }
    return out;
}
