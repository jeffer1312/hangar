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
    "Crawling|Downloading|Globbing|Grepping|Waiting|Loading|Compiling|Building|Installing|Ran|"
    "Making"
)
_TOOL_BLOCK_RE = re.compile(rf"^([A-Z][\w-]*\(|({_TOOL_VERBS})\b)")

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
_STOPS_BY_PROVIDER = {"pi": (_PI_BOX_RE,)}

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
_PI_TOOL_NAME_RE = re.compile(
    r"^(Bash|Read|Write|Edit|MultiEdit|Grep|Glob|Task|Agent|Skill|WebFetch|WebSearch|"
    r"NotebookEdit|TodoWrite|Multiple Tools|Chrome Devtools)[\s:(]"
)


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
    start = -1
    for i, ln in enumerate(lines):
        s = ln.lstrip()
        corpo = s[1:].lstrip()
        if (s[:1] == _ASSISTANT_GLYPH and not _TOOL_BLOCK_RE.match(corpo)
                and not _MCP_CALL_RE.match(corpo)
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
                or _TOOL_BLOCK_RE.match(s) or _MCP_CALL_RE.match(s)):
            break
        if any(r.match(ln) for r in stops):
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

    def __init__(self, name: str, provider: str = "claude"):
        self.name = name
        self.provider = provider
        self.text = ""
        self.version = 0
        self._cond = asyncio.Condition()
        self._task: Optional[asyncio.Task] = None
        self._subs = 0

    @classmethod
    def get(cls, name: str, provider: str = "claude") -> "PreviewBroker":
        # provider: um broker por SESSAO, e toda conexao da mesma sessao traz o mesmo provider —
        # so o primeiro a chegar o fixa. Nao reatribuimos num broker vivo pra nao trocar a leitura
        # debaixo de um subscriber; o broker morre com o ultimo subscriber e o proximo renasce
        # com o provider atual.
        b = cls._brokers.get(name)
        if b is None:
            b = cls(name, provider)
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
            text = extract_assistant_text(pane, self.provider)
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
            # Limpeza SINCRONA (sem await): `async with self._cond` podia ser interrompido por
            # CancelledError no acquire do lock -> _subs nao decrementava e o _loop vazava (polling
            # tmux pra sempre sem subscriber). Sem await entre as linhas = atomico no event loop
            # single-thread, sem corrida com o subscribe() (que incrementa).
            self._subs -= 1
            if self._subs <= 0 and self._task is not None:
                task, self._task = self._task, None
                self._brokers.pop(self.name, None)
                task.cancel()
