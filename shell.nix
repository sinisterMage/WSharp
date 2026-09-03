# Development shell for W#.
#
# rustc invokes `cc` as its linker driver, and a bare NixOS environment does not
# have one on PATH -- without this shell, `cargo build` succeeds for library
# crates but fails to link tests and binaries with "linker `cc` not found".
#
# Usage:
#   nix-shell               # interactive shell with the toolchain on PATH
#   nix-shell --run "cargo test --workspace"
{
  pkgs ? import <nixpkgs> { },
}:

pkgs.mkShell {
  name = "wsharp";

  nativeBuildInputs = [
    # Provides `cc`, `ld` and the crt objects rustc needs to link.
    pkgs.stdenv.cc
  ];

  # rustc and cargo are expected from the ambient system (this project targets
  # 1.95 / edition 2024, matching `rust-toolchain.toml`). To make the shell
  # fully self-contained instead, add `pkgs.rustc` and `pkgs.cargo` above.

  shellHook = ''
    echo "wsharp dev shell: $(rustc --version 2>/dev/null || echo 'rustc not found'), cc=$(command -v cc)"
  '';
}
