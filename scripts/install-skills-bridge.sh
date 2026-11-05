#!/usr/bin/env bash
# Persona, pacotes e skills usam o mesmo instalador nativo.
set -euo pipefail
setup_root="$(cd "$(dirname "$0")/.." && pwd)"
exec "$setup_root/scripts/hangar-harness-setup" persona --repo "$setup_root"
