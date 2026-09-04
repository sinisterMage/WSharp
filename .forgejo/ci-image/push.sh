#!/bin/sh
# Build the CI image and publish it to this instance's container registry.
#
# The runner does not live on the same machine as Forgejo, so a locally built
# image is not enough -- it has to sit somewhere both can reach. Forgejo's own
# container registry is that place, and `.forgejo/workflows/ci.yml` names the
# image by exactly the tag pushed here.
#
# Log in first with a token carrying the `write:package` scope:
#
#     docker login forgejo-hagc.srv1954822.hstgr.cloud -u <user>
#
# Bump TAG when the Rust version in `rust-toolchain.toml` moves, and change the
# workflow to match in the same commit, so a job never runs on a toolchain the
# repository is not asking for.
set -eu

REGISTRY=${REGISTRY:-forgejo-hagc.srv1954822.hstgr.cloud}
OWNER=${OWNER:-ofekbickel}
TAG=${TAG:-rust-1.95}
IMAGE="$REGISTRY/$OWNER/wsharp-ci"

cd "$(dirname "$0")"
# `--provenance=false`: buildx otherwise adds an attestation manifest, turning a
# plain image into an index carrying an `unknown/unknown` platform entry, which
# registries outside Docker Hub tend to reject or mis-report.
docker build --provenance=false -t "$IMAGE:$TAG" -t "$IMAGE:latest" .
docker push "$IMAGE:$TAG"
docker push "$IMAGE:latest"

echo "published $IMAGE:$TAG"
