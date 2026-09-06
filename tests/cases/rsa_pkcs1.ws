// RSA PKCS#1 v1.5 signature verification, and the key checks in front of it.
//
// The vectors are made rather than published, and were made twice. A key came
// from one implementation; every encoded message below -- the valid ones and
// the forged ones alike -- was built from RFC 8017's text and signed with the
// raw private exponent; and then every one was handed back to that first
// implementation, which agreed about all of them. Two implementations that
// share nothing agreeing about a *forgery* is the check that matters here,
// because a verifier that accepts too much fails no test written only from
// the valid side.
//
// **Each way of being wrong gets its own line.** That is the discipline the
// AEADs were held to and it matters more here: the historical PKCS#1 failures
// are all acceptance bugs, and one "a bad signature is refused" check would
// pass with any of them present. The interesting one is "eight bytes of
// padding and a tail" -- a legal prefix, the minimum padding run, a correct
// DigestInfo, and then room left over for whatever an attacker likes. A
// verifier that parses the encoded message left to right accepts it. This one
// builds the only encoding a valid signature can have and compares, so there
// is nothing to parse and nothing to be lenient about.
// expect: sha-256 verifies
// expect: sha-384 verifies
// expect: sha-512 verifies
// expect: a 3072-bit key verifies
// expect: a flipped signature byte refused
// expect: a flipped message byte refused
// expect: another key's signature refused
// expect: the wrong hash refused
// expect: a short signature refused
// expect: a long signature refused
// expect: a signature at the modulus refused
// expect: eight bytes of padding and a tail refused
// expect: a padding byte that is not 0xff refused
// expect: the wrong block type refused
// expect: an even modulus refused
// expect: a tiny modulus refused
// expect: an even exponent refused
// expect: an exponent above 2^32 refused
const rsa = @import("std/rsa");
const hash = @import("std/hash");
const bytes = @import("std/bytes");
const array = @import("std/array");

const N = "a89315af907b0fcd2c0244d980c65b09417078750d5aab2444a1513aac6c48dceab736a4c799345f1feaee904180269d48cac127b83f2e993bbf6b19c77e74a5c7d0dc6654743a9e7fecbeb3cc08052f441e78d0029fdbd42b0ff632093a2ea344dcfaf485041fc90118d626aa24889fef3453850c199375dacaf42b008026877580e72a5900585a6c8b1297329f24305099a911bad082ee3bf70bb73ae1630e0741673250e35639156bd60ee62fa15e9867b227e7223684e096d35b1d211848f20ef78ff541b8c5185a1560221539f487c7b63c703fd464ab4bc9289ee9f0e8b2979a4d1891b48809a9ad1167bd17c83a537d418e5507c35744a4d924aa8fc9";
const E = "010001";
/// A second 2048-bit key, so that "another key's signature" is a real one.
const N_OTHER = "92726e1c2e9678786d9b1cac76edec969d401ad0322a5e73f27ad362cc7ae9b09d044e1f36f51c4d8802064c40484e18874ac9766496c71091e0edde7b189cab8ad6ef6f81bdfa818039dadf5e6340490b62e592ca5a0c107493686addc3116ede2ae52ae3b1e0ea17e6422d62240805b313440a3f7865a3d08de309a933b266696227984f86ee96966af40f1e50ec1e4b32f745649baae678f6c3ca21389b848de69cc4b2112aad28b51061f81dd6767279f62025cdeebc473b703e54694a95c6510e4c5e43679fe6e5c8a2d677e305572fae991f660c1879a1c3266afcacc60fef4044150df64d07d309975b8d1ff50232fa28ff50d63e4d9e0bf042b30bd9";
const N_3072 = "c452dd14e5edd69f2f63abbb8f0092fe38ee33c1ea7b06d784671321dafe54b50468ad0ba44696cd198a0e9ef5badf599a8148da0b95e4cf851523c74fccc3f33fbd263fb4f8b1707b203393a7031446b2183efc6e089c3fd78ae4643de0385421ea657de635422b80fe561ef34eaedf3b2256a0a8741710dd063d41d1798f2ef6446e85821f314d7e54071c92bd67d4b10a75e8efc3f936ad6fc3aa3967c1c9acd66a73e71a47d0e1639565fd9685f0a9c72971352dd856eb2e65431b6fb584739bbc1fed4e1ac008403e6674b1771494b3e35e399eaab3d31147bbe189c72b48aac09831dd10146ab6960641f3d3efe7f5977283471ea7bee525ba02a61b528efdeb5dc4afd40b97a6bed29927a10e379be3ef37d6c0f149edfd19db3ac197c435c0d0346d6fd23f42e8e24d2117b03ca4db440ac87878d9151ed6ec189959dd0d2591c926b4ac7a00db5adea755ff5062bc6581b76dcfff8c98632b65536c128f5bc79c8ea6e27e4faf3098eaad074cbe92181d1ee560c6f4a4658941f0a7";

