#!/usr/bin/env sh
# Render every template under docs-src/ to its output path at the workspace
# root, using `standardoc transform`. Run from the repo root.
#
# Usage : ./scripts/render-docs.sh
#         (use --release for the production binary, default debug for speed)

set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CARGO_FLAGS="${CARGO_FLAGS:---release}"
BIN="cargo run --quiet $CARGO_FLAGS -p standardoc --"

if [ ! -d docs-src ]; then
    echo "render-docs: no docs-src/ folder found at $ROOT — nothing to render."
    exit 0
fi

# shellcheck disable=SC2044
for template in $(find docs-src -type f -name '*.md'); do
    out="${template#docs-src/}"
    echo "  rendering $template -> $out"
    mkdir -p "$(dirname "$out")"
    $BIN transform . "$template" > "$out"
done

echo "render-docs: done."
