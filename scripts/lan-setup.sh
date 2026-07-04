#!/usr/bin/env bash
# Open the LAN firewall so a phone on your network can reach claude-pocket.
#
# One-time setup — ufw/firewalld rules persist across reboots; you do NOT need to run this
# every time you start the app. Raw nftables is the exception: the rule is runtime-only
# (the script tells you how to persist it). Detects ufw / firewalld / nftables. Idempotent.
#
# Usage:
#   sudo ./scripts/lan-setup.sh [PORT]            # default PORT=5173 (vite dev)
#   sudo CP_LAN_SUBNET=192.168.1.0/24 ./scripts/lan-setup.sh 5173   # restrict to your LAN
#
# Tip: find your subnet with `ip route get 1.1.1.1`. Leaving CP_LAN_SUBNET empty opens
# the port on all interfaces (simplest; fine on a trusted home network).
set -euo pipefail

PORT="${1:-5173}"
SUBNET="${CP_LAN_SUBNET:-}"

if [[ $EUID -ne 0 ]]; then
  echo "Needs root. Run: sudo $0 $PORT" >&2
  exit 1
fi

open_ufw() {
  if [[ -n "$SUBNET" ]]; then
    ufw allow from "$SUBNET" to any port "$PORT" proto tcp
  else
    ufw allow "${PORT}/tcp"
  fi
}

open_firewalld() {
  firewall-cmd --permanent --add-port="${PORT}/tcp" >/dev/null
  firewall-cmd --reload >/dev/null
}

open_nftables() {
  # Best-effort: append an accept rule to the inet filter input chain.
  if nft list chain inet filter input >/dev/null 2>&1; then
    if nft list chain inet filter input | grep -q "tcp dport $PORT accept"; then
      echo "rule for port $PORT already present — skipping"
    else
      nft add rule inet filter input tcp dport "$PORT" accept
    fi
    echo "NOTE: raw nftables rules are runtime-only — to survive reboot, add the rule to" >&2
    echo "      your persistent ruleset (e.g. /etc/nftables.conf) as well." >&2
  else
    echo "nftables active but no inet/filter/input chain found — add a rule manually:" >&2
    echo "  nft add rule inet filter input tcp dport $PORT accept" >&2
    return 1
  fi
}

if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi '^Status: active'; then
  echo "firewall: ufw"
  open_ufw
elif systemctl is-active --quiet firewalld 2>/dev/null; then
  echo "firewall: firewalld"
  open_firewalld
elif systemctl is-active --quiet nftables 2>/dev/null || command -v nft >/dev/null 2>&1; then
  echo "firewall: nftables"
  open_nftables
else
  echo "No active firewall detected (ufw/firewalld/nftables) — nothing to open."
  exit 0
fi

echo "Done — TCP ${PORT} open for the LAN${SUBNET:+ (${SUBNET})}."
