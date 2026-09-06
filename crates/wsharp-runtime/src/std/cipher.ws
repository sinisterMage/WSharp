// The AEADs TLS 1.3 uses: ChaCha20-Poly1305 and AES-GCM.
//
// Written in W# for item 10's reason rather than the collector's: a cipher is
// a pure byte-to-byte transform, which the boundary rule would happily allow
// in Rust. The point is to find out what the language cannot say, and the
// answer so far has been one thing -- a `const` table -- which is now in.
//
// **Constant time is a construction here, not a guarantee, and the difference
// matters.** Nothing below indexes a table by a secret byte and nothing exits a
// loop early on a secret comparison, which is what an implementation can
// control. What it cannot control is Cranelift, which is free to turn a
// branchless expression into a branch or a select into a jump, and W# has no
// `black_box` to pin a secret away from an optimisation. So this is a best
// effort against a local attacker rather than a promise. Anyone who needs the
// promise needs a reviewed C library behind an FFI, which is a different item.
//
// The most visible consequence is AES's S-box, which is *computed* rather than
// looked up: a table indexed by a secret byte is precisely the thing that must
// not happen, so `sbox` inverts in GF(2^8) instead. It is far slower than a
// lookup and it is honest. GHASH is table-free for the same reason.
const array = @import("std/array");
const bytes = @import("std/bytes");
const bits = @import("std/bits");

// ---------------------------------------------------------------------------
// ChaCha20 -- RFC 8439
// ---------------------------------------------------------------------------

/// "expand 32-byte k", which is what the first four words of the state spell.
const SIGMA = []u32{ 0x61707865, 0x3320646e, 0x79622d32, 0x6b206574 };

/// The quarter round, on four words of a sixteen-word state.
///
/// Written against indices rather than against four variables because W# has
/// no reference to an array element -- and it reads the same either way, which
/// `tests/cases/chacha_quarter.ws` has said since item 9.
fn quarter(x: []u32, a: i64, b: i64, c: i64, d: i64) void {
    x[a] += x[b];  x[d] ^= x[a];  x[d] = bits.rotl(x[d], 16);
    x[c] += x[d];  x[b] ^= x[c];  x[b] = bits.rotl(x[b], 12);
    x[a] += x[b];  x[d] ^= x[a];  x[d] = bits.rotl(x[d], 8);
    x[c] += x[d];  x[b] ^= x[c];  x[b] = bits.rotl(x[b], 7);
    return;
}

/// The sixteen-word state for a key, nonce and block counter.
fn chacha_state(key: []u8, nonce: []u8, counter: u32) []u32 {
    const s: []u32 = array.new(16);
    var i = 0;
    while (i < 4) : (i += 1) { s[i] = SIGMA[i]; }
    while (i < 12) : (i += 1) { s[i] = bytes.le32(key, (i - 4) * 4); }
    s[12] = counter;
    i = 13;
    while (i < 16) : (i += 1) { s[i] = bytes.le32(nonce, (i - 13) * 4); }
    return s;
}

/// One 64-byte keystream block from `state` into `out`, using `work` as the
/// scratch the twenty rounds run in. Both are the caller's, so a message of
/// any length allocates the same two objects.
fn chacha_block(state: []u32, work: []u32, out: []u8) void {
    var i = 0;
    while (i < 16) : (i += 1) { work[i] = state[i]; }
    // Ten double rounds: four columns, then four diagonals.
    i = 0;
    while (i < 10) : (i += 1) {
        quarter(work, 0, 4, 8, 12);
        quarter(work, 1, 5, 9, 13);
        quarter(work, 2, 6, 10, 14);
        quarter(work, 3, 7, 11, 15);
        quarter(work, 0, 5, 10, 15);
        quarter(work, 1, 6, 11, 12);
        quarter(work, 2, 7, 8, 13);
        quarter(work, 3, 4, 9, 14);
    }
    i = 0;
    while (i < 16) : (i += 1) { bytes.put_le32(out, i * 4, work[i] + state[i]); }
    return;
}

