#!/bin/sh
# Runs packaged executables once (docs/design/release.md §8): the
# application prints its version, the command line reads a test file, and
# `materials-workbench mcp` answers an MCP client on pipes, without a window
# (on Windows, the release build's GUI subsystem must still serve them).
#
#   packaging/smoke-test.sh APPLICATION MW
set -eu
app=$1
mw=$2
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

"$app" --version
"$mw" info "$root/crates/mw-crystal-io/tests/fixtures/rutile.cif"

# One request; the server answers, then sees the input end and exits.
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke-test","version":"0"}}}' |
    MW_CONFIG_DIR="$work/config" "$app" mcp --allow "$work" >"$work/answer" 2>"$work/log" || true
if grep -q '"serverInfo"' "$work/answer"; then
    echo "materials-workbench mcp answered"
else
    echo "materials-workbench mcp did not answer:" >&2
    cat "$work/answer" "$work/log" >&2
    exit 1
fi
