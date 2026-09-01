#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build-icons.sh currently requires macOS sips and iconutil." >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source_root="$(cd "$script_dir/.." && pwd)"
icon_root="$source_root/assets/icon"
svg="$icon_root/convalgen-icon.svg"
png_root="$icon_root/png"
iconset="$icon_root/macos/ConvalGEN.iconset"
icns="$icon_root/macos/ConvalGEN.icns"
ico="$icon_root/windows/ConvalGEN.ico"

mkdir -p "$png_root" "$iconset" "$(dirname "$ico")"

sips -s format png "$svg" --out "$png_root/convalgen-1024.png" >/dev/null
for size in 16 20 24 32 48 64 128 256 512; do
  sips -z "$size" "$size" "$png_root/convalgen-1024.png" \
    --out "$png_root/convalgen-$size.png" >/dev/null
done

cp "$png_root/convalgen-16.png" "$iconset/icon_16x16.png"
cp "$png_root/convalgen-32.png" "$iconset/icon_16x16@2x.png"
cp "$png_root/convalgen-32.png" "$iconset/icon_32x32.png"
cp "$png_root/convalgen-64.png" "$iconset/icon_32x32@2x.png"
cp "$png_root/convalgen-128.png" "$iconset/icon_128x128.png"
cp "$png_root/convalgen-256.png" "$iconset/icon_128x128@2x.png"
cp "$png_root/convalgen-256.png" "$iconset/icon_256x256.png"
cp "$png_root/convalgen-512.png" "$iconset/icon_256x256@2x.png"
cp "$png_root/convalgen-512.png" "$iconset/icon_512x512.png"
cp "$png_root/convalgen-1024.png" "$iconset/icon_512x512@2x.png"
iconutil -c icns "$iconset" -o "$icns"

# ICO permits PNG-compressed representations. Keeping several sizes in one
# container gives Windows a native image at common shell and taskbar scales.
ruby - "$ico" "$png_root" <<'RUBY'
output, png_root = ARGV
sizes = [16, 20, 24, 32, 48, 64, 128, 256]
images = sizes.map { |size| File.binread(File.join(png_root, "convalgen-#{size}.png")) }
offset = 6 + 16 * images.length
entries = sizes.zip(images).map do |size, bytes|
  encoded_size = size == 256 ? 0 : size
  entry = [encoded_size, encoded_size, 0, 0, 1, 32, bytes.bytesize, offset]
    .pack("CCCCvvVV")
  offset += bytes.bytesize
  entry
end
File.binwrite(output, [0, 1, images.length].pack("vvv") + entries.join + images.join)
RUBY

printf '%s\n' "$icns"
printf '%s\n' "$ico"
