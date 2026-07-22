#!/usr/bin/env bash
# Regenerates THIRD_PARTY_NOTICES.md from the two lockfiles.
#
# Requirements: cargo-about (`cargo install cargo-about --locked --features cli`)
# and an installed frontend/node_modules (`npm ci` in frontend/).
#
# CI runs this and fails if the result differs from what is committed, so the
# notices file can never silently fall behind a dependency bump.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! cargo about --version > /dev/null 2>&1; then
  echo "cargo-about is not installed: cargo install cargo-about --locked --features cli" >&2
  exit 1
fi

out=THIRD_PARTY_NOTICES.md
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

{
  cat <<'HEADER'
# Third-Party Notices

ronten itself is distributed under the MIT license (see `LICENSE`). The
binary statically links the Rust dependencies listed below and embeds a
compiled frontend bundle containing the JavaScript and font assets listed
after them. Their licenses and copyright notices are reproduced here.

This file is generated — do not edit it by hand. Run `scripts/gen-third-party.sh`
after changing `Cargo.lock` or `frontend/package-lock.json`.

## Rust dependencies

HEADER

  # --frozen (= --locked --offline) resolves every license text from the
  # local crate sources only: the clearlydefined.io fallback would make the
  # output depend on a network service, which a byte-for-byte drift check
  # cannot rely on. --fail turns an unresolvable license into an error
  # instead of a silently missing notice.
  cargo about generate --frozen --fail about.hbs

  cat <<'HEADER'

## Bundled frontend assets

HEADER

  node scripts/js-notices.mjs
} > "$tmp"

mv "$tmp" "$out"
echo "wrote $out"
