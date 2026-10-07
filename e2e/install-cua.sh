#!/usr/bin/env bash
# Downloads the pinned cua-driver release for this machine into
# target/e2e/cua-driver/, checking it against the sha256 pinned below. The
# hashes are recorded here rather than read from the release's SHA256SUMS so
# a replaced release asset cannot vouch for itself; update them with VERSION.
set -euo pipefail

VERSION=0.34.0
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DEST=${1:-$ROOT/target/e2e/cua-driver}

case $(uname -m) in
  x86_64)
    arch=x86_64
    sha256=629ac96eff829d4dfd5cf221f3f2165c2d813aed91e5efb7b20777a741cd70a7
    ;;
  aarch64 | arm64)
    arch=arm64
    sha256=9db8b9084add57eb97be8164367b24b6be54ed4f3dc01213e64b72d7fc09fddb
    ;;
  *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
asset=cua-driver-rs-$VERSION-linux-$arch-binary.tar.gz
base=https://github.com/trycua/cua/releases/download/cua-driver-rs-v$VERSION

if [[ -x $DEST/cua-driver ]] && "$DEST/cua-driver" --version | { read -r line; [[ $line == "cua-driver $VERSION" ]]; }; then
  exit 0
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL --retry 3 -o "$work/$asset" "$base/$asset"
echo "$sha256  $work/$asset" | sha256sum -c -
rm -rf "$DEST"
mkdir -p "$DEST"
tar -xzf "$work/$asset" -C "$DEST"
"$DEST/cua-driver" --version
