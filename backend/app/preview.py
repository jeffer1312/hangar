import asyncio
import json
import logging
import re
import time
from typing import AsyncIterator, Callable, Optional

from app import tmux
from app.state import _RULE_RE, _is_boundary, _live_spinner
# _dirs: MESMO cache de diretorios de config que a statusline usa, e pelo mesmo motivo (roda por
# sessao, a cada poll). Reusado em vez de copiado — sao os mesmos diretorios e a mesma chave (o stem
# do .jsonl); duas copias so dariam a chance de uma envelhecer.
from app.statusline import _dirs as _config_dirs

_log = logging.getLogger("claude_pocket.preview")

# Preview AO VIVO do bloco de assistente em andamento, lido do pane do tmux (capture-pane -p, texto
# já composto: sem ANSI, sem cursor-move). É a ÚNICA fonte do texto em voo sem perder o REPL
# interativo — o Claude Code só grava no .jsonl a mensagem COMPLETA. Best-effort, heurística acoplada
# ao TUI do Claude Code; o .jsonl continua a verdade canônica que SUBSTITUI o preview no fim.

_ASSISTANT_GLYPH = "●"
_USER_PROMPT_RE = re.compile(r"^\s*❯")


def _norm(s: str) -> str:
    # Normaliza pra casar o texto do PANE (JÁ RENDERIZADO: hard-wrap do terminal, SEM markdown) com o
    # do .jsonl (markdown CRU): tira marcadores de markdown (crase * _ ~ # >) e colapsa espaço. Sem
    # tirar os marcadores, uma msg com formatação ("**Confirma**" vs "Confirma") não casava e o preview
    # já-commitado vazava como bolha duplicada. Usado pra suprimir preview já no transcript.
    return re.sub(r"\s+", " ", re.sub(r"[`*_~#>]", "", s)).strip()
# Bloco ● que e TOOL-CALL/STATUS, nao prosa: "● Bash(...)", "● Reading 4 files…", "● Running 1 shell
# command…", "● Ran 1 shell command". Pular esses mantem o preview na ULTIMA PROSA -> a lista nao fica
# "pulando" entre texto e indicador de ferramenta (o tool aparece como ToolCard quando cai no .jsonl).
# So GERUNDIOS/Ran (= status de tool, raro em prosa) + "Word(" (tool call). Evito passado ambiguo
# (Read/Wrote/Found) que apareceria em prosa.
_TOOL_VERBS = (
    "Running|Reading|Writing|Editing|Searching|Listing|Fetching|Updating|Creating|Deleting|"
    "Crawling|Downloading|Globbing|Grepping|Waiting|Loading|Compiling|Building|Installing|Ran|"
    "Making"
)
_TOOL_BLOCK_RE = re.compile(rf"^([A-Z][\w-]*\(|({_TOOL_VERBS})\b)")

# Chrome novo do Claude Code (medido no pane em 17/08/2026, claude 2.1.233) — DOIS desenhos que as
# regras acima nao conheciam, e que sao a transicao ate toda sessao nascer com o preview_hook (o
# sidecar torna este arquivo inteiro plano B):
#   ● Agent "react-reviewer no diff" finished · 9s     <- aviso de subagente em background que acabou
#     Searched for 2 patterns, read 1 file, ran 2 shell commands   <- resumo de atividade, sem ●
# O aviso usa o MESMO ● da prosa (bullet verde no ANSI, mas aqui e texto puro), entao era ELEITO
# como bloco em voo e a previa virava ele + o resumo — o print do usuario, reproduzido. O resumo
# tambem aparece SOZINHO no fim de um bloco de prosa ("Pushed to minha-branch, ran 3 shell commands").
# Ancora do resumo = a forma medida (termina em "ran N shell command(s)"), nunca vocabulario solto.
_AGENT_FINISHED_RE = re.compile(r'^Agent "[^"]*" finished\b')
_ACTIVITY_SUMMARY_RE = re.compile(r"\bran \d+ shell commands?\s*$")

# Status das ferramentas MCP: "Calling chrome-devtools…". Sem cortar aqui, a linha entrava no preview
# como prosa e — porque aparece e some a cada chamada — o bloco crescia e encolhia sozinho (o "pulo"
# na tela). Fora da lista de verbos DE PROPOSITO: la o verbo casa solto no comeco da linha, e
# "Calling" e inicio de frase comum em ingles ("Calling this an edge case, ..."). Como a lista tambem
# decide qual ● e tool-call, um falso positivo ali DESCARTA o bloco e o preview fica VAZIO — pior que
# vir sujo. Exigir as reticencias no fim amarra a regra a forma real do status.
_MCP_CALL_RE = re.compile(r"^Calling\b[^\n]*(…|\.\.\.)\s*$")

