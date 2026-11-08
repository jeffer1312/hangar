import asyncio
import json
import logging
import re
import time
from typing import AsyncIterator, Callable, Optional

from app import plugin_bridge
from app.hook_state import hook_state
from app.state import _RULE_RE, _is_boundary, _live_spinner, shared_capture
# _dirs: MESMO cache de diretorios de config que a statusline usa, e pelo mesmo motivo (roda por
# sessao, a cada poll). Reusado em vez de copiado — sao os mesmos diretorios e a mesma chave (o stem
# do .jsonl); duas copias so dariam a chance de uma envelhecer.
from app.statusline import dirs_de_config as _config_dirs
from app.plugin_screen import crop_cells

_log = logging.getLogger("hangar.preview")


def crop_to_transcript(name: str, pane: str) -> str:
    """Sem a coluna do painel ancorado: a borda │ dele virava texto da prévia."""
    largura = plugin_bridge.transcript_columns(name)
    if not largura:
        return pane
    return "\n".join(crop_cells(linha, largura) for linha in pane.split("\n"))

# Preview AO VIVO do bloco de assistente em andamento, lido do pane do tmux (capture-pane -p, texto
# já composto: sem ANSI, sem cursor-move). É a ÚNICA fonte do texto em voo sem perder o REPL
# interativo — o Claude Code só grava no .jsonl a mensagem COMPLETA. Best-effort, heurística acoplada
# ao TUI do Claude Code; o .jsonl continua a verdade canônica que SUBSTITUI o preview no fim.

_ASSISTANT_GLYPH = "●"
_USER_PROMPT_RE = re.compile(r"^\s*❯")
_BANNER_RE = re.compile(r"^[\s▐▛█▝▜▀]*Claude Code v\d")
# Aviso de plugin ("● ecc: hooks.json: unknown keys …"), com o ● da prosa. Os cortes por posição
# não bastam: o Claude Code o reimprime no MEIO da conversa, depois do último ❯ (pane real de
# 11/09, sessão em grupo do orquestrar), e ali ele é o último ● e vira prévia. Por conteúdo, então.
_PLUGIN_WARNING_RE = re.compile(r"^[a-z][\w.-]*: hooks\.json: [a-z]")


def _norm(s: str) -> str:
    # Normaliza pra casar o texto do PANE (JÁ RENDERIZADO: hard-wrap do terminal, SEM markdown) com o
    # do .jsonl (markdown CRU): tira marcadores de markdown (crase * _ ~ # >) e colapsa espaço. Sem
    # tirar os marcadores, uma msg com formatação ("**Confirma**" vs "Confirma") não casava e o preview
    # já-commitado vazava como bolha duplicada. Usado pra suprimir preview já no transcript.
    # Marcador de lista no INÍCIO da linha entra na mesma regra: a TUI pinta `- item` como
    # `• item`, e sem tirar os dois o último bloco commitado voltava como bolha fantasma toda vez
    # que o app abria a sessão. Só no início da linha pra hífen de prosa continuar contando.
    s = re.sub(r"(?m)^\s*[-•◦▪]\s+", "", s)
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

_PI_BOX_RE = re.compile(r"^\s*[╭╰][─\s]*[╮╯]\s*$")
_OVERLAY_RULE_RE = re.compile(r"^[\s▔]*▔{10,}[\s▔]*$")


_TODO_PANEL_RE = re.compile(r"^\s*[●○]?\s*Todos \(\d+/\d+\)\s*$")
_ASCII_SPINNER_RE = re.compile(r"^\*\s+\S[^\n]*(…|\))\s*$")


# "… (6 more lines, ctrl+o to expand)" — rodape do bloco DOBRADO, cromo do TUI, nunca prosa. Vem em
# `[2m` (dim), nao em italico, entao nao sai pela regra de cima.


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