const MSG = "5723206974656d2031302073746167652074687265653a20525341207369676e617475726520766572696669636174696f6e2e";
const SIG_256 = "910f83a4cd895392d44ae865589a0b08888718c2a543b33431e77a28d21d5715e386f2c11bcb366af8ff98f8d52c5b0a99dcb25099d1f7f4103dc000ca9e3c191f79d0521b961d62d7e046cd9df1a23f976cbc4c607e6b49d7dbce55f300bee981d3c6b9b6669868d69dc116a7a6926e2d5ea9e53717edeb48b5ca9b759074afc8e32d4dd48bc5c9bba204e9c0a85205741d3c270fdccc11ad7425807a2d043217310f288114007628ad24abac20ce89b6d2ce75d1b1cf7cc7a9a1d5b18cfdec25b2b440b2f880adba15a12a220c569ec157ac0b9c0ab3c8ce4a1d7004041b11a663d7a7e0bc16d36a62a98ac0ea8646e624005caff37e4d4cdc6a38ae05782a";
const SIG_384 = "37e454a9b419d325686fece79aa878c2aa8b954e0cf05eb276803639f66b8589d33896cd4f3eb17a42a741bbc5105f2c86b85434b61df9c97b13c262bb1b6ad7571324cc322167e23e6dc568dbeb2f4721771e0765e3bb57dc5a45c57028ebfd208e1551aa66374727fe89d979dc63727b22fc72e1bba77c205d204b605823c736646f53d9d5661293b90c19000a2e5c34d313dcf6d752d18ea5eb5a59daa4a0c1555e1d58676bb013f599eb04123b04803c95fb4dc65037cd4f9e0bdea68d69001f8caca10dd8c29d550f3ac48491c03aaef566d1ce68497226bd9a8b4b2a98a3eb9819a883a50d95a78207f4247080735abdf1c8efeee10d408e1b37ee9890";
const SIG_512 = "57e5e1b92954bc2cb78fb72c8a212796581c531be7e54b7443f8ba01c63c08ed57ae6d5679c5df657c447f2087d17fe240e9f2016487c71c9e7380703aa01dd96e38792c31e2b6dd6862e009e490fb06758f4dbb49019048be9ce89e10cae4a35879a7c53cd7962ef9a18ce9fd9e8fac2838fceab7b1747d0dd228646ffef79585126665e8d8e4d917ab882b8ca4c059f0ae19c349f8dd77881daf32cfdfa9080bb38e950345be2b22f32573a352f1f474b20f1ce8c5c46176bf2a35322ad6e91a11749e4e409757227afa467db466b19b6edcd3c109888b9b854c81ba9d0dd6439c40e0eee4eab4087a3c86125580f48d6b8b973c67ed560b89ab85b024a96d";
const SIG_3072 = "2a9ad7fa9e7bf28288dc882d2561bb675bb5f73c75fe1f883a5c80901006746b94e9d07074d3c99862482c748de4d4ddaffbc1effd1345062ff061f6432ab73dbf9ff52407e98caa2e83554da501d81fde3838a10c856c092396d67fada037b9c4d9117590a2718ee97d05e3a7e4df9dcbecb4e9516fb9faabbb25df222b8f0e9ed0f14807f1691da541d2e5bdc00a41613370424415f84fd70791075857892d5c100b209812d84ad3d08c2b073e5e1f93c69208155d0ebfa26160ed00b71f1ea4a854afe36d42717d9d5d82a6bc1f3133c6bb6ca5274cd67e94dd62077ceaf0041e93a3f0d90ac30dfb7717cec8b3e9b7a0f3edc2c391d8043eac5e8917762799e400b5beaf14e4cf08d9f9c6cb9e14d9c237702de32af8dc0c49243fe02270855c0c40acb3ef3492f7831c7dcb7767182639d6129d84f3ecd6f44e0c167a60272f6a49d77588460b774a35735afee55e8dd9106900c7dca82c83cc5e638cffcf03830ed45bae8f56328d432ba8d24ec65bba1cf22e34b3d43dd190e6b7f880";
const SIG_OTHER = "8d81e3094546d8cbe577c4120ddc1434b45e416fd1e15b0dca7bd1b07669ffaa52f7877367ac7b294cfcd3ecc06e4efd67b667111bb9ee2f770573e7cfa8413fbe5ca9f90a267e936d7ad442cac9cf6ab79ccde7f3e35729f7adfb2b29491c79f8d12cfd6c0eb228eaad58691832bd73a0dcdac87fea8896dca2974367c58a9f73b3359b5ad6dd344b88506b9333f36c9e2feef01544d5e3b4192ebb0f6cfd6cfb1e605a41fa189aecf5b8fe1f49cde37be672bfd2ba566c3e2776ed508cda2f082e16ba131bf927db204de4e819a6ece566248e495cbc5cb3615239dfbf7d42255ee7235316124b44e8bd20e88ada4609df7713b1b96f01f88a5e477a961823";
const SIG_SHORT_PADDING = "06dc2cda7d51b2620e2612338aab7da218b7d2b69f14d101a4f74f4bdc7c5f12d56288226a350d5145a367fadeb44fb9592ac7e936b822c96e407348afdd81eeaa44370483e84138a2dff2286d51ed0ed939e4c83772d9910f6ff3018838e5bac67565db5272f4b200b67733ef07235a449876c9c43ac8a33b5674317da478cd7d6b2a7fb0be7682cd7a89c27b5381b04ebb8b43ef4900ca368a65e199202d66020086293ea5ded1e6115e0d44d46d85a2d43bb3e68989f9b861a24c9edf13c99821095c7851751b7cce659850a7ab1c84e38ccdba389dbf223aa0c163dc71ba5034e13fa724e5ca3cfcc3a527ab85d0b6a3b11aba1429bba566b0e40faebf43";
const SIG_BAD_PADDING = "28028843bfcf12323cb4fa803f47f975549a6dcb44029a0f4ff910c24a291b1004bbe6ebf425128ace4a4f6d93b455dc002f468b2c394ef2ff067cbd71d7b70bafa36b8441d6cb2e1f03210063b536c69b2cc5a924fb92c73b954e27bfff498c4ed6a085112e95ec8f3012b9bd041c0652f33a57ee2dcd6c55cf76a1b44b833753f2aab121c68d0920e60e31461c84f0dbda6321e8fc9d79f5e5658f90a689d297adfa089fd59fce0bda976f13b942688032a878c041e9b6e7ff6f130bf787660be56b0f4db16eecb40521198fa2970ef249d8c3c8a4f41cc01ec26ff3a1590bd8eaa2f3f303d70740acee4ca5c0b1c1fb3a30e675d93ecafc7317e0d4e57648";
const SIG_BAD_BLOCK_TYPE = "88cc19fe55c998184c19a919825c5882940522808284594267a6912231da63879fca0c0b2b016cc06f076c3d2316251dddd24f0859353eebb4fcc9af4ac33be4b78d4f67755dc270f358ed8e949f62b4d8d08d8d3561133899c263167824c0cbdb4951f80987a6b2833612dc959dc69042d7590a90a21c96893c960dc983229ab335f77181080017cb8bcd724399aa30dd0c616ab1c1b5794786ff9dc98c9dbc88f6164a2eea6f124a6fa72f74650b4826f680bef90281b839ced349ad6c34258c614c5e54332f4fe86ede8ac644b94084623f5781bd3efb6a7f5f700216f5085d00d0a45980581eb4d4e155a6ab69b22fbc94a753ffd4fe4434455970cf1e9e";

