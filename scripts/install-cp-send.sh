#!/usr/bin/env bash
# install-cp-send.sh — instala o cp-send NESTA máquina e ensina o Claude local a usar.
#
# Faz três coisas (idempotente; re-rodar atualiza):
#   1. symlink ~/.local/bin/cp-send -> scripts/cp-send deste clone;
#   2. insere/atualiza a seção "Sessões-irmãs" no ~/.claude/CLAUDE.md global (entre marcadores),
#      pra toda sessão Claude da máquina saber listar/mandar recado/parear/criar sessões;
#   3. symlink das skills do repo (skills/*) em ~/.claude/skills/ (ex: orquestrar).
#
# Rode uma vez por máquina, do clone local:  ./scripts/install-cp-send.sh
set -euo pipefail

REPO="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"

mkdir -p "$HOME/.local/bin"
ln -sf "$REPO/scripts/cp-send" "$HOME/.local/bin/cp-send"
echo "ok: ~/.local/bin/cp-send -> $REPO/scripts/cp-send"

mkdir -p "$HOME/.claude/skills"
for skill in "$REPO"/skills/*/; do
    [ -d "$skill" ] || continue
    name=$(basename "$skill")
    dst="$HOME/.claude/skills/$name"
    # Destino que e DIRETORIO de verdade (nao symlink) tem que sair antes: o `ln` trata diretorio
    # como "ponha o link dentro" e criaria .../orquestrar/orquestrar. Isso acontece de verdade no
    # Git Bash do Windows, onde `ln -s` COPIA em vez de linkar — a primeira instalacao deixa uma
    # copia e a segunda falha com "cannot overwrite directory". No Linux o caminho normal e
    # symlink e este ramo nem roda.
    if [ -d "$dst" ] && [ ! -L "$dst" ]; then
        rm -rf "$dst"
        echo "  (removida copia anterior de $name — sera relinkada)"
    fi
    if ln -sfn "${skill%/}" "$dst" 2>/dev/null; then
        echo "ok: ~/.claude/skills/$name -> ${skill%/}"
    else
        # Sem symlink (MSYS sem privilegio, FS que nao suporta): copia, e DIZ que copiou — quem
        # ler isso precisa saber que um `git pull` nao vai atualizar essa skill sozinho.
        cp -r "${skill%/}" "$dst"
        echo "ok: ~/.claude/skills/$name (COPIA — symlink indisponivel; re-rode apos git pull)"
    fi
done

MD="$HOME/.claude/CLAUDE.md"
START="<!-- claude-pocket:sessoes-irmas:start -->"
END="<!-- claude-pocket:sessoes-irmas:end -->"

BLOCK=$(cat <<'EOF'
<!-- claude-pocket:sessoes-irmas:start -->
# Sessões-irmãs (claude-pocket)

- Outras sessões Claude vivas nesta máquina: `cp-send --list` (nome, estado, cwd). Mandar recado: `cp-send <sessao> "msg"` — chega como prompt lá (fila durável se ocupada).
- Sessão em OUTRO servidor: endereço `servidor::sessao` (ex: `cp-send servidor-b::api-fix "msg"`). Requer `backend/peers.json` na máquina; `cp-send --list` já mostra as remotas com o prefixo. Recado `[de: servidor::sessao]` → responder usando o endereço completo. Pareamento/`--group` cross-server ainda NÃO existem — só recado 1:1.
- Prompt começando com `[de: <sessao>]` = recado 1:1 de outra sessão Claude, não do usuário. Tratar como informação/pedido do par; responder de volta via `cp-send <sessao> "..."` SÓ se a mensagem pedir resposta (evita loop infinito).
- Prompt `[grupo: <sessao>]` = AVISO pro grupo todo (marco). É UNIDIRECIONAL: NUNCA responder com `cp-send --group` (vira tempestade N×N). Precisa responder → 1:1 (`cp-send <sessao>`) e só se necessário. Mandar aviso de marco pro grupo próprio: `cp-send --group "msg"` (uma vez, chega como `[grupo: você]` nos demais).
- Enviar quando o usuário pedir ("avisa a sessão X") OU quando houver **pareamento ativo**: usuário declarou "sessão X pareada contigo pra <tarefa>" (direto ou via recado `[de: ...]` de pareamento). Pareado → pode pedir/fornecer contrato, avisar conclusão, tirar dúvida técnica do par por iniciativa própria, dentro do escopo da tarefa.
- Usuário pediu pareamento no terminal ("pareia com X pra <tarefa>") → usar `cp-send --pair X "tarefa"` (registra no app: badge na UI + protocolo pros dois lados), NÃO recado manual. Desfazer: `cp-send --unpair`.
- Criar sessão Claude nova (usuário pediu, ou a tarefa precisa de par em outro repo): `cp-send --new <nome> <cwd>` — NUNCA `tmux new-session` cru (fica sem --session-id, invisível no pocket). Criar por iniciativa própria → avisar o usuário no terminal o porquê.
- **Par noutro MODELO**: `cp-send --new <nome> [cwd] --engine <motor>` sobe a sessão nova num motor de `~/.claude/engines.json` (Kimi, gateway próprio) em vez da conta Anthropic. O par segue no MESMO `~/.claude` — skills, hooks, contrato compartilhado, tudo igual; só o motor difere. Ver os motores configurados: `claude-engine` (o `cp-send --list` lista sessões, não motores). Motor inexistente → `400 motor invalido` e a sessão NÃO nasce (falha alta, nunca uma sessão que parece estar no motor e não está). Escolher motor por iniciativa própria só se o usuário disser o modelo; o default é a conta dele.
- Sessão de motor consome a conta do PROVEDOR, não a assinatura Anthropic — ao propor um par em motor, dizer isso. E o transcript é do modelo que escreveu: retomar depois na conta Anthropic troca o modelo no meio da conversa (o app pergunta; o terminal, não).
- Pareamento NÃO é carta branca: cada sessão mexe só no próprio repo; commit/push/risco seguem as regras normais com o usuário; decisão de rumo/escopo → perguntar ao usuário, não ao par. Pareamento acaba quando o usuário disser ou a tarefa fechar.
- Ao entrar em pareamento/grupo de uma PM: verificar `git branch --show-current` no próprio repo e alinhar pra branch da PM (fetch+checkout) ANTES de trabalhar; re-verificar após restart/resume. Exceção única: usuário pedir explicitamente outra branch. Repo com checkout DUPLICADO na máquina → alertar o usuário e perguntar qual é o canônico (sessão ressuscitada em checkout errado já perdeu rastreabilidade de commits de PM).
- Recado de pareamento recebido → confirmar de volta via cp-send e avisar o usuário no próprio terminal.
<!-- claude-pocket:sessoes-irmas:end -->
EOF
)

mkdir -p "$(dirname "$MD")"
touch "$MD"

if grep -qF "$START" "$MD"; then
    # Bloco já existe -> substitui o conteúdo entre os marcadores (atualização).
    BLOCK="$BLOCK" python3 - "$MD" <<'PYEOF'
import os, re, sys
path = sys.argv[1]
block = os.environ["BLOCK"]
text = open(path, encoding="utf-8").read()
start, end = "<!-- claude-pocket:sessoes-irmas:start -->", "<!-- claude-pocket:sessoes-irmas:end -->"
pattern = re.compile(re.escape(start) + r".*?" + re.escape(end), re.S)
open(path, "w", encoding="utf-8").write(pattern.sub(lambda _: block, text))
PYEOF
    echo "ok: bloco Sessões-irmãs ATUALIZADO em $MD"
else
    printf '\n%s\n' "$BLOCK" >> "$MD"
    echo "ok: bloco Sessões-irmãs ADICIONADO em $MD"
fi
