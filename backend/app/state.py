import asyncio
import json
import logging
import os
import re
import time
from typing import AsyncIterator, Callable, Optional

from app import tmux
from app.hook_state import hook_state
from app.models import StateEvent
from app.statusline import read as _sidecar_status

_log = logging.getLogger("hangar.state")

SPINNER_GLYPHS = "✻✽✶✺✢·∗✳✦✧"
_OPTION_RE = re.compile(r"^\s*[❯>]?\s*\d+\.\s+(.*\S)\s*$")
# Bordas do box de `preview` do AskUserQuestion: renderiza NA MESMA LINHA da opção -> o label corta
# na primeira borda. Cobre os cantos ARREDONDADOS (╭╮╰╯) e os RETOS (┌┐└┘├┤┬┴┼) mais │ e ─: só os
# arredondados estavam aqui, e o box do preview desenha com os RETOS. Efeito medido: a opção que cai
# na linha da borda de cima vinha "Escolher dimensão +          ┌────────────" e nunca casava com o
# label do sidecar ("Escolher dimensão + valores"), então o gate do sse degradava pro OptionButtons —
# a pergunta perdia descrição E preview no app, e essa opção aparecia VAZIA. Toda pergunta com
# preview caía nisso, que é justamente a que mais precisa do stepper.
# `\s{2,}` antes da borda: o box do preview fica numa COLUNA à direita, então há sempre um vão de
# espaços entre o fim do label e a borda. Sem essa exigência, `─` (que é caractere de box, não o
# travessão —) cortava um label que o usasse no próprio texto: "Rodar tudo ─ inclusive os lentos"
# virava "Rodar tudo" e aí o casamento por prefixo aprovava um label ERRADO. `│` e os cantos seguem
# cortando sozinhos também: o box de `preview` de UMA coluna encosta no label sem o vão.
_BOX_SPLIT_RE = re.compile(r"\s{2,}[│─╭╮╰╯┌┐└┘├┤┬┴┼]|[│╭╮╰╯┌┐└┘├┤┬┴┼]")
# Cursor do picker: ❯ e do Claude, ">" e do Pi (ascii). O do Pi so vale com o rodape de navegacao
# NO FUNDO do pane (ver _menu_block) — sem essa trava, um "> 1. ..." citado em prosa no scrollback
# viraria menu fantasma.
_CURSOR_RE = re.compile(r"^\s*❯\s*\d+\.\s", re.M)
_PI_CURSOR_RE = re.compile(r"^\s*>\s*\d+\.\s", re.M)
_RULE_RE = re.compile(r"^[\s─]*─{10,}[\s─]*$")  # a horizontal rule (the input box border)
# Rodape da caixa ARREDONDADA do composer (`╰───╯`), que e como o Pi desenha o input — ele nunca
# imprime a regua reta do Claude. Sem esta ancora um pane do Pi nao casava nada e o fallback
# devolvia as 2 ultimas linhas nao-vazias, isto e, a borda da caixa (ou conversa) no lugar do chip
# de modelo/contexto/custo. Claude/Codex ficam intactos: o banner de boas-vindas do Claude tambem
# tem `╰───╯`, mas vem ANTES da regua do input, e a ancora e sempre a MAIS BAIXA das duas.
# ponytail: desenho da caixa e calibration knob, igual ao _LOGIN_RE abaixo — se o Pi trocar a
# moldura, ajustar AQUI.
_BOX_BOTTOM_RE = re.compile(r"^\s*╰[─\s]*╯\s*$")


def status_line(pane_text: str) -> Optional[str]:
    """The raw bottom chrome — the user's custom statusline + the mode line — returned
    verbatim so the web shows exactly what the terminal shows (each user has their own).

    It lives below the input box, i.e. after the last horizontal rule (Claude) or the last
    rounded composer box (Pi). No chrome below it -> None: an agent without a statusline
    extension shows NOTHING, never a slice of the conversation.
    """
    lines = pane_text.splitlines()
    last_rule = -1
    for i, ln in enumerate(lines):
        if _RULE_RE.match(ln) or _BOX_BOTTOM_RE.match(ln):
            last_rule = i
    if last_rule >= 0:
        chrome = [ln.rstrip() for ln in lines[last_rule + 1:] if ln.strip()]
    else:
        chrome = [ln.rstrip() for ln in lines if ln.strip()][-2:]
    return "\n".join(chrome) if chrome else None


