#!/usr/bin/env bash
# hangar — instalação completa no Linux/macOS.
#
#   ./install.sh              # interativo
#   ./install.sh --yes        # aceita tudo (não pergunta nada)
#   ./install.sh --check      # só diz o que falta e sai, sem instalar nada
#   ./install.sh --update        # re-aplica só o que um `git pull` não atualiza sozinho
#   ./install.sh --no-frontend   # só o backend (o PWA já roda noutro lugar)
#   ./install.sh --no-wrapper --no-services --no-hangar-send --no-panel   # pula partes
#
# Os sub-scripts (services-setup.sh, lan-setup.sh, install-hangar-send.sh, ...) continuam
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
    # O nome antigo do flag segue aceito: está no histórico de comando de quem já instala assim.
    --no-hangar-send|--no-cp-send)  CPSEND=0 ;;
    --no-panel)    PANEL=0 ;;
    --no-frontend) FRONTEND=0 ;;
    *) echo "flag desconhecida: $arg"; exit 1 ;;
  esac
done

say() {
  # Título numerado ("3/8 ...") ganha a barra de progresso; os demais seguem só em negrito.
  local t="$*"
  if [[ $t =~ ^([0-9]+)/8\  ]]; then
    local n=${BASH_REMATCH[1]}
    printf '\n  \033[1;36m[%s%s] %s\033[0m\n' \
      "$(printf '%*s' "$n" '' | tr ' ' '#')" "$(printf '%*s' "$((8-n))" '' | tr ' ' '-')" "$t"
  else
    printf '\n\033[1m%s\033[0m\n' "$t"
  fi
}
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
nota() { printf '      \033[2m%s\033[0m\n' "$*"; }
falta(){ printf '  \033[33m--\033[0m  %s\n' "$*"; }
erro() { printf '  \033[31mX\033[0m   %s\n' "$*"; }
fail() { erro "$*"; exit 1; }

# Duas gravidades, e a diferença é o que acontece com os passos seguintes:
#  - ESSENCIAL falhou -> para na hora (fail): backend, token e frontend sustentam todos os
#    passos seguintes, e seguir adiante só enterrava a causa;
#  - EXTRA que a pessoa PEDIU falhou -> anota_problema: o app funciona sem ele, mas a falha
#    entra na lista do fim, que diz "terminou com pendências" em vez de "Pronto". Falha que só
#    imprime amarelo e some foi como instalações inteiras saíram com o Tailscale sem publicar
#    e ninguém soube na hora.
PROBLEMAS=()
anota_problema() { erro "$1"; PROBLEMAS+=("$1"); }

gira() { # gira <rótulo> <comando...>: spinner enquanto roda; sem TTY (ou --update), saída direta
  local rotulo=$1; shift
  if [ "$TEM_TTY" = 0 ] || [ "$UPDATE" = 1 ]; then "$@"; return $?; fi
  local tmp; tmp=$(mktemp)
  "$@" >"$tmp" 2>&1 &
  local pid=$! quadros='-\|/' i=0
  while kill -0 "$pid" 2>/dev/null; do
    printf '\r  %s %s ' "${quadros:$i:1}" "$rotulo"
    i=$(( (i+1) % 4 ))
    sleep 0.2
  done
  local rc=0; wait "$pid" || rc=$?
  printf '\r\033[K'
  if [ "$rc" != 0 ]; then cat "$tmp"; fi   # falhou: devolve a saída que o spinner escondeu
  rm -f "$tmp"
  return "$rc"
}

# Toda pergunta lê do TERMINAL, nunca do stdin do script. Sob `curl … | bash` (e sob o
# bootstrap.sh) o stdin é o cano do curl: um `read` normal recebia EOF na hora, devolvia string
# vazia — e vazio aqui vale como "sim". O instalador aceitaria firewall, serviços, Tailscale e
# hangar-send sem ninguém ter respondido nada. Sem terminal (CI, cron), a resposta é NÃO.
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