/// One keystream block, for the vector RFC 8439 section 2.3.2 prints.
pub fn chacha20_block(key: []u8, nonce: []u8, counter: u32) []u8 {
    const out = bytes.new(64);
    chacha_block(chacha_state(key, nonce, counter), array.new(16), out);
    return out;
}

/// XOR `n` bytes of `buf` at `at` with the keystream, in place.
///
/// In place because that is what a record layer wants and because encryption
/// and decryption are the same operation: a stream cipher's inverse is itself.
pub fn chacha20_xor(key: []u8, nonce: []u8, counter: u32, buf: []u8, at: i64, n: i64) void {
    const state = chacha_state(key, nonce, counter);
    const work: []u32 = array.new(16);
    const ks = bytes.new(64);
    var done = 0;
    while (done < n) {
        chacha_block(state, work, ks);
        var take = 64;
        if (done + take > n) { take = n - done; }
        var i = 0;
        while (i < take) : (i += 1) { buf[at + done + i] ^= ks[i]; }
        state[12] += 1;
        done += 64;
    }
    return;
}

// ---------------------------------------------------------------------------
// Poly1305 -- RFC 8439
// ---------------------------------------------------------------------------
//
// The accumulator is 130 bits wide, which is the one place item 9's decision
// not to have a `u128` is felt. It is not an obstruction: five 26-bit limbs in
// `u64`s is the shape every portable implementation uses anyway, because the
// products then fit a 64-bit word with room to spare -- the largest is five
// terms of about 2^54, which is 2^57.
//
// Every step below is a straight-line sequence of arithmetic. There is no
// branch on a secret and no index computed from one.

pub const Poly1305 = struct {
    /// The clamped key, in five 26-bit limbs.
    r: []u64,
    /// The accumulator, likewise.
    h: []u64,
    /// The second half of the key, added at the very end.
    pad: []u8,
    /// A part-filled block, and how much of it is filled.
    block: []u8,
    used: i64,
};

/// `r` with the bits the construction requires to be zero cleared.
///
/// The clamping is what bounds the products above; without it the limbs could
/// carry enough for a 64-bit accumulator to overflow.
pub fn poly1305_init(key: []u8) Poly1305 {
    const r: []u64 = array.new(5);
    r[0] = u64(bytes.le32(key, 0)) & 0x03ffffff;
    r[1] = (u64(bytes.le32(key, 3)) >> 2) & 0x03ffff03;
    r[2] = (u64(bytes.le32(key, 6)) >> 4) & 0x03ffc0ff;
    r[3] = (u64(bytes.le32(key, 9)) >> 6) & 0x03f03fff;
    r[4] = (u64(bytes.le32(key, 12)) >> 8) & 0x000fffff;

    const pad = bytes.new(16);
    bytes.copy(pad, 0, key, 16, 16);
    return Poly1305{
        .r = r,
        .h = array.new(5),
        .pad = pad,
        .block = bytes.new(16),
        .used = 0,
    };
}

