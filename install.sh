#!/usr/bin/env bash
# claude-cockpit — instalação completa no Linux/macOS.
#
#   ./install.sh              # interativo
#   ./install.sh --yes        # aceita tudo (não pergunta nada)
#   ./install.sh --check      # só diz o que falta e sai, sem instalar nada
#   ./install.sh --update        # re-aplica só o que um `git pull` não atualiza sozinho
#   ./install.sh --no-frontend   # só o backend (o PWA já roda noutro lugar)
#   ./install.sh --no-wrapper --no-services --no-cp-send --no-panel   # pula partes
#
# Os sub-scripts (services-setup.sh, lan-setup.sh, install-cp-send.sh, ...) continuam
# rodáveis sozinhos; este aqui só faz com que um comando baste.
set -euo pipefail
cd "$(dirname "$0")"
REPO=$(pwd)

YES=0; CHECK=0; UPDATE=0; WRAPPER=1; SERVICES=1; CPSEND=1; PANEL=1; FRONTEND=1
for arg in "$@"; do
  case "$arg" in
    --yes|-y)      YES=1 ;;
    --check)       CHECK=1 ;;
    # --update: modo do hook post-merge. Re-aplica o que o `git pull` NÃO atualiza (units com
    # caminho cravado, o bloco de protocolo no ~/.claude/CLAUDE.md, deps do backend, build do
    # front) e NÃO toca em nada que peça senha ou decisão: sem instalar dependência, sem token,
    # sem firewall, sem Tailscale. Um hook que para pedindo sudo no meio de um pull é pior que
    # hook nenhum.
    --update)      UPDATE=1; YES=1 ;;
    --no-wrapper)  WRAPPER=0 ;;
    --no-services) SERVICES=0 ;;
    --no-cp-send)  CPSEND=0 ;;
    --no-panel)    PANEL=0 ;;
    --no-frontend) FRONTEND=0 ;;
    *) echo "flag desconhecida: $arg"; exit 1 ;;
  esac
done

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
nota() { printf '      \033[2m%s\033[0m\n' "$*"; }
falta(){ printf '  \033[33m--\033[0m  %s\n' "$*"; }
erro() { printf '  \033[31mX\033[0m   %s\n' "$*"; }
fail() { erro "$*"; exit 1; }

# Toda pergunta lê do TERMINAL, nunca do stdin do script. Sob `curl … | bash` (e sob o
# bootstrap.sh) o stdin é o cano do curl: um `read` normal recebia EOF na hora, devolvia string
# vazia — e vazio aqui vale como "sim". O instalador aceitaria firewall, serviços, Tailscale e
# cp-send sem ninguém ter respondido nada. Sem terminal (CI, cron), a resposta é NÃO.
if { exec 3</dev/tty; } 2>/dev/null; then TEM_TTY=1; else TEM_TTY=0; fi

ask() { # ask "pergunta" -> 0/1 (em --yes, sempre sim; sem terminal, sempre não)
  [ "$YES" = 1 ] && return 0
  if [ "$TEM_TTY" = 0 ]; then
    printf '  \033[2m%s [S/n] -> sem terminal para responder, assumindo NÃO\033[0m\n' "$1"
    return 1
  fi
  local r=''
  printf '  %s [S/n] ' "$1"
  read -r r <&3 || r=''
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
  if [ "$UPDATE" = 1 ]; then erro "$rotulo faltando (--update não instala dependência)"; PENDENTE+=("$rotulo"); return 1; fi
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
  if [ "$UPDATE" = 1 ]; then erro "$rotulo faltando (--update não instala dependência)"; PENDENTE+=("$rotulo"); return 1; fi
  if ask "Rodar esse comando?"; then
    eval "$PKG $pacote" && { ok "$rotulo instalado"; return 0; }
  fi
  erro "$rotulo continua faltando"; PENDENTE+=("$rotulo"); return 1
}

