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
ln -sf "$REPO/scripts/hangar-send" "$HOME/.local/bin/hangar-send"
echo "ok: ~/.local/bin/hangar-send -> $REPO/scripts/hangar-send"

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
- **Qual caminho usar pra falar com outra sessão** (decidir nesta ordem, sempre): (1) tem a ferramenta `SendMessage` E o alvo aparece no `ListAgents`? → usa `SendMessage`. É o caminho nativo do Claude Code (2.1.224+): entrega por socket, sem digitar no terminal, então não corta texto longo nem cola mensagem pela metade. (2) Qualquer outro caso — não tem a ferramenta, alvo em OUTRA máquina (`servidor::sessao`), sessão Codex ou Pi, aviso pro grupo (`--group`), criar sessão (`--new`), parear (`--pair`) → `hangar-send`. Os dois chegam do mesmo jeito no destino, como `[de: <sessao>]`; o app mostra igual. Na dúvida entre os dois, `hangar-send` — ele funciona sempre.
- Sessão em OUTRO servidor: endereço `servidor::sessao` (ex: `hangar-send servidor-b::api-fix "msg"`). Requer `backend/peers.json` na máquina; `hangar-send --list` já mostra as remotas com o prefixo. Recado `[de: servidor::sessao]` → responder usando o endereço completo. Pareamento/`--group` cross-server ainda NÃO existem — só recado 1:1.
- Prompt começando com `[de: <sessao>]` = recado 1:1 de outra sessão Claude, não do usuário. Tratar como informação/pedido do par; responder de volta via `hangar-send <sessao> "..."` SÓ se a mensagem pedir resposta (evita loop infinito).
- Prompt `[grupo: <sessao>]` = AVISO pro grupo todo (marco). É UNIDIRECIONAL: NUNCA responder com `hangar-send --group` (vira tempestade N×N). Precisa responder → 1:1 (`hangar-send <sessao>`) e só se necessário. Mandar aviso de marco pro grupo próprio: `hangar-send --group "msg"` (uma vez, chega como `[grupo: você]` nos demais). Se o `--group` sair com código 3, mande o texto `[grupo: você] ...` por `SendMessage` a cada sessão listada — o aviso NÃO chegou a elas ainda.
- Enviar quando o usuário pedir ("avisa a sessão X") OU quando houver **pareamento ativo**: usuário declarou "sessão X pareada contigo pra <tarefa>" (direto ou via recado `[de: ...]` de pareamento). Pareado → pode pedir/fornecer contrato, avisar conclusão, tirar dúvida técnica do par por iniciativa própria, dentro do escopo da tarefa.
- Usuário pediu pareamento no terminal ("pareia com X pra <tarefa>") → usar `hangar-send --pair X "tarefa"` (registra no app: badge na UI + protocolo pros dois lados), NÃO recado manual. Desfazer: `hangar-send --unpair`.
- Criar sessão nova (usuário pediu, ou a tarefa precisa de par em outro repo): `hangar-send --new <nome> <cwd>` — NUNCA `tmux new-session` cru (fica sem --session-id, invisível no app). Criar por iniciativa própria → avisar o usuário no terminal o porquê. As escolhas de abertura (combináveis) são flags do `--new`, não instalações à parte:
  - **Outro AGENTE** — `--provider <claude|codex|pi|kimi>`: qual CLI sobe no pane. Pedido "abre uma sessão no Kimi/Codex/Pi" → `hangar-send --new <nome> <cwd> --provider kimi` (ou codex/pi). NÃO precisa de motor nem config extra — só do binário instalado. Recado, fila, pareamento e grupo funcionam igual entre providers.
  - **Outro MODELO** — `--engine <motor>`: a sessão Claude nasce num motor de `~/.claude/engines.json` (gateway próprio, DeepSeek, ...) em vez da conta Anthropic. O par segue no MESMO `~/.claude` — skills, hooks, contrato compartilhado, tudo igual; só o motor difere. Motores configurados: `hangar-engine --list` (o `claude-engine` é função de shell interativa, não existe no PATH). Motor inexistente → `400 motor invalido` e a sessão NÃO nasce (falha alta, nunca uma sessão que parece estar no motor e não está). Só vale com provider claude; combinar com codex/kimi/pi dá 400. Escolher motor por iniciativa própria só se o usuário disser o modelo; o default é a conta dele.
  - **Outra CONTA Anthropic** — `--conta <nome>`: a sessão nasce logada noutra assinatura (config dir `~/.claude-<nome>`; skills/hooks symlinkados do `~/.claude`, consumo vai pra cota DELA). Contas configuradas: `hangar-conta --list`; criar uma: `hangar-conta --new <nome>` (o /login é interativo, feito na primeira sessão dela). Conta errada/inexistente falha ANTES de criar nada. Sem o flag, nasce na conta padrão (~/.claude).
  - **MODELO, ESFORÇO e PERMISSÃO de nascença** — `--model <id>`, `--effort <nível>`, `--permissao <modo>`: a sessão já ABRE assim, sem gastar um turno no default errado. Tarefa com modelo definido pelo usuário ("abre no Opus em high") → `hangar-send --new <nome> <cwd> --conta X --model opus --effort high`. SEMPRE preferir isso a pedir `/model` no kick-off: a sessão trabalha ANTES de ler o recado. Modelo aceita alias ou id (`opus`, `sonnet`, `haiku`, `claude-opus-5`…); esforço: `low|medium|high|xhigh|max` (claude; no pi vira `--thinking`, que aceita também `off|minimal`; kimi só `--model`); permissão (só claude): `acceptEdits|auto|bypassPermissions|manual|dontAsk|plan`. Valor inválido → 400 do backend e a sessão NÃO nasce.
- Sessão de motor consome a conta do PROVEDOR, não a assinatura Anthropic — ao propor um par em motor, dizer isso. E o transcript é do modelo que escreveu: retomar depois na conta Anthropic troca o modelo no meio da conversa (o app pergunta; o terminal, não).
- Pareamento NÃO é carta branca: cada sessão mexe só no próprio repo; commit/push/risco seguem as regras normais com o usuário; decisão de rumo/escopo → perguntar ao usuário, não ao par. Pareamento acaba quando o usuário disser ou a tarefa fechar.
- Ao entrar em pareamento/grupo de um ticket: verificar `git branch --show-current` no próprio repo e alinhar pra branch da PM (fetch+checkout) ANTES de trabalhar; re-verificar após restart/resume. Exceção única: usuário pedir explicitamente outra branch. Repo com checkout DUPLICADO na máquina → alertar o usuário e perguntar qual é o canônico (sessão ressuscitada em checkout errado já perdeu rastreabilidade de commits de PM).
- Recado de pareamento recebido → confirmar de volta via hangar-send e avisar o usuário no próprio terminal.
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