/// One 16-byte block into the accumulator.
///
/// `high` is the 2^128 bit the construction appends to every whole block; the
/// final short block has it already written into its padding instead, so it
/// passes zero.
fn poly1305_block(p: Poly1305, m: []u8, at: i64, high: u64) void {
    const r0 = p.r[0];
    const r1 = p.r[1];
    const r2 = p.r[2];
    const r3 = p.r[3];
    const r4 = p.r[4];
    // The wrap-around of the reduction modulo 2^130 - 5: a limb carried off
    // the top comes back multiplied by five.
    const s1 = r1 * 5;
    const s2 = r2 * 5;
    const s3 = r3 * 5;
    const s4 = r4 * 5;

    const h0 = p.h[0] + (u64(bytes.le32(m, at)) & 0x03ffffff);
    const h1 = p.h[1] + ((u64(bytes.le32(m, at + 3)) >> 2) & 0x03ffffff);
    const h2 = p.h[2] + ((u64(bytes.le32(m, at + 6)) >> 4) & 0x03ffffff);
    const h3 = p.h[3] + ((u64(bytes.le32(m, at + 9)) >> 6) & 0x03ffffff);
    const h4 = p.h[4] + (u64(bytes.le32(m, at + 12)) >> 8) + high;

    var d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
    var d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
    var d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
    var d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
    var d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

    var carry = d0 >> 26;
    p.h[0] = d0 & 0x03ffffff;
    d1 += carry;  carry = d1 >> 26;  p.h[1] = d1 & 0x03ffffff;
    d2 += carry;  carry = d2 >> 26;  p.h[2] = d2 & 0x03ffffff;
    d3 += carry;  carry = d3 >> 26;  p.h[3] = d3 & 0x03ffffff;
    d4 += carry;  carry = d4 >> 26;  p.h[4] = d4 & 0x03ffffff;
    p.h[0] += carry * 5;
    carry = p.h[0] >> 26;
    p.h[0] &= 0x03ffffff;
    p.h[1] += carry;
    return;
}

pub fn poly1305_update(p: Poly1305, m: []u8, at: i64, n: i64) void {
    var taken = 0;
    if (p.used > 0) {
        var room = 16 - p.used;
        if (room > n) { room = n; }
        bytes.copy(p.block, p.used, m, at, room);
        p.used += room;
        taken = room;
        if (p.used == 16) {
            poly1305_block(p, p.block, 0, u64(1) << 24);
            p.used = 0;
        }
    }
    while (n - taken >= 16) {
        poly1305_block(p, m, at + taken, u64(1) << 24);
        taken += 16;
    }
    if (n - taken > 0) {
        bytes.copy(p.block, 0, m, at + taken, n - taken);
        p.used = n - taken;
    }
    return;
}

/// The 16-byte tag.
pub fn poly1305_final(p: Poly1305) []u8 {
    if (p.used > 0) {
        // The short block is padded with a one bit and then zeros, which is
        // why it passes no separate high bit.
        bytes.fill(p.block, p.used, 16 - p.used, 0);
        p.block[p.used] = 1;
        poly1305_block(p, p.block, 0, 0);
        p.used = 0;
    }

    var h0 = p.h[0];
    var h1 = p.h[1];
    var h2 = p.h[2];
    var h3 = p.h[3];
    var h4 = p.h[4];

    var carry = h1 >> 26;  h1 &= 0x03ffffff;
    h2 += carry;  carry = h2 >> 26;  h2 &= 0x03ffffff;
    h3 += carry;  carry = h3 >> 26;  h3 &= 0x03ffffff;
    h4 += carry;  carry = h4 >> 26;  h4 &= 0x03ffffff;
    h0 += carry * 5;  carry = h0 >> 26;  h0 &= 0x03ffffff;
    h1 += carry;

    // `h + -p`, so that comparing against 2^130 - 5 is a subtraction rather
    // than a comparison -- which is what keeps the choice below branchless.
    var g0 = h0 + 5;  carry = g0 >> 26;  g0 &= 0x03ffffff;
    var g1 = h1 + carry;  carry = g1 >> 26;  g1 &= 0x03ffffff;
    var g2 = h2 + carry;  carry = g2 >> 26;  g2 &= 0x03ffffff;
    var g3 = h3 + carry;  carry = g3 >> 26;  g3 &= 0x03ffffff;
    var g4 = h4 + carry - (u64(1) << 26);

    // `g4` borrowed, so `h` was already below the modulus and `h` is the
    // answer; otherwise `g` is. Selected with a mask rather than an `if`.
    const use_g = (g4 >> 63) - 1;
    h0 = (h0 & ~use_g) | (g0 & use_g);
    h1 = (h1 & ~use_g) | (g1 & use_g);
    h2 = (h2 & ~use_g) | (g2 & use_g);
    h3 = (h3 & ~use_g) | (g3 & use_g);
    h4 = (h4 & ~use_g) | (g4 & use_g);

    // Five 26-bit limbs back into four 32-bit words, plus the key's second
    // half as a 128-bit addition done a word at a time.
    var w0 = (h0 | (h1 << 26)) & 0xffffffff;
    var w1 = ((h1 >> 6) | (h2 << 20)) & 0xffffffff;
    var w2 = ((h2 >> 12) | (h3 << 14)) & 0xffffffff;
    var w3 = ((h3 >> 18) | (h4 << 8)) & 0xffffffff;

    var f = w0 + u64(bytes.le32(p.pad, 0));
    w0 = f & 0xffffffff;
    f = w1 + u64(bytes.le32(p.pad, 4)) + (f >> 32);
    w1 = f & 0xffffffff;
    f = w2 + u64(bytes.le32(p.pad, 8)) + (f >> 32);
    w2 = f & 0xffffffff;
    f = w3 + u64(bytes.le32(p.pad, 12)) + (f >> 32);
    w3 = f & 0xffffffff;

    const out = bytes.new(16);
    bytes.put_le32(out, 0, u32(w0));
    bytes.put_le32(out, 4, u32(w1));
    bytes.put_le32(out, 8, u32(w2));
    bytes.put_le32(out, 12, u32(w3));
    return out;
}

