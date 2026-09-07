// DEFLATE and the zlib wrapper, against streams Python's `zlib` produced --
// which is the rule every cryptographic case here follows and is worth
// following for a compressor too: a stream written by the same code that reads
// it proves only that the code agrees with itself.
//
// One of each block type, and the two cases a decoder gets wrong. A back
// reference whose distance is shorter than its length is how a run is encoded,
// and copying it as a block move rather than a byte at a time gives the wrong
// answer; and the reader must say how many *input* bytes it consumed, because
// a packfile is a concatenation of these and nothing else says where the next
// one starts.
// expect: stored: 43
// expect: fixed: 20
// expect: dynamic: 4000
// expect: run: 10000
// expect: empty: 0
// expect: truncated: refused
// expect: a wrong checksum: refused
// expect: a bad header: refused
// expect: nothing at all: refused
const array = @import("std/array");
const bytes = @import("std/bytes");
const hash = @import("std/hash");
const inflate = @import("std/inflate");
const text = @import("std/str");

fn check(name: str, hex: str, want_len: i64, want_sha: str) void {
    const src = bytes.from_hex(hex) catch {
        print(text.concat(name, ": the fixture is not hex"));
        return;
    };
    const out = bytes.buf(64);
    const consumed = inflate.zlib(src, 0, out) catch {
        print(text.concat(name, ": refused"));
        return;
    };
    const got = bytes.taken(out);
    var line = name;
    line = text.concat(line, text.concat(": ", text.from_int(array.len(got))));
    if (array.len(got) != want_len) { line = text.concat(line, " WRONG LENGTH"); }
    if (!text.eq(bytes.to_hex(hash.sha1(got)), want_sha)) { line = text.concat(line, " WRONG BYTES"); }
    // The whole fixture is one stream, so the cursor must land on its end.
    if (consumed != array.len(src)) { line = text.concat(line, " WRONG CURSOR"); }
    print(line);
    return;
}

fn refuses(name: str, hex: str) void {
    const src = bytes.from_hex(hex) catch {
        print(text.concat(name, ": the fixture is not hex"));
        return;
    };
    const out = bytes.buf(64);
    const n = inflate.zlib(src, 0, out) catch {
        print(text.concat(name, ": refused"));
        return;
    };
    print(text.concat(name, ": ACCEPTED"));
    return;
}

fn main() i64 {
    check("stored", "7801012b00d4ff74686520717569636b2062726f776e20666f78206a756d7073206f76657220746865206c617a7920646f67613c0ffa", 43, "16312751ef9307c3fd1afbcb993cdc80464ba0f1");
    check("fixed", "78da4b4c8481243800004fdd079f", 20, "c9ba0f7d724228c8b6a410f87135d379da33eb87");
    check("dynamic", "78daedd5575b0e000080d194424994112d51c98a0a2d540a6d14b2ca6a08a5a225346989360d4554b64228a46456940a4546d3ac502aae5cbcbfc2f37c3fe15c1d21558b5d4945ef8769d8f99fb8ff495a6743486ee50f3923e7e82bf57f54cc3d126fbd1b32d3d62fa3ac63e4bcf5c13915dde30d9da22ed70d4c32734fb8d924367d856f7a69bbd49c7541679e76c92ed81a79a9b65f79c9cef81b6f45a72df349bbd736427bcd81d34f3ac7cddf1c71f145dfc4c5dbe30adf0c9e6ab33bb5a45552cb7e5ff6e3ef630d361dbc50f35bc9d4edc8f54611756bef63775b866bae0e3cf9e8db18fd8de1e7aa7b154db6c55e6d109e62e59572bb5962f6aabd590fbe8ed6730c3bfbac476191ebe1825783d42c3d938b3f88cf5a199059fe5946d72134afeaa7bcb14b4cfecbbff086c21b056f02bcc9f066c09b0b6f21bca5f096c35b0b6f0bbc1df0f6c0db0fef10bca3f08ec33b05ef3cbc6bf0eec07b08ef39bcd7f03ec2fb02ef173c2178c3e049c39383a7026f26bc79f00ce199c15b016f1dbcadf076c2f38177005e04bc3878a9f0b2e15d80771dde5d788fe055c36b80d70cef2bbc1e7883e089c39381270f4f159e063c1d7846f0cce1d9c25b0fcf099e3b3c5f7841f022e1c5c34b83771ade457885f04ae03d865703af115e0bbc6ff07ae109c39380371a9e023c3578b3e0e9c233866701cf0ede0678cef03ce0f9c10b8617052f015e3abc33f02ec1bb01ef1ebc27f05ec07b03af15de7778bfe189c01b0e6f0c3c457853e0cd86a7076f113c4b782be139c07381b70b9e3fbc1078d1f012e165c0cb817719de4d78a5f09ec2ab85f7165e1bbc4e787df006c3938437169e123c75789af0f4e199c0b382b70a9e233c57789ef002e085c28b819704ef04bc5c7857e0dd825706af025e1dbc2678edf0bae0f5c313853702de387813e14d85a705cf009e293c6b78abe16d84b70d9e17bcbdf0c2e01d86970c2f135e1ebc7c7845f0eec3ab84570fef1dbc0e78ddf006e089c19382270b4f19de3478daf0e6c35b0ccf069e3dbc4df0dce079c30b84170e2f165e0abc2c7867e115c02b86570eaf0ade4b21416e82dc04b9097213e426c84d909b20b7ff2fb77ff38ba1cd", 4000, "f471b91c2140df8b2b4bf772d1999d74b725a461");
    check("run", "78daedc2010900000002a0adf57f443b02d134000000000000000000706f2e5de16b", 10000, "6db574e55bf713c1ef9caf119d45c2f8bdd11aec");
    check("empty", "78da030000000001", 0, "da39a3ee5e6b4b0d3255bfef95601890afd80709");

    refuses("truncated", "78da2bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd");
    refuses("a wrong checksum", "78da2bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd2d2856c82f4b2d5228014ae72456552aa4e4a70300613c0f05");
    refuses("a bad header", "79da2bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd2d2856c82f4b2d5228014ae72456552aa4e4a70300613c0ffa");
    refuses("nothing at all", "");
    return 0;
}