# ── 1/8 Dependências ─────────────────────────────────────────────────────────
[ "$UPDATE" = 1 ] && say "Modo --update: só o que um git pull não atualiza sozinho" || true
say "1/8 Dependências"
precisa_root "tmux"        tmux   tmux 'sem ele não existe sessão' || true
precisa_home "Claude Code" claude 'curl -fsSL https://claude.ai/install.sh | bash' 'é o que o app pilota' || true
precisa_home "uv"          uv     'curl -LsSf https://astral.sh/uv/install.sh | sh' 'gerencia o venv do backend' || true
if [ "$FRONTEND" = 0 ]; then
  ok "Node: dispensado (--no-frontend)"
elif ! command -v npm >/dev/null; then
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
elif [ "$TEM_TTY" = 0 ]; then
  # Sem terminal não dá pra perguntar, mas token é a ÚNICA tranca do app: gerar um aleatório é
  # o lado seguro (o inseguro seria seguir sem token). O que não pode é fazer isso calado —
  # quem rodou precisa saber que existe um token e onde ele está.
  TOKEN=$(gera_token)
  printf 'CP_AUTH_TOKEN=%s\n' "$TOKEN" >> backend/.env
  # NÃO ecoar o valor: sem tty isto roda em provisionamento automatizado (CI, Ansible, ssh
  # não-interativo) e o stdout vira log. Pior aqui do que em qualquer projeto: o próprio app lê
  # `tmux capture-pane`, então uma instalação dentro de um pane monitorado gravaria o token no
  # histórico da ferramenta que ele protege. O PowerShell já não imprimia — era assimetria.
  falta "sem terminal para perguntar — token ALEATÓRIO gerado (valor em backend/.env)"
  nota "está em backend/.env; é ele que você digita no celular. Pra trocar, edite o arquivo."
else
  echo "  Você vai DIGITAR este token no celular, então escolha algo que lembre."
  echo "  Enter em branco = gera um aleatório de 48 caracteres (seguro, chato de digitar)."
  nota "Com Tailscale a rede já é fechada e o token é a segunda tranca. No Wi-Fi de casa ele"
  nota "é a ÚNICA: quem estiver na rede e acertar a senha roda comando como você."
  while :; do
    printf '  Token: '
    read -r TOKEN <&3 || TOKEN=''
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
if [ "$FRONTEND" = 0 ]; then
  ok "pulado (--no-frontend)"
  nota "UM frontend atende VÁRIOS backends: ele guarda a lista de servidores no próprio"
  nota "navegador (chave cp_servers) e você adiciona cada máquina pelo menu de conta."
  nota "Então dá pra ter o PWA numa VPS e só o backend em cada máquina de trabalho —"
  nota "menos porta aberta e nenhum processo node de graça por aqui."
  nota "Pra ligar este backend ao front que você já usa: abra o PWA, adicione o servidor"
  nota "com a URL que o backend imprime no QR, e cole o token de backend/.env."
else
# Só rebuilda se houver motivo: dist ausente, ou alguma fonte/lockfile mais novo que ele. Num
# re-run logo após um `git pull` sem mudança de front, isso economiza o `npm ci` inteiro.
DIST=frontend/dist/index.html
if [ -f "$DIST" ] && [ -z "$(find frontend/src frontend/package-lock.json frontend/index.html \
                              frontend/vite.config.* -newer "$DIST" -print -quit 2>/dev/null)" ]; then
  ok "frontend já buildado e atualizado (nada mudou desde o último build)"
else
  (cd frontend && npm ci --silent && npm run build --silent)
  ok "buildado em frontend/dist/"
fi
fi

