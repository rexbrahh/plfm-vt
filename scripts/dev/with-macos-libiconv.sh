#!/usr/bin/env bash
set -euo pipefail

if [ "$(uname -s)" = "Darwin" ]; then
  if [ -d "/opt/homebrew/opt/libiconv/lib" ]; then
    export LIBRARY_PATH="/opt/homebrew/opt/libiconv/lib:${LIBRARY_PATH:-}"
  elif [ -d "/usr/local/opt/libiconv/lib" ]; then
    export LIBRARY_PATH="/usr/local/opt/libiconv/lib:${LIBRARY_PATH:-}"
  fi
fi

exec "$@"
