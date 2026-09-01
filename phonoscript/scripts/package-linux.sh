#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "package-linux.sh must run on Linux." >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
phonoscript_root="$(cd "$script_dir/.." && pwd)"
workspace_root="$(cd "$phonoscript_root/.." && pwd)"
docs_root="$workspace_root/docs"
compiled_root="${PHONOSCRIPT_COMPILED_DIR:-$phonoscript_root/compiled}"
platform_root="$compiled_root/linux"

case "$(uname -m)" in
  arm64|aarch64) architecture="aarch64" ;;
  amd64|x86_64) architecture="x86_64" ;;
  *)
    echo "Unsupported Linux architecture: $(uname -m)" >&2
    exit 2
    ;;
esac

package_name="PhonoScript-linux-$architecture"
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
cargo build --release --locked -p phonoscript --bin phonoscript

binary="$target_root/release/phonoscript"
test -x "$binary"
assert_no_machine_paths "$binary"

rm -rf "$platform_root"
mkdir -p \
  "$package_root/bin" \
  "$package_root/docs" \
  "$package_root/validation/analyses" \
  "$package_root/fixtures"

cp "$binary" "$package_root/bin/phonoscript"
cp "$phonoscript_root/LICENSE" "$package_root/LICENSE"
cp "$docs_root/PhonoScript-Language-Manual.pdf" "$package_root/docs/"
cp -R "$phonoscript_root/validation/analyses/." "$package_root/validation/analyses/"
cp "$phonoscript_root/fixtures/reference/"*.ottab "$package_root/fixtures/"

cat > "$package_root/install.sh" <<'INSTALLER'
#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: ./install.sh [--user | --system | --prefix PREFIX]

The default is a current-user installation under $HOME/.local. Set
PHONOSCRIPT_PREFIX to choose another default, or pass --prefix explicitly.
USAGE
}

package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
install_prefix=${PHONOSCRIPT_PREFIX:-"${HOME:?HOME is required}/.local"}

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
  "$install_prefix/share/doc/phonoscript" \
  "$install_prefix/share/phonoscript/validation/analyses" \
  "$install_prefix/share/phonoscript/fixtures"
install -m 755 "$package_dir/bin/phonoscript" "$install_prefix/bin/phonoscript"
install -m 644 "$package_dir/LICENSE" \
  "$install_prefix/share/doc/phonoscript/LICENSE"
cp -R "$package_dir/docs/." "$install_prefix/share/doc/phonoscript/"
cp -R "$package_dir/validation/analyses/." \
  "$install_prefix/share/phonoscript/validation/analyses/"
cp -R "$package_dir/fixtures/." "$install_prefix/share/phonoscript/fixtures/"
find "$install_prefix/share/doc/phonoscript" \
  "$install_prefix/share/phonoscript" -type f -exec chmod 644 {} +

"$install_prefix/bin/phonoscript" --version >/dev/null
printf 'Installed PhonoScript at %s\n' "$install_prefix/bin/phonoscript"
case ":${PATH:-}:" in
  *":$install_prefix/bin:"*) ;;
  *)
    printf 'Add %s to PATH to invoke phonoscript from any directory.\n' \
      "$install_prefix/bin" >&2
    ;;
esac
INSTALLER

chmod 755 "$package_root/bin/phonoscript" "$package_root/install.sh"
find "$package_root/docs" "$package_root/validation" "$package_root/fixtures" \
  -type f -exec chmod 644 {} +

"$package_root/bin/phonoscript" --version >/dev/null
while IFS= read -r -d '' analysis; do
  "$package_root/bin/phonoscript" --quiet "$analysis"
done < <(find "$package_root/validation/analyses" -type f -name '*.phont' -print0 | sort -z)

smoke_root="$(mktemp -d "$platform_root/package-smoke.XXXXXX")"
trap 'rm -rf "$smoke_root"' EXIT
while IFS= read -r -d '' fixture; do
  emitted="$smoke_root/$(basename "${fixture%.ottab}").phont"
  "$package_root/bin/phonoscript" --emit "$fixture" --write "$emitted" --quiet
  "$package_root/bin/phonoscript" --quiet "$emitted"
done < <(find "$package_root/fixtures" -type f -name '*.ottab' -print0 | sort -z)

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
"$extracted/bin/phonoscript" --quiet \
  "$(find "$extracted/validation/analyses" -type f -name '*.phont' -print -quit)"
test -f "$extracted/docs/PhonoScript-Language-Manual.pdf"

install_prefix="$smoke_root/prefix"
"$extracted/install.sh" --prefix "$install_prefix"
PATH="$install_prefix/bin:$PATH" phonoscript --version >/dev/null
PATH="$install_prefix/bin:$PATH" phonoscript --quiet \
  "$(find "$install_prefix/share/phonoscript/validation/analyses" \
    -type f -name '*.phont' -print -quit)"

printf '%s\n' "$archive_path"