# Chamada em voo do Claude Code: a 1ª linha é a descrição do modelo, não "Bash(", e o ● dela pisca. O que
# a separa da prosa é a forma: o parágrafo que começa depois de `i` termina colado no `⎿`.
def _cabecalho_de_ferramenta(lines: list[str], i: int) -> bool:
    for j in range(i + 1, len(lines)):
        ln = lines[j]
        if not ln.strip() or _is_boundary(ln):
            return ln.lstrip().startswith("⎿")
    return False


def extract_assistant_text(pane: str, provider: str = "claude", band_anchor: str | None = None) -> str:
    """Texto do ÚLTIMO bloco de PROSA do assistente (●) do pane, VERBATIM (sem reflow — núcleo seguro).

    Acha o último ● que NÃO é tool-call (início do bloco em voo; blocos anteriores já caíram no
    .jsonl), tira o "● " da 1ª linha e segue até o primeiro chrome: régua (────), próximo boundary
    (●/⎿/spinner), prompt ❯ ou o chrome extra do provider. Sem juntar continuação — o markdown
    bonito vem no snap final do .jsonl.

    provider desconhecido = "claude" = comportamento idêntico ao de sempre.
    """
    lines = pane.splitlines()
    fim = max((i for i, ln in enumerate(lines)
               if _RULE_RE.match(ln) or _OVERLAY_RULE_RE.match(ln) or _PI_BOX_RE.match(ln)),
              default=len(lines))
    # A faixa que os mods desenham fica entre a conversa e a caixa de digitar, e pode começar com ●
    # (o mod de progresso começa): sem este corte ela era eleita a prosa em andamento. A âncora é o
    # primeiro texto da árvore que o plugin do Hangar mandou; a ocorrência mais baixa é a da faixa.
    if band_anchor:
        fim = max((i for i, ln in enumerate(lines[:fim]) if band_anchor in ln), default=fim)
    # E a conversa COMEÇA na última mensagem do usuário: prosa em voo é sempre posterior a ela.
    # O que motivou: o Claude Code imprime o aviso de largada (plugin com chave fora do schema no
    # hooks.json, CLAUDE.md acima do limite) com o MESMO ● do bloco do assistente, e como a varredura
    # pega o ÚLTIMO ●, uma sessão recém-aberta que só rodou ferramenta mostrava o aviso como se fosse
    # o assistente falando. Corte por POSIÇÃO, igual ao do rodapé — qualquer chrome futuro de largada
    # cai fora junto. Sem ❯ no pane (rolou pra fora) -> varre tudo, como antes.
    # O ❯ da CAIXA DE DIGITAR não conta: ela é desenhada régua/❯/régua e cai dentro do `fim` (o corte
    # de rodapé pega a régua de BAIXO), então cortar por ela zeraria a prévia sempre. ❯ logo depois de
    # uma régua é a caixa; qualquer outro é a mensagem que o usuário mandou. Nenhum sobrando -> varre
    # tudo, como antes.
    inicio = max((i + 1 for i, ln in enumerate(lines[:fim])
                  if _USER_PROMPT_RE.match(ln) and not (i and _RULE_RE.match(lines[i - 1]))),
                 default=0)
    # Banner de largada à vista e nenhuma mensagem do usuário: a sessão nunca foi acionada, então
    # não há prosa em voo — só o aviso de largada, que voltaria como prévia.
    if inicio == 0 and any(_BANNER_RE.match(ln) for ln in lines[:fim]):
        return ""
    start = -1
    for i, ln in enumerate(lines[inicio:fim], inicio):  # sem régua, fim == len(lines)
        s = ln.lstrip()
        corpo = s[1:].lstrip()
        if s[:1] == _ASSISTANT_GLYPH and _PLUGIN_WARNING_RE.match(corpo):
            # O aviso ZERA a eleição, não é só pulado: pular faria a varredura cair num ● anterior,
            # prosa já commitada, que voltaria como se estivesse em voo.
            start = -1
            continue
        if (s[:1] == _ASSISTANT_GLYPH and not _TOOL_BLOCK_RE.match(corpo)
                and not _MCP_CALL_RE.match(corpo)
                and not _AGENT_FINISHED_RE.match(corpo)
                and not _TODO_PANEL_RE.match(ln)
                and not _painel_de_subagente(lines, i, corpo)
                and not _cabecalho_de_ferramenta(lines, i)
                ):
            start = i
    if start < 0:
        return ""

    first = lines[start].lstrip()
    first = first[1:].lstrip() if first[:1] == _ASSISTANT_GLYPH else first
    out = [first.rstrip()]
    for j, ln in enumerate(lines[start + 1:], start + 1):
        # Chrome = limite inferior do bloco. _is_boundary pega ●/⎿/spinner (lstrip já tira indent
        # das linhas de continuação da prosa, então elas NÃO disparam aqui). _TOOL_BLOCK_RE corta
        # tb a linha de status de tool ("Running/Ran N shell command") que renderiza 1 frame SEM o
        # ● antes de virar bloco — senão grudava no fim da prosa e piscava.
        s = ln.lstrip()
        if (_RULE_RE.match(ln) or _is_boundary(ln) or _USER_PROMPT_RE.match(ln)
                or _TOOL_BLOCK_RE.match(s) or _MCP_CALL_RE.match(s)
                or _TODO_PANEL_RE.match(ln) or _ASCII_SPINNER_RE.match(s)
                or _ACTIVITY_SUMMARY_RE.search(s)
                # Só no começo de parágrafo: as linhas do mesmo parágrafo dão a mesma resposta.
                or (s and not lines[j - 1].strip() and _cabecalho_de_ferramenta(lines, j - 1))):
            break
        out.append(ln.rstrip())

    while out and not out[-1].strip():
        out.pop()
    return "\n".join(out)