def _question(lines: list[str]) -> Optional[str]:
    """The prompt text just above the first option in the menu region: the last meaningful
    line (skip rules, the ☐/☑ header chip, and blanks). Recebe ja a REGIAO do menu (nao o
    pane inteiro) pra nao pescar uma pergunta perdida no scrollback."""
    found = None
    for line in lines:
        if _OPTION_RE.match(line):
            break
        s = line.strip()
        if not s or _RULE_RE.match(line) or s[:1] in "☐☑":
            continue
        found = s
    return found


# Rodape de navegacao da AskUserQuestion (e de pickers similares). Ancora o limite INFERIOR
# do bloco. O menu nativo de permissao NAO tem esse rodape -> o limite cai num boundary.
_FOOTER_RE = re.compile(r"to navigate|Esc to cancel|Enter to select")


def is_overlay(pane_text: str) -> bool:
    # Overlay so-TUI aberto: rodape de navegacao por teclas no FUNDO do pane (ultimas 8 linhas — nao o
    # pane todo, senao a MESMA frase citada na conversa/scrollback dava falso-positivo). Cobre pickers
    # (/model) e paineis (/status, /config, /help) alem do AskUserQuestion. Fonte unica de "overlay"
    # (StateMonitor e terminal_input.deliverable usam esta).
    return bool(_FOOTER_RE.search("\n".join(pane_text.splitlines()[-8:])))


# Marcadores da tela de welcome/login do Claude Code (tema -> metodo -> URL OAuth -> colar code).
# Nenhum aparece numa sessao ja logada e em uso, entao servem de sinal de "precisa logar". ponytail:
# strings best-effort — se uma versao futura do claude mudar o texto, ajustar AQUI (calibration knob).
# Ancorado na URL OAuth (sempre presente no passo de login) + frases exclusivas do onboarding.
_LOGIN_RE = re.compile(
    r"/oauth/authorize|Paste code here|Select login method|Choose the text style",
    re.I,
)


def is_login(pane_text: str) -> bool:
    """Sessao parada na tela de welcome/login do Claude Code (sem .jsonl ainda)."""
    return bool(_LOGIN_RE.search(pane_text))


# Banner de rate-limit (feature #8). ponytail: texto EXATO do Claude Code nao documentado
# publicamente -- CALIBRATION KNOB, igual ao _LOGIN_RE acima: melhor-esforco, ajustar aqui quando
# confirmado contra o banner real. Cobre variantes plausiveis ("usage limit reached" / "5-hour limit
# reached" / "rate limit") seguidas da frase de reset ("resets at 3pm" / "resets 15:30" / "try again
# at ..."), capturando so o horario.
_LIMIT_RE = re.compile(
    r"(?:usage limit reached|rate limit reached|limit reached)"
    r".{0,80}?"
    r"(?:resets?|reset|try again)\s*(?:at\s*)?"
    r"([0-9]{1,2}(?::[0-9]{2})?\s*(?:am|pm)?)",
    re.I | re.S,
)


def rate_limit_reset(pane_text: str) -> Optional[str]:
    """Horario de reset do limite de uso (string crua, ex: "3pm"/"15:30"), se o pane mostra o
    banner de rate-limit. None numa sessao normal. ponytail: calibration knob -- ver _LIMIT_RE.

    LIMITACAO DE COBERTURA (feature #8): so roda sobre um pane REALMENTE capturado. O StateMonitor
    (chat aberto) captura sempre, entao o campo `limited` funciona la; mas a LISTA (list_with_state)
    fast-pathea sessoes working/idle pelo marcador do hook e PULA a captura -> nesse caminho (o normal
    pra uma sessao rate-limited, que fica working/idle) rate_limit_reset nunca e chamado e limited fica
    False. Ver a nota no fast-path de registry.list_with_state. Nao mover a deteccao pro watchdog antes
    de _LIMIT_RE ser calibrado contra o banner real (hoje e chute nao-calibrado)."""
    m = _LIMIT_RE.search(pane_text)
    return m.group(1).strip() if m else None
