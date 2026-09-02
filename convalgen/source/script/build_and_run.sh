#!/usr/bin/env bash
set -euo pipefail

mode="${1:-run}"
app_name="PhonoScript GUI"
bundle_id="com.alexandrebarroso.phonoscriptgui"
studio_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project_root="$(cd "$studio_root/.." && pwd)"
app_bundle="$project_root/compiled/macos/$app_name.app"
app_binary="$app_bundle/Contents/MacOS/phonoscript-gui"

pkill -x phonoscript-gui >/dev/null 2>&1 || true
"$studio_root/scripts/package-macos.sh" >/dev/null

open_app() {
  /usr/bin/open -n "$app_bundle"
}

case "$mode" in
  run)
    open_app
    ;;
  --debug|debug)
    lldb -- "$app_binary"
    ;;
  --logs|logs)
    open_app
    /usr/bin/log stream --info --style compact --predicate "process == \"phonoscript-gui\""
    ;;
  --telemetry|telemetry)
    open_app
    /usr/bin/log stream --info --style compact --predicate "subsystem == \"$bundle_id\""
    ;;
  --verify|verify)
    open_app
    sleep 1
    pgrep -x phonoscript-gui >/dev/null
    ;;
  *)
    echo "usage: $0 [run|--debug|--logs|--telemetry|--verify]" >&2
    exit 2
    ;;
esac