pub fn poly1305(key: []u8, msg: []u8) []u8 {
    const p = poly1305_init(key);
    poly1305_update(p, msg, 0, array.len(msg));
    return poly1305_final(p);
}

// ---------------------------------------------------------------------------
// ChaCha20-Poly1305 -- RFC 8439 section 2.8
// ---------------------------------------------------------------------------

/// The zeros that bring a MAC input up to a multiple of sixteen.
///
/// Fifteen is the most that can ever be wanted, and a `const` array of them
/// costs nothing to name -- which is what makes writing it this way better
/// than a loop that pads a byte at a time.
const ZEROS = []u8{ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0 };

fn pad16(p: Poly1305, n: i64) void {
    const short = n % 16;
    if (short != 0) { poly1305_update(p, ZEROS, 0, 16 - short); }
    return;
}

/// The one-time Poly1305 key for a message: the first half of ChaCha20's
/// block zero, which is why the message itself starts at block one.
fn poly_key(key: []u8, nonce: []u8) []u8 {
    return bytes.slice(chacha20_block(key, nonce, 0), 0, 32);
}

/// The tag over `aad` and `ciphertext`, with each padded to sixteen bytes and
/// both lengths appended -- the framing that stops a byte moving from one
/// field into the other unnoticed.
fn chacha_tag(key: []u8, nonce: []u8, aad: []u8, ct: []u8, ct_len: i64) []u8 {
    const p = poly1305_init(poly_key(key, nonce));
    poly1305_update(p, aad, 0, array.len(aad));
    pad16(p, array.len(aad));
    poly1305_update(p, ct, 0, ct_len);
    pad16(p, ct_len);
    const lengths = bytes.new(16);
    bytes.put_le64(lengths, 0, u64(array.len(aad)));
    bytes.put_le64(lengths, 8, u64(ct_len));
    poly1305_update(p, lengths, 0, 16);
    return poly1305_final(p);
}

/// Encrypt and authenticate: the ciphertext followed by its 16-byte tag.
///
/// The nonce must never repeat under one key. Not "should": ChaCha20 is a
/// stream cipher, so two messages under one nonce differ by the XOR of their
/// plaintexts, and Poly1305's one-time key stops being one-time -- which
/// hands over the ability to forge, not merely to read. `std/crypto.random`
/// is where a nonce should come from.
pub fn chacha20_poly1305_seal(key: []u8, nonce: []u8, aad: []u8, plaintext: []u8) []u8 {
    const n = array.len(plaintext);
    const out = bytes.new(n + 16);
    bytes.copy(out, 0, plaintext, 0, n);
    chacha20_xor(key, nonce, 1, out, 0, n);
    const tag = chacha_tag(key, nonce, aad, out, n);
    bytes.copy(out, n, tag, 0, 16);
    return out;
}

