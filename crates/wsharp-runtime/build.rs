//! Bake the W# platform name into the runtime for `os.target()`. Cargo's
//! target supplies the default. Joe uses Rust's Android target for its
//! bionic ABI and selects its distribution name with the joe-linux feature.

fn main() {
    // Set by cargo for every build script, and is the triple `--target` named
    // or the host's when it named none.
    let target = std::env::var("TARGET").expect("cargo sets TARGET for a build script");
    let name = if target == "aarch64-linux-android" && cfg!(feature = "joe-linux") {
        "aarch64-joe-linux-bionic"
    } else {
        &target
    };
    println!("cargo::rustc-env=WSHARP_TARGET={name}");
    // Without this, cargo reruns the script whenever anything in the package
    // changes, which for this crate is most commits.
    println!("cargo::rerun-if-changed=build.rs");
}
