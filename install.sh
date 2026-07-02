#!/usr/bin/env bash
# claude-pocket — instalação em um comando.
#
#   ./install.sh              # interativo (pergunta wrapper + serviços)
#   ./install.sh --yes        # não-interativo: instala tudo
#   ./install.sh --no-wrapper --no-services   # só backend + frontend
#
# O que faz: checa dependências, instala deps do backend (uv sync), builda o
# frontend, gera um CP_AUTH_TOKEN forte em backend/.env (se não houver), e
# opcionalmente instala o wrapper do `claude` (scripts/install-claude-wrapper.sh)
# e os serviços systemd de usuário (scripts/services-setup.sh).
set -euo pipefail
cd "$(dirname "$0")"

YES=0; WRAPPER=1; SERVICES=1
for arg in "$@"; do
  case "$arg" in
    --yes|-y) YES=1 ;;
    --no-wrapper) WRAPPER=0 ;;
    --no-services) SERVICES=0 ;;
    *) echo "flag desconhecida: $arg"; exit 1 ;;
  esac
done

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31m✗ %s\033[0m\n' "$*"; exit 1; }
ok()   { printf '\033[32m✓\033[0m %s\n' "$*"; }

ask() { # ask "pergunta" -> 0/1 (em --yes, sempre sim)
  [ "$YES" = 1 ] && return 0
  read -r -p "$1 [S/n] " r
  [ -z "$r" ] || [[ "$r" =~ ^[SsYy] ]]
}

# ── 1. Dependências ──────────────────────────────────────────────────────────
say "1/5 Checando dependências"
command -v tmux   >/dev/null || fail "tmux não encontrado — instale com o gerenciador do teu sistema"
command -v claude >/dev/null || fail "claude (Claude Code) não encontrado — https://code.claude.com"
command -v uv     >/dev/null || fail "uv não encontrado — https://docs.astral.sh/uv/ (curl -LsSf https://astral.sh/uv/install.sh | sh)"
command -v npm    >/dev/null || fail "node/npm não encontrados — instale Node 20+"
node -e 'process.exit(parseInt(process.versions.node) >= 20 ? 0 : 1)' \
  || fail "Node 20+ é necessário (atual: $(node --version))"
ok "tmux, claude, uv e node $(node --version) presentes"

# ── 2. Backend ───────────────────────────────────────────────────────────────
say "2/5 Backend (uv sync)"
(cd backend && uv sync --quiet)
ok "dependências do backend instaladas"

# Token: gera um forte em backend/.env se ainda não existir (o backend recusa
# 'change-me' fora do loopback).
if [ -f backend/.env ] && grep -q '^CP_AUTH_TOKEN=' backend/.env; then
  ok "backend/.env já tem CP_AUTH_TOKEN (mantido)"
else
  TOKEN=$(openssl rand -hex 24 2>/dev/null || python3 -c 'import secrets; print(secrets.token_hex(24))')
  printf 'CP_AUTH_TOKEN=%s\n' "$TOKEN" >> backend/.env
  ok "CP_AUTH_TOKEN gerado em backend/.env"
fi

# ── 3. Frontend ──────────────────────────────────────────────────────────────
say "3/5 Frontend (npm ci + build)"
(cd frontend && npm ci --silent && npm run build --silent)
ok "frontend buildado em frontend/dist/"

# ── 4. Wrapper do claude (recomendado) ───────────────────────────────────────
# Faz todo `claude` abrir dentro do tmux com --session-id próprio — é o que
# permite o app rastrear cada sessão com segurança (sem ele: "⚠ no id").
say "4/5 Wrapper do claude"
if [ "$WRAPPER" = 1 ] && ask "Instalar o wrapper do claude (recomendado)?"; then
  ./scripts/install-claude-wrapper.sh
else
  echo "  pulado — instale depois com ./scripts/install-claude-wrapper.sh"
fi

# ── 5. Serviços systemd (opcional; Linux) ────────────────────────────────────
say "5/5 Serviços systemd de usuário"
if [ "$SERVICES" = 1 ] && command -v systemctl >/dev/null && ask "Rodar backend+frontend como serviços persistentes?"; then
  ./scripts/services-setup.sh
else
  echo "  pulado — depois: ./scripts/services-setup.sh (ou rode na mão, ver abaixo)"
fi

say "Pronto! Próximos passos"
cat <<'EOF'
  • Rodar na mão (sem serviços):
      cd backend && CP_LAN_BIND_IP=auto uv run python -m app.main
      cd frontend && npm run dev          # ou sirva frontend/dist/
  • No celular: abra a URL mostrada no QR do backend e cole o token
    (backend/.env). Guia completo com Tailscale + instalar como PWA:
    docs/USAGE.md
  • Abra sessões com `claude` normal (o wrapper cuida do tmux/session-id).
EOF
