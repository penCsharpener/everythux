#!/usr/bin/env bash
# Build stripped release binaries and package them (tarball, .deb, SHA256SUMS).
# Mirrors .github/workflows/release.yml so local and CI artefacts have the same names.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: ./build-release.sh [OPTIONS]

Builds everythux with the `dist` Cargo profile (release + no debug info + strip) and writes
a tarball, a .deb and SHA256SUMS to the output directory.

Options:
  --out DIR         Output directory (default: dist/ in the repository root)
  --target TRIPLE   Cross-compile for TRIPLE (default: the host triple)
  --no-deb          Do not build the .deb
  --no-tarball      Do not build the .tar.gz
  --skip-checks     Skip `cargo fmt --all --check` and `cargo clippy ... -D warnings`
  --test            Also run `cargo test --workspace --all-features`
  --install-tools   Run `cargo install cargo-deb --locked` if cargo-deb is missing
  -h, --help        Show this help

Outputs (in DIR):
  everythux-<version>-<triple>.tar.gz   binaries + packaging/ + docs/ + README.md + LICENSE*
  everythux_<version>-1_<arch>.deb      xz-compressed, from [package.metadata.deb]
  SHA256SUMS                            checksums of the files above
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

info() {
    printf '==> %s\n' "$*" >&2
}

# Human-readable size of a file.
hsize() {
    if command -v numfmt >/dev/null 2>&1; then
        numfmt --to=iec --suffix=B "$(stat -c %s "$1")"
    else
        printf '%s bytes' "$(stat -c %s "$1")"
    fi
}

OUT=""
TARGET=""
DO_DEB=1
DO_TARBALL=1
DO_CHECKS=1
DO_TEST=0
INSTALL_TOOLS=0

while [ $# -gt 0 ]; do
    case "$1" in
        --out)
            [ $# -ge 2 ] || die "--out needs a directory"
            OUT="$2"
            shift 2
            ;;
        --out=*)
            OUT="${1#--out=}"
            shift
            ;;
        --target)
            [ $# -ge 2 ] || die "--target needs a triple"
            TARGET="$2"
            shift 2
            ;;
        --target=*)
            TARGET="${1#--target=}"
            shift
            ;;
        --no-deb)
            DO_DEB=0
            shift
            ;;
        --no-tarball)
            DO_TARBALL=0
            shift
            ;;
        --skip-checks)
            DO_CHECKS=0
            shift
            ;;
        --test)
            DO_TEST=1
            shift
            ;;
        --install-tools)
            INSTALL_TOOLS=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "unknown option: $1"
            ;;
    esac
done