# Chrome de FIM DE BLOCO que só o Pi desenha: a caixa arredondada do composer (╭───╮ … ╰───╯), que
# fica logo abaixo da resposta em voo. Nenhuma das paradas do Claude casa nela (a régua reta é outro
# desenho), então o preview do Pi vinha com a borda da caixa E a statusline (🤖 modelo … 💵 custo)
# coladas no fim do texto — capturas reais em .superpowers/sdd/2026-07-27-pi-adapter/.
# Por PROVIDER, e não por forma, porque AQUI o chamador sabe (sse.merged_events já ramifica por
# provider pra escolher a fonte do preview): assim a extração do Claude/Codex fica byte-idêntica,
# sem a chance de uma prosa dele que desenhe um box virar preview truncado.
# ponytail: desenho da caixa é calibration knob, igual ao _BOX_BOTTOM_RE do state.py.
_PI_BOX_RE = re.compile(r"^\s*[╭╰][─\s]*[╮╯]\s*$")
# Separador do overlay do /model: `▔` (U+2594), não a régua reta. Medido nas fixtures
# pane_model_picker_*.txt — nelas o _RULE_RE não casa NADA.
_OVERLAY_RULE_RE = re.compile(r"^[\s▔]*▔{10,}[\s▔]*$")
# Eco do prompt do usuario no pane do Kimi: ` ✨ <texto>`. E o `❯` do Claude com outro glifo — sem
# ele o preview passava da resposta ANTERIOR direto pra dentro da pergunta seguinte, e a bolha do
# assistente terminava com o texto que o proprio usuario digitou.
_KIMI_PROMPT_RE = re.compile(r"^\s*✨\s")

# Rodape de dica do Kimi, DUAS formas — capturadas do pane em 11/08/2026, 150 quadros:
#     '  ⠋ working... · Tip: /goal for multi-step work with a clear finish line'   (em voo)
#     '  🌓 · Tip: Try /dance for a hidden Easter egg'                              (parado)
# O glifo GIRA nas duas (as 8 fases da lua U+1F311..U+1F318 e o ciclo braille U+2800..U+28FF
# aparecem entre os quadros), entao a ancora e a FORMA da linha — spinner + ` · Tip: ` — e nunca um
# glifo fixo. Nenhum dos dois cai nas paradas que ja existiam: SPINNER_GLYPHS (state.py) nao tem
# braille nem lua, e o _ASCII_SPINNER_RE casa asterisco.
_KIMI_TIP_RE = re.compile(r"^\s*[⠀-⣿\U0001f311-\U0001f318]\s.*·\s*Tip:")

# Linha de spinner do Kimi SEM a dica ("  ⠋ working..." sozinha — medida na fixture _KIMI_PENSANDO
# dos testes): o glifo e braille (U+2800..U+28FF) ou lua (U+1F311..U+1F318), nenhum dos dois em
# SPINNER_GLYPHS — sem esta parada, a continuacao da prosa engolia o spinner e tudo que vinha
# abaixo dele nos frames em que a dica nao aparece. Prosa nunca comeca com braille/lua.
_KIMI_SPINNER_RE = re.compile(r"^\s*[⠀-⣿\U0001f311-\U0001f318]")

# Header do painel de Todo da TUI do Kimi (o painel inteiro e descrito mais abaixo, junto da regra
# de item — ele so entra aqui porque a tupla de paradas e montada em nivel de modulo).
_KIMI_TODO_HEAD_RE = re.compile(r"^\s*Todo\s*$")

_STOPS_BY_PROVIDER = {"pi": (_PI_BOX_RE,),
                      # Kimi desenha o composer com a MESMA caixa arredondada do Pi (medido num
                      # pane real, 0.34.0) -> a ancora de corte e a mesma. Mas a caixa e o rodape
                      # MAIS BAIXO: entre ela e a resposta ainda cabem o eco do prompt e a dica, e
                      # por isso os dois precisam parar o preview por conta propria.
                      "kimi": (_PI_BOX_RE, _KIMI_PROMPT_RE, _KIMI_TIP_RE, _KIMI_SPINNER_RE,
                               _KIMI_TODO_HEAD_RE)}