# ── 5/8 Wrappers do claude e do codex ────────────────────────────────────────
# Sem eles um `claude` que VOCÊ abre no terminal é invisível pro app: sem --session-id o backend
# não sabe qual transcript é daquela sessão, e fora do tmux não há pane pra ler estado nem
# receber input. Sessão criada PELO app funciona de qualquer jeito; isto é a outra direção.
say "5/8 Wrappers do claude e do codex"
# Já instalado -> nem pergunta. Re-rodar o install.sh depois de um `git pull` deve pegar só o
# que falta, sem obrigar a responder S/n pro que já está de pé.
# Já instalado -> RE-RODA sem perguntar, em vez de pular. "Instalado" não é "atualizado": os
# sub-scripts geram conteúdo (blocos de rc, units, o texto do protocolo no ~/.claude/CLAUDE.md)
# que um `git pull` sozinho não atualiza. Eles são idempotentes, então re-rodar é barato; o que
# não pode voltar é perguntar S/n pro que já está de pé.
if [ -e "$HOME/.local/bin/cp-engine" ]; then
  ./scripts/install-claude-wrapper.sh >/dev/null && ok "wrappers atualizados" || erro "wrappers do claude/codex falharam ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$WRAPPER" = 1 ] && ask "Instalar (recomendado)?"; then
  ./scripts/install-claude-wrapper.sh
else
  nota "pulado — sessão aberta no terminal não vai aparecer no app"
  nota "depois: ./scripts/install-claude-wrapper.sh"
fi

# ── 6/8 Acesso pelo celular ──────────────────────────────────────────────────
say "6/8 Acesso pelo celular"
if [ "$UPDATE" = 1 ]; then
  ok "pulado no --update (firewall e Tailscale pedem senha; nada aqui muda com git pull)"
else
echo "  Duas formas, e elas não competem:"
echo "    LAN       — celular no mesmo Wi-Fi. Precisa liberar as portas no firewall."
echo "    Tailscale — VPN pessoal. Funciona de QUALQUER lugar sem expor nada pra internet."
nota "Fora de casa, use Tailscale. NUNCA abra porta pra internet pública: o app roda o"
nota "claude como VOCÊ, então um host exposto é execução remota na sua máquina."

