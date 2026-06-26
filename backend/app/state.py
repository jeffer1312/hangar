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
    # Consecutive unchanged polls before a spinner candidate is treated as a frozen
    # completed-turn marker (idle) rather than a live, animating spinner (working).
    STALE_LIMIT = 3

    def __init__(self, name: str, poll: float = 0.75):
        self.name = name
        self.poll = poll

    async def stream(self) -> AsyncIterator[StateEvent]:
        last_key = object()
        uninit = object()
        prev_spinner = uninit
        stale = 0
        while True:
            if not tmux.has_session(self.name):
                yield StateEvent(session=self.name, state="dead")
                return
            pane = tmux.capture_pane(self.name)
            state, label, question, options = classify(pane)

            if state == "working":
                # The live spinner animates (glyph cycles, elapsed time ticks); a leftover
                # completed-turn marker is byte-identical poll after poll. Only treat it as
                # working once we've actually seen it change — proof of life. On the very
                # first sight we can't tell fresh from stale, so we wait for a change.
                spinner = _live_spinner(pane)
                if prev_spinner is uninit:
                    stale = self.STALE_LIMIT
                elif spinner != prev_spinner:
                    stale = 0
                else:
                    stale += 1
                prev_spinner = spinner
                if stale >= self.STALE_LIMIT:
                    state, label = "idle", None
            else:
                prev_spinner = None
                stale = 0

            status = status_line(pane)
            key = (state, label, question, tuple(options or ()), status)
            if key != last_key:
                last_key = key
                yield StateEvent(session=self.name, state=state, label=label,
                                 question=question, options=options, status_line=status)
            await asyncio.sleep(self.poll)