# Bloco de FERRAMENTA do Pi. O _TOOL_BLOCK_RE exige "Nome(" colado, e o Pi escreve de pelo menos
# cinco jeitos — medidos no pane em 01/08/2026:
#     ● Bash cd "/home/..."        ● Bash: 2 done • ctrl+o to toggle    ● Write  (81 lines)
#     ● Edit  (2 edits)            ● Multiple Tools: 3 done • bash, chrome_devtools_navigate_page
# Nenhum casava, então a chamada entrava na prévia como se fosse prosa: a cada ferramenta o bloco em
# voo trocava de conteúdo E DE ALTURA, e a conversa pulava debaixo de quem estava lendo no celular.
#
# Exige as DUAS coisas — nome de ferramenta no cabeçalho E corpo em box-drawing na linha seguinte
# (árvore `├└`, diff `│▌`, resultado `⎿`). Só a estrutura NÃO serve, e o caminho até aqui foi:
#   1. só o `└`  -> "Estrutura final:" / " └ src/" (prosa apresentando árvore) virava ferramenta;
#   2. só os glifos, com guard de `:` no fim -> "Aqui vai o trecho pra comparar" / "│ codigo"
#      continuava caindo, porque a frase não precisa terminar em dois-pontos pra introduzir um
#      trecho. Achado da review, reproduzido no código.
# Nos dois casos o bloco era DESCARTADO e a prévia ficava VAZIA — pior que vir suja, a mesma
# armadilha documentada no _MCP_CALL_RE acima. Errar pro lado do vazamento é recuperável (fica um
# cabeçalho feio por um poll); errar pro lado do descarte apaga o texto que a pessoa está lendo.
# O painel de tarefas ("Todos (11/13)") também é salvo por isto: tem forma de ferramenta, mas não
# tem nome de ferramenta — e é o único bloco desses que o usuário quer ver, dobrado pela bolha.
#
# A lista é calibration knob: nome novo que o Pi passe a desenhar entra aqui, e o custo de esquecer
# um é só o vazamento cosmético. Só pro Pi, como o resto do chrome por provider (o Claude marca
# resultado com `⎿` e nunca chega neste ramo — o `and` curto-circuita antes).
_PI_CORPO_RE = re.compile(r"^\s*[├└│▌⎿]")
# CORREÇÃO 03/08/2026 — o painel de tarefas NÃO vai mais pra prévia (o usuário não quer vê-lo lá).
# Hoje o Pi o desenha com `○` (círculo VAZIO, U+25CB), não com o `●` da prosa, então ele nunca é
# eleito começo de bloco; a parada abaixo é o que impede de ser ENGOLIDO pelo bloco de cima quando o
# chrome entre os dois escapa. Aceita os dois glifos (e nenhum) porque o desenho é calibration knob.
_TODO_PANEL_RE = re.compile(r"^\s*[●○]?\s*Todos \(\d+/\d+\)\s*$")
# Frame ASCII do spinner. O ciclo medido no pane em 03/08/2026 (Pi 0.82.1) é `✻✽✶✺✢·` MAIS o `*`
# (U+002A) — e esse último está FORA de SPINNER_GLYPHS, logo _is_boundary não parava nele. Um frame
# em seis, a prévia engolia a linha de status ("* Boondoggling… (thinking with high effort · 12s)")
# e, junto com ela, o painel de Todos inteiro que vem logo abaixo: era isso que aparecia no celular.
# Exige a forma da linha (termina em `…` ou em `)`), não só o glifo, pra um bullet de markdown em
# prosa ("* item") não truncar o texto no meio.
_ASCII_SPINNER_RE = re.compile(r"^\*\s+\S[^\n]*(…|\))\s*$")
# ── Raciocinio do Kimi: so a COR separa ────────────────────────────────────────────────────────
# Medido no pane em 14/08/2026 (Kimi 0.36, K3 high). O raciocinio e a resposta usam o MESMO `●`:
#     pensamento: ESC[38;2;136;136;136m● ESC[3m17 × 23 = 17 × 20 + 17 × 3 …ESC[0m   (cinza + ITALICO)
#     resposta:   ESC[38;2;224;224;224m● ESC[39m17 × 23 = 391.                      (claro, sem italico)
# Em texto puro (o `capture-pane` de sempre, sem `-e`) as duas linhas sao identicas — `● …` — e por
# isso a previa mostrava o raciocinio como se fosse a mensagem, ate o bloco real commitar. O
# transcript nunca teve esse problema: la o `part.type == "think"` ja e descartado no parser.
# "Tem italico na linha" NAO serve como regra, e isso foi MEDIDO depois de a primeira versao errar:
# a resposta usa o mesmo `[3m` pra enfase de markdown no meio da frase —
#     ESC[38;2;224;224;224m● ESC[39mO ESC[3mtesteESC[0m ESC[1mpassouESC[0m com sucesso.
# ou seja, `*teste*` vira italico DENTRO da prosa. Cortar a linha inteira ali apagaria a resposta.
#
# A regra e a POSICAO + a COR, os dois: e rascunho quando o italico ABRE o conteudo (antes dele so
# ha espaco e o bullet `●`) E a linha carrega o cinza do rascunho. Enfase no meio da frase tem texto
# antes do `[3m`; resposta comecando em italico ainda tem a cor clara no bullet. As linhas de
# continuacao do rascunho passam nas duas (medido: `   ESC[3mESC[38;2;136;136;136mnever replied…`).
#
# Erra-se sempre pro lado de MANTER: vazar rascunho e cosmetico e reversivel; engolir resposta e
# perda de conteudo, calada. Por isso o RGB entra como condicao E, nao OU — se o tema mudar a cor,
# o pior caso e o vazamento voltar, nunca a resposta sumir.
_KIMI_ITALICO = "\x1b[3m"
_KIMI_CINZA = "38;2;136;136;136"
_ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[a-zA-Z]")
# Status de ferramenta CONCLUIDA do Kimi: "● Used ReadMediaFile (…/arquivo.png) · image (…)" — o
# passado generico "Used" + o nome da ferramenta, medido no pane em 19/08/2026 (Kimi, k3). Nao casa
# no _TOOL_BLOCK_RE (que exige "Nome(" colado no INICIO da linha, e a lista de verbos evita passado
# ambiguo de proposito), entao a linha era eleita como bloco de prosa e a previa mostrava o status
# da ferramenta como texto — quando o ToolCard bonito ja chega pelo tool.call do wire. A forma
# exigida (palavra com maiuscula + parentese) e o que amarra a regra ao desenho real: "Used" em
# prosa inglesa ("Used correctly, ...") tem a palavra seguinte minuscula e nao casa. Especifico do
# Kimi, como o resto do chrome por provider — no Claude "Used" nem aparece.
_KIMI_USED_RE = re.compile(r"^Used\s+[A-Z][\w-]*\s*\(")