porta_liberada() { # 0 = já tem regra pra esta porta
  if command -v ufw >/dev/null; then
    sudo -n ufw status 2>/dev/null | grep -q "^$1" && return 0
  fi
  if command -v firewall-cmd >/dev/null; then
    firewall-cmd --list-ports 2>/dev/null | grep -q "$1/tcp" && return 0
  fi
  return 1
}
if command -v ufw >/dev/null || command -v firewall-cmd >/dev/null; then
  if [ "$FRONTEND" = 0 ]; then PORTAS=(8765); else PORTAS=(8765 5173); fi
  if porta_liberada "${PORTAS[0]}" && { [ ${#PORTAS[@]} = 1 ] || porta_liberada 5173; }; then
    ok "portas 8765 e 5173 já liberadas no firewall"
  else
    nota "Liberar precisa de senha de administrador. Por fora seria:"
    nota "    sudo ./scripts/lan-setup.sh 8765 && sudo ./scripts/lan-setup.sh 5173"
    if ask "Liberar as portas 8765 e 5173 agora (vai pedir a senha)?"; then
      sudo ./scripts/lan-setup.sh 8765 && sudo ./scripts/lan-setup.sh 5173 && ok "portas liberadas" || erro "liberar portas no firewall falhou"
    fi
  fi
else
  nota "sem ufw/firewalld — provavelmente não há firewall bloqueando (padrão de Arch/CachyOS)"
fi

if command -v tailscale >/dev/null; then
  ok "Tailscale já instalado"
  if tailscale status >/dev/null 2>&1; then
    ok "e já está conectado ao teu tailnet"
    nota "Ponha o nome .ts.net em CP_PUBLIC_URL no backend/.env pra o QR sair com ele"
    nota "em vez do IP da LAN."
  else
    falta "instalado mas NÃO logado — rode: sudo tailscale up"
  fi
else
  echo "  O Tailscale põe teu PC e teu celular na mesma rede privada, de qualquer lugar do"
  echo "  mundo, sem abrir nenhuma porta pra internet. É como usar o app fora de casa."
  nota "A instalação é do sistema, então ela PEDE SUA SENHA de administrador."
  nota "Prefere fazer por fora? Rode isto e depois chame o install.sh de novo:"
  nota "    curl -fsSL https://tailscale.com/install.sh | sh"
  if ask "Instalar agora (vai pedir a senha)?"; then
    curl -fsSL https://tailscale.com/install.sh | sh && ok "Tailscale instalado" || erro "instalação do Tailscale falhou"
    nota "Falta logar: rode 'sudo tailscale up' e instale o Tailscale também no celular."
  else
    nota "pulado — o app segue funcionando na LAN (mesmo Wi-Fi)"
  fi
fi

fi

# ── 7/8 Rodar sozinho + sessões-irmãs + painel ───────────────────────────────
say "7/8 Serviços, cp-send e painel"
if ! command -v systemctl >/dev/null; then
  nota "serviços: sem systemd nesta máquina — rode backend e frontend na mão"
elif systemctl --user list-unit-files claude-cockpit-backend.service >/dev/null 2>&1 &&
     systemctl --user cat claude-cockpit-backend.service >/dev/null 2>&1; then
  # O caminho do node e o WorkingDirectory ficam CRAVADOS dentro da unit — git pull não os
  # muda. O próprio services-setup.sh só reinicia o que mudou de verdade, então re-rodar aqui
  # não derruba a conexão SSE do celular à toa.
  if [ "$FRONTEND" = 0 ]; then ./scripts/services-setup.sh --backend-only >/dev/null
  else ./scripts/services-setup.sh >/dev/null; fi
  ok "serviços atualizados ($(systemctl --user is-active claude-cockpit-backend.service 2>/dev/null))"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$SERVICES" = 1 ] && ask "Rodar backend+frontend como serviços de usuário (sobrevivem a fechar o terminal)?"; then
  if [ "$FRONTEND" = 0 ]; then ./scripts/services-setup.sh --backend-only
  else ./scripts/services-setup.sh; fi
  nota "Pra sobreviver a logout/reboot também: loginctl enable-linger \$USER"
else
  nota "pulado — rodando na mão, fechar o terminal derruba o backend"
fi

if [ -e "$HOME/.local/bin/cp-send" ]; then
  # O binário é symlink (atualiza sozinho), mas o bloco "Sessões-irmãs" do ~/.claude/CLAUDE.md
  # sai de um heredoc deste script: sem re-rodar, as sessões novas leem o protocolo VELHO.
  ./scripts/install-cp-send.sh >/dev/null && ok "cp-send + skills atualizados" || erro "cp-send + skills falharam ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$CPSEND" = 1 ] && ask "Instalar cp-send + skills (sessões conversam entre si e se pareiam)?"; then
  ./scripts/install-cp-send.sh
else
  nota "pulado — depois: ./scripts/install-cp-send.sh"
fi

# Sessões sobrevivendo a reboot: TPM + resurrect + continuum + um timer systemd que salva.
# Fica DEPOIS dos serviços de propósito — é o único passo aqui que clona repositório de
# terceiro (os plugins do tmux), então merece pergunta própria mesmo com --yes já dito.
if [ -d "$HOME/.tmux/plugins/tmux-resurrect" ]; then
  ./scripts/tmux-persist-setup.sh >/dev/null && ok "persistência de sessões atualizada" || erro "persistência de sessões falhou ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
else
  nota "Opcional: fazer as sessões voltarem depois de um reboot/OOM, com a conversa junto."
  nota "Clona 3 plugins de tmux de terceiros (tpm, resurrect, continuum) no teu ~/.tmux."
  ask "Instalar a persistência de sessões?" && ./scripts/tmux-persist-setup.sh
fi

# Painel flutuante + tray. Só Hyprland com Quickshell (testado no rice end-4/dots-hyprland).
if ! { command -v qs >/dev/null && pgrep -x Hyprland >/dev/null; }; then
  nota "painel do desktop: pulado (requer Hyprland + Quickshell)"
# Detecta pelo SYMLINK, nao pela unit systemd: medido nesta maquina, o painel roda sob `flock`
# direto (`qs -n -c claude-pocket`) e nao existe cp-panel.service — checar a unit dava "ausente"
# num painel instalado e vivo.
elif [ -e "$HOME/.local/bin/cp-panel-open" ]; then
  ok "painel + tray já instalados"
  # Único passo que NÃO re-roda sozinho: o painel é a única coisa aqui que está VISIVELMENTE
  # em execução no teu desktop, e a forma como ele sobe pode divergir da que o script gera
  # (medido: rodando sob flock, sem unit systemd). Re-instalar por conta própria mudaria algo
  # que funciona, sem pedir. Os arquivos são symlink, então QML e scripts já vêm do git pull.
  nota "atualizar de propósito (muda como o painel sobe): ./scripts/install-cp-panel.sh"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$PANEL" = 1 ] && ask "Instalar painel flutuante + tray (SUPER+SHIFT+U)?"; then
  ./scripts/install-cp-panel.sh
fi

# ── Atualizar sozinho no próximo git pull (opcional) ─────────────────────────
# Hook post-merge: roda depois de todo `git pull` bem-sucedido. A escolha de atualizar continua
# sendo tua — o pull é que dispara, e o pull você deu. Sem isto, um pull te deixa com código
# novo e units/protocolo velhos, e nada avisa.
HOOK=".git/hooks/post-merge"
if [ "$UPDATE" = 1 ]; then
  :   # o próprio hook está rodando; não se re-instala no meio da própria execução
elif [ ! -d .git ]; then
  nota "sem .git (cópia sem histórico?) — hook de atualização indisponível"
elif [ -f "$HOOK" ] && grep -q 'install.sh --update' "$HOOK"; then
  ok "hook de atualização já instalado"
  nota "remover: rm $HOOK"
elif [ -f "$HOOK" ]; then
  falta "já existe um $HOOK que não é nosso — não vou mexer nele"
  nota "pra somar, acrescente a linha:  ./install.sh --update"
else
  echo "  Daqui pra frente, um 'git pull' traz código novo — mas as units do systemd e o texto"
  echo "  do protocolo guardam cópia própria e ficariam velhos. Este hook re-aplica isso sozinho."
  nota "Ele só roda no pull, que é você quem dá. Nada nele pede senha. Desligar: rm $HOOK"
  if ask "Deixar o próximo 'git pull' já se atualizar sozinho?"; then
  # Corpo vem de scripts/post-merge.hook, FONTE ÚNICA compartilhada com o install.ps1 (que passou a
  # instalar o mesmo hook no Windows, onde ninguém o instalava). Era um heredoc aqui; com dois
  # instaladores, duas cópias inline divergiriam — e divergência de cópia duplicada é a família de
  # bug mais caro deste projeto.
    if [ -f scripts/post-merge.hook ]; then
      cp scripts/post-merge.hook "$HOOK"
      chmod +x "$HOOK"
      ok "hook instalado — o próximo 'git pull' já se atualiza sozinho"
    else
      falta "scripts/post-merge.hook não encontrado — hook não instalado"
    fi
  else
    nota "pulado — depois de um git pull, rode ./install.sh --update na mão"
  fi
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

  Mais de uma máquina? UM frontend atende VÁRIOS backends: ele guarda a lista de
  servidores no próprio navegador e você adiciona cada máquina pelo menu de conta.
  Dá pra deixar o PWA num lugar só (uma VPS, por exemplo) e nas outras rodar apenas
  o backend, com ./install.sh --no-frontend. O front é leve (~94 MB contra ~149 MB do
  backend, medido), então instalá-lo por padrão não custa caro — a flag existe pra
  quem já tem o PWA noutro lugar, não porque ele pese.
EOF
