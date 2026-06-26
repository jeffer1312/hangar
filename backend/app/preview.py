import asyncio
import re
from typing import AsyncIterator, Optional

from app import tmux
from app.state import _RULE_RE, _is_boundary, _live_spinner

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
    "Crawling|Downloading|Globbing|Grepping|Waiting|Loading|Compiling|Building|Installing|Ran"
)
_TOOL_BLOCK_RE = re.compile(rf"^([A-Z][\w-]*\(|({_TOOL_VERBS})\b)")


def extract_assistant_text(pane: str) -> str:
    """Texto do ÚLTIMO bloco de PROSA do assistente (●) do pane, VERBATIM (sem reflow — núcleo seguro).

    Acha o último ● que NÃO é tool-call (início do bloco em voo; blocos anteriores já caíram no
    .jsonl), tira o "● " da 1ª linha e segue até o primeiro chrome: régua (────), próximo boundary
    (●/⎿/spinner) ou prompt ❯. Sem juntar continuação — o markdown bonito vem no snap final do .jsonl.
    """
    lines = pane.splitlines()
    start = -1
    for i, ln in enumerate(lines):
        s = ln.lstrip()
        if s[:1] == _ASSISTANT_GLYPH and not _TOOL_BLOCK_RE.match(s[1:].lstrip()):
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
        if _RULE_RE.match(ln) or _is_boundary(ln) or _USER_PROMPT_RE.match(ln) or _TOOL_BLOCK_RE.match(s):
            break
        out.append(ln.rstrip())

    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


class PreviewBroker:
    """UM loop de capture por SESSÃO (não por conexão). Faz poll do pane, extrai o texto do bloco em
    voo, guarda o último num slot e acorda os subscribers via Condition. Ref-count: liga no 1º
    subscriber, desliga no último. Evita N× subprocess numa tempestade de reconexão do iOS (cada
    conexão zumbi viveria minutos), que é o que mata no mobile."""

    _brokers: dict[str, "PreviewBroker"] = {}

    def __init__(self, name: str):
        self.name = name
        self.text = ""
        self.version = 0
        self._cond = asyncio.Condition()
        self._task: Optional[asyncio.Task] = None
        self._subs = 0

    @classmethod
    def get(cls, name: str) -> "PreviewBroker":
        b = cls._brokers.get(name)
        if b is None:
            b = cls(name)
            cls._brokers[name] = b
        return b

    async def _loop(self) -> None:
        # SEMPRE extrai o último bloco ● (NÃO gateia por spinner): a detecção de spinner pisca falso
        # por 1 frame durante o redraw, e gatear nisso fazia o broker emitir "" -> a bolha SUMIA e
        # voltava toda hora (flicker). O front limpa o preview por reconcile (coberto pelo .jsonl) /
        # idle. O spinner serve só pra CADÊNCIA: rápido trabalhando, devagar ocioso. Diff-gate (só
        # notifica em mudança) evita spam.
        while True:
            try:
                pane = await asyncio.to_thread(tmux.capture_pane, self.name)
            except Exception:
                pane = ""
            working = _live_spinner(pane) is not None
            text = extract_assistant_text(pane)
            if text != self.text:
                async with self._cond:
                    self.text = text
                    self.version += 1
                    self._cond.notify_all()
            await asyncio.sleep(0.15 if working else 0.75)

    async def subscribe(self) -> AsyncIterator[str]:
        """Emite o texto mais recente (full-replace) a cada mudança. Coalescido por natureza: um
        subscriber lento perde frames intermediários e pega só o último (version + slot único)."""
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
                    text = self.text
                yield text
        finally:
            async with self._cond:
                self._subs -= 1
                if self._subs <= 0 and self._task is not None:
                    self._task.cancel()
                    self._task = None
                    self._brokers.pop(self.name, None)
