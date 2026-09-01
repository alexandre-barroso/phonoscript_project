#!/bin/sh
set -eu

usage() {
  printf '%s\n' \
    'Usage: ./install.sh [--user | --system | --prefix PREFIX]' \
    '' \
    'The default is a current-user installation under $HOME/.local. Set' \
    'PHONOSCRIPT_PREFIX to choose another default, or pass --prefix explicitly.'
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
install -m 644 "$package_dir/BUILD-PROVENANCE.txt" \
  "$install_prefix/share/doc/phonoscript/BUILD-PROVENANCE.txt"
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