if [ "$UPDATE" = 0 ] && [ "$CHECK" = 0 ]; then
  echo
  echo "  +--------------------------------------------------+"
  echo "  |  hangar — instalacao                             |"
  echo "  |  cada etapa prova o que fez antes da proxima;    |"
  echo "  |  se algo falhar, eu paro ali e digo o conserto   |"
  echo "  +--------------------------------------------------+"
fi

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
(cd backend && uv sync --quiet) || fail "uv sync falhou — o backend ficou sem as dependências"
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
# Prova, não ausência de erro: o passo inteiro vale zero se o token não estiver de fato no arquivo.
grep -q '^CP_AUTH_TOKEN=.\{8,\}' backend/.env 2>/dev/null \
  || fail "o token não foi gravado em backend/.env — sem ele o celular não entra"
nota "É esse token que você digita no celular na primeira conexão."

# ── 4/8 Frontend ─────────────────────────────────────────────────────────────
# O CI compila o front a cada push na main e publica o resultado na release `dist-latest`. Baixar
# de lá evita o passo mais lento e mais frágil da instalação (o `npm ci` + build local).
DIST_URL=https://github.com/jeffer1312/hangar/releases/download/dist-latest
baixar_dist() { # 0 = frontend/dist agora tem o build DESTE commit
  command -v curl >/dev/null 2>&1 && command -v tar >/dev/null 2>&1 || return 1
  local sha_local sha_remoto tmp
  sha_local=$(git rev-parse HEAD 2>/dev/null) || return 1
  # Árvore suja no front = quem está editando quer o SEU código na tela, não o do CI.
  [ -z "$(git status --porcelain -- frontend 2>/dev/null)" ] || return 1
  # O .sha primeiro, que são 200 bytes: dist de OUTRO commit serve tela velha contra API nova, e
  # esse defeito é mudo. Não bateu (CI ainda compilando, push agorinha) → cai no build local.
  sha_remoto=$(curl -fsSL --max-time 15 "$DIST_URL/frontend-dist.sha" 2>/dev/null) || return 1
  [ "$sha_remoto" = "$sha_local" ] || return 1
  tmp=$(mktemp -d "frontend/.dist-baixado.XXXXXX") || return 1
  # Extrai ao LADO do dist e só então troca: um download interrompido no meio não pode deixar a
  # máquina sem front nenhum — o build local depois nem roda, porque este caminho já disse "ok".
  if curl -fsSL --max-time 180 "$DIST_URL/frontend-dist.tar.gz" 2>/dev/null | tar -xzf - -C "$tmp" \
     && [ -f "$tmp/index.html" ]; then
    rm -rf frontend/dist && mv "$tmp" frontend/dist && return 0
  fi
  rm -rf "$tmp"
  return 1
}
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
elif baixar_dist; then
  ok "dist baixado do CI (não precisou compilar aqui)"
else
  # Sem --silent no --update (o modo que o BOTÃO Atualizar usa): a caixinha da tela mostra esta
  # saída ao vivo, e com --silent o npm não imprime nada — a tela fica idêntica a uma travada
  # durante o minuto do `npm ci`. No modo interativo o --silent fica, pra não poluir o terminal.
  QUIETO=--silent; [ "$UPDATE" = 1 ] && QUIETO=
  # A flag vai ANTES do nome do script: no npm 11 `npm run build --silent` não é mais consumida
  # pelo npm, ela é repassada ao script e chega no `vite build`, que morre com CACError.
  build_front() { (cd frontend && npm ci $QUIETO && npm run $QUIETO build); }
  gira "npm ci + build do frontend" build_front \
    && [ -f "$DIST" ] && ok "buildado em frontend/dist/" \
    || fail "o build do frontend falhou — corrige o erro acima e re-roda (ele continua de onde parou)"
fi
fi

