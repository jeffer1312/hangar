import asyncio
import re
from typing import AsyncIterator, Optional

from app import tmux
from app.models import StateEvent

SPINNER_GLYPHS = "✻✽✶✺✢·∗✳✦✧"
_OPTION_RE = re.compile(r"^\s*❯?\s*\d+\.\s+(.*\S)\s*$")
_CURSOR_RE = re.compile(r"^\s*❯\s*\d+\.\s", re.M)
_RULE_RE = re.compile(r"^[\s─]*─{10,}[\s─]*$")  # a horizontal rule (the input box border)


def status_line(pane_text: str) -> Optional[str]:
    """The raw bottom chrome — the user's custom statusline + the mode line — returned
    verbatim so the web shows exactly what the terminal shows (each user has their own).

    It lives below the input box, i.e. after the last horizontal rule in the pane.
    """
    lines = pane_text.splitlines()
    last_rule = -1
    for i, ln in enumerate(lines):
        if _RULE_RE.match(ln):
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
    for i, ln in enumerate(lines):
        if _CURSOR_RE.match(ln):
            cursor = i  # cursor mais ao fundo = o picker vivo
    if cursor is None:
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
        options = [m.group(1).strip()
                   for m in (_OPTION_RE.match(ln) for ln in region) if m]
        if options:
            return ("awaiting_input", None, _question(region), options)

    spinner = _live_spinner(pane_text)
    if spinner is not None:
        return ("working", spinner[2:].strip(), None, None)

    return ("idle", None, None, None)


class StateMonitor:
    # Polls com o MESMO spinner antes de tratá-lo como marcador de turn CONCLUÍDO congelado (idle)
    # em vez de spinner vivo animando (working).
    STALE_LIMIT = 3
    # Polls CONSECUTIVOS sem spinner antes de confirmar idle. O capture-pane às vezes pega o TUI
    # mid-redraw (sem a linha do spinner por 1 frame); sem este debounce o estado piscava working
    # <-> idle e a UI (spinner/botão stop/scroll) ficava "pulando" o tempo todo durante o streaming.
    IDLE_DEBOUNCE = 4

    def __init__(self, name: str, poll: float = 0.75):
        self.name = name
        self.poll = poll

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

            status = status_line(pane)
            # Overlay so-TUI aberto: rodape de navegacao presente NO FUNDO do pane. So as ultimas linhas
            # (nao o pane inteiro): o overlay sempre renderiza o rodape no rodape; procurar no pane todo
            # dava FALSO-POSITIVO quando a MESMA frase ("Esc to cancel") aparecia na CONVERSA/scrollback
            # (ex: uma msg citando o rodape abria o espelho por cima do chat). Inclui pickers (/model) e
            # paineis sem opcoes numeradas (/status, /config, /help). O front decide: com `options` (menu
            # nativo) usa botoes; sem opcoes mas overlay=True abre o espelho pra navegar via teclas.
            overlay = is_overlay(pane)
            login = is_login(pane)
            key = (state, label, question, tuple(options or ()), status, overlay, login)
            if key != last_key:
                last_key = key
                held_state, held_label = state, label
                yield StateEvent(session=self.name, state=state, label=label,
                                 question=question, options=options, status_line=status,
                                 overlay=overlay, login=login)
            await asyncio.sleep(self.poll)
