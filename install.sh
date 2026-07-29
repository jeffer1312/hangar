#!/usr/bin/env bash
# claude-cockpit — instalação completa no Linux/macOS.
#
#   ./install.sh              # interativo
#   ./install.sh --yes        # aceita tudo (não pergunta nada)
#   ./install.sh --check      # só diz o que falta e sai, sem instalar nada
#   ./install.sh --no-wrapper --no-services --no-cp-send --no-panel   # pula partes
#
# Os sub-scripts (services-setup.sh, lan-setup.sh, install-cp-send.sh, ...) continuam
# rodáveis sozinhos; este aqui só faz com que um comando baste.
set -euo pipefail
cd "$(dirname "$0")"
REPO=$(pwd)

YES=0; CHECK=0; WRAPPER=1; SERVICES=1; CPSEND=1; PANEL=1
for arg in "$@"; do
  case "$arg" in
    --yes|-y)      YES=1 ;;
    --check)       CHECK=1 ;;
    --no-wrapper)  WRAPPER=0 ;;
    --no-services) SERVICES=0 ;;
    --no-cp-send)  CPSEND=0 ;;
    --no-panel)    PANEL=0 ;;
    *) echo "flag desconhecida: $arg"; exit 1 ;;
  esac
done

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
nota() { printf '      \033[2m%s\033[0m\n' "$*"; }
falta(){ printf '  \033[33m--\033[0m  %s\n' "$*"; }
erro() { printf '  \033[31mX\033[0m   %s\n' "$*"; }
fail() { erro "$*"; exit 1; }

ask() { # ask "pergunta" -> 0/1 (em --yes, sempre sim)
  [ "$YES" = 1 ] && return 0
  read -r -p "  $1 [S/n] " r
  [ -z "$r" ] || [[ "$r" =~ ^[SsYy] ]]
}

PENDENTE=()

# Gerenciador de pacotes do sistema, pro único dep que precisa de root (tmux).
detecta_pkg() {
  for p in pacman apt-get dnf zypper apk brew; do
    command -v "$p" >/dev/null || continue
    case "$p" in
      pacman)  echo "sudo pacman -S --needed" ;;
      apt-get) echo "sudo apt-get install -y" ;;
      dnf)     echo "sudo dnf install -y" ;;
      zypper)  echo "sudo zypper install -y" ;;
      apk)     echo "sudo apk add" ;;
      brew)    echo "brew install" ;;   # brew nunca com sudo, de propósito
    esac
    return
  done
}
PKG=$(detecta_pkg)

# Instala o que cai no $HOME sem root. Separado de propósito do tier que precisa de sudo:
# um instalador que pede senha sem avisar é como se perde a confiança de quem está rodando.
precisa_home() { # precisa_home <rótulo> <cmd> <comando de instalação> <pra quê>
  local rotulo=$1 cmd=$2 instalacao=$3 porque=$4
  if command -v "$cmd" >/dev/null; then ok "$rotulo"; return 0; fi
  if [ "$CHECK" = 1 ]; then falta "$rotulo — $porque"; nota "$instalacao"; PENDENTE+=("$rotulo"); return 1; fi
  echo "  .. $rotulo não encontrado ($porque)"
  nota "$instalacao"
  if ask "Instalar agora? (vai pro teu \$HOME, sem sudo)"; then
    eval "$instalacao" >/dev/null 2>&1 || true
    # O instalador põe em ~/.local/bin, que pode não estar no PATH DESTE shell.
    export PATH="$HOME/.local/bin:$HOME/.local/share/fnm:$PATH"
    hash -r 2>/dev/null || true
    if command -v "$cmd" >/dev/null; then ok "$rotulo instalado"; return 0; fi
  fi
  erro "$rotulo continua faltando"; PENDENTE+=("$rotulo"); return 1
}

precisa_root() { # precisa_root <rótulo> <cmd> <pacote> <pra quê>
  local rotulo=$1 cmd=$2 pacote=$3 porque=$4
  if command -v "$cmd" >/dev/null; then ok "$rotulo"; return 0; fi
  if [ -z "$PKG" ]; then
    erro "$rotulo faltando e não reconheci o gerenciador de pacotes — instale $pacote na mão"
    PENDENTE+=("$rotulo"); return 1
  fi
  if [ "$CHECK" = 1 ]; then falta "$rotulo — $porque"; nota "$PKG $pacote"; PENDENTE+=("$rotulo"); return 1; fi
  echo "  .. $rotulo não encontrado ($porque)"
  nota "$PKG $pacote     <- precisa de senha de administrador"
  if ask "Rodar esse comando?"; then
    eval "$PKG $pacote" && { ok "$rotulo instalado"; return 0; }
  fi
  erro "$rotulo continua faltando"; PENDENTE+=("$rotulo"); return 1
}