# Painel de Todo da TUI do Kimi (medido nos quadros de 19/08/2026, durante o trabalho desta
# costura): entre a regua e a caixa do composer ele desenha o header "  Todo" e os itens
# "   ✓ feito" / "   ● atual" — e o item ATUAL usa o MESMO ● da prosa. Como a caixa do composer
# fica ABAIXO do painel, o corte por posicao (fim) nao o exclui da varredura, e o item era eleito
# bloco em voo: a previa mostrava "Testes backend (costura, ...)" no lugar do texto, reproduzido
# num quadro real no teste. So a POSICAO dentro do painel separa o ● de item do ● de prosa.
# (O _KIMI_TODO_HEAD_RE em si e definido mais acima, antes da tupla de paradas por provider.)
_KIMI_TODO_ITEM_RE = re.compile(r"^\s*[✓●○]\s")


def _kimi_linha_de_todo(lines: list[str], i: int) -> bool:
    """A linha `i` esta dentro do painel de Todo do Kimi? Sobe da linha ate achar o header "Todo"
    (e painel) ou uma linha que nao e item nem vazia (a prosa/ferramenta de cima — nao e painel)."""
    for j in range(i, -1, -1):
        ln = lines[j]
        if _KIMI_TODO_HEAD_RE.match(ln):
            return True
        if ln.strip() and not _KIMI_TODO_ITEM_RE.match(ln):
            return False
    return False
# "… (6 more lines, ctrl+o to expand)" — rodape do bloco DOBRADO, cromo do TUI, nunca prosa. Vem em
# `[2m` (dim), nao em italico, entao nao sai pela regra de cima.
_KIMI_DOBRADO_RE = re.compile(r"^\s*\.\.\.\s*\(\d+ more lines")


def _e_rascunho_kimi(ln: str) -> bool:
    """A linha e raciocinio do Kimi? (ver o bloco de comentario acima — posicao do italico + cor)."""
    i = ln.find(_KIMI_ITALICO)
    if i < 0 or _KIMI_CINZA not in ln:
        return False
    # Antes do italico so pode haver espaco e o bullet: e ele ABRINDO o conteudo. Com texto antes,
    # e enfase no meio da prosa — resposta de verdade.
    return _ANSI_RE.sub("", ln[:i]).strip(" ●") == ""


def sem_pensamento_kimi(pane_colorido: str) -> str:
    """Tira o bloco de raciocinio do pane COLORIDO do Kimi e devolve texto puro (sem ANSI).

    Recebe o pane com `-e` e devolve sem: quem consome depois (`extract_assistant_text`,
    `_live_spinner`) casa texto puro, e um `\\x1b[...m` no meio quebraria cada regex.
    """
    out = []
    for ln in pane_colorido.splitlines():
        if _e_rascunho_kimi(ln):
            continue
        limpa = _ANSI_RE.sub("", ln)
        if _KIMI_DOBRADO_RE.match(limpa):
            continue
        out.append(limpa)
    return "\n".join(out)


_PI_TOOL_NAME_RE = re.compile(
    r"^(Bash|Read|Write|Edit|MultiEdit|Grep|Glob|Task|Agent|Skill|WebFetch|WebSearch|"
    r"NotebookEdit|TodoWrite|Multiple Tools|Chrome Devtools)[\s:(]"
)


# PAINEL DO SUBAGENTE AO VIVO do Claude, medido no pane em 03/08/2026:
#     ● Subagent Subagent (1 line)
#      └ The code patterns look clean. Now let me verify compilation and run the module tests.
# Ele usa o MESMO ● da prosa e fica embaixo de tudo, entao a varredura (que pega o ULTIMO ●) elegia
# ele — e como o subagente reescreve aquela linha a cada frame, a previa trocava de conteudo E DE
# ALTURA sem parar: a conversa pulava debaixo de quem estava lendo. Nao ha nada a perder escondendo:
# o ToolCard da chamada, logo acima, ja diz qual subagente esta rodando.
#
# Exige as DUAS coisas — "Subagent <algo>" no cabecalho E o `└` na primeira linha nao-vazia abaixo —
# pelo mesmo motivo do _pi_bloco_de_tool: falso positivo aqui DESCARTA o bloco e a previa fica
# VAZIA, que e pior que vir suja. Uma prosa que comece com a palavra "Subagent" nao desenha `└`.
_SUBAGENT_HEAD_RE = re.compile(r"^Subagent\s+\S")
_SUBAGENT_CORPO_RE = re.compile(r"^\s*└")


def _painel_de_subagente(lines: list[str], i: int, corpo: str) -> bool:
    """Bloco `i` e o painel do subagente ao vivo: cabecalho `Subagent <nome>` E corpo em `└`."""
    if not _SUBAGENT_HEAD_RE.match(corpo):
        return False
    for ln in lines[i + 1:]:
        if not ln.strip():
            continue
        return bool(_SUBAGENT_CORPO_RE.match(ln))
    return False


def _pi_bloco_de_tool(lines: list[str], i: int, corpo: str) -> bool:
    """Bloco `i` é chamada de ferramenta do Pi: nome de ferramenta no cabeçalho E corpo desenhado em
    box-drawing na primeira linha não-vazia abaixo."""
    if not _PI_TOOL_NAME_RE.match(corpo):
        return False
    for ln in lines[i + 1:]:
        if not ln.strip():
            continue
        return bool(_PI_CORPO_RE.match(ln))
    return False