fn main() i64 {
    const msg = hex(MSG);
    const sha256 = hash.sha256_hash();
    const k = rsa.public_key(hex(N), hex(E)) catch return 1;

    rsa.verify_pkcs1(k, sha256, msg, hex(SIG_256)) catch return 2;
    print("sha-256 verifies");
    rsa.verify_pkcs1(k, hash.sha384_hash(), msg, hex(SIG_384)) catch return 3;
    print("sha-384 verifies");
    rsa.verify_pkcs1(k, hash.sha512_hash(), msg, hex(SIG_512)) catch return 4;
    print("sha-512 verifies");
    // A different width, so that nothing above is a limb count written twice.
    const k3 = rsa.public_key(hex(N_3072), hex(E)) catch return 5;
    rsa.verify_pkcs1(k3, sha256, msg, hex(SIG_3072)) catch return 6;
    print("a 3072-bit key verifies");

    const sig = hex(SIG_256);
    refuse(k, sha256, msg, flip(sig, 100), "a flipped signature byte refused");
    refuse(k, sha256, flip(msg, 3), sig, "a flipped message byte refused");
    refuse(k, sha256, msg, hex(SIG_OTHER), "another key's signature refused");
    // The digest and the DigestInfo prefix both change with the hash, so a
    // signature read under the wrong one fails twice over.
    refuse(k, hash.sha384_hash(), msg, sig, "the wrong hash refused");
    refuse(k, sha256, msg, bytes.slice(sig, 0, 255), "a short signature refused");
    refuse(k, sha256, msg, bytes.concat(sig, bytes.new(1)), "a long signature refused");
    // The modulus is 256 bytes and is not below itself, so it is refused
    // before any exponentiation happens.
    refuse(k, sha256, msg, hex(N), "a signature at the modulus refused");
    refuse(k, sha256, msg, hex(SIG_SHORT_PADDING), "eight bytes of padding and a tail refused");
    refuse(k, sha256, msg, hex(SIG_BAD_PADDING), "a padding byte that is not 0xff refused");
    refuse(k, sha256, msg, hex(SIG_BAD_BLOCK_TYPE), "the wrong block type refused");

    // And the key itself, which is the first thing a certificate hands over.
    bad_key(flip(hex(N), 255), hex(E), "an even modulus refused");
    bad_key(bytes.new(32), hex(E), "a tiny modulus refused");
    bad_key(hex(N), bytes.new(3), "an even exponent refused");
    // Not wrong, just expensive: `modexp` is one modular multiplication per
    // exponent bit, so this is a peer deciding how much work we do.
    bad_key(hex(N), hex("0100000000000001"), "an exponent above 2^32 refused");
    return 0;
}

fn refuse(k: rsa.PublicKey, h: hash.Hash, msg: []u8, sig: []u8, note: str) void {
    rsa.verify_pkcs1(k, h, msg, sig) catch |e| {
        if (e == error.BadSignature) { print(note); }
        return;
    };
    return;
}

fn bad_key(n: []u8, e: []u8, note: str) void {
    const k = rsa.public_key(n, e) catch |raised| {
        if (raised == error.BadKey) { print(note); }
        return;
    };
    return;
}

fn flip(b: []u8, at: i64) []u8 {
    const out = bytes.slice(b, 0, array.len(b));
    out[at] ^= 1;
    return out;
}

fn hex(s: str) []u8 { return bytes.from_hex(s) catch bytes.new(0); }
