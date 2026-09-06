// RSASSA-PSS signature verification -- what TLS 1.3 signs its
// `CertificateVerify` with, and what a modern certificate is signed with.
//
// Made and cross-checked exactly as the PKCS#1 v1.5 vectors were: every
// encoded message here, valid or forged, was built from RFC 8017 section 9.1
// and signed with the raw private exponent, and a second implementation was
// then asked about each one and agreed about all of them.
//
// PSS has more ways to be wrong than v1.5 has, because it has more structure:
// a trailer byte, a salt whose length the verifier has to be told, a separator
// inside the masked block, and the bits by which the encoded message is
// shorter than the modulus, which have to be zero. Each gets a line, because
// each is a separate check and a verifier missing any one of them still
// verifies every honest signature.
// expect: sha-256 with a 32-byte salt verifies
// expect: sha-256 with no salt verifies
// expect: sha-384 with a 48-byte salt verifies
// expect: a 3072-bit key verifies
// expect: a flipped signature byte refused
// expect: a flipped message byte refused
// expect: another key's signature refused
// expect: the wrong salt length refused
// expect: the wrong hash refused
// expect: a trailer that is not 0xbc refused
// expect: an uncleared leading bit refused
// expect: a separator that is not 0x01 refused
// expect: a signature at the modulus refused
const rsa = @import("std/rsa");
const hash = @import("std/hash");
const bytes = @import("std/bytes");
const array = @import("std/array");

const N = "a89315af907b0fcd2c0244d980c65b09417078750d5aab2444a1513aac6c48dceab736a4c799345f1feaee904180269d48cac127b83f2e993bbf6b19c77e74a5c7d0dc6654743a9e7fecbeb3cc08052f441e78d0029fdbd42b0ff632093a2ea344dcfaf485041fc90118d626aa24889fef3453850c199375dacaf42b008026877580e72a5900585a6c8b1297329f24305099a911bad082ee3bf70bb73ae1630e0741673250e35639156bd60ee62fa15e9867b227e7223684e096d35b1d211848f20ef78ff541b8c5185a1560221539f487c7b63c703fd464ab4bc9289ee9f0e8b2979a4d1891b48809a9ad1167bd17c83a537d418e5507c35744a4d924aa8fc9";
const E = "010001";
const N_3072 = "c452dd14e5edd69f2f63abbb8f0092fe38ee33c1ea7b06d784671321dafe54b50468ad0ba44696cd198a0e9ef5badf599a8148da0b95e4cf851523c74fccc3f33fbd263fb4f8b1707b203393a7031446b2183efc6e089c3fd78ae4643de0385421ea657de635422b80fe561ef34eaedf3b2256a0a8741710dd063d41d1798f2ef6446e85821f314d7e54071c92bd67d4b10a75e8efc3f936ad6fc3aa3967c1c9acd66a73e71a47d0e1639565fd9685f0a9c72971352dd856eb2e65431b6fb584739bbc1fed4e1ac008403e6674b1771494b3e35e399eaab3d31147bbe189c72b48aac09831dd10146ab6960641f3d3efe7f5977283471ea7bee525ba02a61b528efdeb5dc4afd40b97a6bed29927a10e379be3ef37d6c0f149edfd19db3ac197c435c0d0346d6fd23f42e8e24d2117b03ca4db440ac87878d9151ed6ec189959dd0d2591c926b4ac7a00db5adea755ff5062bc6581b76dcfff8c98632b65536c128f5bc79c8ea6e27e4faf3098eaad074cbe92181d1ee560c6f4a4658941f0a7";