def extract_assistant_text(pane: str, provider: str = "claude") -> str:
    """Texto do ÚLTIMO bloco de PROSA do assistente (●) do pane, VERBATIM (sem reflow — núcleo seguro).

    Acha o último ● que NÃO é tool-call (início do bloco em voo; blocos anteriores já caíram no
    .jsonl), tira o "● " da 1ª linha e segue até o primeiro chrome: régua (────), próximo boundary
    (●/⎿/spinner), prompt ❯ ou o chrome extra do provider. Sem juntar continuação — o markdown
    bonito vem no snap final do .jsonl.

    provider desconhecido = "claude" = comportamento idêntico ao de sempre.
    """
    stops = _STOPS_BY_PROVIDER.get(provider, ())
    lines = pane.splitlines()
    # A conversa acaba no ÚLTIMO CHROME DE RODAPÉ: dali pra baixo é a caixa de digitar, a
    # statusline, as dicas e — o que motivou este corte — o PAINEL DE SUBAGENTES ("● main" /
    # "◯ general-purpose Grepping… 1m 34s"). Ele marca o agente principal com o MESMO ● do bloco do
    # assistente (U+25CF, medido no pane em 01/08/2026), e como a varredura pega o ÚLTIMO ●, o
    # painel ganhava sempre: em sessão com subagente rodando a prévia era ele, nunca o texto.
    # Corte por POSIÇÃO, não por vocabulário — qualquer chrome futuro que reuse ● cai fora junto.
    # São TRÊS desenhos porque cada estado do TUI usa o seu, e olhar só a régua reta deixava dois
    # buracos (achado da review, reproduzido nas fixtures do repo):
    #   ─  régua do composer do Claude          ▔  separador do overlay do /model
    #   ╭╮ ╰╯  caixa do composer do Pi (que nunca desenha régua reta — sem isto o corte era no-op
    #          no caminho dele)
    # Nenhum deles presente (pane recém-aberto, ou estreito demais pros 10 traços) -> varre tudo,
    # como antes: prévia suja é melhor que prévia nenhuma.
    fim = max((i for i, ln in enumerate(lines)
               if _RULE_RE.match(ln) or _OVERLAY_RULE_RE.match(ln) or _PI_BOX_RE.match(ln)),
              default=len(lines))
    start = -1
    for i, ln in enumerate(lines[:fim]):   # sem régua, fim == len(lines) e a fatia é a lista toda
        s = ln.lstrip()
        corpo = s[1:].lstrip()
        if (s[:1] == _ASSISTANT_GLYPH and not _TOOL_BLOCK_RE.match(corpo)
                and not _MCP_CALL_RE.match(corpo)
                and not _AGENT_FINISHED_RE.match(corpo)
                and not _TODO_PANEL_RE.match(ln)
                and not _painel_de_subagente(lines, i, corpo)
                and not (provider == "kimi" and _KIMI_USED_RE.match(corpo))
                and not (provider == "kimi" and _kimi_linha_de_todo(lines, i))
                and not (provider == "pi" and _pi_bloco_de_tool(lines, i, corpo))):
            start = i
    if start < 0:
        return ""

    first = lines[start].lstrip()
    first = first[1:].lstrip() if first[:1] == _ASSISTANT_GLYPH else first
    out = [first.rstrip()]
    for ln in lines[start + 1:]:
        # Chrome = limite inferior do bloco. _is_boundary pega ●/⎿/spinner (lstrip já tira indent
        # das linhas de continuação da prosa, então elas NÃO disparam aqui). _TOOL_BLOCK_RE corta
        # tb a linha de status de tool ("Running/Ran N shell command") que renderiza 1 frame SEM o
        # ● antes de virar bloco — senão grudava no fim da prosa e piscava.
        s = ln.lstrip()
        if (_RULE_RE.match(ln) or _is_boundary(ln) or _USER_PROMPT_RE.match(ln)
                or _TOOL_BLOCK_RE.match(s) or _MCP_CALL_RE.match(s)
                or _TODO_PANEL_RE.match(ln) or _ASCII_SPINNER_RE.match(s)
                or (provider == "kimi" and _KIMI_USED_RE.match(s))
                or _ACTIVITY_SUMMARY_RE.search(s)):
            break
        if any(r.match(ln) for r in stops):
            break
        out.append(ln.rstrip())

    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


# ── Previa publicada pelo PROPRIO agente (sidecar), preferida ao pane ─────────────────────────────
# Mesmo contrato da statusline: quem tem o dado exato publica; o pane vira plano B. Aqui o publicador
# e a extensao do Pi (scripts/pi/cp-state.ts), que recebe o texto do assistente token a token pelo
# `message_update` — sem largura de janela, sem markdown ja pintado, sem precisar adivinhar o que e
# desenho da TUI. Todas as regras deste arquivo (verbo de ferramenta, caixa do composer, spinner,
# painel de Todos) existem so pra essa adivinhacao; pelo sidecar elas nem entram em cena.
#
# Tres estados, e a diferenca entre os dois ultimos importa:
#   None  -> nao ha sidecar (ou envelheceu) -> CAI NO PANE. Sessao aberta antes da extensao existir,
#            ou sem /reload depois de instalar, nao pode ficar sem previa nenhuma.
#   ""    -> o agente disse que NAO ha texto em voo (turno fechou). E resposta, nao ausencia: cair
#            no pane aqui traria de volta o bloco ja commitado como bolha duplicada.
#   texto -> o bloco em voo, verbatim.
_PREVIEW_SUBDIR = ".claude-pocket-preview"
# Teto de idade: o publicador zera a previa no fim do turno, entao um texto parado por MUITO tempo
# so acontece se a extensao morreu no meio (crash, /reload). Dai em diante o pane volta a mandar, em
# vez de congelar na tela a ultima frase que ela alcancou a publicar.
_PREVIEW_MAX_AGE = 600.0