# ── Janela nativa (Electron, shell/) ──────────────────────────────────────────
# Só as DEPENDÊNCIAS, nunca o `npm run dist`. O `git pull` traz o `main.cjs` novo, e quem roda o
# app a partir do repo já o executa no próximo start — mas se o `package-lock.json` do shell mudar
# (Electron novo, dependência nova), a janela roda com dependência velha e nada avisa. Empacotar
# (AppImage/NSIS) é outra coisa: leva minutos e produz um INSTALADOR, que alguém ainda tem que
# instalar — publicação, não atualização, e não cabe num botão que roda sozinho.
if [ -d shell ] && [ -f shell/package.json ]; then
  # Compara com `node_modules/.package-lock.json`, que o npm reescreve a CADA instalação — e não
  # com a pasta `node_modules`, cuja data não acompanha o que aconteceu dentro dela. Medido aqui:
  # a pasta era de 16/08 e o lock de 22/08, então a condição dava "precisa" toda vez e um `npm ci`
  # de minutos rodaria em cada atualização, à toa.
  MARCA_SHELL=shell/node_modules/.package-lock.json
  if [ ! -f "$MARCA_SHELL" ] || [ shell/package-lock.json -nt "$MARCA_SHELL" ]; then
    say "Janela nativa (Electron)"
    QUIETO_SHELL=--silent; [ "$UPDATE" = 1 ] && QUIETO_SHELL=
    build_shell() { (cd shell && npm ci $QUIETO_SHELL); }
    if gira "npm ci da janela nativa" build_shell; then
      ok "dependências da janela instaladas"
    else
      # Não derruba a atualização: o app funciona no navegador sem a janela nativa.
      falta "npm ci do shell/ falhou — a janela nativa pode não abrir (rode: cd shell && npm ci)"
      # Marca canônica (não traduzida, não colorida): é como o motor da atualização sabe que algo
      # ficou pra trás sem o instalador precisar falhar inteiro. Sem ela, a tela dizia "Atualizado"
      # com a janela nativa quebrada, e a única pista era uma linha amarela perdida no log.
      echo "##HANGAR-AVISO## a janela nativa (Electron) ficou com dependencias desatualizadas"
    fi
  else
    ok "janela nativa já com as dependências em dia"
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
if [ -e "$HOME/.local/bin/hangar-engine" ]; then
  ./scripts/install-claude-wrapper.sh >/dev/null && ok "wrappers atualizados" || anota_problema "wrappers do claude/codex falharam ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$WRAPPER" = 1 ] && ask "Instalar (recomendado)?"; then
  ./scripts/install-claude-wrapper.sh || anota_problema "wrappers do claude/codex não instalaram"
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
  # A 5173 só entra quando ESTA máquina tem serviço de front (instalação antiga que ficou no
  # `vite preview`). Sem ele quem serve a interface é o backend, na 8765, e abrir uma porta que
  # ninguém escuta é furo aberto de graça. Mesma pergunta que o services-setup.sh faz depois.
  PORTAS=(8765)
  [ "$FRONTEND" = 1 ] && [ -f "$HOME/.config/systemd/user/hangar-frontend.service" ] && PORTAS+=(5173)
  LISTA="${PORTAS[*]}"
  FALTA=0
  for p in "${PORTAS[@]}"; do porta_liberada "$p" || FALTA=1; done
  if [ "$FALTA" = 0 ]; then
    ok "porta(s) $LISTA já liberada(s) no firewall"
  else
    nota "Liberar precisa de senha de administrador. Por fora seria:"
    for p in "${PORTAS[@]}"; do nota "    sudo ./scripts/lan-setup.sh $p"; done
    if ask "Liberar a(s) porta(s) $LISTA agora (vai pedir a senha)?"; then
      OK_FW=1
      for p in "${PORTAS[@]}"; do sudo ./scripts/lan-setup.sh "$p" || OK_FW=0; done
      [ "$OK_FW" = 1 ] && ok "portas liberadas" || anota_problema "liberar portas no firewall falhou"
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
    if curl -fsSL https://tailscale.com/install.sh | sh && command -v tailscale >/dev/null; then
      ok "Tailscale instalado"
    else
      anota_problema "instalação do Tailscale falhou"
    fi
    nota "Falta logar: rode 'sudo tailscale up' e instale o Tailscale também no celular."
  else
    nota "pulado — o app segue funcionando na LAN (mesmo Wi-Fi)"
  fi