/// Verify and decrypt. `error.AuthenticationFailed` if the tag does not match,
/// and nothing is decrypted in that case -- the tag is checked first, on the
/// ciphertext, which is the whole point of an AEAD.
pub fn chacha20_poly1305_open(key: []u8, nonce: []u8, aad: []u8, sealed: []u8) ![]u8 {
    if (array.len(sealed) < 16) { return error.AuthenticationFailed; }
    const n = array.len(sealed) - 16;
    const want = chacha_tag(key, nonce, aad, sealed, n);
    if (!bytes.equal(want, bytes.slice(sealed, n, n + 16))) {
        return error.AuthenticationFailed;
    }
    const out = bytes.slice(sealed, 0, n);
    chacha20_xor(key, nonce, 1, out, 0, n);
    return out;
}

// ---------------------------------------------------------------------------
// AES -- FIPS 197
// ---------------------------------------------------------------------------
//
// Encryption only. GCM is counter mode, so it enciphers even to decrypt and
// never calls the inverse cipher; writing one would add a hundred lines of
// code that no test could reach, in a file where unreached code is the exact
// shape a security bug hides in.
//
// The state is a flat sixteen-byte buffer in column-major order, which is how
// the standard numbers it and also the only shape the language offers: a place
// being assigned must have a variable or a field at its base, so `s[r][c] = v`
// is not written here. It reads better this way regardless.

/// The round constants, `x^(i-1)` in GF(2^8). Index 0 is never used.
const RCON = []u8{ 0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36 };

/// Multiply in GF(2^8) modulo `x^8 + x^4 + x^3 + x + 1`.
///
/// The peasant's algorithm with the conditionals turned into masks: `0 - bit`
/// is all ones or all zeros, so each step either mixes a value in or mixes
/// nothing in, and takes the same instructions either way.
fn gmul(a: u8, b: u8) u8 {
    var result: u8 = 0;
    var x = a;
    var y = b;
    var i = 0;
    while (i < 8) : (i += 1) {
        const take = 0 - (y & 1);
        result ^= x & take;
        const overflow = 0 - ((x >> 7) & 1);
        x = (x << 1) ^ (0x1b & overflow);
        y >>= 1;
    }
    return result;
}

/// The multiplicative inverse in GF(2^8), which is `x^254`; zero inverts to
/// zero, which the exponentiation gives for free.
///
/// **This is where the S-box would be a table, and is not.** A 256-byte table
/// indexed by a byte of the state is indexed by a byte that depends on the
/// key, and where that index lands decides which cache line is touched. The
/// exponentiation is about a hundred times slower and touches the same
/// instructions whatever the input.
fn ginv(x: u8) u8 {
    const x2 = gmul(x, x);
    const x4 = gmul(x2, x2);
    const x8 = gmul(x4, x4);
    const x16 = gmul(x8, x8);
    const x32 = gmul(x16, x16);
    const x64 = gmul(x32, x32);
    const x128 = gmul(x64, x64);
    var r = gmul(x2, x4);
    r = gmul(r, x8);
    r = gmul(r, x16);
    r = gmul(r, x32);
    r = gmul(r, x64);
    return gmul(r, x128);
}

/// The S-box: invert, then the affine transform the standard writes as a
/// matrix and every implementation writes as four rotations.
fn sbox(x: u8) u8 {
    const b = ginv(x);
    return b ^ bits.rotl(b, 1) ^ bits.rotl(b, 2) ^ bits.rotl(b, 3) ^ bits.rotl(b, 4) ^ 0x63;
}

