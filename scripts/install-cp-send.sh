#!/usr/bin/env bash
# install-cp-send.sh — instala o cp-send NESTA máquina e ensina o Claude local a usar.
#
# Faz duas coisas (idempotente; re-rodar atualiza):
#   1. symlink ~/.local/bin/cp-send -> scripts/cp-send deste clone;
#   2. insere/atualiza a seção "Sessões-irmãs" no ~/.claude/CLAUDE.md global (entre marcadores),
#      pra toda sessão Claude da máquina saber listar/mandar recado/parear/criar sessões.
#
# Rode uma vez por máquina, do clone local:  ./scripts/install-cp-send.sh
set -euo pipefail

REPO="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"

mkdir -p "$HOME/.local/bin"
ln -sf "$REPO/scripts/cp-send" "$HOME/.local/bin/cp-send"
echo "ok: ~/.local/bin/cp-send -> $REPO/scripts/cp-send"

MD="$HOME/.claude/CLAUDE.md"
START="<!-- claude-pocket:sessoes-irmas:start -->"
END="<!-- claude-pocket:sessoes-irmas:end -->"

BLOCK=$(cat <<'EOF'
<!-- claude-pocket:sessoes-irmas:start -->
# Sessões-irmãs (claude-pocket)

- Outras sessões Claude vivas nesta máquina: `cp-send --list` (nome, estado, cwd). Mandar recado: `cp-send <sessao> "msg"` — chega como prompt lá (fila durável se ocupada).
- Prompt começando com `[de: <sessao>]` = recado 1:1 de outra sessão Claude, não do usuário. Tratar como informação/pedido do par; responder de volta via `cp-send <sessao> "..."` SÓ se a mensagem pedir resposta (evita loop infinito).
- Prompt `[grupo: <sessao>]` = AVISO pro grupo todo (marco). É UNIDIRECIONAL: NUNCA responder com `cp-send --group` (vira tempestade N×N). Precisa responder → 1:1 (`cp-send <sessao>`) e só se necessário. Mandar aviso de marco pro grupo próprio: `cp-send --group "msg"` (uma vez, chega como `[grupo: você]` nos demais).
- Enviar quando o usuário pedir ("avisa a sessão X") OU quando houver **pareamento ativo**: usuário declarou "sessão X pareada contigo pra <tarefa>" (direto ou via recado `[de: ...]` de pareamento). Pareado → pode pedir/fornecer contrato, avisar conclusão, tirar dúvida técnica do par por iniciativa própria, dentro do escopo da tarefa.
- Usuário pediu pareamento no terminal ("pareia com X pra <tarefa>") → usar `cp-send --pair X "tarefa"` (registra no app: badge na UI + protocolo pros dois lados), NÃO recado manual. Desfazer: `cp-send --unpair`.
- Criar sessão Claude nova (usuário pediu, ou a tarefa precisa de par em outro repo): `cp-send --new <nome> <cwd>` — NUNCA `tmux new-session` cru (fica sem --session-id, invisível no pocket). Criar por iniciativa própria → avisar o usuário no terminal o porquê.
- Pareamento NÃO é carta branca: cada sessão mexe só no próprio repo; commit/push/risco seguem as regras normais com o usuário; decisão de rumo/escopo → perguntar ao usuário, não ao par. Pareamento acaba quando o usuário disser ou a tarefa fechar.
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