fi

fi

# ── 7/8 Rodar sozinho + sessões-irmãs + painel ───────────────────────────────
say "7/8 Serviços, hangar-send e painel"
if ! command -v systemctl >/dev/null; then
  nota "serviços: sem systemd nesta máquina — rode backend e frontend na mão"
elif systemctl --user list-unit-files hangar-backend.service >/dev/null 2>&1 &&
     systemctl --user cat hangar-backend.service >/dev/null 2>&1; then
  # O caminho do node e o WorkingDirectory ficam CRAVADOS dentro da unit — git pull não os
  # muda. O próprio services-setup.sh só reinicia o que mudou de verdade, então re-rodar aqui
  # não derruba a conexão SSE do celular à toa.
  # O `|| true` é pra chegar na prova: quem decide é o is-active logo abaixo, e sem ele o
  # set -e abortaria aqui com a saída do setup mas sem a mensagem.
  if [ "$FRONTEND" = 0 ]; then ./scripts/services-setup.sh --backend-only >/dev/null || true
  else ./scripts/services-setup.sh >/dev/null || true; fi
  # A prova é o serviço ACTIVE, não o exit 0 do setup — unit reescrita e serviço morto é o
  # par que um "ok" sem checagem esconderia.
  [ "$(systemctl --user is-active hangar-backend.service 2>/dev/null)" = active ] \
    && ok "serviços atualizados (active)" \
    || anota_problema "serviços atualizados mas o hangar-backend não está active"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$SERVICES" = 1 ] && ask "Rodar backend+frontend como serviços de usuário (sobrevivem a fechar o terminal)?"; then
  if [ "$FRONTEND" = 0 ]; then ./scripts/services-setup.sh --backend-only || true
  else ./scripts/services-setup.sh || true; fi
  [ "$(systemctl --user is-active hangar-backend.service 2>/dev/null)" = active ] \
    || anota_problema "serviços instalados mas o hangar-backend não está active"
  nota "Pra sobreviver a logout/reboot também: loginctl enable-linger \$USER"
else
  nota "pulado — rodando na mão, fechar o terminal derruba o backend"
fi

if [ -e "$HOME/.local/bin/hangar-send" ]; then
  # O binário é symlink (atualiza sozinho), mas o bloco "Sessões-irmãs" do ~/.claude/CLAUDE.md
  # sai de um heredoc deste script: sem re-rodar, as sessões novas leem o protocolo VELHO.
  ./scripts/install-hangar-send.sh >/dev/null && ok "hangar-send + skills atualizados" || anota_problema "hangar-send + skills falharam ao atualizar"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$CPSEND" = 1 ] && ask "Instalar hangar-send + skills (sessões conversam entre si e se pareiam)?"; then
  ./scripts/install-hangar-send.sh || anota_problema "hangar-send + skills não instalaram"
else
  nota "pulado — depois: ./scripts/install-hangar-send.sh"
fi

# Ponte de skills pro Pi e pro Kimi. Roda SEMPRE (inclusive no --update): quem cria sessão nesses
# dois agentes é este app, e uma sessão nascida assim não enxerga as skills do Claude sem a ponte.
# Sai 0 e não faz nada quando nenhum dos dois está instalado.
./scripts/install-skills-bridge.sh >/dev/null 2>&1 \
  && ok "ponte de skills (Pi/Kimi) atualizada" \
  || nota "ponte de skills pulada — depois: ./scripts/install-skills-bridge.sh"

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
  ask "Instalar a persistência de sessões?" && { ./scripts/tmux-persist-setup.sh || anota_problema "persistência de sessões não instalou"; }
fi

# Painel flutuante + tray. Só Hyprland com Quickshell (testado no rice end-4/dots-hyprland).
if ! { command -v qs >/dev/null && pgrep -x Hyprland >/dev/null; }; then
  nota "painel do desktop: pulado (requer Hyprland + Quickshell)"
