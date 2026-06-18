#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

help=$(bash "$repo/install.sh" --help)
grep -q -- '--user USER' <<<"$help"
grep -q -- '--no-user' <<<"$help"
grep -q -- '--debug' <<<"$help"
grep -q -- 'release build is the default' <<<"$help"

if bash "$repo/install.sh" --user test --no-user >/dev/null 2>&1; then
    echo "installer accepted conflicting authorization options" >&2
    exit 1
fi

if bash "$repo/install.sh" --unknown >/dev/null 2>&1; then
    echo "installer accepted an unknown option" >&2
    exit 1
fi