def read_sidecar(stem: Optional[str]) -> Optional[str]:
    """Texto em voo publicado pela sessao `stem`, "" quando nao ha nenhum, None pra cair no pane."""
    if not stem:
        return None
    for base in _config_dirs():
        f = base / _PREVIEW_SUBDIR / f"{stem}.json"
        try:
            o = json.loads(f.read_text(encoding="utf-8"))
        except OSError:
            continue                 # ausente e o caso NORMAL (sessao sem a extensao / nao-Pi)
        except ValueError:
            _log.debug("preview: sidecar ilegivel path=%s", f, exc_info=True)
            continue
        if not isinstance(o, dict):
            # JSON valido do tipo errado (`null`, lista) nao levanta ValueError e o .get() abaixo
            # explodiria — o mesmo acidente que ja derrubou a resolucao de estado pela statusline.
            _log.debug("preview: sidecar nao e objeto path=%s tipo=%s", f, type(o).__name__)
            continue
        text, ts = o.get("text"), o.get("ts")
        if not isinstance(text, str):
            continue
        if isinstance(ts, (int, float)) and time.time() - ts > _PREVIEW_MAX_AGE:
            # Este e o descarte que importa operacionalmente: "a extensao morreu no meio do turno".
            # Sem o log, quem for entender por que a previa ficou parada ate cair no pane nao tem o
            # que procurar — os outros dois descartes desta funcao ja logam.
            _log.debug("preview: sidecar velho descartado path=%s idade=%.0fs", f, time.time() - ts)
            continue
        return text
    return None


def _extrair_continuacao_kimi(pane: str) -> str:
    """Texto do TOPO do pane ate o primeiro chrome, pra quando o bloco em voo estourou a janela e a
    linha do ● subiu junto — a TUI do Kimi rola o bloco ate o comeco sair da tela, e sem o marcador
    o extract_assistant_text devolve "".

    Sem o ● nao da pra saber onde o bloco comeca, entao esta funcao NAO decide se o texto e
    continuacao de verdade: quem decide e a COSTURA no broker. Se nao colar no acumulado (saida de
    ferramenta, conversa velha parada na tela), e descartado la — por isso aqui vale a forma mais
    simples: tudo do topo ate o chrome, sem adivinhar bloco."""
    out = []
    for ln in pane.splitlines():
        s = ln.lstrip()
        if (_RULE_RE.match(ln) or _OVERLAY_RULE_RE.match(ln) or _is_boundary(ln)
                or _USER_PROMPT_RE.match(ln) or _KIMI_SPINNER_RE.match(ln)
                or _KIMI_TODO_HEAD_RE.match(ln) or _KIMI_TODO_ITEM_RE.match(ln)
                or _KIMI_USED_RE.match(s)):
            break
        if any(r.match(ln) for r in _STOPS_BY_PROVIDER["kimi"]):
            break
        out.append(ln.rstrip())
    while out and not out[0].strip():
        out.pop(0)
    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


# ── Costura do texto em voo do Kimi (pane em tela alternativa NAO tem scrollback) ────────────────
# Medido em 19/08/2026 nesta maquina: a TUI do Kimi roda com alternate_on=1, entao o que sobe da
# tela SE PERDE (history tinha 12 linhas) — o capture devolve so a janela visivel (~45 linhas) e o
# comeco de uma resposta longa em geracao some antes de commitar. Como o broker le o pane a cada
# 150ms enquanto trabalha, cada quadro traz a CAUDA do bloco: colando quadro novo no acumulado pela
# sobreposicao (o fim do acumulado casa com o comeco do quadro), o texto em voo vira incremental —
# so cresce no fim, igual aos deltas que o hook MessageDisplay publica pro Claude. E o que permite
# ao front mostrar a previa do Kimi SEM o teto de 10 linhas do `.plain` (que existe porque texto
# raspado troca inteiro e instavel; o costurado nao troca).
#
# Best-effort como tudo neste arquivo: se a TUI redesenhar de um jeito inesperado e a sobreposicao
# nao casar, o acumulado recomeca do quadro atual (perde o topo ate o commit) — cosmetico, a bolha
# canonica do wire sempre corrige. Erro de cola NUNCA pode apagar texto ja commitado: a supressao
# (preview_is_committed) e o swap final valem do mesmo jeito.
_COSTURA_MIN = 24   # sobreposicao menor que isto e coincidencia (bloco NOVO), nao continuacao:
                    # a 150ms de cadencia, dois quadros de uma janela de ~45 linhas se sobrepoem
                    # quase inteiros; menos que uma frase de overlap = o bloco mudou de identidade.


