#!/usr/bin/env bash
# Publishes signed syntax packs to the pack host, an nginx directory served
# as $PACK_BASE_URL behind Cloudflare, over SSH.
#
#   publish.sh PACKS_DIR
#
# PACKS_DIR holds <target>/index.json and the packs, as the Syntax packs
# workflow's sign job leaves them (`syntax-pack index --base-url`). Order:
#
# 1. `syntax-pack verify` every target's index: signed by $PACK_INDEX_KEY,
#    every pinned language, local files matching, immutable URLs. Nothing
#    is uploaded unless every target passes.
# 2. Upload each file to the path its URL names, never replacing a file
#    already there. URLs carry the file's SHA-256, so an existing file
#    should already be identical.
# 3. Download every file through the public URL and check its SHA-256.
# 4. Only then replace each <target>/index.json, atomically on the host,
#    and check the public copies.
#
# Environment: SYNTAX_PACK (the tool binary), PACK_BASE_URL, PACK_INDEX_KEY,
# TARGETS (space-separated), PACK_SSH_DEST (user@host), PACK_REMOTE_DIR
# (the directory served as PACK_BASE_URL), and optionally
# PACK_SSH_KEY_FILE and PACK_SSH_KNOWN_HOSTS_FILE. PACK_DRY_RUN=1 stops
# after step 1 and prints the URLs it would upload.
set -euo pipefail

packs=${1:?usage: publish.sh PACKS_DIR}
: "${SYNTAX_PACK:?}" "${PACK_BASE_URL:?}" "${PACK_INDEX_KEY:?}" "${TARGETS:?}"
base=${PACK_BASE_URL%/}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

for target in $TARGETS; do
  "$SYNTAX_PACK" verify --out "$packs" --target "$target" \
    --key "$PACK_INDEX_KEY" --base-url "$base" >"$work/$target.tsv"
  echo "$target: $(wc -l <"$work/$target.tsv") files verified"
done

stage=$work/stage
for target in $TARGETS; do
  while IFS=$'\t' read -r file url _; do
    rel=${url#"$base"/}
    mkdir -p "$stage/$(dirname "$rel")"
    cp "$file" "$stage/$rel"
  done <"$work/$target.tsv"
done

if [ "${PACK_DRY_RUN:-}" = 1 ]; then
  for target in $TARGETS; do cut -f2 "$work/$target.tsv"; done
  exit 0
fi

: "${PACK_SSH_DEST:?}" "${PACK_REMOTE_DIR:?}"
remote=${PACK_REMOTE_DIR%/}
ssh_cmd=(ssh -o BatchMode=yes -o StrictHostKeyChecking=yes)
if [ -n "${PACK_SSH_KEY_FILE:-}" ]; then
  ssh_cmd+=(-i "$PACK_SSH_KEY_FILE" -o IdentitiesOnly=yes)
fi
if [ -n "${PACK_SSH_KNOWN_HOSTS_FILE:-}" ]; then
  ssh_cmd+=(-o "UserKnownHostsFile=$PACK_SSH_KNOWN_HOSTS_FILE")
fi
rsync_ssh="${ssh_cmd[*]}"

rsync -rt --ignore-existing --chmod=D755,F644 -e "$rsync_ssh" \
  "$stage/" "$PACK_SSH_DEST:$remote/"

sha_of_url() {
  curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors "$1" | sha256sum | cut -d' ' -f1
}

for target in $TARGETS; do
  while IFS=$'\t' read -r _ url sha; do
    served=$(sha_of_url "$url")
    if [ "$served" != "$sha" ]; then
      echo "::error::$url serves SHA-256 $served, the index lists $sha"
      exit 1
    fi
  done <"$work/$target.tsv"
  echo "$target: every pack file is served"
done

for target in $TARGETS; do
  tmp=".index.json.$$"
  rsync -t --chmod=F644 -e "$rsync_ssh" \
    "$packs/$target/index.json" "$PACK_SSH_DEST:$remote/$target/$tmp"
  "${ssh_cmd[@]}" "$PACK_SSH_DEST" \
    "mv -f $(printf %q "$remote/$target/$tmp") $(printf %q "$remote/$target/index.json")"
done

for target in $TARGETS; do
  expected=$(sha256sum <"$packs/$target/index.json" | cut -d' ' -f1)
  served=$(sha_of_url "$base/$target/index.json")
  if [ "$served" != "$expected" ]; then
    echo "::error::$base/$target/index.json is not the index just published"
    exit 1
  fi
  echo "$target: published $base/$target/index.json"
done