_PREVIEW_SUBDIR = ".hangar-preview"
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
            estado = hook_state.get_state(stem)
            if (text and estado is not None and estado[0] == "working"
                    and time.time() - estado[1] <= _PREVIEW_MAX_AGE):
                # Turno longo só de ferramentas não publica texto novo e não é publicador morto:
                # cair no pane aqui mostrava como mensagem a linha que a TUI desenha ao lado do spinner.
                # O marcador também tem que ser recente: cada ferramenta o renova, e um agente que
                # morreu no meio do turno o deixa preso em "working" para sempre.
                _log.debug("preview: sidecar velho mantido, sessao trabalhando path=%s idade=%.0fs",
                           f, time.time() - ts)
                return text
            if text:
                # Este e o descarte que importa operacionalmente: "a extensao morreu no meio do
                # turno". Sem o log, quem for entender por que a previa ficou parada ate cair no
                # pane nao tem o que procurar — os outros dois descartes desta funcao ja logam.
                _log.debug("preview: sidecar velho descartado path=%s idade=%.0fs",
                           f, time.time() - ts)
                continue
            # "" NAO envelhece: e a resposta "nada em voo" do fim do turno, e descarta-la mandava o
            # broker raspar o pane de uma sessao PARADA — o ultimo bloco commitado voltava como
            # bolha fantasma ao reabrir o app. Preco aceito: extensao que morreu logo APOS o Stop
            # deixa a sessao sem previa (nem pane) ate voltar a publicar; dai o log.
            _log.debug("preview: sidecar vazio e velho, honrado mesmo assim path=%s idade=%.0fs",
                       f, time.time() - ts)
        return text
    return None


                    # a 150ms de cadencia, dois quadros de uma janela de ~45 linhas se sobrepoem
                    # quase inteiros; menos que uma frase de overlap = o bloco mudou de identidade.


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
        self.full = False
        self._gen = 0
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
        """Zera texto e acumulado e AVANCA A EPOCA: o /clear trocou de transcript, e o que estava
        em voo era da conversa APAGADA. Sem isto o broker (que sobrevive ao clear — e por nome de
        sessao, nao de transcript) republicava o texto velho no primeiro yield de uma reconexao,
        com a supressao desarmada (committed zerado no mesmo evento): bolha fantasma (achado da
        review). A epoca cobre a janela que zerar os campos nao cobre: um frame capturado ANTES do
        reset e publicado DEPOIS (o capture tava em voo no to_thread) — o _loop confere a epoca no
        publish e descarta. Nao notifica: quem chama (o __reset__ do SSE) ja limpa o front."""
        self.text = ""
        self.md = False
        self.full = False
        self._gen += 1
        from app.state import forget_frame
        forget_frame(self.name)

    async def _loop(self) -> None:
        from app import terminal_observer
        async with terminal_observer.lease(self.name, self.provider if self.provider == "claude" else None,
                                           lambda: self.stem_get() if self.stem_get is not None else None) as source:
            heartbeat = asyncio.create_task(source.watch()) if self.provider == "claude" else None
            try:
                await self._observe_loop()
            finally:
                if heartbeat is not None:
                    heartbeat.cancel()
                    try:
                        await heartbeat
                    except asyncio.CancelledError:
                        if asyncio.current_task().cancelling():
                            raise

    async def _observe_loop(self) -> None:
        # SEMPRE extrai o último bloco ● (NÃO gateia por spinner): a detecção de spinner pisca falso
        # por 1 frame durante o redraw, e gatear nisso fazia o broker emitir "" -> a bolha SUMIA e
        # voltava toda hora (flicker). O front limpa o preview por reconcile (coberto pelo .jsonl) /
        # idle. O spinner serve só pra CADÊNCIA: rápido trabalhando, devagar ocioso. Diff-gate (só
        # notifica em mudança) evita spam.
        working = False
        from app import terminal_observer
        while True:
            if terminal_observer.retired(self.name):
                # As conexões SSE podem reconhecer o /clear em rodadas diferentes.
                await asyncio.sleep(0.75)
                continue
            # Epoca do poll: se o reset() (/clear) cair no MEIO desta iteracao, o frame capturado
            # e da conversa apagada — o publish la embaixo confere e descarta.
            gen = self._gen
            frame_tag = terminal_observer.stamp(self.name)
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
                try:
                    # A chamada dos OUTROS providers fica byte-identica de proposito (nem o
                    # argumento `lines` explicito): este e o poll mais quente do backend e ha dublê
                    # de teste com assinatura de um argumento so.
                    # Texto puro sai do quadro compartilhado com o monitor de estado; trabalhando, a
                    # idade máxima fica abaixo da cadência pra prévia não andar mais devagar.
                    pane = await shared_capture(self.name, 0.1 if working else 0.5)
                except terminal_observer.ObservationFailed:
                    # Falha do Rust não troca de fonte nem apaga a bolha: o monitor de estado mostra
                    # o erro, e a prévia espera a próxima rodada com o texto que tinha.
                    await asyncio.sleep(0.75)
                    continue
                except Exception:
                    _log.debug("preview: capture_pane falhou sessao=%s", self.name, exc_info=True)
                    pane = ""
                if self.provider == "claude":
                    pane = crop_to_transcript(self.name, pane)
                analysis = terminal_observer.frame_analysis(self.name, pane) if self.provider == "claude" else None
                working = (analysis["spinner"] if analysis is not None else _live_spinner(pane)) is not None
                # A análise do Rust não conhece a faixa dos mods: com ela na tela, quem corta é o Python.
                anchor = plugin_bridge.band_anchor(self.name) if self.provider == "claude" else None
                text = (analysis["preview"] if analysis is not None and not anchor
                        else extract_assistant_text(pane, self.provider, anchor) if self.provider == "claude"
                        else extract_assistant_text(pane, self.provider))
                full = False
            if self._gen != gen or frame_tag != terminal_observer.stamp(self.name):
                # reset() caiu no MEIO deste poll: o frame capturado e da conversa APAGADA —
                # nem publica, nem deixa o acumulado renascer com ela (achado da review).
                # Cobre SO o frame em voo: um pane velho ainda nao redesenhado na iteracao
                # SEGUINTE passaria — na pratica o jsonl_watcher so enfileira o __reset__ apos
                # >=4s de transcript novo, e o pane ja limpou (documentado na review).
                _log.debug("preview: frame descartado pela epoca sessao=%s", self.name)
            else:
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
            if self._task is None or self._task.done():
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
