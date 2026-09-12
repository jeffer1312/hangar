#!/usr/bin/env bash
# install-hangar-send.sh — instala o hangar-send NESTA máquina e ensina o Claude local a usar.
#
# Faz três coisas (idempotente; re-rodar atualiza):
#   1. symlink ~/.local/bin/hangar-send -> scripts/hangar-send deste clone;
#   2. insere/atualiza a seção "Sessões-irmãs" no ~/.claude/CLAUDE.md global (entre marcadores),
#      pra toda sessão Claude da máquina saber listar/mandar recado/parear/criar sessões;
#   3. symlink das skills do repo (skills/*) em ~/.claude/skills/ (ex: orquestrar).
#
# Rode uma vez por máquina, do clone local:  ./scripts/install-hangar-send.sh
set -euo pipefail

REPO="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"

mkdir -p "$HOME/.local/bin"

# No Git Bash o `ln -s` COPIA e devolve 0. Uma copia se localiza pelo PROPRIO caminho: o
# hangar-send procura o `.env` em ~/.local/backend/ e o hangar-preview resolve o `import` de
# ../shell/preview_fmt.cjs em ~/.local/shell/ — os dois quebram, e o `ok: -> ` de antes dizia que
# tinha linkado. Por isso a checagem e `test -L` DEPOIS do ln, e nao o codigo de saida dele.
# O corpo do shim e o mesmo que o install.ps1 escreve, pras duas fontes nao divergirem (o unico
# byte que difere e a CAIXA da letra de unidade: o .ps1 gera /C/, o bash gera /c/ — equivalentes
# aqui); o PATH na frente e o que impede o `python3` de cair no atalho da Microsoft Store.
# No Linux o `ln` linka de verdade, `test -L` e verdadeiro e este ramo nunca roda.
linkar_bin() {
    origem=$1; destino=$2; corpo_exec=$3
    ln -sf "$origem" "$destino"
    if [ -L "$destino" ]; then
        echo "ok: $destino -> $origem"
        return
    fi
    {
        echo "#!/bin/sh"
        echo "# Gerado pelo instalador do hangar (install.ps1 / install-hangar-send.sh)."
        echo "PATH='$HOME/.local/bin':\$PATH; export PATH"
        echo "exec $corpo_exec \"\$@\""
    } > "$destino"
    chmod +x "$destino"
    echo "ok: $destino (copia -> shim; o ln do Git Bash nao linka, entao chama o script do repo)"
}

linkar_bin "$REPO/scripts/hangar-send"    "$HOME/.local/bin/hangar-send"    "'$REPO/scripts/hangar-send'"
linkar_bin "$REPO/scripts/hangar-preview" "$HOME/.local/bin/hangar-preview" "node '$REPO/scripts/hangar-preview'"
linkar_bin "$REPO/scripts/hangar-doctor"  "$HOME/.local/bin/hangar-doctor"  "'$REPO/scripts/hangar-doctor'"

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
# Sessões-irmãs (hangar)