# Run from the repository root regardless of the caller's cwd. Resolve --out against the
# caller's cwd first, so a relative path means what the user expects.
CALLER_PWD="$PWD"
case "$OUT" in
    "") ;;
    /*) ;;
    *) OUT="$CALLER_PWD/$OUT" ;;
esac
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd -- "$ROOT"
[ -n "$OUT" ] || OUT="$ROOT/dist"

command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH"

if [ "$DO_DEB" -eq 1 ] && ! cargo deb --version >/dev/null 2>&1; then
    if [ "$INSTALL_TOOLS" -eq 1 ]; then
        info "installing cargo-deb"
        cargo install cargo-deb --locked
    else
        printf 'error: cargo-deb is not installed. Install it with:\n  cargo install cargo-deb --locked\n' >&2
        printf 'or pass --install-tools to let this script do it (or use --no-deb).\n' >&2
        exit 1
    fi
fi

# --- metadata ---------------------------------------------------------------------------------
METADATA="$(cargo metadata --no-deps --format-version 1)"
if command -v jq >/dev/null 2>&1; then
    VERSION="$(printf '%s' "$METADATA" | jq -r '.packages[] | select(.name == "everythux-app") | .version')"
    TARGET_DIR="$(printf '%s' "$METADATA" | jq -r '.target_directory')"
elif command -v python3 >/dev/null 2>&1; then
    VERSION="$(printf '%s' "$METADATA" | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
print(next(p["version"] for p in meta["packages"] if p["name"] == "everythux-app"))')"
    TARGET_DIR="$(printf '%s' "$METADATA" | python3 -I -c '
import json, sys
print(json.load(sys.stdin)["target_directory"])')"
else
    die "need jq or python3 to read cargo metadata"
fi
[ -n "$VERSION" ] && [ "$VERSION" != "null" ] || die "could not read the everythux-app version"

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
[ -n "$HOST_TRIPLE" ] || die "could not determine the host triple from rustc -vV"
TRIPLE="${TARGET:-$HOST_TRIPLE}"

if [ -n "$TARGET" ]; then
    BIN_DIR="$TARGET_DIR/$TARGET/dist"
    TARGET_ARGS=(--target "$TARGET")
else
    BIN_DIR="$TARGET_DIR/dist"
    TARGET_ARGS=()
fi

info "everythux $VERSION for $TRIPLE -> $OUT"

# --- checks -----------------------------------------------------------------------------------
if [ "$DO_CHECKS" -eq 1 ]; then
    info "cargo fmt --all --check"
    cargo fmt --all --check
    info "cargo clippy"
    cargo clippy --workspace --all-targets --all-features -- -D warnings
else
    info "skipping fmt/clippy (--skip-checks)"
fi

if [ "$DO_TEST" -eq 1 ]; then
    info "cargo test"
    if command -v dbus-run-session >/dev/null 2>&1; then
        dbus-run-session -- cargo test --workspace --all-features
    else
        echo "warning: dbus-run-session not found, D-Bus mock tests may fail" >&2
        cargo test --workspace --all-features
    fi
fi

# --- build ------------------------------------------------------------------------------------
info "cargo build --profile dist"
cargo build --profile dist --locked -p everythux-app -p everythux-cli "${TARGET_ARGS[@]}"

BINS=(everythux everythux-cli)
for b in "${BINS[@]}"; do
    [ -x "$BIN_DIR/$b" ] || die "missing build output: $BIN_DIR/$b"
done

# Make sure no debug sections survived (the profile strips them; this is the safety net).
has_debug_sections() {
    readelf -S -W "$1" 2>/dev/null | grep -q '\.debug_'
}

if command -v readelf >/dev/null 2>&1; then
    for b in "${BINS[@]}"; do
        if has_debug_sections "$BIN_DIR/$b"; then
            info "$b still has .debug_* sections, running strip --strip-all"
            if [ "$TRIPLE" != "$HOST_TRIPLE" ]; then
                echo "warning: host strip may not understand $TRIPLE binaries" >&2
            fi
            strip --strip-all "$BIN_DIR/$b" || die "strip failed for $b"
            if has_debug_sections "$BIN_DIR/$b"; then
                die "$b still has debug sections after strip"
            fi
        fi
    done
else
    echo "warning: readelf not found, cannot verify that the binaries are stripped" >&2
fi

# --- packaging --------------------------------------------------------------------------------
mkdir -p -- "$OUT"
SUM_FILES=()

if [ "$DO_TARBALL" -eq 1 ]; then
    NAME="everythux-${VERSION}-${TRIPLE}"
    TARBALL="$OUT/$NAME.tar.gz"
    STAGE="$(mktemp -d)"
    # shellcheck disable=SC2064  # expand STAGE now; it never changes
    trap "rm -rf -- \"\${STAGE:?}\"" EXIT
    mkdir -p -- "$STAGE/$NAME"
    for b in "${BINS[@]}"; do
        cp -- "$BIN_DIR/$b" "$STAGE/$NAME/"
    done
    cp -r -- packaging docs README.md "$STAGE/$NAME/"
    for f in LICENSE LICENSE-MIT LICENSE-APACHE; do
        if [ -e "$f" ]; then cp -- "$f" "$STAGE/$NAME/"; fi
    done
    info "writing $TARBALL"
    tar -C "$STAGE" --sort=name --owner=0 --group=0 --numeric-owner -czf "$TARBALL" "$NAME"
    SUM_FILES+=("$(basename -- "$TARBALL")")
fi

DEB=""
if [ "$DO_DEB" -eq 1 ]; then
    DEB_ARGS=(-p everythux-app --profile dist --no-build --output "$OUT/")
    if [ -n "$TARGET" ]; then DEB_ARGS+=(--target "$TARGET"); fi
    # Best compression: xz, cargo-deb's default in 3.x. Pass it explicitly when supported so an
    # older default (gzip) cannot sneak in.
    if cargo deb --help 2>&1 | grep -q -- '--compress-type'; then
        DEB_ARGS+=(--compress-type xz)
        COMPRESSION="xz"
    else
        COMPRESSION="cargo-deb default"
    fi
    info "cargo deb (${COMPRESSION})"
    # cargo-deb prints the path of the .deb on stdout.
    DEB="$(cargo deb "${DEB_ARGS[@]}" | tail -n 1)"
    [ -f "$DEB" ] || die "cargo deb did not produce a .deb (got: '$DEB')"
    SUM_FILES+=("$(basename -- "$DEB")")
fi

if [ "${#SUM_FILES[@]}" -gt 0 ]; then
    info "writing SHA256SUMS"
    (cd -- "$OUT" && sha256sum -- "${SUM_FILES[@]}" >SHA256SUMS)
fi

# --- summary ----------------------------------------------------------------------------------
echo
echo "Release build: everythux $VERSION ($TRIPLE)"
printf '%-52s %12s\n' "ARTEFACT" "SIZE"
printf '%-52s %12s\n' "--------" "----"
for b in "${BINS[@]}"; do
    printf '%-52s %12s\n' "binary: $b" "$(hsize "$BIN_DIR/$b")"
done
if [ "$DO_TARBALL" -eq 1 ]; then
    printf '%-52s %12s\n' "$(basename -- "$TARBALL")" "$(hsize "$TARBALL")"
fi
if [ -n "$DEB" ]; then
    printf '%-52s %12s\n' "$(basename -- "$DEB")" "$(hsize "$DEB")"
    if command -v dpkg-deb >/dev/null 2>&1; then
        INSTALLED_KIB="$(dpkg-deb -f "$DEB" Installed-Size 2>/dev/null || true)"
        if [ -n "$INSTALLED_KIB" ]; then
            printf '%-52s %12s\n' "  installed size (dpkg-deb)" "${INSTALLED_KIB} KiB"
        fi
    fi
fi
if [ "${#SUM_FILES[@]}" -gt 0 ]; then
    printf '%-52s %12s\n' "SHA256SUMS" "$(hsize "$OUT/SHA256SUMS")"
fi
echo
echo "Output directory: $OUT"
