# The image CI runs in.
#
# Forgejo executes a JavaScript action -- `actions/checkout`, `actions/cache` --
# by running `node` *inside the job container*, not on the runner host. An image
# without one fails at the very first step with
#
#     exec: "node": executable file not found in $PATH
#
# so a Rust image alone cannot serve. Debian bookworm's own `nodejs` package is
# 18, older than the node20 runtime those actions declare, which is why node is
# copied from the official image here rather than installed with apt.
#
# Build and publish with `.forgejo/ci-image/push.sh`.

FROM node:22-bookworm-slim AS node

# The version `rust-toolchain.toml` asks for, so rustup has nothing to download
# at job time. Not `-slim` and not `-alpine`: rustc shells out to `cc` to link,
# and an image without one lets library crates compile and then fails the tests
# and binaries at link time -- the trap CLAUDE.md describes.
FROM rust:1.95-bookworm

# Both stages are bookworm, so this binary links against the libc and libstdc++
# already present here. npm and npx are not needed by the actions themselves,
# but an action that shells out to them is a confusing failure otherwise.
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

# The official Rust image installs rustup's *minimal* profile, which leaves out
# both of these -- `cargo fmt` there reports the component is not installed. The
# `lint` job needs them, and `rust-toolchain.toml` asks for them, so bake them in
# rather than have every job fetch them over the network.
RUN rustup component add rustfmt clippy

# Fail the build, rather than the first CI job, if any of this is missing.
RUN node --version \
 && npm --version \
 && rustc --version \
 && cargo fmt --version \
 && cargo clippy --version \
 && cc --version | head -1 \
 && git --version