const MSG = "5723206974656d2031302073746167652074687265653a20525341207369676e617475726520766572696669636174696f6e2e";
const SIG_S32 = "24b27e51d8a97e8386a0811f5ffca73fba26c6720e895eb15e387545451ce49964fd3be07a753b44eb75a89feb1e9052f54327d062038aeb5ef2d1fa1fbdfb3118364f62b60142dce70e990ce72573e0f49b94dd08b1617000d8add41ab801eb5f466f6556119bf53ea99c841adbcdab7082b1cd52e91afddf24974510fd80f3ef24f66013bd442ef6b9508797f0e0169b5f34e0613fbc6d8f08af9e86a544469ca793bdc75a05ec7d189bf63ac3f33c9b796a3749a5f4bc4b11fb9b1922643d013a4082b3d0189285eb98848e34af24e8fbfad29bfe8d8c24855c655203e915f81993911c2c75e8670eb01780e9d5f847955b6bb1eaf0d3e032b8beb844f4e5";
const SIG_S0 = "8dd3daf16596fc2a67be9ef91b43381f8d83c5a664b9f59df3e9c1d4912daadb3badddab229f1cd7856fd0db9965cfc704ec86ea1e0bb1198752b968e6df1b062dbaafd4e2bc567a6167ee1e9b4f6b5c1bbc55707f64433964a1cd23113600369475cafd26cb41972405c7300747e7396b3c2c1e47d1c81edd1a704b9927759c70d8f978743b660a35491698bc8b7dc3118d75a889257dcec83c3487516781877de114a8f947d66e2b82b2e6d539e94df095c9a39428489943b82fb1b43846f4fb15e00765a01b05d6554b5876d2a53c404d72ba95bded72b9ef0a1386d3eee0f0a89541de7abe3288a5054ae1692c1a8a4e08860bfc984661ba9596dd308162";
const SIG_384 = "25c1b04fb4c5a7e920dc8099a9f1b2446fd8f846f07304903941de4cf02a9448410cebdcd1a72d3a5f83ee59c1ba6429c6f9157cedcb0a30826a2c5e4adbad0daff003b9fedc5d8fa41b4a0bda93757ef1023f253c9964c279631b428edb9c300a3feeaf205fb3798505ab6f741d5f8c85baa2ed26a30fc12e4b0628ee6172868cb1fa06dd68053b9f0203d007a12aa39be7cdc291b7bdfea5a8e830a440b3efb75d416781ab86a37bb02dbc87568119e8c5bbaaa0cf532d742b42bf7b888e4b5c2f62fee6988b982184eb490402ae30845fe2cbf0a4f488e3f9e5a9883f3080fc3c179df5ee26184d379a38bd0690549ca33cb955dfad8219df1f2c37ac3532";
const SIG_3072 = "9cf603e59acfa168a3b630fa9eeaef85dbc7f5dd8459afd97432edfb4dfcf6e277e1f34863d96bb7d2516d7951e0637b621b30daf8862d3705dfd52ff433772a64c3a5208f58faf120fdcd0bec8f651d2e873eedd9311b3a5b3a0038d7788cafb729263e102107bf11e660c1bfafbc69d6b3baea0885216f74abbdc75d93ffbbb16495d08f14b3c42f582859bf018e0e3546cbf9a5b21d88033535b3584c6a00325ff3dfb59890682ec6c4f7db3b16b1154dbe69f9a1900b19a78bd5052490987580134f0d9e994991386925d0225d4fa727b7baa486d1d47f31ad7688aab5ff4a2a9b21806936406623cd837000cdedadeab6a24250272ad744ae2414df3419dd0617884845a42deaad424aeff53df48f73fcd0507c05751954e856518e99f9def14d93747c3ec76430003ce84df122ef1cbcecf37fddeb42c25d788fceadb547e51c0a943f7eab724540e85c4daed38b56b637a1255f48285b26e3c6a3d2ad6c00dd342a904f2463ecdf8efac70d90a9938c31cf36338271994f7eb7ebcc76";
const SIG_OTHER_KEY = "8d81e3094546d8cbe577c4120ddc1434b45e416fd1e15b0dca7bd1b07669ffaa52f7877367ac7b294cfcd3ecc06e4efd67b667111bb9ee2f770573e7cfa8413fbe5ca9f90a267e936d7ad442cac9cf6ab79ccde7f3e35729f7adfb2b29491c79f8d12cfd6c0eb228eaad58691832bd73a0dcdac87fea8896dca2974367c58a9f73b3359b5ad6dd344b88506b9333f36c9e2feef01544d5e3b4192ebb0f6cfd6cfb1e605a41fa189aecf5b8fe1f49cde37be672bfd2ba566c3e2776ed508cda2f082e16ba131bf927db204de4e819a6ece566248e495cbc5cb3615239dfbf7d42255ee7235316124b44e8bd20e88ada4609df7713b1b96f01f88a5e477a961823";
const SIG_BAD_TRAILER = "238343d2530ba919bae63ebfd90f3f92fed312ed2aab198554114d4a8f84b3963dbcbccdb45cdcc777b96b1f5053bbd2080dfa71f84b98fffa852a3b3465d20f9a036b0da11f6d7b784f9fac569b479e04f44ae9078af2ef7506656eb7cd718b046c00b6a1db90548fd90195fd02a7fdfb9245396fed7976d02e64c1a04dd8776dcb2ffd22f24215e8a566f2145bce90c61076d6357e77338df086ebc6198d614044a61cf7326c4198bed71f3e191b06b6366715c2984a9c6597662964eacb61f2400889177530cea5b460ea4256cb4f0c9e90051da9a8d7f7b0d91ab029583d0a1e8fc4b028e4f7a4522bcf4b4445c72ad66e32a8fb52ce1b42ef6633fb2def";
const SIG_DIRTY_TOP = "19d0a32e28114857f1fe0f3688cb2990d50b211d46c332e61a408fdd150ba1400b55cbab956c552c3262e6dda5da30c7ad6695bf97437960dabe485836fa3ef36376dbe406689a8f483b53863382a68e33b8b660ca86d49a54c7da99f5fe3373ee75169dc7b1ac85bee8308d8bd8ce7f09cbfeadd532136fc913d26c2afc613d64542ab2a8e32b37eb07f3188e496bc250105dc19603c5e9aae32cad048aee375082a5b75cc8d678dbcee769264ff68fd8b543f7cc337aa20685235d2764771cfbd149a698bbe3827481cb3a6a5ca25b69f51b589fce85bb0899140772e8c47a549695b992274682f92bd4aae767d4ec4d3deb074dd5750de11f4ac6537e8e73";
const SIG_BAD_SEPARATOR = "48df872527d30145e0872cad4a686d951157555da5e7954a67576db4b36f30f2ddaa97f641e693cb08d9c7675c77736a8f2aac3cdfce761455c82a8a9b4ceb2fb1bd1d1d31cc52b181daff71f0ea5a21a2501cb2db12e700ab59310a8a068cd0304efc6893d631507949f8706db252b7918a45e5d5e535ee6667e2ccca8916516580a89df07cc2a350f236c1656b1453978c198135d0c2051b9d380ef95183646c04487edb495ff7df6d17abf8730b1e717774af6e094c8af99704289ab0739f53b5764e71ac77728242b97fa34ab5e34624f79543d65588154ae337e4c108737026af73cc937870ecb404997aab6171cbc5848c69ae346e231c5d97b2b53a6c";