# ── 1/8 Dependências ─────────────────────────────────────────────────────────
say "1/8 Dependências"
precisa_root "tmux"        tmux   tmux 'sem ele não existe sessão' || true
precisa_home "Claude Code" claude 'curl -fsSL https://claude.ai/install.sh | bash' 'é o que o app pilota' || true
precisa_home "uv"          uv     'curl -LsSf https://astral.sh/uv/install.sh | sh' 'gerencia o venv do backend' || true
if ! command -v npm >/dev/null; then
  precisa_home "Node 20+" node \
    'curl -fsSL https://fnm.vercel.app/install | bash && "$HOME/.local/share/fnm/fnm" install 22 && "$HOME/.local/share/fnm/fnm" default 22' \
    'o frontend é Svelte' || true
elif ! node -e 'process.exit(parseInt(process.versions.node) >= 20 ? 0 : 1)' 2>/dev/null; then
  erro "Node 20+ é necessário (atual: $(node --version))"; PENDENTE+=("Node 20+")
else
  ok "node $(node --version)"
fi

# Codex é OPCIONAL: o app é primariamente um cockpit de Claude Code. Exigir o binário aqui
# travava a instalação inteira de quem só usa Claude.
if command -v codex >/dev/null; then ok "codex"; else
  falta "codex ausente — sessões Codex indisponíveis, o resto funciona"
  nota "habilitar depois: https://developers.openai.com/codex/cli"
fi
command -v git >/dev/null && ok "git" || falta "git ausente — o painel de git e o chip de branch ficam vazios"