# Glifos que marcam a BORDA do box do picker: bullet de assistente, junta de tool-result e
# spinners. Scrollback (incl. listas numeradas perdidas) vive alem dessas linhas.
_BOUNDARY_GLYPHS = "●⎿" + SPINNER_GLYPHS


def _is_boundary(line: str) -> bool:
    s = line.lstrip()
    return bool(s) and s[0] in _BOUNDARY_GLYPHS


def _menu_block(lines: list[str]) -> Optional[tuple[int, int]]:
    """Bounds [top, bot) do menu de selecao contiguo que contem o cursor ❯, ou None.

    Escopar a este bloco e o que mantem linhas numeradas do SCROLLBACK FORA das opcoes: subindo
    paramos no primeiro boundary (bullet/spinner); descendo paramos no rodape de navegacao ou
    no proximo boundary. Sem cursor ❯ N. nao ha menu (uma lista numerada solta nao e widget)."""
    cursor = None
    pi_cursor = False
    for i, ln in enumerate(lines):
        if _CURSOR_RE.match(ln):
            cursor, pi_cursor = i, False   # cursor mais ao fundo = o picker vivo
        elif _PI_CURSOR_RE.match(ln):
            cursor, pi_cursor = i, True
    if cursor is None:
        return None
    # Cursor do Pi ("> N.") so e picker VIVO se o rodape de navegacao estiver nas ultimas linhas do
    # pane (mesma janela do is_overlay). Uma citacao do picker no scrollback tem o "> 1." mas o
    # rodape dela subiu junto — sem a trava ela travava o app num menu fantasma.
    if pi_cursor and not _FOOTER_RE.search("\n".join(lines[-8:])):
        return None
    # Um menu VIVO substitui o composer de input. Se ABAIXO do cursor renderiza o composer vivo (linha
    # de prompt "❯ " vazia ou com rascunho — comeca com ❯ mas NAO e "❯ N." de opcao), entao este "❯ N."
    # e PROSA citada no scrollback (ex: o assistente citando o menu nativo "❯ 1. Yes, switch...") e nao
    # um widget selecionavel. Sem essa guarda a citacao trava o app num menu fantasma (awaiting_input).
    if any(ln.lstrip()[:1] == "❯" and not _CURSOR_RE.match(ln)
           for ln in lines[cursor + 1:]):
        return None
    # Subindo do cursor, o topo do picker e o CHIP header (☐/☑ da AskUserQuestion) ou um boundary
    # (bullet/spinner do menu nativo, que nao tem chip). Parar no chip mantem FORA do bloco uma
    # LISTA NUMERADA EM PROSA da mensagem do assistente — ela vive acima do chip e, sem essa
    # ancora, "1. ... 2. ..." da prosa entrariam como opcoes falsas. A pergunta fica entre o chip
    # e as opcoes (com linha em branco no meio), entao continua dentro da regiao.
    top = 0
    for i in range(cursor - 1, -1, -1):
        s = lines[i].lstrip()
        if _is_boundary(lines[i]) or (bool(s) and s[0] in "☐☑"):
            top = i + 1
            break
    bot = len(lines)
    for i in range(cursor + 1, len(lines)):
        if _FOOTER_RE.search(lines[i]) or _is_boundary(lines[i]):
            bot = i
            break
    return (top, bot)


def _live_spinner(pane_text: str) -> Optional[str]:
    """The bottom-most spinner-glyph line (raw, incl. glyph), or None.

    The live status line sits at the bottom, just above the input box. Completed-turn
    markers ("✻ Worked for 8s") linger ABOVE it in the scrollback and look identical to a
    live spinner — so we take the bottom-most candidate and let StateMonitor decide
    live-vs-frozen by whether it animates.
    """
    for line in reversed(pane_text.splitlines()):
        s = line.strip()
        if len(s) >= 2 and s[0] in SPINNER_GLYPHS and s[1] == " ":
            return s
    return None