/// The S-box, exposed so that a test can check it is a permutation.
///
/// For the reason `panic_index` and the `gc_*` counters are exposed: a piece
/// the language's security rests on should be assertable from W#. A wrong
/// inversion would still produce a cipher, and the property that says it is
/// the wrong one is that the S-box has stopped being a bijection.
pub fn aes_sub_byte(x: u8) u8 { return sbox(x); }

/// An expanded key: `16 * (rounds + 1)` bytes, and how many rounds they are.
pub const Aes = struct { round_keys: []u8, rounds: i64 };

/// Expand a 16- or 32-byte key. Any other length is a mistake in the caller
/// rather than in its data, so it asserts.
pub fn aes_init(key: []u8) Aes {
    const nk = array.len(key) / 4;
    assert(nk == 4 or nk == 8);
    const rounds = nk + 6;
    const total = 4 * (rounds + 1);
    const w = bytes.new(total * 4);
    bytes.copy(w, 0, key, 0, array.len(key));

    var i = nk;
    while (i < total) : (i += 1) {
        var t0 = w[(i - 1) * 4];
        var t1 = w[(i - 1) * 4 + 1];
        var t2 = w[(i - 1) * 4 + 2];
        var t3 = w[(i - 1) * 4 + 3];
        if (i % nk == 0) {
            // RotWord, then SubWord, then the round constant.
            const spare = t0;
            t0 = sbox(t1) ^ RCON[i / nk];
            t1 = sbox(t2);
            t2 = sbox(t3);
            t3 = sbox(spare);
        } else {
            // AES-256 substitutes again at the halfway word of each group.
            if (nk > 6 and i % nk == 4) {
                t0 = sbox(t0);
                t1 = sbox(t1);
                t2 = sbox(t2);
                t3 = sbox(t3);
            }
        }
        w[i * 4] = w[(i - nk) * 4] ^ t0;
        w[i * 4 + 1] = w[(i - nk) * 4 + 1] ^ t1;
        w[i * 4 + 2] = w[(i - nk) * 4 + 2] ^ t2;
        w[i * 4 + 3] = w[(i - nk) * 4 + 3] ^ t3;
    }
    return Aes{ .round_keys = w, .rounds = rounds };
}

fn add_round_key(state: []u8, k: Aes, round: i64) void {
    bytes.xor(state, 0, k.round_keys, round * 16, 16);
    return;
}

fn sub_bytes(state: []u8) void {
    var i = 0;
    while (i < 16) : (i += 1) { state[i] = sbox(state[i]); }
    return;
}

/// Row `r` rotated left by `r`. Row `r` is the bytes at `r`, `r+4`, `r+8`,
/// `r+12`, because the state is stored a column at a time.
fn shift_rows(state: []u8) void {
    var r = 1;
    while (r < 4) : (r += 1) {
        const a = state[r];
        const b = state[r + 4];
        const c = state[r + 8];
        const d = state[r + 12];
        if (r == 1) {
            state[r] = b; state[r + 4] = c; state[r + 8] = d; state[r + 12] = a;
        }
        if (r == 2) {
            state[r] = c; state[r + 4] = d; state[r + 8] = a; state[r + 12] = b;
        }
        if (r == 3) {
            state[r] = d; state[r + 4] = a; state[r + 8] = b; state[r + 12] = c;
        }
    }
    return;
}

fn mix_columns(state: []u8) void {
    var c = 0;
    while (c < 4) : (c += 1) {
        const a0 = state[c * 4];
        const a1 = state[c * 4 + 1];
        const a2 = state[c * 4 + 2];
        const a3 = state[c * 4 + 3];
        state[c * 4] = gmul(a0, 2) ^ gmul(a1, 3) ^ a2 ^ a3;
        state[c * 4 + 1] = a0 ^ gmul(a1, 2) ^ gmul(a2, 3) ^ a3;
        state[c * 4 + 2] = a0 ^ a1 ^ gmul(a2, 2) ^ gmul(a3, 3);
        state[c * 4 + 3] = gmul(a0, 3) ^ a1 ^ a2 ^ gmul(a3, 2);
    }
    return;
}

