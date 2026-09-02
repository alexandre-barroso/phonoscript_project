#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "package-macos.sh must run on macOS." >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_root="$(cd "$script_dir/.." && pwd)"
convalgen_root="$(cd "$source_root/.." && pwd)"
workspace_root="$(cd "$convalgen_root/.." && pwd)"
phonoscript_root="$workspace_root/phonoscript"
docs_root="$workspace_root/docs"
compiled_root="${CONVALGEN_COMPILED_DIR:-$convalgen_root/compiled}"
platform_root="$compiled_root/macos"

case "$(uname -m)" in
  arm64|aarch64) architecture="aarch64" ;;
  x86_64) architecture="x86_64" ;;
  *)
    echo "Unsupported macOS architecture: $(uname -m)" >&2
    exit 2
    ;;
esac

app_bundle="$platform_root/PhonoScript GUI.app"
app_macos="$app_bundle/Contents/MacOS"
app_resources="$app_bundle/Contents/Resources"
archive_path="$platform_root/PhonoScript-GUI-macos-$architecture.zip"
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
cargo build --release --locked -p convalgen --bin phonoscript-gui
cargo build --release --locked -p phonoscript --bin phonoscript

convalgen_binary="$target_root/release/phonoscript-gui"
phonoscript_binary="$target_root/release/phonoscript"
test -x "$convalgen_binary"
test -x "$phonoscript_binary"
assert_no_machine_paths "$convalgen_binary"
assert_no_machine_paths "$phonoscript_binary"

rm -rf "$platform_root"
mkdir -p \
  "$app_macos" \
  "$app_resources/bin" \
  "$app_resources/Documentation" \
  "$app_resources/Projects" \
  "$app_resources/Validation/analyses" \
  "$app_resources/Validation/fixtures"

cp "$convalgen_binary" "$app_macos/phonoscript-gui"
cp "$phonoscript_binary" "$app_resources/bin/phonoscript"
cp "$source_root/macos/Info.plist" "$app_bundle/Contents/Info.plist"
cp "$source_root/assets/icon/macos/PhonoScript-GUI.icns" \
  "$app_resources/PhonoScript-GUI.icns"
cp "$convalgen_root/LICENSE" "$app_resources/PHONOSCRIPT-GUI-LICENSE"
cp "$phonoscript_root/LICENSE" "$app_resources/PHONOSCRIPT-LICENSE"
cp "$docs_root/PhonoScript-GUI-User-Guide.pdf" \
  "$app_resources/Documentation/PhonoScript-GUI-User-Guide.pdf"
cp "$docs_root/PhonoScript-Language-Manual.pdf" \
  "$app_resources/Documentation/PhonoScript-Language-Manual.pdf"
cp "$convalgen_root/projects/dissertation-complete.ottab" \
  "$app_resources/Projects/dissertation-complete.ottab"
cp -R "$phonoscript_root/validation/analyses/." \
  "$app_resources/Validation/analyses/"
cp "$phonoscript_root/fixtures/reference/"*.ottab \
  "$app_resources/Validation/fixtures/"

chmod 755 "$app_macos/phonoscript-gui" "$app_resources/bin/phonoscript"
find "$app_resources/Documentation" "$app_resources/Projects" \
  "$app_resources/Validation" -type f -exec chmod 644 {} +

plutil -lint "$app_bundle/Contents/Info.plist" >/dev/null
"$app_resources/bin/phonoscript" --version >/dev/null
while IFS= read -r -d '' analysis; do
  "$app_resources/bin/phonoscript" --quiet "$analysis"
done < <(find "$app_resources/Validation/analyses" \
  -type f -name '*.phont' -print0 | sort -z)

smoke_root="$(mktemp -d "$platform_root/package-smoke.XXXXXX")"
trap 'rm -rf "$smoke_root"' EXIT
while IFS= read -r -d '' fixture; do
  emitted="$smoke_root/$(basename "${fixture%.ottab}").phont"
  "$app_resources/bin/phonoscript" --emit "$fixture" --write "$emitted" --quiet
  "$app_resources/bin/phonoscript" --quiet "$emitted"
done < <(find "$app_resources/Validation/fixtures" \
  -type f -name '*.ottab' -print0 | sort -z)

# These are local ad-hoc signatures. A public macOS release still requires a
# Developer ID signature and Apple notarization by the release owner.
codesign --force --sign - "$app_macos/phonoscript-gui"
codesign --force --sign - "$app_resources/bin/phonoscript"
codesign --force --sign - "$app_bundle"
codesign --verify --deep --strict --verbose=2 "$app_bundle"
xattr -cr "$app_bundle"

/usr/bin/ditto -c -k --norsrc --noextattr --noqtn --noacl --keepParent \
  "$app_bundle" "$archive_path"

/usr/bin/ditto -x -k "$archive_path" "$smoke_root/archive"
extracted_app="$smoke_root/archive/PhonoScript GUI.app"
codesign --verify --deep --strict --verbose=2 "$extracted_app"
extracted_interpreter="$extracted_app/Contents/Resources/bin/phonoscript"
sample="$(find "$extracted_app/Contents/Resources/Validation/analyses" \
  -type f -name '*.phont' -print -quit)"
"$extracted_interpreter" --quiet "$sample"
test -f "$extracted_app/Contents/Resources/Documentation/PhonoScript-GUI-User-Guide.pdf"
test -f "$extracted_app/Contents/Resources/Documentation/PhonoScript-Language-Manual.pdf"
test -f "$extracted_app/Contents/Resources/Projects/dissertation-complete.ottab"

printf '%s\n' "$archive_path"