if [ "$CHECK" = 1 ]; then
  [ ${#PENDENTE[@]} -eq 0 ] && { say "Nada faltando."; exit 0; }
  say "Faltam: ${PENDENTE[*]}"; exit 1
fi
[ ${#PENDENTE[@]} -eq 0 ] || fail "faltam: ${PENDENTE[*]}"

# ── 2/8 Backend ──────────────────────────────────────────────────────────────
say "2/8 Backend"
(cd backend && uv sync --quiet)
ok "dependências instaladas"
nota "psutil NÃO entra aqui: no Linux existe /proc e ele é mais rápido (ver app/procinfo.py)"

# ── 3/8 Token de acesso ──────────────────────────────────────────────────────
say "3/8 Token de acesso"
gera_token() { openssl rand -hex 24 2>/dev/null || python3 -c 'import secrets; print(secrets.token_hex(24))'; }
if [ -f backend/.env ] && grep -q '^CP_AUTH_TOKEN=' backend/.env; then
  ok "backend/.env já tem CP_AUTH_TOKEN (mantido)"
elif [ "$YES" = 1 ]; then
  printf 'CP_AUTH_TOKEN=%s\n' "$(gera_token)" >> backend/.env
  ok "token aleatório gerado (--yes não pergunta)"
else
  echo "  Você vai DIGITAR este token no celular, então escolha algo que lembre."
  echo "  Enter em branco = gera um aleatório de 48 caracteres (seguro, chato de digitar)."
  nota "Com Tailscale a rede já é fechada e o token é a segunda tranca. No Wi-Fi de casa ele"
  nota "é a ÚNICA: quem estiver na rede e acertar a senha roda comando como você."
  while :; do
    read -r -p "  Token: " TOKEN
    [ -z "$TOKEN" ] && { TOKEN=$(gera_token); ok "aleatório gerado"; break; }
    # O backend só recusa o literal 'change-me'; o piso de 8 é daqui, pra senha curta não passar.
    [ ${#TOKEN} -lt 8 ] && { erro "curto demais — no mínimo 8 caracteres"; continue; }
    [ "$TOKEN" = "change-me" ] && { erro "esse valor o backend recusa de propósito"; continue; }
    break
  done
  printf 'CP_AUTH_TOKEN=%s\n' "$TOKEN" >> backend/.env
  ok "CP_AUTH_TOKEN gravado em backend/.env"
fi
nota "É esse token que você digita no celular na primeira conexão."

# ── 4/8 Frontend ─────────────────────────────────────────────────────────────
say "4/8 Frontend"
(cd frontend && npm ci --silent && npm run build --silent)
ok "buildado em frontend/dist/"

# ── 5/8 Wrappers do claude e do codex ────────────────────────────────────────
# Sem eles um `claude` que VOCÊ abre no terminal é invisível pro app: sem --session-id o backend
# não sabe qual transcript é daquela sessão, e fora do tmux não há pane pra ler estado nem
# receber input. Sessão criada PELO app funciona de qualquer jeito; isto é a outra direção.
say "5/8 Wrappers do claude e do codex"
if [ "$WRAPPER" = 1 ] && ask "Instalar (recomendado)?"; then
  ./scripts/install-claude-wrapper.sh
else
  nota "pulado — sessão aberta no terminal não vai aparecer no app"
  nota "depois: ./scripts/install-claude-wrapper.sh"
fi

# ── 6/8 Acesso pelo celular ──────────────────────────────────────────────────
say "6/8 Acesso pelo celular"
echo "  Duas formas, e elas não competem:"
echo "    LAN       — celular no mesmo Wi-Fi. Precisa liberar as portas no firewall."
echo "    Tailscale — VPN pessoal. Funciona de QUALQUER lugar sem expor nada pra internet."
nota "Fora de casa, use Tailscale. NUNCA abra porta pra internet pública: o app roda o"
nota "claude como VOCÊ, então um host exposto é execução remota na sua máquina."

if command -v ufw >/dev/null || command -v firewall-cmd >/dev/null; then
  if ask "Liberar as portas 8765 e 5173 no firewall? (pede sudo)"; then
    sudo ./scripts/lan-setup.sh 8765 && sudo ./scripts/lan-setup.sh 5173 && ok "portas liberadas"
  fi
else
  nota "sem ufw/firewalld — provavelmente não há firewall bloqueando (padrão de Arch/CachyOS)"
fi

if command -v tailscale >/dev/null; then
  ok "Tailscale já instalado"
  nota "Depois do 'tailscale up', ponha o nome .ts.net em CP_PUBLIC_URL no backend/.env"
  nota "pra o QR sair com o endereço certo em vez do IP da LAN."
elif ask "Instalar o Tailscale? (VPN pessoal — acesso de fora de casa)"; then
  curl -fsSL https://tailscale.com/install.sh | sh && ok "Tailscale instalado"
  nota "Falta logar: rode 'sudo tailscale up' e instale o Tailscale também no celular."
fi

# ── 7/8 Rodar sozinho + sessões-irmãs + painel ───────────────────────────────
say "7/8 Serviços, cp-send e painel"
if [ "$SERVICES" = 1 ] && command -v systemctl >/dev/null && ask "Rodar backend+frontend como serviços de usuário (sobrevivem a fechar o terminal)?"; then
  ./scripts/services-setup.sh
  nota "Pra sobreviver a logout/reboot também: loginctl enable-linger \$USER"
else
  nota "pulado — rodando na mão, fechar o terminal derruba o backend"
fi

if [ "$CPSEND" = 1 ] && ask "Instalar cp-send + skills (sessões conversam entre si e se pareiam)?"; then
  ./scripts/install-cp-send.sh
else
  nota "pulado — depois: ./scripts/install-cp-send.sh"
fi

# Painel flutuante + tray. Só Hyprland com Quickshell (testado no rice end-4/dots-hyprland).
if [ "$PANEL" = 1 ] && command -v qs >/dev/null && pgrep -x Hyprland >/dev/null; then
  ask "Instalar painel flutuante + tray (SUPER+SHIFT+U)?" && ./scripts/install-cp-panel.sh
else
  nota "painel do desktop: pulado (requer Hyprland + Quickshell)"
fi

# ── 8/8 Checagem de fumaça ───────────────────────────────────────────────────
# Até aqui foi tudo instalação. Este passo separa "instalou" de "funciona" — e é o mesmo passo
# que, do lado Windows, pegou o backend não importando por causa de um `import fcntl`.
say "8/8 Checagem de fumaça"
(cd backend && uv run python -c "from app import api, registry, procinfo, projects" ) \
  && ok "o backend importa" || fail "o backend não importa nesta máquina"

(cd backend && uv run python -c "from app import procinfo; assert procinfo._TEM_PROC, 'sem /proc'") \
  && ok "leitura de processo via /proc funcionando" \
  || nota "sem /proc (macOS?) — o backend usa psutil, confira se ele foi instalado"

S="cp-fumaca-$$"
if tmux new-session -d -s "$S" -c /tmp 'sh' 2>/dev/null; then
  tmux kill-session -t "=$S" 2>/dev/null
  ok "o multiplexador cria e mata sessão"
else
  fail "o tmux não criou uma sessão de teste — o app não vai abrir sessão"
fi

say "Pronto"
cat <<EOF
  Rodar na mão (se você pulou os serviços):
      cd backend  && CP_LAN_BIND_IP=auto uv run python -m app.main
      cd frontend && npm run dev

  No celular: abra a URL do QR que o backend imprime e digite o token de backend/.env.
  Guia completo (Tailscale, instalar como PWA, cada tela): docs/USAGE.md
EOF