def _costurar(acum: str, novo: str) -> tuple[str, bool]:
    """Cola o quadro atual do pane no texto em voo acumulado (ver o bloco acima). Devolve
    (novo acumulado, colou). `novo` vazio = quadro sem prosa (ferramenta rodando, spinner):
    MANTEM o acumulado — o bloco em voo continua, e o front segura o ultimo texto do mesmo jeito.

    `colou` = False so no recomeço (sem sobreposicao confiavel: o texto TROCOU inteiro, nao
    cresceu no fim). E o que deixa a flag `full` do broker honesta: publicar "incremental" num
    frame de troca derrotaria o teto de 10 linhas exatamente no caso que ele existe (achado da
    review). O recomeço em si e normal — acontece a cada bloco novo apos um commit; o caso
    anomalo e recomeço no MEIO de um bloco (a TUI redesenhou), e ele degrada pro mesmo recomeço."""
    if not novo:
        return acum, True
    if not acum:
        return novo, True
    if novo.startswith(acum):
        return novo, True     # nada rolou pra fora da tela: o quadro contem o texto inteiro
    # Maior k tal que o SUFIXO do acumulado == PREFIXO do quadro: o quadro e a janela visivel da
    # cauda do mesmo texto; o que passa de k e linha nova que o scroll esconderia sem a costura.
    for k in range(min(len(acum), len(novo)), _COSTURA_MIN - 1, -1):
        if acum.endswith(novo[:k]):
            return acum + novo[k:], True
    return novo, False         # sem sobreposicao confiavel: bloco novo (proximo part) -> recomeca