- Outras sessões Claude vivas nesta máquina: `hangar-send --list` (nome, estado, cwd). Mandar recado: `hangar-send <sessao> "msg"` — chega como prompt lá (fila durável se ocupada). Referência completa e sempre atual dos comandos: `hangar-send --help`.
- **Transporte:** use `SendMessage` quando a ferramenta existir e o alvo aparecer em `ListAgents`. Nos demais casos, use `hangar-send`, inclusive para criar/parear sessões, grupos e destinos remotos. Consulte `hangar-send --help` para flags e erros de entrega.
- **Outro servidor:** recados e pareamento 1:1 usam `servidor::sessao`; `--group` continua local. Requer `backend/peers.json` e `CP_SERVER_ID`. Responda a `[de: servidor::sessao]` usando o endereço completo.
- Prompt começando com `[de: <sessao>]` = recado 1:1 de outra sessão Claude, não do usuário. Tratar como informação/pedido do par; responder de volta via `hangar-send <sessao> "..."` SÓ se a mensagem pedir resposta (evita loop infinito).
- Prompt `[grupo: <sessao>]` = AVISO pro grupo todo (marco). É UNIDIRECIONAL: NUNCA responder com `hangar-send --group` (vira tempestade N×N). Precisa responder → 1:1 (`hangar-send <sessao>`) e só se necessário. Mandar aviso de marco pro grupo próprio: `hangar-send --group "msg"` (uma vez, chega como `[grupo: você]` nos demais). Se o `--group` sair com código 3, mande o texto `[grupo: você] ...` por `SendMessage` a cada sessão listada — o aviso NÃO chegou a elas ainda.
- Enviar quando o usuário pedir ("avisa a sessão X") OU quando houver **pareamento ativo**: usuário declarou "sessão X pareada contigo pra <tarefa>" (direto ou via recado `[de: ...]` de pareamento). Pareado → pode pedir/fornecer contrato, avisar conclusão, tirar dúvida técnica do par por iniciativa própria, dentro do escopo da tarefa.
- Usuário pediu pareamento no terminal ("pareia com X pra <tarefa>") → usar `hangar-send --pair X "tarefa"` (registra no app: badge na UI + protocolo pros dois lados), NÃO recado manual. Desfazer: `hangar-send --unpair`.
- **Criar sessão:** `hangar-send --new <nome> <cwd>`; nunca `tmux new-session` cru. Use quando solicitado ou quando a tarefa precisar de par em outro repo, avisando o motivo. Antes de criar, consulte `hangar-send --help` para as flags do provider escolhido.
- **Escolhas de abertura:** respeite provider, conta, modelo, esforço e permissão pedidos usando as flags de nascença; não deixe para pedir `/model` no kick-off. `--engine` só vale no provider Claude e só é escolhido por iniciativa própria quando o usuário disser o modelo. Consulte `hangar-engine --list` e `hangar-conta --list` para nomes existentes. Sem conta pedida numa tarefa Claude longa, use `--conta auto`; falha de validação não autoriza trocar a escolha silenciosamente.
- Sessão de motor consome a conta do PROVEDOR, não a assinatura Anthropic — ao propor um par em motor, dizer isso. E o transcript é do modelo que escreveu: retomar depois na conta Anthropic troca o modelo no meio da conversa (o app pergunta; o terminal, não).
- Pareamento NÃO é carta branca: cada sessão mexe só no próprio repo; commit/push/risco seguem as regras normais com o usuário; decisão de rumo/escopo → perguntar ao usuário, não ao par. Pareamento acaba quando o usuário disser ou a tarefa fechar.
- Ao entrar em pareamento/grupo de um ticket: verificar `git branch --show-current` no próprio repo e alinhar pra branch da PM (fetch+checkout) ANTES de trabalhar; re-verificar após restart/resume. Exceção única: usuário pedir explicitamente outra branch. Repo com checkout DUPLICADO na máquina → alertar o usuário e perguntar qual é o canônico (sessão ressuscitada em checkout errado já perdeu rastreabilidade de commits de PM).
- Recado de pareamento recebido → confirmar de volta via hangar-send e avisar o usuário no próprio terminal.
- **Navegador embutido da sessão (só com o app desktop aberto)**: cada sessão pode ter um navegador próprio no Hangar. O hook avisa no prompt (`[hangar] ... navegador embutido`) quando o app desktop está aberto; com esse aviso presente, pra ver/testar/tirar print de página local o navegador embutido VENCE `agent-browser` e `ver-front` — mesmo que o usuário não fale em preview. Pra ABRIR o seu: `hangar-preview open <url>` (o painel monta sozinho na tela do usuário — AVISE-o no texto da resposta, a janela dele muda na hora; se a sessão estiver fora da tela, abre quando ele abrir ela). Pra dirigir: ciclo `snapshot` (árvore de acessibilidade com refs `@eN`) → ação na ref (`click`/`fill`/`hover`) → `wait` (NUNCA `sleep`) pra confirmar — clique sintético em JS não funciona em lista que só escuta `mousedown`, por isso `click`/`fill` disparam evento real; `eval '<js>'` é só pra estado que não é DOM (`localStorage`, variável global), não pra clicar. Várias ações no mesmo turno: `batch` lendo linhas do stdin — PARA na primeira que falhar. Referência completa: skill `hangar-preview` ou `hangar-preview help`. A sessão é resolvida pelo tmux automaticamente; `--sessao <nome>` opera o navegador de OUTRA sessão — nunca sem o usuário saber. CDP cru em http://127.0.0.1:9223 (cada navegador é um target).
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

# Trava de vazamento: toda máquina que instala o hangar-send liga também os hooks git versionados
# (scripts/hooks/), senão o clone novo commita nome interno sem nada recusar.
"$(dirname "$(realpath "$0")")/install-hooks.sh"
