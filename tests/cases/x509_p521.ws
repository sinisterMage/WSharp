// Where the curves stop: P-256 and P-384 are read, P-521 is refused.
//
// The guard for LIMITATIONS.md, "`std/tls` cannot verify a chain through a P-521
// key". Three real SubjectPublicKeyInfos, one per curve, so that the boundary
// asserted is the *curve* and not "EC keys" or "long keys" -- a case with only
// the P-521 key in it would pass if `parse_spki` had stopped reading EC keys
// altogether.
//
// The keys were generated with node's `crypto.generateKeyPairSync('ec', ..)` and
// exported as DER SPKI, which is the independent implementation the rest of this
// suite's cryptographic fixtures are held to. They are public keys and nothing
// signs with them here, so they are fixtures rather than secrets.
//
// Why P-521 is not the third table of constants that P-384 was: `std/nistec` is
// one curve implementation parameterised by limb count, coordinate size and
// scalar width over `std/bignum`'s generic Montgomery multiplication, and 521
// bits is not a whole number of 32-bit limbs. A limb is 32 bits because there is
// no 64x64 -> 128 product to build a wider one from -- `bits.mulhi` does not
// exist -- so P-521 needs either a partial top limb threaded through the
// arithmetic or that multiply, and neither is a table.
//
// The refusal is `error.BadKey` at `parse_spki`, not a crash and not a key that
// silently fails to verify later: "we do not do that curve" and "the signature
// is wrong" are different answers to a caller. A root in the *store* this
// library cannot read is dropped and the rest are used (`parse_all`); one in a
// *chain* is a refusal (`verify_chain`). That asymmetry is deliberate -- answering
// both the same way either makes a machine with one odd root unusable or makes a
// broken chain acceptable.
// expect: P-256 is read
// expect: P-384 is read
// expect: P-521 is refused
const x509 = @import("std/x509");
const bytes = @import("std/bytes");

/// `prime256v1`, from node.
const P256 = "3059301306072a8648ce3d020106082a8648ce3d03010703420004860c0a0dfdc1fd2f2347a51111cd6f883358e46ca8b72941de56594ff5d1f7d0197fa935708d74f41d4756c8f3b1a7a2eecec7479046b1a47cf5c4ce4b40fd3f";
/// `secp384r1`, from node.
const P384 = "3076301006072a8648ce3d020106052b810400220362000488135c1dcd75aa44213931dccf77b1425360baa0efb1a1995f474a7b64bd7ebedfcb7ecc4b1f8f582b6bcb46961fcf11133334b4ee48f3944445307712eaae1fb4c79690e891eda3db7dab7759761b1ee88e41f84b9c0c5ddc64d0194cb705d0";
/// `secp521r1`, from node. Well-formed DER naming curve 1.3.132.0.35.
const P521 = "30819b301006072a8648ce3d020106052b8104002303818600040058d21e03857efe69dc5ca62a1ed07e6301570af602a5e9806e5437c8dacd56844c29ded4ccffac726f2fdfad55acf812369a97e004432641bff479d9c3a62d0c2a015004b468e424cebd044e9cc66d93e89c4f373b0003e8c296dd270b3ec8d8de4902ae88831b3dbc1410e83a389e549c1ef9de6dbe2b29210b7410ff4600916d90f3";

fn main() i64 {
    if (readable(P256)) { print("P-256 is read"); }
    if (readable(P384)) { print("P-384 is read"); }
    if (!readable(P521)) { print("P-521 is refused"); }
    return 0;
}

/// Whether `parse_spki` gives a key back, rather than which key.
///
/// The curve is the only thing being asserted, and the three DERs differ in
/// nothing else, so "did it raise" is the whole question.
fn readable(der_hex: str) bool {
    const spki = bytes.from_hex(der_hex) catch { return false; };
    const key = x509.parse_spki(spki) catch { return false; };
    return true;
}