def classify(pane_text: str) -> tuple[str, Optional[str], Optional[str], Optional[list[str]]]:
    """Return (state, label, question, options).

    'working' -> label is the live spinner text; 'awaiting_input' -> question +
    options; otherwise 'idle'. 'dead' is decided by the caller (StateMonitor).

    NOTE: a single static pane cannot tell a live spinner from a frozen completed-turn
    marker (both render as "<glyph> <word> for <N>s"). classify reports 'working' for any
    spinner candidate; StateMonitor downgrades a non-animating one to 'idle'.
    """
    lines = pane_text.splitlines()
    block = _menu_block(lines)
    if block is not None:
        top, bot = block
        region = lines[top:bot]
        # AskUserQuestion com `preview` renderiza um box (│...│) NA MESMA LINHA da opção — sem o
        # corte, o conteúdo do preview entrava no label ("Alfabético (obedece │ using System..."). A
        # label ainda pode vir truncada por wrap de coluna; o gate do stepper (sse) casa por prefixo.
        options = [_BOX_SPLIT_RE.split(m.group(1))[0].strip()
                   for m in (_OPTION_RE.match(ln) for ln in region) if m]
        options = [o for o in options if o]
        if options:
            return ("awaiting_input", None, _question(region), options)

    spinner = _live_spinner(pane_text)
    if spinner is not None:
        return ("working", spinner[2:].strip(), None, None)

    return ("idle", None, None, None)


# Folga entre a ultima escrita no transcript e o Stop do MESMO turno. Medido em 18 sessoes Kimi
# reais (13/08/2026): numa sessao que terminou o turno a diferenca e 0.0s — o Stop chega no mesmo
# segundo da ultima linha. A sessao travada media +1089s. 2s separa os dois casos com folga.
KIMI_FOLGA_S = 2.0

# Fronteiras de turno no wire.jsonl do Kimi (levantado em 14/08/2026 sobre todos os wire.jsonl da
# maquina: 185 turn.prompt, 166 turn.ended, 29 turn.steer, 3 turn.cancel — nao ha outro `turn.*`).
# `turn.steer` e o usuario falando NO MEIO do turno, entao mantem aberto; `turn.cancel` e o Esc, e
# medido que sempre vem seguido de um `turn.ended`.
_KIMI_ABRE = ("turn.prompt", "turn.steer")
_KIMI_FECHA = ("turn.ended", "turn.cancel")
_KIMI_TURNO_RE = re.compile(rb'"type"\s*:\s*"(turn\.[a-z]+)"')
_KIMI_CHUNK = 64 << 10
_KIMI_TETO = 4 << 20


# Uma linha por arquivo, nao por chamada: quem chama e o poll da lista (~5s por sessao), entao um
# wire ilegivel viraria uma linha de log a cada ciclo, pra sempre. O sinal aqui e "aconteceu", nao a
# frequencia. Sem expiracao de proposito — o conjunto tem no maximo uma entrada por sessao viva.
_KIMI_AVISADOS: set[str] = set()


def _avisa_uma_vez(chave: str, msg: str, *args) -> None:
    if chave in _KIMI_AVISADOS:
        return
    _KIMI_AVISADOS.add(chave)
    _log.warning(msg, *args)