# Detecta pelo SYMLINK, nao pela unit systemd: medido nesta maquina, o painel roda sob `flock`
# direto (`qs -n -c hangar`) e nao existe hangar-panel.service — checar a unit dava "ausente"
# num painel instalado e vivo.
elif [ -e "$HOME/.local/bin/hangar-panel-open" ]; then
  ok "painel + tray já instalados"
  # Único passo que NÃO re-roda sozinho: o painel é a única coisa aqui que está VISIVELMENTE
  # em execução no teu desktop, e a forma como ele sobe pode divergir da que o script gera
  # (medido: rodando sob flock, sem unit systemd). Re-instalar por conta própria mudaria algo
  # que funciona, sem pedir. Os arquivos são symlink, então QML e scripts já vêm do git pull.
  nota "atualizar de propósito (muda como o painel sobe): ./scripts/install-hangar-panel.sh"
elif [ "$UPDATE" = 1 ]; then
  :   # não instala coisa nova num --update; isso é decisão, não atualização
elif [ "$PANEL" = 1 ] && ask "Instalar painel flutuante + tray (SUPER+SHIFT+U)?"; then
  ./scripts/install-hangar-panel.sh || anota_problema "painel do desktop não instalou"
fi

# ── Passos de atualização: marcar como já feitos ─────────────────────────────
# Uma instalação do ZERO já satisfaz todo passo de `docs/atualizacoes/` — eles existem pra levar
# uma máquina ANTIGA até aqui. Sem esta marca, a primeira vez que essa máquina apertasse Atualizar
# no app, ela rodaria a história inteira de passos, todos já cumpridos por este instalador.
# No --update NÃO se marca nada: ali a máquina é justamente a antiga, e os passos precisam rodar.
if [ "$UPDATE" = 1 ]; then
  :
else
  say "Passos de atualização"
  if uv run --directory backend python -c "from app import atualizacoes; atualizacoes.marcar_todos()" 2>/dev/null; then
    ok "marcados como já aplicados (instalação nova)"
  else
    nota "não consegui marcar agora — o app resolve no primeiro Atualizar"
  fi
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

# ── Portão final: extra que a pessoa pediu e falhou NÃO passa em branco ────────────────────
if [ ${#PROBLEMAS[@]} -gt 0 ]; then
  say "Terminou com pendências"
  for p in "${PROBLEMAS[@]}"; do falta "$p"; done
  echo "  O que já estava no ar continua no ar — e é por isso que isto precisa ser dito alto:"
  echo "  a tela pode seguir funcionando e a instalação PARECER boa. Resolve a lista e re-roda;"
  echo "  o instalador é idempotente e continua de onde parou."
  # No --update (hook do pull / botão do app) sai 0 mesmo assim: derrubar a atualização inteira
  # por causa de um extra opcional seria pior — e o ##HANGAR-AVISO## já carrega o aviso pra tela.
  [ "$UPDATE" = 1 ] || exit 1
fi

say "Pronto"
PORTA_FIM=$(grep '^CP_PORT=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- || true)
PORTA_FIM=${PORTA_FIM:-8765}
URL_FIM=$(grep '^CP_PUBLIC_URL=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- || true)
# O valor do token só aparece com terminal: sem TTY isto roda em provisionamento e o stdout
# vira log — mesma regra do passo 3/8.
TOKEN_FIM="(está em backend/.env)"
[ "$TEM_TTY" = 1 ] && TOKEN_FIM=$(grep '^CP_AUTH_TOKEN=' backend/.env 2>/dev/null | tail -1 | cut -d= -f2- || true)
echo
echo "  +---------------------------------------------------------------"
echo "   RESUMO"
echo "   token   : $TOKEN_FIM"
echo "   local   : http://127.0.0.1:$PORTA_FIM"
if [ -n "$URL_FIM" ]; then echo "   celular : $URL_FIM"
else echo "   celular : não publicado no Tailscale"; fi
echo "  +---------------------------------------------------------------"
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
