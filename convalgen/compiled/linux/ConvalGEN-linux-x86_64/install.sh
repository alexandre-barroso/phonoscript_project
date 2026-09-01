#!/bin/sh
set -eu

usage() {
  printf '%s\n' \
    'Usage: ./install.sh [--user | --system | --prefix PREFIX]' \
    '' \
    'The default is a current-user installation under $HOME/.local. Set' \
    'CONVALGEN_PREFIX to choose another default, or pass --prefix explicitly.'
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