def _kimi_turno_aberto(jsonl: str, teto: int = _KIMI_TETO) -> Optional[bool]:
    """Ha turno ABERTO no fim do wire.jsonl? True/False; None = nao deu pra saber.

    Le o arquivo de TRAS PRA FRENTE ate a primeira linha que seja uma fronteira de turno — na
    pratica isso e uma ou duas linhas, porque o wire de uma sessao parada termina no `turn.ended`.
    O `teto` existe pro caso patologico (arquivo sem nenhum evento de turno): melhor devolver None e
    cair no mtime do que varrer megabytes a cada poll.

    O regex e so o filtro barato — quem decide e o `type` de TOPO da linha, via json. Sem isso, uma
    mensagem do usuario CITANDO "turn.ended" (este commit, por exemplo) seria lida como fronteira.
    """
    try:
        with open(jsonl, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            fim = pos = fh.tell()
            resto = b""
            while pos > 0 and fim - pos < teto:
                passo = min(_KIMI_CHUNK, pos)
                pos -= passo
                fh.seek(pos)
                linhas = (fh.read(passo) + resto).split(b"\n")
                # linhas[0] so esta INTEIRA quando ja chegamos ao inicio do arquivo; ate la ela e o
                # rabo cortado pelo chunk e volta colada no proximo pedaco.
                resto, inicio = (b"", 0) if pos == 0 else (linhas[0], 1)
                for ln in reversed(linhas[inicio:]):
                    if not _KIMI_TURNO_RE.search(ln):
                        continue
                    try:
                        tipo = json.loads(ln).get("type")
                    except (ValueError, AttributeError):
                        continue
                    if tipo in _KIMI_FECHA:
                        return False
                    if tipo in _KIMI_ABRE:
                        return True
    except OSError:
        # None faz o chamador voltar a decidir SO pelo mtime — que e exatamente o comportamento que
        # esta funcao existe pra corrigir. Sem log, "decidiu False" e "desistiu e caiu no mtime"
        # ficam indistinguiveis, e uma sessao presa em "em execucao" nao tem onde ser diagnosticada.
        _avisa_uma_vez(jsonl, "kimi: nao deu pra ler o fim do wire jsonl=%s", jsonl)
        return None
    _avisa_uma_vez(jsonl, "kimi: nenhuma fronteira de turno no fim de %s — decidindo pelo mtime",
                   jsonl)
    return None


def _kimi_mtime_da_sessao(jsonl: str) -> Optional[float]:
    """mtime mais NOVO entre os wires da sessao: `<sessao>/agents/*/wire.jsonl`.

    O main nao e a unica prova de vida. Quando ele delega pra subagentes (tool `Agent`), quem escreve
    sao os wires `agents/agent-N/` — o main fica calado o turno inteiro. Olhar so pra ele fazia a
    sessao parecer parada exatamente durante o trabalho mais longo.

    Erro (dir sumiu, permissao) devolve o mtime do proprio `jsonl`, e None se nem ele der.

    So varre quando a pasta avo se chama `agents` — o layout real do Kimi. Sem esse gate, um caminho
    de outro formato (dublê de teste, transcript de outro provider) faria a varredura cair numa pasta
    qualquer e adotar o mtime de um `wire.jsonl` que nao e desta sessao.
    """
    agents = os.path.dirname(os.path.dirname(jsonl))       # .../agents/main/wire.jsonl -> .../agents
    if os.path.basename(agents) != "agents":
        try:
            return os.path.getmtime(jsonl)
        except OSError:
            return None
    try:
        novos = []
        with os.scandir(agents) as it:
            for e in it:
                if not e.is_dir():
                    continue
                try:
                    novos.append(os.path.getmtime(os.path.join(e.path, "wire.jsonl")))
                except OSError:
                    continue                                # subagente sem wire ainda: ignora
        if novos:
            return max(novos)
    except OSError:
        # Cair aqui devolve a sessao pro mtime do MAIN, que e exatamente o criterio cego que esta
        # funcao existe pra corrigir — e sem log isso fica indistinguivel de "nao ha subagente".
        # Mesma disciplina do `_kimi_turno_aberto`: uma linha por caminho, nao por poll.
        _avisa_uma_vez(agents, "kimi: nao deu pra varrer %s — mtime da sessao cai no main", agents)
    try:
        return os.path.getmtime(jsonl)
    except OSError:
        return None


def corrige_ocioso_kimi(marker, jsonl: Optional[str], folga: float = KIMI_FOLGA_S):
    """Kimi: marcador 'idle' velho + transcript crescendo = turno ANDANDO, nao sessao parada.

    No Kimi, um turno que comeca a partir de um prompt ENFILEIRADO na TUI nao dispara
    UserPromptSubmit nem TurnStarted (medido 13/08/2026: uma sessao Kimi ficou com o
    marcador congelado em 'idle' as 08:38:35 enquanto escrevia codigo ate 08:56 — 18 minutos
    aparecendo "pronta" na lista e no chat). O hook nao tem como cobrir isso: o evento nao existe.

    O Stop, porem, e o ULTIMO evento do turno — sempre DEPOIS da ultima escrita no transcript. Logo
    transcript mais novo que um marcador OCIOSO indica turno em andamento. Sem raspar o pane, de
    proposito: o spinner do Kimi e fase de lua, fora de SPINNER_GLYPHS, e nunca seria detectado la —
    foi por isso que o fallback visual tambem nao salvou.

    O MTIME NAO DECIDE NADA — quem decide e a ultima FRONTEIRA de turno do wire do main
    (`_kimi_turno_aberto`): `turn.prompt`/`turn.steer` por ultimo = turno andando; `turn.ended`/
    `turn.cancel` = parada. Duas medicoes de 14/08/2026 mataram o mtime como criterio, cada uma por
    um lado:

      - mtime NOVO com a sessao parada: turno fechou 08:28:44 e as 08:40:46 o Kimi gravou um
        `config.update` (o system prompt inteiro, ~90KB). 12min a frente do marcador -> a lista
        dizia "em execucao" com o pane no prompt.
      - mtime PARADO com a sessao trabalhando: o main delegou pra SUBAGENTES (tool `Agent`), que
        escrevem no wire DELES (`agents/agent-N/wire.jsonl`) e nao no do main. O main ficou calado
        4 minutos; nesse meio tempo um subagente terminou, o Stop disparou com o session_id da
        SESSAO (subagente roda no mesmo processo) e o marcador virou `idle` — com o marcador e o
        mtime no MESMO segundo, o portao antigo nem chegava a olhar a fronteira e a sessao aparecia
        "pronta" enquanto o terminal mostrava "Running 2 agents".

    O mtime da SESSAO (o mais novo entre `agents/*/wire.jsonl`, main + subagentes) so e lido no
    caminho degradado, o unico em que ele ainda decide alguma coisa.

    Preserva o caso que criou esta funcao: prompt ENFILEIRADO na TUI grava `turn.prompt` sem
    disparar hook nenhum, entao a fronteira o pega. E continua sem raspar pane.

    So corrige idle -> working. 'awaiting_input' segue seu caminho (a pergunta so existe no pane) e
    'working' ja esta certo.

    SEM teto de idade, por decisao. A tentacao e dizer "transcript parado ha 10min nao prova nada" e
    voltar pro marcador — resolveria o caso do Kimi que MORRE no meio do turno (os dois numeros
    congelam na ordem errada e a sessao fica "trabalhando" pra sempre). Mas o preco e pior que a
    doenca: um turno legitimamente calado por mais que esse teto — um build, uma suite longa, um
    comando que nao escreve nada no wire — voltaria a "idle" e dispararia, de uma vez, o loop
    re-promptando texto por cima do comando em execucao, o `then` sendo CONSUMIDO e o push de
    "terminou". Estado errado numa sessao morta o dono VE (o pane esta ali, parado); automacao
    escrevendo numa sessao viva ele nao ve. Entre os dois, erra-se pro lado visivel.

    Limitacao conhecida que isso deixa: Kimi que morre dentro de um pane que continua vivo aparece
    "em execucao" ate o pane ser fechado. O custo por poll e UMA leitura do rabo do wire — medida em
    0.057ms num wire de 5,4MB, porque a busca para na primeira fronteira e o fim do arquivo e onde
    ela esta. O scandir da pasta de agentes NAO entra nesse custo: ele so roda no caminho degradado.
    O `folga` tambem so sobrevive la, unico lugar onde o mtime ainda decide.

    Recebe o CAMINHO (e nao o mtime ja lido) porque sao tres chamadores — a lista, o monitor do chat
    aberto e o gatilho de automacoes — e a leitura com try/except tem que ser a mesma nos tres."""
    if not marker or marker[0] != "idle":
        return marker
    if not jsonl:
        return marker
    aberto = _kimi_turno_aberto(jsonl)
    if aberto is False:
        return marker
    if aberto is None:
        # Degradado (wire ilegivel/sem fronteira): unico caminho em que o mtime ainda decide, e ai
        # ele e o da SESSAO — o do main sozinho nao ve o subagente trabalhando. Erra pro lado de
        # "trabalhando"; `_kimi_turno_aberto` ja avisou uma vez no log.
        mtime = _kimi_mtime_da_sessao(jsonl)
        if mtime is None or mtime <= marker[1] + folga:
            return marker
        return ("working", mtime)
    # Turno ABERTO: nao ha o que medir — nenhum chamador le o ts deste marcador (todos usam [0], e o
    # `last_activity` da lista sai de um stat proprio do registry). Calcular o mtime da sessao aqui
    # seria um scandir por poll, no caminho MAIS quente, pra um valor que ninguem consome.
    return ("working", marker[1])


class StateMonitor:
    # Polls com o MESMO spinner antes de tratá-lo como marcador de turn CONCLUÍDO congelado (idle)
    # em vez de spinner vivo animando (working).
    STALE_LIMIT = 3
    # Polls CONSECUTIVOS sem spinner antes de confirmar idle. O capture-pane às vezes pega o TUI
    # mid-redraw (sem a linha do spinner por 1 frame); sem este debounce o estado piscava working
    # <-> idle e a UI (spinner/botão stop/scroll) ficava "pulando" o tempo todo durante o streaming.
    IDLE_DEBOUNCE = 4
    # Polls sem spinner no pane apos os quais um marcador de hook "working" deixa de ser confiavel
    # (claude morreu mid-turn sem disparar Stop -> marcador preso em working; o pane e a verdade).
    HOOK_WORKING_GRACE = 8

    def __init__(self, name: str, poll: float = 0.75,
                 sid_get: Optional[Callable[[], Optional[str]]] = None,
                 hook_grace: Optional[int] = HOOK_WORKING_GRACE,
                 transcript_get: Optional[Callable[[], Optional[str]]] = None):
        self.name = name
        self.poll = poll
        # hook_grace: apos quantos polls SEM SPINNER o marcador "working" deixa de valer. None =
        # nunca expira. SPINNER_GLYPHS sao os do Claude; num pane cujo loader o classify nao le (o
        # do Pi e braille, ⠋⠙⠹...), `no_spinner` sobe durante o turno inteiro e a grace derrubava o
        # estado pra idle NO MEIO da conversa. Sem spinner legivel o marcador e a unica verdade —
        # e a mesma politica da lista (registry.py:719, que honra o marcador sem grace nenhuma).
        # ponytail: o preco de None e marcador "working" preso se o agente morrer mid-turn sem
        # emitir o evento de fim; upgrade = ensinar o classify a ler o spinner desse provider.
        self.hook_grace = hook_grace
        # sid_get: session-id VIVO da sessao (muda no /clear) -> ancora o estado nos marcadores dos
        # hooks (deterministicos) em vez de depender so da leitura visual do pane. None = so pane.
        self.sid_get = sid_get
        # transcript_get: caminho do transcript VIVO. So o Kimi passa — e a segunda fonte de
        # `corrige_ocioso_kimi`, pro chat aberto nao mostrar "pronta" uma sessao que esta no meio de
        # um turno vindo da fila da TUI (ver a docstring da funcao). None = comportamento de sempre.
        self.transcript_get = transcript_get

    def _marcador(self):
        """Marcador do hook, ja corrigido quando ha transcript pra contradizer um idle velho."""
        m = hook_state.get_state(self.sid_get())
        if m is None or self.transcript_get is None:
            return m
        return corrige_ocioso_kimi(m, self.transcript_get())

    async def stream(self) -> AsyncIterator[StateEvent]:
        last_key = object()
        prev_spinner = None
        frozen = 0          # polls com o mesmo spinner (congelado = turn acabou)
        no_spinner = 0      # polls consecutivos sem spinner (filtra redraw transiente)
        held_state = "idle"
        held_label = None
        while True:
            if not await asyncio.to_thread(tmux.has_session, self.name):
                yield StateEvent(session=self.name, state="dead")
                return
            pane = await asyncio.to_thread(tmux.capture_pane, self.name)
            state, label, question, options = classify(pane)
            spinner = _live_spinner(pane)

            if state == "awaiting_input":
                # Menu real (AskUserQuestion/permissão) -> estado autoritativo, sem debounce.
                prev_spinner = None
                frozen = 0
                no_spinner = 0
            elif spinner is not None:
                no_spinner = 0
                frozen = frozen + 1 if spinner == prev_spinner else 0
                prev_spinner = spinner
                # Spinner CONGELADO (byte-idêntico) por STALE_LIMIT polls = marcador de turn concluído.
                state, label = ("idle", None) if frozen >= self.STALE_LIMIT else ("working", label)
            else:
                # Sem spinner NESTE frame: pode ser redraw transiente. Só vira idle após IDLE_DEBOUNCE
                # polls seguidos sem spinner; antes disso, SEGURA o último working (debounce anti-flicker).
                no_spinner += 1
                prev_spinner = None
                frozen = 0
                if held_state == "working" and no_spinner < self.IDLE_DEBOUNCE:
                    state, label = "working", held_label

            # Ancora de hook: working/idle dos marcadores (UserPromptSubmit/PreToolUse/Stop) e
            # deterministico — corrige o pane mal-lido (spinner congelado, redraw). O pane segue
            # dono de awaiting_input/overlay (menus NAO disparam hook) e de dead. Marcador
            # "working" preso (claude morto mid-turn) expira via HOOK_WORKING_GRACE.
            if self.sid_get is not None and state in ("working", "idle"):
                # `_marcador` LE disco quando ha transcript (corrige_ocioso_kimi -> rabo do wire +
                # scandir da pasta de agentes) e este laco e uma corrotina que roda a cada 0.75s por
                # chat aberto: sincrono aqui, seguraria o event loop do backend inteiro. Mesma regra
                # do `capture_pane` logo acima e do git status em registry._decorate_git.
                m = await asyncio.to_thread(self._marcador)
                if m is not None:
                    if m[0] == "idle" and state == "working":
                        state, label = "idle", None
                    elif m[0] == "working" and state == "idle" \
                            and (self.hook_grace is None or no_spinner < self.hook_grace):
                        state = "working"

            # Sidecar primeiro: a linha do pane ja vem cortada na largura da janela (ver
            # app/statusline.py). Sem sidecar, segue o pane.
            status = _sidecar_status(self.sid_get() if self.sid_get else None) or status_line(pane)
            # Overlay so-TUI aberto: rodape de navegacao presente NO FUNDO do pane. So as ultimas linhas
            # (nao o pane inteiro): o overlay sempre renderiza o rodape no rodape; procurar no pane todo
            # dava FALSO-POSITIVO quando a MESMA frase ("Esc to cancel") aparecia na CONVERSA/scrollback
            # (ex: uma msg citando o rodape abria o espelho por cima do chat). Inclui pickers (/model) e
            # paineis sem opcoes numeradas (/status, /config, /help). O front decide: com `options` (menu
            # nativo) usa botoes; sem opcoes mas overlay=True abre o espelho pra navegar via teclas.
            overlay = is_overlay(pane)
            login = is_login(pane)
            # Rate-limit radar (feature #8): banner de limite de uso no pane, best-effort (ver
            # rate_limit_reset/_LIMIT_RE). limited deriva do proprio reset (achou horario -> limited).
            limit_reset = rate_limit_reset(pane)
            limited = limit_reset is not None
            # Loop runner: le o sidecar (barato) e leva no MESMO evento -> chip 🔁 no Chat mobile sem
            # reter o sessionsStore. Entra no key pra re-emitir quando SO o loop muda (sessao parada).
            from app.loop import LoopLink
            loop_d = LoopLink(self.name).get()
            loop_status = loop_d.get("status") if loop_d else None
            loop_iter = loop_d.get("iter") if loop_d else None
            loop_max = loop_d.get("max_iters") if loop_d else None
            key = (state, label, question, tuple(options or ()), status, overlay, login,
                   limited, limit_reset, loop_status, loop_iter, loop_max)
            if key != last_key:
                last_key = key
                held_state, held_label = state, label
                yield StateEvent(session=self.name, state=state, label=label,
                                 question=question, options=options, status_line=status,
                                 overlay=overlay, login=login,
                                 limited=limited, limit_reset=limit_reset,
                                 loop_status=loop_status, loop_iter=loop_iter, loop_max=loop_max)
            await asyncio.sleep(self.poll)
