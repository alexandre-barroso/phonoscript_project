#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "package-linux.sh must run on Linux." >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_root="$(cd "$script_dir/.." && pwd)"
convalgen_root="$(cd "$source_root/.." && pwd)"
workspace_root="$(cd "$convalgen_root/.." && pwd)"
phonoscript_root="$workspace_root/phonoscript"
docs_root="$workspace_root/docs"
compiled_root="${CONVALGEN_COMPILED_DIR:-$convalgen_root/compiled}"
platform_root="$compiled_root/linux"

case "$(uname -m)" in
  arm64|aarch64) architecture="aarch64" ;;
  amd64|x86_64) architecture="x86_64" ;;
  *)
    echo "Unsupported Linux architecture: $(uname -m)" >&2
    exit 2
    ;;
esac

package_name="ConvalGEN-linux-$architecture"
package_root="$platform_root/$package_name"
archive_path="$platform_root/$package_name.tar.gz"
target_root="${CARGO_TARGET_DIR:-$workspace_root/target}"
if [[ "$target_root" != /* ]]; then
  target_root="$workspace_root/$target_root"
fi

append_remap_flag() {
  local original="$1"
  local replacement="$2"
  RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$original=$replacement"
}

assert_no_machine_paths() {
  local binary="$1"
  if strings "$binary" | LC_ALL=C grep -Eq '/Users/|/Volumes/|/home/runner/|[A-Za-z]:\\Users\\'; then
    echo "Machine-specific path found in $binary." >&2
    exit 1
  fi
}

append_remap_flag "$workspace_root" "."
append_remap_flag "${CARGO_HOME:-${HOME:?HOME is required}/.cargo}" ".cargo"
export RUSTFLAGS

cd "$workspace_root"
cargo build --release --locked -p convalgen --bin convalgen
cargo build --release --locked -p phonoscript --bin phonoscript

convalgen_binary="$target_root/release/convalgen"
phonoscript_binary="$target_root/release/phonoscript"
test -x "$convalgen_binary"
test -x "$phonoscript_binary"
assert_no_machine_paths "$convalgen_binary"
assert_no_machine_paths "$phonoscript_binary"

rm -rf "$platform_root"
mkdir -p \
  "$package_root/bin" \
  "$package_root/share/applications" \
  "$package_root/share/icons/hicolor/scalable/apps" \
  "$package_root/share/mime/packages" \
  "$package_root/share/doc/convalgen" \
  "$package_root/share/convalgen/projects" \
  "$package_root/share/phonoscript/validation/analyses" \
  "$package_root/share/phonoscript/fixtures"

cp "$convalgen_binary" "$package_root/bin/convalgen"
cp "$phonoscript_binary" "$package_root/bin/phonoscript"
cp "$source_root/linux/org.convalgen.app.desktop" \
  "$package_root/share/applications/"
cp "$source_root/linux/org.convalgen.app.xml" \
  "$package_root/share/mime/packages/"
cp "$source_root/assets/icon/convalgen-icon.svg" \
  "$package_root/share/icons/hicolor/scalable/apps/org.convalgen.app.svg"
for size in 16 32 48 64 128 256 512; do
  icon_dir="$package_root/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$icon_dir"
  cp "$source_root/assets/icon/png/convalgen-$size.png" \
    "$icon_dir/org.convalgen.app.png"
done
cp "$convalgen_root/LICENSE" \
  "$package_root/share/doc/convalgen/CONVALGEN-LICENSE"
cp "$phonoscript_root/LICENSE" \
  "$package_root/share/doc/convalgen/PHONOSCRIPT-LICENSE"
cp "$docs_root/ConvalGEN-User-Guide.pdf" \
  "$package_root/share/doc/convalgen/ConvalGEN-User-Guide.pdf"
cp "$docs_root/PhonoScript-Language-Manual.pdf" \
  "$package_root/share/doc/convalgen/PhonoScript-Language-Manual.pdf"
cp "$convalgen_root/projects/dissertation-complete.ottab" \
  "$package_root/share/convalgen/projects/dissertation-complete.ottab"
cp -R "$phonoscript_root/validation/analyses/." \
  "$package_root/share/phonoscript/validation/analyses/"
cp "$phonoscript_root/fixtures/reference/"*.ottab \
  "$package_root/share/phonoscript/fixtures/"

cat > "$package_root/install.sh" <<'INSTALLER'
#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: ./install.sh [--user | --system | --prefix PREFIX]

The default is a current-user installation under $HOME/.local. Set
CONVALGEN_PREFIX to choose another default, or pass --prefix explicitly.
USAGE
}

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
install_prefix=${CONVALGEN_PREFIX:-"${HOME:?HOME is required}/.local"}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --user)
      install_prefix="${HOME:?HOME is required}/.local"
      shift
      ;;
    --system)
      install_prefix=/usr/local
      shift
      ;;
    --prefix)
      if [ "$#" -lt 2 ]; then
        echo "--prefix requires a directory." >&2
        exit 2
      fi
      install_prefix=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'Unknown installer option: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

install -d \
  "$install_prefix/bin" \
  "$install_prefix/share/applications" \
  "$install_prefix/share/icons/hicolor/scalable/apps" \
  "$install_prefix/share/mime/packages" \
  "$install_prefix/share/doc/convalgen" \
  "$install_prefix/share/convalgen/projects" \
  "$install_prefix/share/phonoscript/validation/analyses" \
  "$install_prefix/share/phonoscript/fixtures"
install -m 755 "$package_dir/bin/convalgen" "$install_prefix/bin/convalgen"
install -m 755 "$package_dir/bin/phonoscript" "$install_prefix/bin/phonoscript"
cp -R "$package_dir/share/." "$install_prefix/share/"
find "$install_prefix/share/doc/convalgen" \
  "$install_prefix/share/convalgen" \
  "$install_prefix/share/phonoscript" -type f -exec chmod 644 {} +

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$install_prefix/share/applications" >/dev/null 2>&1 || true
fi
if command -v update-mime-database >/dev/null 2>&1; then
  update-mime-database "$install_prefix/share/mime" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$install_prefix/share/icons/hicolor" >/dev/null 2>&1 || true
fi

"$install_prefix/bin/phonoscript" --version >/dev/null
printf 'Installed ConvalGEN and PhonoScript under %s\n' "$install_prefix"
case ":${PATH:-}:" in
  *":$install_prefix/bin:"*) ;;
  *)
    printf 'Add %s to PATH to invoke convalgen and phonoscript.\n' \
      "$install_prefix/bin" >&2
    ;;
esac
INSTALLER

chmod 755 \
  "$package_root/bin/convalgen" \
  "$package_root/bin/phonoscript" \
  "$package_root/install.sh"
find "$package_root/share" -type f -exec chmod 644 {} +

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate \
    "$package_root/share/applications/org.convalgen.app.desktop"
fi
if command -v xmllint >/dev/null 2>&1; then
  xmllint --noout "$package_root/share/mime/packages/org.convalgen.app.xml"
fi

"$package_root/bin/phonoscript" --version >/dev/null
while IFS= read -r -d '' analysis; do
  "$package_root/bin/phonoscript" --quiet "$analysis"
done < <(find "$package_root/share/phonoscript/validation/analyses" \
  -type f -name '*.phont' -print0 | sort -z)

smoke_root="$(mktemp -d "$platform_root/package-smoke.XXXXXX")"
trap 'rm -rf "$smoke_root"' EXIT
while IFS= read -r -d '' fixture; do
  emitted="$smoke_root/$(basename "${fixture%.ottab}").phont"
  "$package_root/bin/phonoscript" --emit "$fixture" --write "$emitted" --quiet
  "$package_root/bin/phonoscript" --quiet "$emitted"
done < <(find "$package_root/share/phonoscript/fixtures" \
  -type f -name '*.ottab' -print0 | sort -z)

tar \
  --sort=name \
  --mtime='UTC 2026-01-01' \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  -C "$platform_root" \
  -czf "$archive_path" \
  "$package_name"

tar -C "$smoke_root" -xzf "$archive_path"
extracted="$smoke_root/$package_name"
sample="$(find "$extracted/share/phonoscript/validation/analyses" \
  -type f -name '*.phont' -print -quit)"
"$extracted/bin/phonoscript" --quiet "$sample"
test -f "$extracted/share/doc/convalgen/ConvalGEN-User-Guide.pdf"
test -f "$extracted/share/convalgen/projects/dissertation-complete.ottab"

install_prefix="$smoke_root/prefix"
"$extracted/install.sh" --prefix "$install_prefix"
PATH="$install_prefix/bin:$PATH" phonoscript --version >/dev/null
PATH="$install_prefix/bin:$PATH" phonoscript --quiet \
  "$(find "$install_prefix/share/phonoscript/validation/analyses" \
    -type f -name '*.phont' -print -quit)"

printf '%s\n' "$archive_path"
