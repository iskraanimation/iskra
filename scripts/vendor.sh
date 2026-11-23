#!/usr/bin/env bash
# Copies the vendored ArtCraft crates into crates/third_party/ at the pinned commits below.
#
#   scripts/vendor.sh [ARTCRAFT_DIR]     (default: ../ArtCraft, with one clone per repo inside)
#
# Each crate's sources are copied unchanged, then the patches in scripts/patches/ (Iskra's own
# changes, each saying why) are applied: the script stops if one no longer applies, to be redone
# against the new upstream code or dropped. Its Cargo.toml is ours (the package is renamed to
# iskra-<repo>-<crate>, see ATTRIBUTION.md) and is never overwritten. To take upstream changes:
# bump the commit here, run the script, review `git diff`, update ATTRIBUTION.md.
set -euo pipefail

PHOTOCRAFT_COMMIT=ff4738810134d7f7e68b49f2e4f231b686282ff3
EFFECTCRAFT_COMMIT=9581b53c3f8945683252af0eb5fac79e0e43aed4
FILMCRAFT_COMMIT=abf44fda9015f4b7a96dc338f6042499bf8e9d9f
VECTORCRAFT_COMMIT=4a7cba0dd32e4ac249e4dc50af486b89531481aa

PHOTOCRAFT_CRATES="geom cms color raster paint tablet psd"
EFFECTCRAFT_CRATES="time geom color keyframe raster"
FILMCRAFT_CRATES="bitstream testkit h264enc prores aac isobmff"
VECTORCRAFT_CRATES="geom pathops trace"

root="$(cd "$(dirname "$0")/.." && pwd)"
artcraft="${1:-$root/../ArtCraft}"

vendor() {
    local repo="$1" commit="$2" crates="$3"
    local src="$artcraft/$repo" dest="$root/crates/third_party/$repo"
    git -C "$src" cat-file -e "$commit^{commit}" 2>/dev/null || { echo "$src has no commit $commit (clone or fetch it)" >&2; exit 1; }
    mkdir -p "$dest"
    for f in LICENSE-MIT LICENSE-APACHE NOTICE; do
        git -C "$src" show "$commit:$f" > "$dest/$f"
    done
    for c in $crates; do
        local tmp
        tmp="$(mktemp -d)"
        git -C "$src" archive "$commit" "crates/$c" | tar -x -C "$tmp"
        rm "$tmp/crates/$c/Cargo.toml"
        mkdir -p "$dest/$c"
        find "$dest/$c" -mindepth 1 -maxdepth 1 ! -name Cargo.toml -exec rm -rf {} +
        cp -R "$tmp/crates/$c/." "$dest/$c/"
        rm -rf "$tmp"
        echo "$repo/$c @ ${commit:0:12}"
    done
}

vendor photocraft "$PHOTOCRAFT_COMMIT" "$PHOTOCRAFT_CRATES"
vendor effectcraft "$EFFECTCRAFT_COMMIT" "$EFFECTCRAFT_CRATES"
vendor filmcraft "$FILMCRAFT_COMMIT" "$FILMCRAFT_CRATES"
vendor vectorcraft "$VECTORCRAFT_COMMIT" "$VECTORCRAFT_CRATES"

for patch in "$root"/scripts/patches/*.patch; do
    [ -e "$patch" ] || continue
    git -C "$root" apply "$patch" || { echo "$patch no longer applies: redo it against the new code, or drop it" >&2; exit 1; }
    echo "patched: $(basename "$patch")"
done
