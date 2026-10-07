#!/usr/bin/env bash
# Downloads the pinned cua-driver release for this machine into
# target/e2e/cua-driver/, checking it against the release's SHA256SUMS.
set -euo pipefail

VERSION=0.34.0
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DEST=${1:-$ROOT/target/e2e/cua-driver}

case $(uname -m) in
  x86_64) arch=x86_64 ;;
  aarch64 | arm64) arch=arm64 ;;
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
curl -fsSL --retry 3 -o "$work/SHA256SUMS" "$base/SHA256SUMS"
(cd "$work" && awk -v a="$asset" '$2 == a || $2 == "*"a' SHA256SUMS | sha256sum -c -)
rm -rf "$DEST"
mkdir -p "$DEST"
tar -xzf "$work/$asset" -C "$DEST"
"$DEST/cua-driver" --version
