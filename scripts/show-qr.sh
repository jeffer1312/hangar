#!/usr/bin/env bash
# Os dois QR do boot, sem reiniciar o serviço nem carregar o backend Python.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
exec "$root/scripts/hangar-server-command" pairing --backend "$root/backend"
