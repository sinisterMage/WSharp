//! The one fact about this build that the source cannot state: what it is
//! being built *for*.
//!
//! `std::env::consts` answers with `ARCH` and `OS` separately and never with a
//! vendor or an environment, so there is no way to spell `x86_64-unknown-linux-gnu`
//! from inside the crate. It could be reassembled from a dozen `cfg!` arms --
//! `target_env = "musl"`, `target_env = "msvc"`, a vendor chosen per OS -- but
//! that is a table of things to be wrong about, and cargo already knows the
//! answer and puts it in `TARGET`.
//!
//! This is why the crate that declares no dependencies has a build script. It
//! adds no code and reads no files; it copies one environment variable.
//!
//! What reads it: `os.target()`, which is how a program written in W# picks
//! which release artifact belongs to the machine it is running on. That is a
//! version manager's first question, and the triple has to be spelled the same
//! way the thing that built the artifact spelled it.

fn main() {
    // Set by cargo for every build script, and is the triple `--target` named
    // or the host's when it named none.
    let target = std::env::var("TARGET").expect("cargo sets TARGET for a build script");
    println!("cargo::rustc-env=WSHARP_TARGET={target}");
    // Without this, cargo reruns the script whenever anything in the package
    // changes, which for this crate is most commits.
    println!("cargo::rerun-if-changed=build.rs");
}