class PreviewBroker:
    """UM loop de capture por SESSÃO (não por conexão). Faz poll do pane, extrai o texto do bloco em
    voo, guarda o último num slot e acorda os subscribers via Condition. Ref-count: liga no 1º
    subscriber, desliga no último. Evita N× subprocess numa tempestade de reconexão do iOS (cada
    conexão zumbi viveria minutos), que é o que mata no mobile."""

    _brokers: dict[str, "PreviewBroker"] = {}

    def __init__(self, name: str, provider: str = "claude",
                 stem_get: Optional[Callable[[], Optional[str]]] = None):
        self.name = name
        self.provider = provider
        # stem_get: chave do sidecar (o stem do .jsonl VIVO — acompanha o rebind do /clear, igual ao
        # sid_get do StateMonitor). None = so pane, que e o caminho de quem nao publica.
        self.stem_get = stem_get
        self.text = ""
        # Origem do ULTIMO texto: sidecar do agente (markdown cru) x pane (ja pintado pela TUI).
        # Anda junto com `text` -- quem le os dois no mesmo instante ve o par coerente.
        self.md = False
        # full: o texto e INCREMENTAL (so cresce no fim) — sidecar/deltas do agente, ou a costura
        # do pane do Kimi (ver _costurar). E o que libera o front a mostrar a previa SEM o teto de
        # 10 linhas do texto raspado. Anda junto com text/md pelo mesmo motivo do par md.
        self.full = False
        # Acumulado da costura (so provider kimi; ver _costurar). Vive no broker e nao no _loop
        # pra sobreviver a reconexoes de subscriber — o texto em voo nao pode recomecar do quadro
        # atual so porque o celular trocou de rede.
        self._kimi_acum = ""
        self.version = 0
        self._cond = asyncio.Condition()
        self._task: Optional[asyncio.Task] = None
        self._subs = 0

    @classmethod
    def get(cls, name: str, provider: str = "claude",
            stem_get: Optional[Callable[[], Optional[str]]] = None) -> "PreviewBroker":
        # provider: um broker por SESSAO, e toda conexao da mesma sessao traz o mesmo provider —
        # so o primeiro a chegar o fixa. Nao reatribuimos num broker vivo pra nao trocar a leitura
        # debaixo de um subscriber; o broker morre com o ultimo subscriber e o proximo renasce
        # com o provider atual.
        #
        # stem_get NAO segue essa politica: o provider nunca muda na vida da sessao, o stem MUDA (e
        # justamente por causa do /clear que este parametro existe). Como a closure fecha sobre o
        # `current_jsonl` da CONEXAO, travar a do primeiro subscriber deixa o broker lendo o stem da
        # sessao anterior quando essa conexao cai e outra segue viva (celular + desktop) — a previa
        # da sessao nova mostraria texto da velha. Entao a conexao mais recente manda: toda conexao
        # viva reconverge pro mesmo valor depois do /clear, e uma closure morta nunca fica no lugar.
        b = cls._brokers.get(name)
        if b is None:
            b = cls(name, provider, stem_get)
            cls._brokers[name] = b
        elif stem_get is not None:
            b.stem_get = stem_get
        return b

    def reset(self) -> None:
        """Zera texto e acumulado: o /clear trocou de transcript, e o que estava em voo era da
        conversa APAGADA. Sem isto o broker (que sobrevive ao clear — e por nome de sessao, nao de
        transcript) republicava o texto velho no primeiro yield de uma reconexao, com a supressao
        desarmada (committed zerado no mesmo evento): bolha fantasma da conversa anterior (achado
        da review). Nao notifica: quem chama (o __reset__ do SSE) ja limpa o front por conta."""
        self._kimi_acum = ""
        self.text = ""
        self.md = False
        self.full = False

    async def _loop(self) -> None:
        # SEMPRE extrai o último bloco ● (NÃO gateia por spinner): a detecção de spinner pisca falso
        # por 1 frame durante o redraw, e gatear nisso fazia o broker emitir "" -> a bolha SUMIA e
        # voltava toda hora (flicker). O front limpa o preview por reconcile (coberto pelo .jsonl) /
        # idle. O spinner serve só pra CADÊNCIA: rápido trabalhando, devagar ocioso. Diff-gate (só
        # notifica em mudança) evita spam.
        while True:
            # Sidecar primeiro: quando o agente publica, nem chega a rodar o capture-pane (um
            # subprocess a cada 150ms por sessao — o poll mais caro que o backend tem).
            stem = self.stem_get() if self.stem_get else None
            try:
                doAgente = await asyncio.to_thread(read_sidecar, stem)
            except Exception:
                # read_sidecar ja trata sozinho o esperado (ausente, ilegivel, tipo errado). O que
                # cai aqui e bug de verdade, e sem rastro a sessao voltaria pro pane PRA SEMPRE
                # parecendo "sessao sem extensao" — indistinguivel de fora.
                _log.debug("preview: leitura do sidecar falhou stem=%s", stem, exc_info=True)
                doAgente = None
            if doAgente is not None:
                text, md, full = doAgente, True, True   # o agente publica incremental -> sem teto
                working = bool(text)   # cadencia: rapido enquanto ha texto crescendo
            else:
                md = False
                # Kimi: pane COM cor, porque so o italico separa raciocinio de resposta (ver
                # sem_pensamento_kimi). Os outros seguem no texto puro de sempre — `-e` ali seria
                # custo e risco por nada.
                kimi = self.provider == "kimi"
                try:
                    # A chamada dos OUTROS providers fica byte-identica de proposito (nem o
                    # argumento `lines` explicito): este e o poll mais quente do backend e ha dublê
                    # de teste com assinatura de um argumento so.
                    pane = await (asyncio.to_thread(tmux.capture_pane, self.name, 200, True) if kimi
                                  else asyncio.to_thread(tmux.capture_pane, self.name))
                except Exception:
                    # Sem log isto congela a previa do Kimi no ultimo texto com full=True,
                    # indistinguivel de "geracao longa em andamento" (achado da review — o
                    # except vizinho, do sidecar, ja loga com exc_info pelo mesmo motivo).
                    _log.debug("preview: capture_pane falhou sessao=%s", self.name, exc_info=True)
                    pane = ""
                if kimi:
                    pane = sem_pensamento_kimi(pane)
                working = _live_spinner(pane) is not None
                text = extract_assistant_text(pane, self.provider)
                if kimi:
                    if not text and self._kimi_acum:
                        # O bloco estourou a janela e o ● subiu junto (medido nos quadros de
                        # 19/08/2026): tenta o topo do pane como continuacao. A COSTURA e o
                        # portao — se nao colar no acumulado (saida de ferramenta, conversa
                        # velha), cai fora e o acumulado fica como esta.
                        cont = _extrair_continuacao_kimi(pane)
                        colado, colou = _costurar(self._kimi_acum, cont)
                        text = colado if colou else ""
                    # Costura a janela visivel no acumulado (o pane em tela alternativa PERDE o que
                    # sobe — ver _costurar): a previa do Kimi vira incremental e ganha o sem-teto.
                    acum_antes = self._kimi_acum
                    text, colou = _costurar(acum_antes, text)
                    self._kimi_acum = text
                    # full so vale se a costura COLOU neste frame: no recomeco o texto trocou
                    # inteiro, e publicar "incremental" derrotaria o teto exatamente no caso que
                    # ele existe (achado da review).
                    full = colou
                    if not colou:
                        _log.debug("preview: costura recomecou sessao=%s (%d chars do acumulado "
                                   "fora; normal a cada bloco novo, anomalo no meio de um)",
                                   self.name, len(acum_antes))
                else:
                    full = False
            if text != self.text or md != self.md or full != self.full:
                async with self._cond:
                    self.text = text
                    self.md = md
                    self.full = full
                    self.version += 1
                    self._cond.notify_all()
            await asyncio.sleep(0.15 if working else 0.75)

    async def subscribe(self) -> AsyncIterator[tuple[str, bool, bool]]:
        """Emite `(texto, md, full)` mais recente (full-replace) a cada mudança. Coalescido por
        natureza: um subscriber lento perde frames intermediários e pega só o último (version +
        slot único).

        O `md` e o `full` saem JUNTOS, sob o mesmo lock que leu o texto — e não relidos do broker
        depois. Ler em momentos diferentes só funciona porque hoje não há `await` de verdade entre
        eles; qualquer coisa inserida ali no futuro (throttle, log, backpressure) desemparelharia
        o trio calado, e a bolha renderizaria markdown de uma leitura com o texto de outra."""
        async with self._cond:
            self._subs += 1
            if self._task is None:
                self._task = asyncio.create_task(self._loop())
        last = -1
        try:
            while True:
                async with self._cond:
                    await self._cond.wait_for(lambda: self.version != last)
                    last = self.version
                    text, md, full = self.text, self.md, self.full
                yield text, md, full
        finally:
            # Limpeza SINCRONA (sem await): `async with self._cond` podia ser interrompido por
            # CancelledError no acquire do lock -> _subs nao decrementava e o _loop vazava (polling
            # tmux pra sempre sem subscriber). Sem await entre as linhas = atomico no event loop
            # single-thread, sem corrida com o subscribe() (que incrementa).
            self._subs -= 1
            if self._subs <= 0 and self._task is not None:
                task, self._task = self._task, None
                self._brokers.pop(self.name, None)
                task.cancel()