/// Encipher `state` in place.
pub fn aes_encrypt_block(k: Aes, state: []u8) void {
    add_round_key(state, k, 0);
    var round = 1;
    while (round < k.rounds) : (round += 1) {
        sub_bytes(state);
        shift_rows(state);
        mix_columns(state);
        add_round_key(state, k, round);
    }
    // The last round has no MixColumns, which is what makes decryption
    // possible at all and is the standard's one asymmetry.
    sub_bytes(state);
    shift_rows(state);
    add_round_key(state, k, k.rounds);
    return;
}

/// One enciphered block of `input`, as a new buffer.
pub fn aes_encrypt(k: Aes, input: []u8) []u8 {
    const out = bytes.slice(input, 0, 16);
    aes_encrypt_block(k, out);
    return out;
}

// ---------------------------------------------------------------------------
// AES-GCM -- NIST SP 800-38D
// ---------------------------------------------------------------------------
//
// GHASH is multiplication in GF(2^128), and it is written here as 128 shifts
// and exclusive-ors rather than as the four-bit table every fast
// implementation uses. The table is indexed by the data being authenticated,
// which under decryption is attacker-supplied and under encryption is the
// plaintext; either way it is a secret-dependent index, which is the thing
// this file does not do. It is also the shape that needs no `u128`.

/// `x = x * h` in GF(2^128), in the bit order GCM defines -- which is the
/// reverse of the usual one, so the reduction polynomial appears as `0xe1`
/// in the *top* byte and the shift goes right.
fn ghash_mul(x: []u8, h: []u8) void {
    var zhi: u64 = 0;
    var zlo: u64 = 0;
    var vhi = bytes.be64(h, 0);
    var vlo = bytes.be64(h, 8);
    const xhi = bytes.be64(x, 0);
    const xlo = bytes.be64(x, 8);

    var i = 0;
    while (i < 128) : (i += 1) {
        // Which half, and which bit of it -- both decided by the loop counter
        // and neither by the data.
        var word = xhi;
        if (i >= 64) { word = xlo; }
        const take = 0 - ((word >> u64(63 - (i % 64))) & 1);
        zhi ^= vhi & take;
        zlo ^= vlo & take;

        const carried = 0 - (vlo & 1);
        vlo = (vlo >> 1) | (vhi << 63);
        vhi = (vhi >> 1) ^ (0xe100000000000000 & carried);
    }

    bytes.put_be64(x, 0, zhi);
    bytes.put_be64(x, 8, zlo);
    return;
}

/// Fold `n` bytes into the accumulator, a block at a time, zero-padding a
/// short final block -- which is what makes the length fields at the end
/// necessary rather than decorative.
fn ghash_update(y: []u8, h: []u8, data: []u8, at: i64, n: i64) void {
    var done = 0;
    while (done < n) {
        var take = 16;
        if (done + take > n) { take = n - done; }
        var i = 0;
        while (i < take) : (i += 1) { y[i] ^= data[at + done + i]; }
        ghash_mul(y, h);
        done += 16;
    }
    return;
}

/// An expanded key and the hash subkey that goes with it.
pub const AesGcm = struct { key: Aes, h: []u8 };

pub fn aes_gcm_init(key: []u8) AesGcm {
    const k = aes_init(key);
    // H is the cipher applied to a block of zeros.
    const h = bytes.new(16);
    aes_encrypt_block(k, h);
    return AesGcm{ .key = k, .h = h };
}

