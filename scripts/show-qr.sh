#!/usr/bin/env bash
# Print the pairing QR on demand — the same QR the backend prints at startup, without
# restarting the server. Useful when the backend runs in the background.
#
# Reads CP_AUTH_TOKEN / CP_PUBLIC_URL from the environment or backend/.env:
#   CP_AUTH_TOKEN=... CP_PUBLIC_URL=https://you.ts.net ./scripts/show-qr.sh
set -euo pipefail
cd "$(dirname "$0")/../backend"
exec uv run python -c "from app.config import settings; from app.main import print_pairing; print_pairing(settings)"