fn main() i64 {
    const msg = hex(MSG);
    const sha256 = hash.sha256_hash();
    const k = rsa.public_key(hex(N), hex(E)) catch return 1;

    rsa.verify_pss(k, sha256, msg, hex(SIG_S32), 32) catch return 2;
    print("sha-256 with a 32-byte salt verifies");
    // A zero-length salt is legal and makes PSS deterministic, which is the
    // case where the salt-length argument stops being cosmetic.
    rsa.verify_pss(k, sha256, msg, hex(SIG_S0), 0) catch return 3;
    print("sha-256 with no salt verifies");
    rsa.verify_pss(k, hash.sha384_hash(), msg, hex(SIG_384), 48) catch return 4;
    print("sha-384 with a 48-byte salt verifies");
    const k3 = rsa.public_key(hex(N_3072), hex(E)) catch return 5;
    rsa.verify_pss(k3, sha256, msg, hex(SIG_3072), 32) catch return 6;
    print("a 3072-bit key verifies");

    const sig = hex(SIG_S32);
    refuse(k, sha256, msg, flip(sig, 100), 32, "a flipped signature byte refused");
    refuse(k, sha256, flip(msg, 3), sig, 32, "a flipped message byte refused");
    refuse(k, sha256, msg, hex(SIG_OTHER_KEY), 32, "another key's signature refused");
    // A signature is valid only for the salt length it was made with, which is
    // why that length is the caller's to state rather than the signature's to
    // claim.
    refuse(k, sha256, msg, sig, 0, "the wrong salt length refused");
    refuse(k, hash.sha384_hash(), msg, sig, 32, "the wrong hash refused");
    refuse(k, sha256, msg, hex(SIG_BAD_TRAILER), 32, "a trailer that is not 0xbc refused");
    refuse(k, sha256, msg, hex(SIG_DIRTY_TOP), 32, "an uncleared leading bit refused");
    refuse(k, sha256, msg, hex(SIG_BAD_SEPARATOR), 32, "a separator that is not 0x01 refused");
    refuse(k, sha256, msg, hex(N), 32, "a signature at the modulus refused");
    return 0;
}

fn refuse(k: rsa.PublicKey, h: hash.Hash, msg: []u8, sig: []u8, salt: i64, note: str) void {
    rsa.verify_pss(k, h, msg, sig, salt) catch |e| {
        if (e == error.BadSignature) { print(note); }
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