/// The pre-counter block for a nonce.
///
/// Twelve bytes is the ordinary case and gets the nonce with a one after it;
/// any other length is hashed down, which is the general construction and is
/// why an eight-byte or a sixty-byte nonce is legal at all.
fn counter_block(g: AesGcm, iv: []u8) []u8 {
    const j0 = bytes.new(16);
    if (array.len(iv) == 12) {
        bytes.copy(j0, 0, iv, 0, 12);
        j0[15] = 1;
        return j0;
    }
    ghash_update(j0, g.h, iv, 0, array.len(iv));
    const lengths = bytes.new(16);
    bytes.put_be64(lengths, 8, u64(array.len(iv)) * 8);
    ghash_update(j0, g.h, lengths, 0, 16);
    return j0;
}

/// Increment the counter's last four bytes, wrapping within them alone --
/// which is what the `32` in `inc32` means and what keeps the nonce untouched.
fn inc32(b: []u8) void {
    bytes.put_be32(b, 12, bytes.be32(b, 12) + 1);
    return;
}

/// Counter mode over `n` bytes of `buf`, in place.
fn gctr(g: AesGcm, icb: []u8, buf: []u8, at: i64, n: i64) void {
    const counter = bytes.slice(icb, 0, 16);
    const ks = bytes.new(16);
    var done = 0;
    while (done < n) {
        bytes.copy(ks, 0, counter, 0, 16);
        aes_encrypt_block(g.key, ks);
        var take = 16;
        if (done + take > n) { take = n - done; }
        var i = 0;
        while (i < take) : (i += 1) { buf[at + done + i] ^= ks[i]; }
        inc32(counter);
        done += 16;
    }
    return;
}

fn gcm_tag(g: AesGcm, j0: []u8, aad: []u8, ct: []u8, ct_len: i64) []u8 {
    const s = bytes.new(16);
    ghash_update(s, g.h, aad, 0, array.len(aad));
    ghash_update(s, g.h, ct, 0, ct_len);
    // Both lengths in bits, which is what stops a byte moving from the
    // additional data into the ciphertext without changing the tag.
    const lengths = bytes.new(16);
    bytes.put_be64(lengths, 0, u64(array.len(aad)) * 8);
    bytes.put_be64(lengths, 8, u64(ct_len) * 8);
    ghash_update(s, g.h, lengths, 0, 16);

    const tag = bytes.slice(j0, 0, 16);
    aes_encrypt_block(g.key, tag);
    bytes.xor(tag, 0, s, 0, 16);
    return tag;
}

/// Encrypt and authenticate: the ciphertext followed by its 16-byte tag.
///
/// As with ChaCha20-Poly1305, the nonce must never repeat under one key: GCM
/// is counter mode, and a repeat leaks the hash subkey's action on the
/// difference, which is a forgery rather than merely a disclosure.
pub fn aes_gcm_seal(g: AesGcm, iv: []u8, aad: []u8, plaintext: []u8) []u8 {
    const n = array.len(plaintext);
    const out = bytes.new(n + 16);
    bytes.copy(out, 0, plaintext, 0, n);

    const j0 = counter_block(g, iv);
    const first = bytes.slice(j0, 0, 16);
    inc32(first);
    gctr(g, first, out, 0, n);
    bytes.copy(out, n, gcm_tag(g, j0, aad, out, n), 0, 16);
    return out;
}

/// Verify and decrypt. The tag is checked against the *ciphertext*, before
/// anything is deciphered -- which is the property that makes an AEAD one.
pub fn aes_gcm_open(g: AesGcm, iv: []u8, aad: []u8, sealed: []u8) ![]u8 {
    if (array.len(sealed) < 16) { return error.AuthenticationFailed; }
    const n = array.len(sealed) - 16;
    const j0 = counter_block(g, iv);
    if (!bytes.equal(gcm_tag(g, j0, aad, sealed, n), bytes.slice(sealed, n, n + 16))) {
        return error.AuthenticationFailed;
    }
    const out = bytes.slice(sealed, 0, n);
    const first = bytes.slice(j0, 0, 16);
    inc32(first);
    gctr(g, first, out, 0, n);
    return out;
}
