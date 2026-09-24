"""CodexAdapter: junta o rollout (historico, via parse_rollout_line) com o app-server (JSON-RPC
ao vivo, via AppServerClient) por tras do `Adapter` Protocol. O coracao e map_state(), que
traduz uma notification do app-server (camelCase) num resultado NEUTRO e testavel; state_monitor
consome isso e emite o StateEvent real do app.

Nomes/shapes confirmados contra codex-cli 0.141.0 em docs/codex-app-server-contract.md.

Task 5 completou o Adapter Protocol: alem de map_state/transcript_stream/state_monitor/send_prompt/
deliverable, agora tem drain/spawn_command/transcript_path + o lifecycle vivo (attach/ensure_running/
close_sync). ensure_running e o resume LAZY: pos-restart do backend o processo app-server morre mas
o sidecar duravel (app.adapters.codex.sessions) guarda thread_id/rollout_path/cwd -> ensure_running
reabre o AppServerClient e retoma pelo thread/resume sob demanda."""
import asyncio
import json
import logging
import os
import shlex
import signal
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import AsyncIterator, Callable, Optional

from app.adapters.codex import sem_terminal
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex.appserver import AppServerClient
from app.adapters.codex.async_questions import AsyncQuestions
from app.adapters.codex.lancador import (APPROVAL, CLIENT_INFO, SANDBOX,
                                          comando_do_lancador)
from app.hook_state import hook_state
from app.models import session_key
from app.procinfo import pid_vivo
from app.adapters.preview_push import PushPreviewSource
from app.adapters.codex.rollout import parse_rollout_line
from app import codex_contas
from app import tmux
from app.pqueue import PromptQueue
from app.send_executor import send_thread
from app.state import StateEvent
from app.transcript import ChatEvent, TranscriptTailer

_log = logging.getLogger("hangar.codex.adapter")



def matar_app_server(name: str) -> None:
    """Mata o app-server DAQUELA sessao pelo pid do sidecar. Best-effort, idempotente.

    Desde o lancador unico o servidor nao e mais filho do backend: `client.terminate()` nao tem
    processo pra matar (ver AppServerClient.tem_processo_proprio) e virava um no-op silencioso.
    Normalmente quem o derruba e o proprio lancador ao ver a TUI sair; isto cobre o caso em que ele
    nao chegou la (SIGKILL no pane), que e como app-server orfao ja ficou escutando em loopback.
    """
    meta = codex_sessions.load(name) or {}
    pid = meta.get("app_pid")
    if not isinstance(pid, int) or not pid_vivo(pid):
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError as exc:
        # warning, nao debug: o pid esta VIVO (checado acima) e nao morreu. E exatamente o
        # app-server orfao escutando em loopback que esta funcao existe pra evitar, e em `debug`
        # ninguem ve.
        _log.warning("codex: nao deu pra matar o app-server pid=%s name=%s: %s", pid, name, exc)


def _effort_da_thread(result: dict) -> str | None:
    """O esforço padrão da thread na resposta de `thread/start`/`thread/resume`.

    O campo chama `reasoningEffort`, e não `effort` como o do `turn/start` — medido no codex-cli
    0.153.4. Lendo `effort` a pílula do app nascia vazia com o terminal mostrando `max` ao lado do
    modelo, que vem da MESMA resposta e por isso aparecia.
    """
    return result.get("reasoningEffort") or result.get("effort")


def ensure_tmux_tui(name: str, cwd: str, thread_id: str | None, endpoint: str,
                    *, replace: bool = False, initial_prompt: str | None = None,
                    model: str | None = None, effort: str | None = None,
                    codex_home: str | None = None,
                    codex_account: str | None = None) -> None:
    """Garante uma TUI Codex anexavel no tmux, ligada ao app-server do backend.

    ``replace`` e usado no resume lazy apos restart do backend: uma pane antiga aponta para o
    endpoint do processo anterior e nao pode ser reaproveitada.
    """
    if tmux.has_session(name):
        if not replace:
            return
        tmux.kill_session(name)
    if thread_id:
        argv = ["codex", "resume", "--remote", endpoint, "--no-alt-screen"]
        if model:
            argv += ["--model", model]
        if effort:
            argv += ["--config", f'model_reasoning_effort="{effort}"']
        argv.append(thread_id)
    else:
        # Na criacao, a TUI precisa ser a dona do thread/start: um thread aberto pelo cliente JSON-
        # RPC ainda sem turno nao tem rollout no disco e `codex resume` o rejeita. O backend captura
        # o thread/started emitido por esta TUI e passa a controlar a mesma thread.
        argv = [
            "codex", "--remote", endpoint, "--no-alt-screen", "-C", cwd,
            "--sandbox", SANDBOX, "--ask-for-approval", APPROVAL,
        ]
        if initial_prompt:
            argv.append(initial_prompt)
    command = shlex.join(argv)
    if codex_home:
        path = str(Path(codex_home).expanduser().absolute())
        if os.name == "posix":
            prefix = ["env"]
            secondary = codex_account not in (None, "default")
            if codex_account is None and codex_home:
                secondary = True
            if secondary:
                for key in os.environ:
                    upper = key.upper()
                    if (codex_contas._AUTH_ENV.fullmatch(upper)
                            or upper in codex_contas._PROVIDER_ENV
                            or (upper in codex_contas._RUNTIME_ENV
                                and upper not in {"HOME", "USERPROFILE"})):
                        prefix += ["-u", key]
            command = shlex.join(prefix + [f"CODEX_HOME={path}", *argv])
        else:
            parts = [f'set "CODEX_HOME={path}"']
            secondary = codex_account not in (None, "default")
            if codex_account is None and codex_home:
                secondary = True
            if secondary:
                parts += [f'set "{key}="' for key in os.environ
                          if (codex_contas._AUTH_ENV.fullmatch(key.upper())
                              or key.upper() in codex_contas._PROVIDER_ENV
                              or (key.upper() in codex_contas._RUNTIME_ENV
                                  and key.upper() not in {"HOME", "USERPROFILE"}))]
            command = shlex.join(["cmd.exe", "/d", "/s", "/c",
                                  " && ".join(parts + [command])])
    if not tmux.new_session(name, cwd, command, provider="codex"):
        raise RuntimeError(f"nao foi possivel criar a TUI Codex no tmux: {name}")


@dataclass
class MappedState:
    """Resultado CRU e testavel de map_state — NAO e o StateEvent do app (StateEvent exige
    `state` e nao tem campo de preview). state_monitor() traduz isto pro StateEvent real;
    preview_delta e consumido em paralelo por CodexAdapter._state_stream (acumula por turno e
    empurra pro PushPreviewSource — ver Task 5b), fora do StateEvent. token_usage/rate_limits
    sao os snapshots CRUS (shape do app-server) das notifications de mesmo nome — quem acumula
    por sessao e monta o status_line completo e o _state_stream (Task D), nao este mapper."""
    state: Optional[str] = None          # "working" | "idle" | None (neutro/sem info)
    status_line: Optional[str] = None
    preview_delta: Optional[str] = None
    token_usage: Optional[dict] = None
    rate_limits: Optional[dict] = None
    buffering: Optional[bool] = None


def map_state(notif: dict) -> MappedState:
    """Mapeia UMA notification do app-server (`{"method": ..., "params": ...}`) -> MappedState.
    Method desconhecido (ou shape incompleto) -> MappedState() neutro, nunca levanta."""
    method = notif.get("method")
    params = notif.get("params") or {}

    if method == "model/safetyBuffering/updated" and isinstance(params.get("showBufferingUi"), bool):
        return MappedState(buffering=params["showBufferingUi"])

    if method == "turn/started":
        return MappedState(state="working")
    if method == "turn/completed":
        return MappedState(state="idle")

    if method == "thread/status/changed":
        status_type = (params.get("status") or {}).get("type")
        if status_type == "active":
            return MappedState(state="working")
        if status_type == "idle":
            return MappedState(state="idle")
        return MappedState()

    if method == "thread/tokenUsage/updated":
        # So captura o snapshot CRU (total/last/modelContextWindow) -- quem formata pro
        # status_line completo (Task D) e _format_status_line, chamado pelo _state_stream com o
        # que estiver acumulado por sessao (model/effort/rate_limits inclusive).
        usage = params.get("tokenUsage") or {}
        if not usage:
            return MappedState()
        return MappedState(token_usage=usage)

    if method == "account/rateLimits/updated":
        # Task D: antes ignorado. Guarda o snapshot cru (primary/secondary) pro _state_stream
        # acumular por sessao -- mesmo shape de account/rateLimits/read (ver read_rate_limits).
        rate_limits = params.get("rateLimits") or {}
        if not rate_limits or rate_limits.get("limitId") not in (None, "codex"):
            return MappedState()
        return MappedState(rate_limits=rate_limits)

    if method == "item/agentMessage/delta":
        return MappedState(preview_delta=params.get("delta"))

    return MappedState()


def _turn_problem(notif: dict) -> Optional[tuple[str, str]]:
    """(código, detalhe) quando a notification diz que o turno não anda: `error` com nova
    tentativa, ou turno fechado como `failed`. Sem isto a sessão fica "trabalhando" calada."""
    method = notif.get("method")
    params = notif.get("params") or {}
    run = params.get("run") or {}
    if method == "hook/completed" and run.get("eventName") == "userPromptSubmit" \
            and run.get("status") in ("blocked", "stopped"):
        # O turno fecha como `completed`, sem erro: o motivo do hook só existe nesta notificação.
        partes = Path(run.get("sourcePath") or "").parts
        origem = partes[partes.index("cache") + 2] if "cache" in partes[:-2] else (run.get("sourcePath") or "hook")
        motivo = next((e.get("text") for e in run.get("entries") or []
                       if e.get("kind") in ("stop", "feedback", "error") and e.get("text")), "")
        return "codex_prompt_bloqueado", f"{origem}: {motivo}".strip(": ")[:300]
    if method == "error":
        erro = params.get("error") or {}
        codigo = "codex_sem_conexao" if params.get("willRetry") else "headless_turno_erro"
    elif method == "turn/completed" and (params.get("turn") or {}).get("status") == "failed":
        erro = (params.get("turn") or {}).get("error") or {}
        codigo = "headless_turno_erro"
    else:
        return None
    # O detalhe pode ser a página HTML de erro do provedor: só a primeira linha serve na faixa.
    detalhe = str(erro.get("additionalDetails") or erro.get("message") or "").strip()
    return codigo, detalhe.split("\n")[0][:300]


def _fmt_tok(n: float) -> str:
    """Formata contagem de tokens com sufixo k/M (ex: 14389 -> "14k"), como o parseStatusLine do
    front ja entende (frontend/src/lib/statusline.ts toNumber). Sem sufixo abaixo de 1000."""
    n = int(round(n))
    if abs(n) >= 1_000_000:
        return f"{round(n / 1_000_000)}M"
    if abs(n) >= 1_000:
        return f"{round(n / 1_000)}k"
    return str(n)


def _format_context_segment(token_usage: dict) -> Optional[str]:
    """Monta a secao `💬 <in>/<out> <ctxUsed>/<ctxTotal>` (Task D).

    O contexto USADO na janela e o input do ULTIMO turno (`last.inputTokens` = o historico inteiro
    reenviado ao modelo naquele turno), NAO o `total_token_usage.total` -- este e ACUMULADO: soma o
    input de TODOS os turnos, entao infla a cada mensagem mesmo trivial. Bug real observado: 5 turnos
    de "Ola" davam total=97k (38% de 258k) enquanto o contexto real era ~19k (8%), estavel. Exige
    last.input + window; sem isso omite a secao (best-effort)."""
    last = token_usage.get("last") or {}
    used = last.get("inputTokens")
    window = token_usage.get("modelContextWindow")
    if not used or not window:
        return None
    turn_out = last.get("outputTokens") or 0
    return f"💬 {_fmt_tok(used)}/{_fmt_tok(turn_out)} {_fmt_tok(used)}/{_fmt_tok(window)}"


def _format_reset(resets_at: float, now: float) -> str:
    """Tempo relativo curto ate resets_at (epoch-s), ex: "2d1h", "34m". Sem '│'/'⚡'/'📅'/'🕐'
    (o parseStatusLine do front corta o campo de reset nesses caracteres)."""
    delta = max(0, int(resets_at - now))
    days, rem = divmod(delta, 86400)
    hours, rem = divmod(rem, 3600)
    minutes = rem // 60
    if days:
        return f"{days}d{hours}h" if hours else f"{days}d"
    if hours:
        return f"{hours}h{minutes}m" if minutes else f"{hours}h"
    return f"{minutes}m"


# janela ~5h (windowDurationMins~300) -> chip ⚡5h; janela semanal (~10080) -> chip 📅7d. Tolerancia
# pra nao exigir o minuto exato (o app-server pode devolver 299/301 etc.).
_WINDOW_5H = (270, 330)
_WINDOW_7D = (10020, 10140)


def _format_rate_window(window: Optional[dict], now: float) -> Optional[str]:
    if not window:
        return None
    mins = window.get("windowDurationMins")
    pct = window.get("usedPercent")
    if mins is None or pct is None:
        return None
    if _WINDOW_5H[0] <= mins <= _WINDOW_5H[1]:
        emoji, label = "⚡", "5h"
    elif _WINDOW_7D[0] <= mins <= _WINDOW_7D[1]:
        emoji, label = "📅", "7d"
    else:
        return None
    seg = f"{emoji}{label}:{round(pct)}%"
    resets_at = window.get("resetsAt")
    if resets_at:
        seg += f" ↺{_format_reset(resets_at, now)}"
    return seg


def _format_rate_limit_segments(rate_limits: dict, now: float) -> list[str]:
    segs = []
    for key in ("primary", "secondary"):
        seg = _format_rate_window(rate_limits.get(key), now)
        if seg:
            segs.append(seg)
    return segs


def format_status_line(
    model: Optional[str],
    effort: Optional[str],
    token_usage: Optional[dict],
    rate_limits: Optional[dict],
    now: Optional[float] = None,
) -> Optional[str]:
    """Formatador PURO do status_line completo (Task D), testavel isolado do stream. Monta
    `🤖 <model> (<effort>) │ 💬 <in>/<out> <used>/<total> │ ⚡5h:<pct>% ↺<reset> │ 📅7d:...`
    casando os regexes do parseStatusLine do front (ver docs no topo de statusline.ts) -- cada
    secao e best-effort, o que faltar e omitido. None se nao sobrar nenhuma secao."""
    now = time.time() if now is None else now
    parts: list[str] = []
    if model:
        seg = f"🤖 {model}"
        if effort:
            seg += f" ({effort})"
        parts.append(seg)
    if token_usage:
        ctx = _format_context_segment(token_usage)
        if ctx:
            parts.append(ctx)
    if rate_limits:
        parts.extend(_format_rate_limit_segments(rate_limits, now))
    if not parts:
        return None
    return " │ ".join(parts)


# Quanto do FIM do rollout basta pra achar o ultimo `token_count` e o ultimo `turn_context`. Um
# turno grande (varias ferramentas) cabe folgado; nao achando, a linha sai incompleta em vez de
# custar a leitura do arquivo inteiro a cada poll da lista.
_STATUS_TAIL = 512 << 10


def status_line_do_rollout(path: str, now: Optional[float] = None) -> Optional[str]:
    """A statusline do CARD, montada do proprio rollout. None quando nao ha nada util.

    O card e a lista, sem SSE aberto — e a TUI do Codex nao pode ser raspada (sem regua nem caixa
    de composer, a captura devolveria as duas ultimas linhas verbatim, uma segunda statusline).
    Entao a linha sai do mesmo arquivo que o chat ja le. Os campos do rollout sao snake_case; o
    `format_status_line` fala o camelCase do app-server, e a traducao mora aqui."""
    tc = ctx = limites = None
    try:
        with open(path, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            fh.seek(max(0, fh.tell() - _STATUS_TAIL))
            linhas = fh.read().split(b"\n")
    except OSError:
        return None
    for ln in reversed(linhas):
        if not ln.strip():
            continue
        try:
            obj = json.loads(ln)
        except (ValueError, AttributeError):
            continue
        if not isinstance(obj, dict):
            continue
        payload = obj.get("payload")
        if not isinstance(payload, dict):
            continue
        rates = payload.get("rate_limits")
        if limites is None and payload.get("type") == "token_count" and isinstance(rates, dict) \
                and rates.get("limit_id") in (None, "codex"):
            limites = rates
        if tc is None and payload.get("type") == "token_count":
            tc = payload
        elif ctx is None and obj.get("type") == "turn_context":
            ctx = payload
        if tc is not None and ctx is not None and limites is not None:
            break

    info = (tc or {}).get("info") or {}
    total = info.get("last_token_usage") or {}
    usage = {"last": {"inputTokens": total.get("input_tokens"),
                      "outputTokens": total.get("output_tokens")},
             "modelContextWindow": info.get("model_context_window")} if total else None
    limites = limites or {}
    janelas = {k: {"windowDurationMins": (limites.get(k) or {}).get("window_minutes"),
                   "usedPercent": (limites.get(k) or {}).get("used_percent"),
                   "resetsAt": (limites.get(k) or {}).get("resets_at")}
               for k in ("primary", "secondary") if limites.get(k)}
    modo = ((ctx or {}).get("collaboration_mode") or {}).get("settings") or {}
    return format_status_line((ctx or {}).get("model"), modo.get("reasoning_effort"),
                              usage, janelas or None, now)


class CodexAdapter:
    provider = "codex"
    # Backoff inicial/teto do retry de assinatura (_subscribe_when_ready). Atributo de classe pra
    # o teste encurtar sem precisar remendar asyncio.sleep global (que viraria busy-loop).
    SUBSCRIBE_RETRY_BASE = 1.0
    SUBSCRIBE_RETRY_MAX = 10.0
    # Tentativas ate gritar no log. ~10 cobre folgado o tempo do 1o turno gravar o rollout (caso
    # esperado); passar disso e sinal de erro permanente, nao de sessao ociosa.
    SUBSCRIBE_WARN_AFTER = 10
    # Teto pra considerar morto um turno de sessao NAO assinada (sem turn/completed pra
    # limpar). Generoso: so existe pra impedir bloqueio permanente, nao pra paralelizar.
    UNSUBSCRIBED_TURN_TTL = 300.0

    def __init__(self) -> None:
        # nome da sessao tmux -> {"client": AppServerClient, "thread_id": str, "state": str,
        # "in_progress": bool}. Vazio ate attach() ser chamado (Task 5, ao spawnar o app-server
        # e dar thread/start) — sem entrada, a sessao e tratada como "dead"/"deliverable".
        self._sessions: dict[str, dict] = {}
        # Lock por-nome pra ensure_running: sem isto, 2 chamadores concorrentes pro mesmo nome sem
        # client vivo (ex: SSE reconnect + /input logo apos restart) podiam ambos passar pelo `sess
        # is None`, ambos spawnar+resume, e o 2o attach() sobrescrever o 1o AppServerClient no dict
        # -- o 1o (subprocess + reader task) ficava orfao, nunca fechado.
        self._locks: dict[str, asyncio.Lock] = {}
        self._delivery_locks: dict[str, asyncio.Lock] = {}
        # Backend-owned watcher: se a TUI tmux morre (inclusive terminal fechado com o helper junto),
        # remove o sidecar e encerra o app-server. O cleanup nao pode depender do processo wrapper.
        self._tmux_watchers: dict[str, asyncio.Task] = {}
        # Task de assinatura da thread (thread/resume com retry) — ver _subscribe_when_ready.
        self._subscribers: dict[str, asyncio.Task] = {}
        self._falhas_subida: dict[str, int] = {}
        # Sessão sem terminal que não sobe: (código, detalhe) que a lista e o StateEvent mostram —
        # senão o card só vira "dead" sem pista.
        self._problemas: dict[str, tuple[str, str | None]] = {}
        self._loop: asyncio.AbstractEventLoop | None = None

    def _start_tmux_watcher(self, name: str) -> None:
        old = self._tmux_watchers.pop(name, None)
        if old is not None:
            old.cancel()
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            # attach() tambem e usado por testes/callers sync sem event loop.
            return
        self._tmux_watchers[name] = loop.create_task(
            self._watch_tmux(name), name=f"codex-tmux-watch-{name}"
        )

    async def _watch_tmux(self, name: str) -> None:
        try:
            # Grace de startup: attach() ocorre logo apos tmux.new_session, mas evita qualquer
            # falso negativo transitorio no primeiro poll.
            await asyncio.sleep(1.0)
            # None (tmux nao respondeu) NAO e sessao morta: um has-session que estourou o teto
            # com a maquina carregada ja matou o app-server de toda sessao Codex viva.
            while name in self._sessions and \
                    await asyncio.to_thread(tmux.sessao_existe, name) is not False:
                await asyncio.sleep(1.0)
            sess = self._sessions.pop(name, None)
            if sess is None:
                return
            sub = self._subscribers.pop(name, None)
            if sub is not None:
                sub.cancel()
            bomba = sess.get("bomba")
            if bomba is not None:
                bomba.cancel()
            # Antes do delete: o pid do app-server mora no sidecar. Normalmente o lancador ja o
            # derrubou ao ver a TUI sair; isto cobre o pane morto de SIGKILL.
            matar_app_server(name)
            term = getattr(sess["client"], "terminate", None)
            if callable(term):
                term()
            codex_sessions.delete(name)
            from app import registry as registry_mod
            if registry_mod.apos_saida_codex:
                registry_mod.apos_saida_codex(name)
            await asyncio.to_thread(PromptQueue(name).clear)
            PushPreviewSource._sources.pop(name, None)
            _log.info("codex tmux encerrou: cleanup automatico name=%s", name)
        except asyncio.CancelledError:
            raise
        except Exception:
            _log.exception("codex tmux watcher falhou name=%s", name)
        finally:
            current = self._tmux_watchers.get(name)
            if current is asyncio.current_task():
                self._tmux_watchers.pop(name, None)

    def start_subscription(self, name: str, cwd: str) -> None:
        """Assina a thread criada pela TUI, em background (ver _subscribe_when_ready)."""
        old = self._subscribers.pop(name, None)
        if old is not None:
            old.cancel()
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return  # callers sync/testes sem event loop
        self._subscribers[name] = loop.create_task(
            self._subscribe_when_ready(name, cwd), name=f"codex-subscribe-{name}"
        )

    async def _subscribe_when_ready(self, name: str, cwd: str) -> None:
        """Chama `thread/resume` ate colar — e o que torna o backend ASSINANTE da thread.

        POR QUE RETENTAR (medido contra codex-cli 0.144.6): as notifications do app-server sao
        POR ASSINATURA, nao broadcast. Um cliente que nao deu thread/start nem thread/resume so
        recebe eventos globais (thread/status/changed, app/list/updated) -- nada de turn/started,
        turn/completed, item/agentMessage/delta ou thread/tokenUsage/updated. Era essa surdez que
        travava tudo: estado congelado, sem preview, sem statusline, e o drain-on-complete (que
        mora no turn/completed) nunca disparando -> msg do celular presa pra sempre na fila.

        E por que nao da pra assinar na hora: enquanto a thread nao tem NENHUM turno, o rollout
        ainda nao existe em disco e o app-server responde `no rollout found for thread id`. Logo
        a assinatura so e possivel depois do 1o turno -- daí o retry. O 1o turno nao perde conteudo
        no app: o chat vem do tail do rollout, que e completo independente de assinatura.
        """
        delay = self.SUBSCRIBE_RETRY_BASE
        attempts = 0
        while name in self._sessions:
            sess = self._sessions.get(name)
            if sess is None:
                return
            if sess.get("subscribed"):
                return
            revision = sess.get("state_revision", 0)
            try:
                # Assinar eventos não pode sobrescrever as permissões escolhidas na sessão.
                result = await sess["client"].request("thread/resume", {
                    "threadId": sess["thread_id"],
                    "cwd": cwd,
                })
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                # "no rollout found" ate o 1o turno e ESPERADO; qualquer outro erro (thread invalida,
                # app-server recusando) e permanente e ficava indistinguivel — retry mudo pra sempre.
                # Agora: debug a cada tentativa, e UM warning quando passa do orcamento, pra a sessao
                # surda aparecer no log em vez de so "o chat parou".
                attempts += 1
                sess["subscribe_error"] = str(exc)
                _log.debug("codex assinatura falhou (tentativa %d) name=%s: %s", attempts, name, exc)
                if attempts == self.SUBSCRIBE_WARN_AFTER:
                    _log.warning(
                        "codex NAO assinou apos %d tentativas name=%s: %s — sessao segue sem "
                        "turn/*, preview nem statusline; envio continua funcionando",
                        attempts, name, exc,
                    )
                await asyncio.sleep(delay)
                delay = min(delay * 1.5, self.SUBSCRIBE_RETRY_MAX)
                continue
            sess["subscribed"] = True
            sess["async_questions"].hydrate(result.get("thread") or {})
            sess.pop("subscribe_error", None)
            # O resume tambem devolve o default da THREAD -> alimenta o display (pill/statusline).
            # `or` e nao setdefault: o attach() ja criou as chaves com None, entao setdefault nunca
            # sobrescreveria e o 🤖 sumia da statusline (visto na verificacao ao vivo).
            sess["default_model"] = sess.get("default_model") or result.get("model")
            sess["default_effort"] = _effort_da_thread(result)
            if revision == sess.get("state_revision", 0):
                self._restore_turn(sess, result.get("thread") or {})
            for fila in sess.get("ouvintes", []):
                fila.put_nowait(self._question_state(name, sess))
            _log.info("codex assinado: thread=%s name=%s", sess["thread_id"], name)
            return

    def _start_bomba(self, name: str, sess: dict) -> asyncio.Task | None:
        bomba = sess.get("bomba")
        if bomba is not None and not bomba.done():
            return bomba
        sess.pop("bomba_error", None)
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return None
        bomba = loop.create_task(
            self._bombear(name, sess["client"], sess), name=f"codex-pump-{name}"
        )
        sess["bomba"] = bomba
        return bomba

    def attach(self, name: str, client: AppServerClient, thread_id: str,
               model: Optional[str] = None, effort: Optional[str] = None,
               default_model: Optional[str] = None, default_effort: Optional[str] = None,
               *, watch_tmux: bool = False, subscribed: bool = False) -> None:
        """Liga uma sessao (por nome) a um AppServerClient + threadId ja vivos. Chamado pelo
        registry.create_codex (spawn novo, sem model/effort ainda -- sessao nova) e por
        ensure_running (resume pos-restart, passando model/effort lidos do sidecar -- Task C:
        a escolha sobrevive ao restart).

        model/effort = ESCOLHA explicita do usuario (o que vai no turn/start). default_model/
        default_effort = o default da THREAD (devolvido por thread/start/thread/resume) -- so pra
        DISPLAY (pill/statusline) quando nao ha escolha; nunca mandado pro app-server. Efemero
        (nao vai pro sidecar): re-populado a cada attach (create ou resume), suficiente porque
        list_models()/state_monitor sempre chamam ensure_running antes de ler o display.

        subscribed=True quando quem chama JA fez o thread/resume (caminho ensure_running); o
        create_codex passa False e dispara start_subscription, que retenta ate o rollout existir."""
        try:
            self._loop = asyncio.get_running_loop()
        except RuntimeError:
            pass
        anterior = self._sessions.get(name)
        if anterior is not None and anterior.get("bomba") is not None:
            anterior["bomba"].cancel()
        sess = {"client": client, "thread_id": thread_id,
                "state": "idle", "in_progress": False,
                "model": model, "effort": effort,
                "default_model": default_model, "default_effort": default_effort,
                "subscribed": subscribed, "ouvintes": [], "async_questions": AsyncQuestions(thread_id)}
        self._sessions[name] = sess
        if subscribed:
            sess["async_questions"].hydrate({})
        self._start_bomba(name, sess)
        if watch_tmux:
            self._start_tmux_watcher(name)

    async def _conectar(self, name: str, meta: dict) -> Optional[AppServerClient]:
        """Liga o backend ao app-server que o LANCADOR subiu (o caminho normal desde o ticket 03).

        O servidor mora no pane, nao aqui: nada e spawnado e a TUI nao e recriada. O pid vem antes
        do endereco de proposito — porta de loopback e reciclada, entao conectar so pelo endpoint
        pode cair num processo alheio que tomou a porta. Pid morto (ou handshake sem resposta) e
        sessao MORTA: devolver None deixa quem chamou tratar como sessao que acabou, em vez de
        ressuscitar uma TUI que nao existe mais.

        A assinatura vai pelo retry de sempre (start_subscription) e nao por um thread/resume aqui:
        a sessao pode estar no turno ZERO, quando o rollout ainda nao existe e o resume e recusado.
        """
        if not pid_vivo(meta["app_pid"]):
            _log.info("codex: app-server morto (pid=%s) name=%s — sessao encerrada",
                      meta.get("app_pid"), name)
            return None
        client = AppServerClient()
        try:
            await client.connect(meta["endpoint"])
            await client.request("initialize", {"clientInfo": CLIENT_INFO, "capabilities": {"experimentalApi": True}})
        # Estreito de proposito: sao as falhas de CONVERSA com o servidor, as unicas que significam
        # "sessao morta". Um `except Exception` aqui transformaria erro de programacao (um nome
        # errado, um shape mudado) em "sessao morta" no log — a falha viraria um card sumindo, sem
        # ninguem nunca ver o traceback.
        except (ConnectionError, OSError, TimeoutError, RuntimeError) as exc:
            await client.close()
            _log.warning("codex: handshake falhou em %s name=%s: %s", meta["endpoint"], name, exc)
            return None
        except Exception:
            await client.close()
            raise
        thread = {}
        try:
            result = await client.request("thread/read", {"threadId": meta["thread_id"], "includeTurns": False})
            thread = result.get("thread") or {}
        except Exception:
            _log.warning("codex: não foi possível recuperar o turno de %s", name, exc_info=True)
        # Publicar a sessão antes da leitura liberaria envios concorrentes como se estivesse ociosa.
        self.attach(name, client, meta["thread_id"], model=meta.get("model"),
                    effort=meta.get("effort"), watch_tmux=True)
        self._sessions[name].update(endpoint=meta["endpoint"], app_pid=meta["app_pid"])
        self._restore_turn(self._sessions[name], thread, include_turns=False)
        self.start_subscription(name, meta.get("cwd") or ".")
        _log.info("codex: conectado ao app-server do pane endpoint=%s name=%s",
                  meta["endpoint"], name)
        return client

    async def ensure_running(self, name: str) -> Optional[AppServerClient]:
        """Garante um AppServerClient VIVO pra sessao Codex `name` (ligacao LAZY):
        - ja ha client vivo no dict -> retorna ele (caso quente).
        - senao, le o sidecar duravel; sem sidecar -> None (sessao Codex desconhecida).
        - sidecar com endpoint/app_pid (lancador unico) -> _conectar: o servidor e do pane, o
          backend so se liga nele e sobreviver ao restart do backend deixa de ser problema dele.
        - sidecar SEM endpoint (sessao nascida no desenho antigo, em que o app-server era filho do
          backend) -> reabre o app-server, initialize, RETOMA o thread via `thread/resume` e recria
          a TUI, mas SO quando nao ha pane. Sem este ramo, uma sessao viva ficaria inalcancavel so
          por ter nascido antes da atualizacao; com ele sem a guarda de pane, o primeiro acesso pelo
          app matava o pane em que a pessoa estava trabalhando.

        Lock por-nome (IMPORTANT 1): sem ele, 2 chamadores concorrentes pro mesmo nome sem client
        vivo spawnavam 2 AppServerClient e o 2o attach() sobrescrevia o 1o no dict, vazando o
        subprocess orfao do 1o. setdefault no dict de locks e seguro sem lock proprio: nao ha
        `await` entre o get e o set, entao nenhuma outra corrotina roda no meio (cooperativo)."""
        sess = self._sessions.get(name)
        meta = codex_sessions.load(name)
        if sess is not None and (not meta or not meta.get("endpoint") or meta.get("thread_id") == sess["thread_id"]):
            return sess["client"]
        lock = self._locks.setdefault(name, asyncio.Lock())
        async with lock:
            # Double-check: outro chamador pode ter terminado de spawnar enquanto esperavamos o lock.
            sess = self._sessions.get(name)
            meta = codex_sessions.load(name)
            if sess is not None and (not meta or not meta.get("endpoint") or meta.get("thread_id") == sess["thread_id"]):
                return sess["client"]
            if meta is None:
                return None
            if meta.get("headless"):
                if sess is not None:
                    return sess["client"]
                return await self._ligar_sem_terminal(name, meta)
            if sess is not None:
                # A TUI trocou de conversa; só a conexão antiga termina, nunca o app-server do pane.
                self._sessions.pop(name, None)
                tasks = [self._tmux_watchers.pop(name, None), self._subscribers.pop(name, None), sess.get("bomba")]
                tasks = [task for task in tasks if task is not None and task is not asyncio.current_task()]
                for task in tasks:
                    if task is not None:
                        task.cancel()
                await asyncio.gather(*(task for task in tasks if task is not None), return_exceptions=True)
                await sess["client"].close()
                await PushPreviewSource.get(name).push("")
            if meta.get("endpoint") and meta.get("app_pid"):
                return await self._conectar(name, meta)
            # Daqui pra baixo o app-server e SPAWNADO e a TUI e RECRIADA — o pane atual morre. Isso
            # so pode acontecer quando nao ha pane nenhum: com pane vivo, recriar destroi a tela de
            # quem esta trabalhando ali, e era esse o efeito de reiniciar o backend. Sessao que ficou
            # sem controle vivo e sessao MORTA pra quem abriu o chat (o _state_stream emite `dead`,
            # que tem tela propria) e segue ociosa na lista — a lista nao tem coluna de morto, entao
            # some-la faria o card desaparecer enquanto a pessoa usa a TUI.
            if await send_thread(tmux.has_session, name):
                _log.info("codex: sessao %s sem controle vivo, mas o pane existe — nao recriado",
                          name)
                return None
            if self._falhas_subida.get(name, 0) >= self.TETO_SUBIDAS:
                return None
            client = AppServerClient()
            home_kw = ({"codex_home": meta["codex_home"]}
                        if meta.get("codex_home") else {})
            if meta.get("codex_account"):
                home_kw["codex_account"] = meta["codex_account"]
            try:
                endpoint = await client.start_shared(**home_kw)
                await client.request("initialize", {"clientInfo": CLIENT_INFO, "capabilities": {"experimentalApi": True}})
                result = await client.request("thread/resume", {
                    "threadId": meta["thread_id"],
                    "cwd": meta.get("cwd"),
                    "sandbox": SANDBOX,
                    "approvalPolicy": APPROVAL,
                })
            except Exception:
                # resume falhou (app-server morreu, thread perdido, etc.): nao deixa o subprocess orfao.
                await client.close()
                raise
            # thread/resume devolve {"thread": {"id","path",...}}; reusa o thread_id do sidecar como
            # fonte de verdade (o id nao muda no resume).
            thread_id = (result.get("thread") or {}).get("id") or meta["thread_id"]
            try:
                ensure_tmux_tui(
                    name, meta.get("cwd") or ".", thread_id, endpoint, replace=True,
                    model=meta.get("model"), effort=meta.get("effort"),
                    **home_kw,
                )
            except Exception:
                await client.close()
                raise
            # model/effort (Task C): repovoa a escolha do sidecar no dict quente, senao o 1o
            # turn/start pos-restart perderia a escolha ate a proxima chamada de set_model.
            # default_model/default_effort: o default da thread tambem vem no thread/resume
            # response (mesmo campo `model` do thread/start) -- so pra display, nao sobrescreve a
            # escolha acima.
            # subscribed=True: o thread/resume acima JA assinou esta thread (pos-restart o rollout
            # existe, entao ele cola de primeira) -> nao precisa da task de retry.
            self.attach(name, client, thread_id, model=meta.get("model"), effort=meta.get("effort"),
                        default_model=result.get("model"), default_effort=_effort_da_thread(result),
                        watch_tmux=True, subscribed=True)
            self._sessions[name]["async_questions"].hydrate(result.get("thread") or {})
            self._restore_turn(self._sessions[name], result.get("thread") or {})
            _log.info("codex ensure_running: resumed thread=%s name=%s", thread_id, name)
            return client

    # ── sem terminal ───────────────────────────────────────────────────────────────────────

    # Subidas seguidas que falharam (por sessão). No teto, para de tentar até ação do usuário
    # (encerrar/recriar) — o watch_sessions passa a cada 2s e viraria um spam de spawn.
    TETO_SUBIDAS = 3

    async def _ligar_sem_terminal(self, name: str, meta: dict, *, reabrir: bool = True) -> Optional[AppServerClient]:
        """Religa no cano vivo da sessão sem terminal; sem cano (ou cano morto), sobe outro."""
        cano = meta.get("cano") or {}
        if cano:
            ligado = await sem_terminal.conectar(cano)
            if ligado is not None:
                client, snap = ligado
                if snap.get("saiu") is None:
                    try:
                        await sem_terminal.initialize(client)
                        thread = {}
                        if meta.get("thread_id"):
                            result = await client.request("thread/read", {"threadId": meta["thread_id"],
                                                                          "includeTurns": False})
                            thread = result.get("thread") or {}
                        if not reabrir and (thread.get("status") or {}).get("type") not in {"active", "idle"}:
                            raise RuntimeError("não foi possível confirmar o estado do turno; permissão mantida")
                    except Exception:
                        await client.close()
                        if not reabrir:
                            raise
                        _log.warning("codex sem terminal: religação no cano falhou name=%s", name, exc_info=True)
                        return await self._subir_sem_terminal(name, meta)
                    self.attach(name, client, meta.get("thread_id") or "", model=meta.get("model"),
                                effort=meta.get("effort"), subscribed=True)
                    self._sessions[name].update(headless=True, cano=cano)
                    self._restore_turn(self._sessions[name], thread, include_turns=False)
                    _log.info("codex sem terminal: religado name=%s cano=%s", name, cano.get("pid"))
                    return client
                await client.close()
                _log.info("codex sem terminal: app-server saiu rc=%s name=%s — subindo outro",
                          snap.get("saiu"), name)
        if not reabrir:
            raise RuntimeError("não foi possível reconectar ao Codex; permissão mantida")
        return await self._subir_sem_terminal(name, meta)

    async def _subir_sem_terminal(self, name: str, meta: dict) -> Optional[AppServerClient]:
        esforco_recusado = None
        falhas = self._falhas_subida.get(name, 0)
        if falhas >= self.TETO_SUBIDAS:
            # Desistiu: o motivo da última queda fica em _problemas; o watch_sessions continua
            # passando, mas sem spawn nem log a cada 2s. Só ação do usuário (encerrar) reabre.
            return None
        await asyncio.to_thread(sem_terminal.matar, meta)
        try:
            cano = await sem_terminal.subir(meta)
            ligado = await sem_terminal.conectar(cano, esperar=10.0)
            if ligado is None:
                raise RuntimeError("cano não escutou em 10s")
            client, _ = ligado
            try:
                await sem_terminal.initialize(client)
                approval, sandbox = sem_terminal.politica(meta.get("permission_mode"))
                result = None
                if meta.get("thread_id"):
                    retomada = {"threadId": meta["thread_id"], "cwd": meta.get("cwd"),
                                "approvalPolicy": approval, "sandbox": sandbox}
                    try:
                        try:
                            result = await client.request("thread/resume", retomada)
                        except RuntimeError as exc:
                            if "Model provider" not in str(exc) or "not found" not in str(exc):
                                raise
                            # A conversa guarda o provedor em que nasceu; se ele saiu da config, o
                            # resume recusa e o histórico ficaria preso. Volta pro provedor nativo.
                            # ponytail: "openai" fixo; ler o padrão do config.toml se alguém usar outro.
                            _log.warning("codex sem terminal: provedor da thread sumiu name=%s: %s", name, exc)
                            result = await client.request("thread/resume", {**retomada, "modelProvider": "openai"})
                    except RuntimeError as exc:
                        # Thread aberta por RPC que nunca teve turno não tem rollout, e o resume
                        # a recusa: nada a perder, abre outra.
                        if "no rollout found" not in str(exc):
                            raise
                        _log.info("codex sem terminal: thread %s sem rollout — abrindo outra name=%s",
                                  meta["thread_id"], name)
                if result is None:
                    params: dict = {"cwd": meta.get("cwd"), "approvalPolicy": approval, "sandbox": sandbox}
                    if meta.get("model"):
                        params["model"] = meta["model"]
                    result = await client.request("thread/start", params)
                if meta.get("effort"):
                    # `thread/start` aceita `model`, mas não tem campo de esforço: sem este update
                    # o nível escolhido na tela cai calado no `model_reasoning_effort` do
                    # config.toml. Sem TUI, ninguém mais aplica a escolha.
                    try:
                        await client.request("thread/settings/update", {
                            "threadId": (result.get("thread") or {}).get("id") or meta.get("thread_id"),
                            "model": meta.get("model") or result.get("model"),
                            "effort": meta["effort"]})
                    except Exception as exc:
                        # A thread já está aberta: derrubar a sessão por causa do nível seria trocar
                        # uma escolha perdida por uma sessão que não existe. O nível fica o do
                        # config.toml e isso APARECE.
                        esforco_recusado = str(exc)[:300]
                        _log.warning("codex sem terminal: esforço %s recusado name=%s: %s",
                                     meta["effort"], name, exc)
            except Exception:
                await client.close()
                raise
        except Exception as exc:
            from app.procinfo import _descendant_pids
            from app.registry import _esperar_saida
            self._falhas_subida[name] = falhas + 1
            failed_meta = codex_sessions.load(name)
            pid = ((failed_meta or {}).get("cano") or {}).get("pid")
            pids = ([int(pid), *await asyncio.to_thread(_descendant_pids, int(pid))] if pid else [])
            await asyncio.to_thread(sem_terminal.matar, failed_meta)
            await asyncio.to_thread(_esperar_saida, pids)
            if any(pid_vivo(pid) for pid in pids):
                self._falhas_subida[name] = self.TETO_SUBIDAS
                message = "O processo Codex ainda está encerrando; não abri outro processo."
                self._problemas[name] = ("codex_headless_nao_subiu", message)
                raise sem_terminal.ShutdownPending(message) from exc
            codex_sessions.update(name, cano=None)
            self._problemas[name] = ("codex_headless_nao_subiu", str(exc)[:300])
            if falhas + 1 >= self.TETO_SUBIDAS:
                _log.warning("codex sem terminal: desistiu de subir após %d tentativas name=%s: %s",
                             falhas + 1, name, exc)
            else:
                _log.warning("codex sem terminal: subida %d/%d falhou name=%s: %s",
                             falhas + 1, self.TETO_SUBIDAS, name, exc)
            raise
        self._falhas_subida.pop(name, None)
        self._problemas.pop(name, None)
        if esforco_recusado:
            self._problemas[name] = ("codex_esforco_nao_aplicado", esforco_recusado)
        thread = result.get("thread") or {}
        thread_id = thread.get("id") or meta.get("thread_id")
        rollout = thread.get("path") or sem_terminal.rollout_de(thread_id, meta.get("codex_home"))
        meta = codex_sessions.update(name, thread_id=thread_id, rollout_path=rollout) or meta
        self.attach(name, client, thread_id, model=meta.get("model"), effort=meta.get("effort"),
                    default_model=result.get("model"), default_effort=_effort_da_thread(result),
                    subscribed=True)
        self._sessions[name].update(headless=True, cano=meta.get("cano"))
        self._sessions[name]["async_questions"].hydrate(thread)
        self._restore_turn(self._sessions[name], thread)
        _log.info("codex sem terminal: subiu name=%s thread=%s cano=%s", name, thread_id,
                  (meta.get("cano") or {}).get("pid"))
        return client

    async def restart(self, name: str) -> None:
        """Mata o app-server da sessão sem terminal e sobe outro na mesma conversa. É a saída de
        um turno que nunca fecha: o que estava em voo se perde, o histórico fica."""
        async with self._locks.setdefault(name, asyncio.Lock()):
            meta = codex_sessions.load(name)
            if not meta or not meta.get("headless"):
                raise ValueError("reiniciar só vale para sessão Codex sem terminal")
            sess = self._sessions.pop(name, None)
            if sess is not None:
                await sess["client"].close()
            self._falhas_subida.pop(name, None)
            await self._subir_sem_terminal(name, meta)

    async def open_terminal(self, name: str) -> None:
        """Continua a mesma thread no pane. O chamador segura a trava de entrega."""
        from app.registry import _env_sessao, _exigir_lancador_codex, _esperar_saida
        from app.procinfo import _descendant_pids

        current = await self.read_settings(name)
        async with self._locks.setdefault(name, asyncio.Lock()):
            meta = codex_sessions.load(name)
            if not meta or not meta.get("headless"):
                raise ValueError("A sessão Codex não está sem terminal.")
            sess = self._sessions.get(name)
            if not sess or not sess.get("turn_state_known") or sess.get("in_progress"):
                raise ValueError("Espere a sessão ficar ociosa antes de abrir o terminal.")
            if sess["client"].server_requests or sess["async_questions"].pending():
                raise ValueError("Responda às perguntas e permissões antes de abrir o terminal.")
            if any(e.get("delivered") is False for e in await send_thread(PromptQueue(name).load)):
                raise ValueError("Há mensagens na fila esperando entrega.")
            if not meta.get("rollout_path") or not Path(meta["rollout_path"]).is_file():
                raise ValueError("Envie a primeira mensagem antes de abrir esta conversa no terminal.")
            await asyncio.to_thread(_exigir_lancador_codex)
            account = codex_contas.resolve_account(meta.get("codex_account") or "default")
            if str(account.home.expanduser().absolute()) != meta.get("codex_home"):
                raise ValueError("A conta Codex mudou; a conversa não foi transferida.")
            if await asyncio.to_thread(tmux.has_session, name):
                raise ValueError("Já existe um terminal com este nome.")
            meta = {**meta, "model": current["model"], "effort": current["effort"]}
            approval, sandbox = sem_terminal.politica(meta.get("permission_mode"))
            command = tmux.join_cmd(comando_do_lancador(
                meta["cwd"], thread_id=meta["thread_id"], model=meta["model"], effort=meta["effort"],
                codex_home=meta["codex_home"], codex_account=account.id,
                approval=approval, sandbox=sandbox))
            env = _env_sessao(None, bool(meta.get("jev")), provider="codex")["env"]
            if meta.get("key"):
                env["CP_SESSION_KEY"] = meta["key"]
            cano_pid = (meta.get("cano") or {}).get("pid")
            pids = ([int(cano_pid), *await asyncio.to_thread(_descendant_pids, int(cano_pid))]
                    if cano_pid else [])
            self.close_sync(name, preserve_preview=True)
            await sess["client"].close()
            await asyncio.to_thread(_esperar_saida, pids)
            if any(pid_vivo(pid) for pid in pids):
                self._falhas_subida[name] = self.TETO_SUBIDAS
                raise RuntimeError("O processo antigo ainda está vivo; nenhum terminal foi aberto.")
            created = False
            launcher_pid = None
            try:
                codex_sessions.update(name, headless=False, cano=None, endpoint=None, app_pid=None,
                                      tui_pid=None, model=meta["model"], effort=meta["effort"])
                created = await asyncio.to_thread(tmux.new_session, name, meta["cwd"], command,
                                                  provider="codex", env=env)
                if not created:
                    raise RuntimeError("Não foi possível criar o terminal.")
                launcher_pid = await asyncio.to_thread(tmux.pane_pid, name)
                launched = await self._wait_terminal(name, meta["thread_id"])
                if await self._conectar(name, launched) is None:
                    raise RuntimeError("O app-server do terminal não respondeu.")
                if current.get("mode") in {"plan", "default"}:
                    await self.set_mode(name, current["mode"])
            except Exception as exc:
                _log.warning("codex: troca para terminal falhou name=%s: %s", name, exc)
                launched = codex_sessions.load(name) or {}
                new_pids = [launched[k] for k in ("app_pid", "tui_pid", "launcher_pid") if launched.get(k)]
                if launcher_pid:
                    new_pids += [launcher_pid, *await asyncio.to_thread(_descendant_pids, launcher_pid)]
                if created and await asyncio.to_thread(tmux.has_session, name):
                    if not await asyncio.to_thread(tmux.kill_session, name):
                        raise RuntimeError("Não consegui fechar o terminal; não abri outro processo.") from exc
                failed_session = self._sessions.get(name)
                self.close_sync(name, preserve_preview=True)
                if failed_session:
                    await failed_session["client"].close()
                await asyncio.to_thread(_esperar_saida, new_pids)
                if any(pid_vivo(pid) for pid in new_pids):
                    raise RuntimeError("O terminal ainda está encerrando; não abri outro processo.") from exc
                restored = {**meta, "cano": None, "endpoint": None, "app_pid": None,
                            "tui_pid": None, "launcher_pid": None}
                # O lançador remove o sidecar ao sair; a restauração também cobre esse caso.
                with codex_sessions._locked(name):
                    codex_sessions._write(name, restored)
                try:
                    if await self._subir_sem_terminal(name, restored) is None:
                        raise RuntimeError("O Codex não respondeu.")
                    if current.get("mode") in {"plan", "default"}:
                        await self.set_mode(name, current["mode"])
                except Exception as restore_error:
                    raise RuntimeError(f"A troca falhou ({exc}) e a sessão não reiniciou: {restore_error}") from restore_error
                raise RuntimeError(f"A troca falhou; a conversa continua sem terminal: {exc}") from exc

    async def _wait_terminal(self, name: str, thread_id: str) -> dict:
        # Pane criado não prova que a TUI carregou a conversa.
        deadline = time.monotonic() + 45
        probe = AppServerClient()
        try:
            while time.monotonic() < deadline:
                launched = codex_sessions.load(name) or {}
                if launched.get("tui_pid") and pid_vivo(launched["tui_pid"]):
                    if not probe.endpoint:
                        await probe.connect(launched["endpoint"])
                        await probe.request("initialize", {"clientInfo": CLIENT_INFO})
                    loaded = await probe.request("thread/loaded/list", {})
                    if thread_id in loaded.get("data", []):
                        return launched
                if not await asyncio.to_thread(tmux.has_session, name):
                    raise RuntimeError("O terminal encerrou antes de retomar a conversa.")
                await asyncio.sleep(0.2)
            raise RuntimeError("O terminal não retomou a conversa em 45 segundos.")
        finally:
            await probe.close()

    async def open_headless(self, name: str) -> None:
        """Continua a thread do terminal no cano; o chamador segura a trava de entrega."""
        from app.registry import _env_sessao, _exigir_lancador_codex, _esperar_saida, _jev_do_processo
        from app.procinfo import _descendant_pids

        current = await self.read_settings(name)
        async with self._locks.setdefault(name, asyncio.Lock()):
            meta = codex_sessions.load(name)
            if not meta or meta.get("headless"):
                raise ValueError("A sessão Codex não está no terminal.")
            sess = self._sessions.get(name)
            if not sess or not sess.get("turn_state_known") or sess.get("in_progress"):
                raise ValueError("Espere a sessão ficar ociosa antes de continuar sem terminal.")
            if sess["client"].server_requests or sess["async_questions"].pending():
                raise ValueError("Responda às perguntas e permissões antes de trocar de modo.")
            if any(e.get("delivered") is False for e in await send_thread(PromptQueue(name).load)):
                raise ValueError("Há mensagens na fila esperando entrega.")
            if not meta.get("rollout_path") or not Path(meta["rollout_path"]).is_file():
                raise ValueError("Envie a primeira mensagem antes de continuar sem terminal.")
            account = codex_contas.resolve_account(meta.get("codex_account") or "default")
            if str(account.home.expanduser().absolute()) != meta.get("codex_home"):
                raise ValueError("A conta Codex mudou; a conversa não foi transferida.")
            # O sidecar guarda a abertura, não uma mudança posterior pelo /permissions da TUI.
            snapshot = await sess["client"].request("thread/resume", {"threadId": meta["thread_id"]})
            sandbox = {"readOnly": "read-only", "workspaceWrite": "workspace-write",
                       "dangerFullAccess": "danger-full-access"}.get((snapshot.get("sandbox") or {}).get("type"))
            approval = snapshot.get("approvalPolicy")
            permission = next((mode for mode, policy, boundary, _ in sem_terminal.MODOS
                               if (policy, boundary) == (approval, sandbox)), None)
            if permission is None:
                raise ValueError("Esta política de permissões não é suportada sem terminal; a sessão foi mantida.")
            self._restore_turn(sess, snapshot.get("thread") or {}, include_turns=False)
            if not sess.get("turn_state_known") or sess.get("in_progress"):
                raise ValueError("A sessão iniciou um turno; espere terminar antes de trocar de modo.")
            await asyncio.to_thread(_exigir_lancador_codex)
            pane_pid = await asyncio.to_thread(tmux.pane_pid, name)
            meta = {**meta, "model": snapshot.get("model") or current["model"],
                    "effort": _effort_da_thread(snapshot) or current["effort"],
                    "permission_mode": permission, "key": meta.get("key") or sem_terminal.nova_chave(),
                    "jev": await asyncio.to_thread(_jev_do_processo, meta.get("app_pid") or pane_pid)
                           if meta.get("app_pid") or pane_pid else bool(meta.get("jev"))}
            command = tmux.join_cmd(comando_do_lancador(
                meta["cwd"], thread_id=meta["thread_id"], model=meta["model"], effort=meta["effort"],
                codex_home=meta["codex_home"], codex_account=account.id, approval=approval, sandbox=sandbox))
            env = _env_sessao(None, bool(meta.get("jev")), provider="codex")["env"]
            env["CP_SESSION_KEY"] = meta["key"]
            pids = [meta[k] for k in ("app_pid", "tui_pid", "launcher_pid") if meta.get(k)]
            if pane_pid:
                pids += [pane_pid, *await asyncio.to_thread(_descendant_pids, pane_pid)]
            watcher = self._tmux_watchers.pop(name, None)
            if watcher is not None:
                watcher.cancel()
                await asyncio.gather(watcher, return_exceptions=True)
            # O lançador não deve apagar o sidecar enquanto trocamos de transporte.
            codex_sessions.update(name, app_pid=None)
            if not await asyncio.to_thread(tmux.kill_session, name):
                codex_sessions.update(name, app_pid=meta.get("app_pid"))
                self._start_tmux_watcher(name)
                raise RuntimeError("Não foi possível fechar o terminal; o modo foi mantido.")
            self.close_sync(name, preserve_preview=True)
            await sess["client"].close()
            await asyncio.to_thread(_esperar_saida, pids)
            if any(pid_vivo(pid) for pid in pids):
                # Sem o dono, ensure_running cairia no caminho legado e abriria outro servidor.
                codex_sessions.update(name, app_pid=meta.get("app_pid"))
                self._falhas_subida[name] = self.TETO_SUBIDAS
                raise RuntimeError("O terminal ainda está encerrando; não abri outro processo.")
            headless = {**meta, "headless": True, "cano": None, "endpoint": None,
                        "app_pid": None, "tui_pid": None, "launcher_pid": None}
            try:
                with codex_sessions._locked(name):
                    codex_sessions._write(name, headless)
                if await self._subir_sem_terminal(name, headless) is None:
                    raise RuntimeError("O Codex não respondeu.")
                if (self._sessions.get(name) or {}).get("thread_id") != meta["thread_id"]:
                    raise RuntimeError("O Codex não retomou a conversa original.")
                if current.get("mode") in {"plan", "default"}:
                    await self.set_mode(name, current["mode"])
            except sem_terminal.ShutdownPending:
                raise
            except Exception as exc:
                _log.warning("codex: troca para sem terminal falhou name=%s: %s", name, exc)
                failed = self._sessions.get(name)
                cano_pid = ((codex_sessions.load(name) or {}).get("cano") or {}).get("pid")
                new_pids = ([int(cano_pid), *await asyncio.to_thread(_descendant_pids, int(cano_pid))]
                            if cano_pid else [])
                self.close_sync(name, preserve_preview=True)
                if failed:
                    await failed["client"].close()
                await asyncio.to_thread(_esperar_saida, new_pids)
                if any(pid_vivo(pid) for pid in new_pids):
                    self._falhas_subida[name] = self.TETO_SUBIDAS
                    raise RuntimeError("O Codex sem terminal ainda está encerrando; não abri outro processo.") from exc
                restored = {**meta, "headless": False, "cano": None, "endpoint": None,
                            "app_pid": None, "tui_pid": None, "launcher_pid": None}
                try:
                    with codex_sessions._locked(name):
                        codex_sessions._write(name, restored)
                    if not await asyncio.to_thread(tmux.new_session, name, meta["cwd"], command,
                                                   provider="codex", env=env):
                        raise RuntimeError("Não foi possível reabrir o terminal.")
                    launched = await self._wait_terminal(name, meta["thread_id"])
                    if await self._conectar(name, launched) is None:
                        raise RuntimeError("O terminal não respondeu.")
                    if current.get("mode") in {"plan", "default"}:
                        await self.set_mode(name, current["mode"])
                except Exception as restore_error:
                    raise RuntimeError(f"A troca falhou ({exc}) e o terminal não voltou: {restore_error}") from restore_error
                raise RuntimeError(f"A troca falhou; a conversa continua no terminal: {exc}") from exc

    async def warm_sessions(self) -> None:
        """Reconecta sidecars Codex em série, sem atrasar a subida do backend."""
        for meta in await asyncio.to_thread(codex_sessions.list_all):
            name = meta.get("name")
            if not name:
                continue
            sess = self._sessions.get(name)
            if sess and sess["thread_id"] == meta.get("thread_id") and not sess["client"].closed:
                continue
            try:
                await self.ensure_running(name)
            except Exception:
                _log.warning("codex: aquecimento falhou name=%s", name, exc_info=True)
            await asyncio.sleep(0)

    async def watch_sessions(self) -> None:
        # Sessões abertas pelo terminal também precisam publicar perguntas antes de abrir o chat.
        while True:
            try:
                await self.warm_sessions()
            except Exception:
                _log.exception("codex: falha ao descobrir sessões; nova tentativa no próximo ciclo")
            await asyncio.sleep(2)

    def close_sync(self, name: str, *, preserve_preview: bool = False) -> None:
        """Encerramento SINCRONO do client vivo (chamado pelo registry.kill, que e sync). Manda
        SIGTERM best-effort no app-server e esquece a sessao da memoria; o read loop (loop
        principal) ve o EOF e roda seu finally. NAO apaga o sidecar duravel -- isso e o kill().

        O SIGTERM vai pelo PID do sidecar: desde o lancador unico o servidor nao e filho do backend,
        entao `client.terminate()` sozinho seria um no-op e o servidor sobreviveria ao encerrar."""
        matar_app_server(name)
        sem_terminal.matar(codex_sessions.load(name))
        self._falhas_subida.pop(name, None)
        self._problemas.pop(name, None)
        sess = self._sessions.pop(name, None)
        watcher = self._tmux_watchers.pop(name, None)
        if watcher is not None:
            watcher.cancel()
        sub = self._subscribers.pop(name, None)
        if sub is not None:
            sub.cancel()
        if not preserve_preview:
            PushPreviewSource._sources.pop(name, None)
        if sess is None:
            return
        bomba = sess.get("bomba")
        if bomba is not None:
            bomba.cancel()
        term = getattr(sess["client"], "terminate", None)
        if callable(term):
            term()

    def delivery_lock(self, name: str) -> asyncio.Lock:
        return self._delivery_locks.setdefault(name, asyncio.Lock())

    def rename(self, old: str, new: str) -> None:
        def rearmar() -> None:
            for task in (self._tmux_watchers.pop(old, None), self._subscribers.pop(old, None)):
                if task is not None:
                    task.cancel()
            PushPreviewSource._sources.pop(old, None)
            sess = self._sessions.pop(old, None)
            lock = self._locks.pop(old, None)
            if lock is not None:
                self._locks[new] = lock
            if old in self._falhas_subida:
                self._falhas_subida[new] = self._falhas_subida.pop(old)
            if old in self._problemas:
                self._problemas[new] = self._problemas.pop(old)
            if sess is None:
                return
            bomba = sess.pop("bomba", None)
            if bomba is not None:
                bomba.cancel()
            for fila in sess.get("ouvintes", []):
                fila.put_nowait(None)
            sess["ouvintes"] = []
            self._sessions[new] = sess
            if not sess.get("headless"):
                self._start_tmux_watcher(new)
            self._start_bomba(new, sess)
            if not sess.get("subscribed"):
                meta = codex_sessions.load(new) or {}
                self.start_subscription(new, meta.get("cwd") or ".")

        try:
            asyncio.get_running_loop()
        except RuntimeError:
            if self._loop is not None and self._loop.is_running():
                done = threading.Event()
                errors: list[BaseException] = []

                def run() -> None:
                    try:
                        rearmar()
                    except BaseException as exc:
                        errors.append(exc)
                    finally:
                        done.set()

                self._loop.call_soon_threadsafe(run)
                if not done.wait(5):
                    raise RuntimeError("timeout ao rearmar sessão Codex renomeada")
                if errors:
                    raise errors[0]
            else:
                rearmar()
        else:
            rearmar()

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        # Mesma mecanica de tail (backfill do tail + watch de append) do Claude, so trocando o
        # parser pro shape do rollout do Codex (snake_case, envelope {type, payload}).
        # Garante o dir do rollout (~/.codex/sessions/YYYY/MM/DD/): na 1a sessao Codex do dia o
        # Codex ainda nao criou a pasta do dia quando o SSE abre o tail -> awatch(parent) em
        # transcript.follow() levantaria FileNotFoundError e derrubaria o SSE inteiro (chat nao
        # carrega/atualiza). mkdir idempotente fecha essa janela; o Codex grava ali de todo jeito.
        Path(path).parent.mkdir(parents=True, exist_ok=True)
        return TranscriptTailer(path, parse_line=parse_rollout_line).follow(start_offset)

    async def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        while True:
            client = (self._sessions.get(name) or {}).get("client")
            async for event in self._state_stream(name):
                if client is None:
                    client = (self._sessions.get(name) or {}).get("client")
                yield event
            lock = self._locks.get(name)
            current = self._sessions.get(name)
            # Trocar o transporte encerra a fonte antiga; os ouvintes continuam na mesma sessão.
            if not (lock and lock.locked()) and (not current or current["client"] is client):
                return

    def snapshot(self, name: str, thread_id: str) -> StateEvent | None:
        """Estado conhecido da thread viva, compartilhado entre a lista e o chat."""
        sess = self._sessions.get(name)
        if not sess or sess["thread_id"] != thread_id or not sess.get("subscribed"):
            return None
        if sess["client"].closed or sess.get("bomba_error"):
            return None
        if not sess.get("turn_state_known") and not sess.get("in_progress"):
            return None
        return self._question_state(name, sess)

    def _question_state(self, name: str, sess: dict) -> StateEvent:
        from .questions import pending
        blocking = pending(sess["client"], sess["thread_id"])
        question = blocking or sess["async_questions"].pending()
        state = "awaiting_input" if blocking or (question and sess["state"] == "idle") else sess["state"]
        aprovacao = self._aprovacao_pendente(sess)
        if aprovacao is not None:
            state = "awaiting_input"
        problema = sess.get("turn_problem") or (None, None)
        return StateEvent(session=name, state=state,
                          label="Compactando…" if sess.get("compacting") else None,
                          problema=problema[0], problema_detalhe=problema[1],
                          status_line=self._status_line(sess), codex_mode=sess.get("mode"),
                          codex_buffering=sess.get("codex_buffering", False),
                          codex_question=question, headless=bool(sess.get("headless")),
                          question=sem_terminal.texto_da_aprovacao(aprovacao) if aprovacao else None,
                          options=list(sem_terminal.OPCOES_APROVACAO) if aprovacao else None)

    @staticmethod
    def _aprovacao_pendente(sess: dict) -> Optional[dict]:
        if not sess.get("headless"):
            return None
        for req in sess["client"].server_requests.values():
            if req.get("method") in sem_terminal.APROVACOES:
                return req
        return None

    def problema_de(self, name: str) -> str | None:
        """Código do problema da sessão (não subiu, ou o turno em voo não fala com o provedor)."""
        p = self._problemas.get(name) or (self._sessions.get(name) or {}).get("turn_problem")
        return p[0] if p else None

    def aprovacao_pendente(self, name: str) -> tuple[str | None, list[str] | None]:
        """(pergunta, opções) do cartão de aprovação em aberto da sessão sem terminal, pra lista."""
        sess = self._sessions.get(name)
        req = self._aprovacao_pendente(sess) if sess else None
        if req is None:
            return None, None
        return sem_terminal.texto_da_aprovacao(req), list(sem_terminal.OPCOES_APROVACAO)

    async def select(self, name: str, option: int) -> bool:
        """Resposta ao pedido de aprovação em aberto (sem terminal). False = nada pendente."""
        sess = self._sessions.get(name)
        req = self._aprovacao_pendente(sess) if sess else None
        if req is None:
            return False
        await sess["client"].respond(req["id"], {"decision": sem_terminal.decisao(option)})
        return True

    def permission_modes_sem_terminal(self, name: str) -> dict:
        meta = codex_sessions.load(name) or {}
        return sem_terminal.modos_para_tela(meta.get("permission_mode"))

    async def set_permission_mode_sem_terminal(self, name: str, modo: str) -> dict:
        """Troca o modo da sessão sem terminal. approvalPolicy vale no próximo turno; sandbox só
        muda reabrindo o app-server (thread/resume), após confirmar que está ociosa."""
        nome = next((m[0] for m in sem_terminal.MODOS if m[0].lower() == modo.strip().lower()), None)
        if nome is None:
            raise ValueError("modo desconhecido: " + modo + " (os modos são: "
                             + ", ".join(m[0] for m in sem_terminal.MODOS) + ")")
        lock = self._locks.setdefault(name, asyncio.Lock())
        async with self.delivery_lock(name), lock:
            meta = codex_sessions.load(name) or {}
            sandbox_antes = sem_terminal.politica(meta.get("permission_mode"))[1]
            if sem_terminal.politica(nome)[1] == sandbox_antes:
                codex_sessions.update(name, permission_mode=nome)
                return {"current": nome}
            sess = self._sessions.get(name)
            # Após restart, a ausência na memória não prova que o cano está ocioso.
            if sess is None and meta.get("cano"):
                await self._ligar_sem_terminal(name, meta, reabrir=False)
                sess = self._sessions.get(name)
            if sess is not None and not sess.get("in_progress") and not sess.get("turn_state_known"):
                await self.read_settings(name)
                if not sess.get("in_progress") and not sess.get("turn_state_known"):
                    raise RuntimeError("não foi possível confirmar o estado do turno; permissão mantida")
            if sess is not None and (sess.get("in_progress") or sess["client"].server_requests):
                raise sem_terminal.Ocupada(
                    "a sessão está trabalhando; mudar o sandbox reiniciaria o Codex — espere ela terminar")
            meta = codex_sessions.update(name, permission_mode=nome) or meta
            sess = self._sessions.pop(name, None)
            if sess is not None:
                bomba = sess.get("bomba")
                if bomba is not None:
                    bomba.cancel()
                await sess["client"].close()
                PushPreviewSource._sources.pop(name, None)
            self._falhas_subida.pop(name, None)
            await self._subir_sem_terminal(name, meta)
        return {"current": nome}

    async def _recusar_pedido(self, name: str, client: AppServerClient, req: dict) -> None:
        metodo = req.get("method")
        try:
            await client.respond(req["id"], None, erro={"code": -32601,
                                                         "message": f"{metodo} não é atendido pelo Hangar sem terminal"})
            texto = f"O Codex pediu `{metodo}`, que a sessão sem terminal não atende; o pedido foi recusado."
        except Exception as exc:
            _log.warning("codex sem terminal: não consegui recusar %s name=%s", metodo, name, exc_info=True)
            texto = (f"O Codex pediu `{metodo}`, que a sessão sem terminal não atende, e a recusa não "
                     f"chegou nele ({exc}). O turno pode ficar preso; interrompa se não andar.")
        try:
            await asyncio.to_thread(PromptQueue(name).append_saida_local, texto)
        except Exception:
            _log.exception("codex sem terminal: nota local não gravada name=%s", name)

    def async_question_status(self, name: str) -> tuple[int, str | None]:
        sess = self._sessions.get(name)
        if sess is None:
            return 0, None
        questions = sess["async_questions"]
        first = questions.pending()
        return questions.count, first["questions"][0]["question"] if first else None

    async def answer_questions(self, name: str, request_id: int | str | None, answers: list[dict]) -> None:
        from .questions import pending, response
        client = await self.ensure_running(name)
        if client is None:
            raise ValueError("A sessão não está disponível.")
        if isinstance(request_id, str) and request_id.startswith("async:"):
            sess = self._sessions[name]
            async with self.delivery_lock(name):
                if self._sessions.get(name) is not sess:
                    raise ValueError("A conversa mudou antes da resposta.")
                text = sess["async_questions"].response(request_id, answers)
                # Mesmo envio da TUI: o app-server inicia ou orienta o turno da conversa de origem.
                await client.request("turn/start", {"threadId": sess["thread_id"],
                                                   "input": [{"type": "text", "text": text}]})
                sess["async_questions"].record_answer(request_id, text)
                for listener in sess.get("ouvintes", []):
                    listener.put_nowait(self._question_state(name, sess))
            return
        question = pending(client, self._sessions[name]["thread_id"])
        if question is None or request_id != question["request_id"] or type(request_id) is not type(question["request_id"]):
            raise ValueError("A pergunta já foi respondida ou cancelada.")
        await client.respond(request_id, response(question, answers))

    async def _state_stream(self, name: str) -> AsyncIterator[StateEvent]:
        try:
            client = await self.ensure_running(name)
        except Exception:
            # resume falhou (app-server indisponivel) -> dead pro front em vez de derrubar o SSE.
            _log.exception("codex state_monitor: ensure_running falhou name=%s", name)
            client = None
        if client is None:
            # Sessao Codex desconhecida (sem client vivo e sem sidecar) -> "dead" pro front, igual
            # ao StateMonitor do Claude quando a sessao tmux some.
            #
            # A sessao que ainda NAO abriu a thread (a TUI parada num seletor dela) tambem cai aqui,
            # e quem a cobre e a LISTA: `list_with_state` le o pane dessa sessao so pra achar menu, e
            # o chat monta os botoes com isso. Foi tentado resolver aqui, caindo no monitor de pane
            # enquanto nao ha thread, e nao presta: este monitor nunca acabaria sozinho quando a
            # thread abrisse (o chat ficaria no fallback ate reconectar), e no teste ele roda pra
            # sempre, porque `has_session` ali e um mock que responde sempre "sim".
            problema = self._problemas.get(name)
            yield StateEvent(session=name, state="dead",
                             problema=problema[0] if problema else None,
                             problema_detalhe=problema[1] if problema else None)
            return
        sess = self._sessions[name]
        # A fila do app-server tem UM consumidor por sessao (a bomba); cada SSE e um ouvinte que
        # recebe copia dos StateEvents. Dois consumidores na mesma fila DIVIDIAM os deltas: desktop
        # e celular no mesmo chat mostravam metade da frase cada um, sobrescrevendo a previa.
        fila: asyncio.Queue = asyncio.Queue()
        ouvintes: list[asyncio.Queue] = sess.setdefault("ouvintes", [])
        ouvintes.append(fila)
        bomba = self._start_bomba(name, sess)
        try:
            # Retrato do que ja se sabe: quem reabre o chat no meio de um turno nao espera a
            # proxima notification pra ver estado, contexto e limites.
            try:
                await self.read_settings(name)
                await self.read_rate_limits(name)
            except Exception:
                _log.warning("codex: não foi possível atualizar os controles de %s", name)
            yield self._question_state(name, sess)
            if bomba is not None and bomba.done():
                erro = sess.get("bomba_error")
                if erro is not None:
                    raise erro
                return
            while True:
                ev = await fila.get()
                if ev is None:
                    return
                if isinstance(ev, BaseException):
                    raise ev     # a bomba quebrou: sobe ate o pump do SSE, que fecha e reconecta
                yield ev
        finally:
            if fila in ouvintes:
                ouvintes.remove(fila)

    @staticmethod
    def _status_line(sess: dict) -> Optional[str]:
        # model-or-default (mesma regra de current_model): sem escolha explicita, mostra o
        # default real da thread em vez de omitir o 🤖 inteiro.
        return format_status_line(
            sess.get("model") or sess.get("default_model"),
            sess.get("effort") or sess.get("default_effort"),
            sess.get("token_usage"), sess.get("rate_limits"),
        )

    async def _bombear(self, name: str, client: AppServerClient, sess: dict) -> None:
        """Le a fila do app-server, aplica o efeito de cada notification na sessao (estado, previa,
        drain-on-complete) e espalha os StateEvents pros ouvintes do chat ou da chamada de voz.

        Roda numa task propria, entao uma excecao aqui nao sobe sozinha: ela e repassada aos
        ouvintes (que a levantam no SSE) e o sentinela final sai SEMPRE — sem isso cada ouvinte
        ficava em `fila.get()` pra sempre, com a tela "conectada" e muda."""
        ouvintes = sess["ouvintes"]   # a lista DESTA bomba: a sucessora ganha outra (ver _state_stream)

        def espalhar(ev) -> None:
            for fila in ouvintes:
                fila.put_nowait(ev)

        try:
            await self._consumir(name, client, sess, espalhar)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            sess["bomba_error"] = exc
            _log.exception("codex bomba quebrou name=%s", name)
            espalhar(exc)
        finally:
            espalhar(None)

    async def _consumir(self, name: str, client: AppServerClient, sess: dict, espalhar) -> None:
        # Buffer do turno em voo (deltas sao INCREMENTAIS -- concatena; ver docs/codex-app-server-
        # contract.md).
        buf = ""
        async for notif in client.notifications():
            if self._sessions.get(name) is not sess:
                return
            params = notif.get("params") or {}
            if notif.get("method") == "thread/started" and sess.get("app_pid"):
                thread = params.get("thread") or {}
                if thread.get("id") != sess["thread_id"]:
                    await asyncio.to_thread(codex_sessions.switch_thread, name, thread, sess["thread_id"],
                                            endpoint=sess["endpoint"], app_pid=sess["app_pid"])
            # O app-server também publica estados de outras threads, inclusive subagentes.
            if params.get("threadId") is not None and params["threadId"] != sess["thread_id"]:
                continue
            if sess.get("voice_events") is not None:
                from app.codex_voice import forward
                forward(sess, notif)
            if sess.get("headless") and notif.get("id") is not None:
                # Pedido do servidor: na TUI quem responde é ela; aqui é o cartão do app.
                if notif["method"] in sem_terminal.APROVACOES:
                    espalhar(self._question_state(name, sess))
                    continue
                if notif["method"] != "item/tool/requestUserInput":
                    await self._recusar_pedido(name, client, notif)
                    continue
            mapped = map_state(notif)
            method = notif.get("method")
            compact_updated = method in {"item/started", "item/completed"} and \
                (params.get("item") or {}).get("type") == "contextCompaction"
            if compact_updated:
                sess["compacting"] = method == "item/started"
            elif method == "turn/completed":
                sess.pop("compacting", None)
            current_turn = not sess.get("turn_id") or params.get("turnId") in (None, sess["turn_id"])
            response_started = current_turn and (bool(mapped.preview_delta) or (
                method == "item/completed" and (params.get("item") or {}).get("type") == "agentMessage"
                and bool((params.get("item") or {}).get("text"))
            ))
            if method == "turn/started" or mapped.state == "idle":
                sess.pop("codex_response_started", None)
            elif response_started:
                sess["codex_response_started"] = True
            buffering_updated = False
            if mapped.buffering is not None:
                if params.get("threadId") != sess["thread_id"] or not sess["in_progress"] or sess.get("codex_response_started"):
                    continue
                if sess.get("turn_id") and params.get("turnId") != sess["turn_id"]:
                    continue
                buffering_updated = sess.get("codex_buffering", False) != mapped.buffering
                sess["codex_buffering"] = mapped.buffering
            elif method == "turn/started" or mapped.state == "idle" or response_started:
                buffering_updated = bool(sess.pop("codex_buffering", False))
            async_updated = method in {"item/started", "item/completed"} and \
                sess["async_questions"].observe(params.get("item") or {})
            if mapped.state is not None:
                sess["state_revision"] = sess.get("state_revision", 0) + 1
            settings_updated = method == "thread/settings/updated"
            if settings_updated:
                if params.get("threadId") != sess["thread_id"]:
                    continue
                settings = params.get("threadSettings") or {}
                sess["model"] = settings.get("model")
                sess["effort"] = sess["default_effort"] = settings.get("effort")
                sess["mode"] = (settings.get("collaborationMode") or {}).get("mode", "default")
                sess["settings_revision"] = sess.get("settings_revision", 0) + 1
            turn_problem = _turn_problem(notif) if current_turn else None
            problem_updated = False
            if turn_problem is not None:
                problem_updated = sess.get("turn_problem") != turn_problem
                sess["turn_problem"] = turn_problem
            elif method == "turn/started" or response_started or \
                    (current_turn and method == "item/started"
                     and (params.get("item") or {}).get("type") != "userMessage") or \
                    (method == "turn/completed" and sess.get("turn_problem", ("",))[0] == "codex_sem_conexao"):
                # Reconectou (chegou resposta ou qualquer item novo, inclusive só ferramenta) ou o
                # turno fechou sem erro: o aviso não vale mais.
                problem_updated = sess.pop("turn_problem", None) is not None
            if method == "turn/started":
                buf = ""  # novo turno -- zera pra nao vazar o texto do turno anterior
                # guarda o turnId do turno em voo (turn/interrupt exige threadId+turnId).
                turn_id = ((notif.get("params") or {}).get("turn") or {}).get("id")
                if turn_id:
                    sess["turn_id"] = turn_id
            elif mapped.preview_delta is not None:
                buf += mapped.preview_delta
                await PushPreviewSource.get(name).push(buf)
            elif method in ("item/started", "item/completed") and \
                    ((notif.get("params") or {}).get("item") or {}).get("type") == "agentMessage":
                # Um turno pode ter varios agentMessage (preambulo "Vou conferir…" + resposta). O
                # completado vira bolha propria pelo rollout; se ficasse no buffer, a previa
                # mostrava "Vou conferir.Resposta" ate o turno fechar.
                buf = ""
                await PushPreviewSource.get(name).push("")
            elif method == "turn/completed":
                # o texto final ja caiu no rollout -> vira ChatEvent autoritativo via
                # transcript_stream; o sse.py tambem suprime via _already_committed. Limpa aqui pra
                # nao deixar o ultimo delta pendurado ate o proximo turno.
                await PushPreviewSource.get(name).push("")
                # Marca idle ANTES de drenar (nao depender do thread/status/changed idle ter chegado
                # antes -- a ordem das notifications do app-server nao e garantida). A drain chama
                # send_prompt -> deliverable(), que le in_progress: se ficasse True aqui, deliverable
                # daria False, send_prompt viraria "deferred", a drain reverteria e a entrada
                # enfileirada ficaria presa pra sempre (perda silenciosa). Tambem zera o turn_id: o
                # turno morreu -> interrupt vira no-op em vez de mandar turn/interrupt de turno morto.
                sess["state"] = "idle"
                sess["in_progress"] = False
                sess["turn_id"] = None
                # Turno terminou: a bomba única e permanente entrega a fila mesmo sem SSE aberto.
                # Best-effort: falha aqui nunca derruba o consumidor do app-server.
                try:
                    await self.drain(name, "")
                except Exception:
                    _log.exception("codex drain-on-complete falhou name=%s", name)
                if self._sessions.get(name) is not sess:
                    return
            if mapped.state is not None:
                sess["state"] = mapped.state
                was = sess["in_progress"]
                sess["in_progress"] = mapped.state == "working"
                sess["turn_state_known"] = not sess["in_progress"] or bool(sess.get("turn_id"))
                # Carimba QUANDO o turno comecou. O TTL de deliverable() mede a partir daqui; sem
                # isto um in_progress vindo do stream (nao do send_prompt) ficava com marco 0 e
                # expirava de imediato, liberando envio no meio de um turno vivo.
                if sess["in_progress"] and not was:
                    sess["in_progress_since"] = time.monotonic()
            # Task D: acumula tokenUsage/rateLimits por sessao (snapshot mais recente de cada) --
            # sao notifications esparsas, nao vem toda hora, entao guarda no dict quente pra
            # sobreviver ate o proximo StateEvent emitido (mesmo que seja por outro motivo, tipo
            # turn/started).
            if mapped.token_usage is not None:
                sess["token_usage"] = mapped.token_usage
            if mapped.rate_limits is not None:
                sess["rate_limits"] = mapped.rate_limits
            question_updated = async_updated or method in ("item/tool/requestUserInput", "serverRequest/resolved")
            if mapped.state is None and mapped.token_usage is None and mapped.rate_limits is None and not settings_updated and not question_updated and not buffering_updated and not problem_updated and not compact_updated:
                # Neutro (method desconhecido) ou so preview_delta: StateEvent nao tem campo de
                # preview -> nada a emitir aqui (o preview ja foi empurrado acima, fora do
                # StateEvent -- efeito colateral adicional, nao substitui).
                continue
            # status_line SEMPRE montado com o que ha de mais recente acumulado (model/effort do
            # dict quente + token_usage/rate_limits guardados acima) -- nao so quando ESTE notif
            # trouxe token/limite novo, senao o front perderia contexto/limites em StateEvents de
            # working/idle puros (a maioria).
            espalhar(self._question_state(name, sess))
        # notifications() terminou = EOF do app-server (o read loop empurra o sentinela ao morrer).
        # Dead-detection (backlog T4-m2): emite dead pra o front + limpa a sessao da memoria (o
        # sidecar duravel fica; ensure_running reabre num acesso futuro). getattr: um client FAKE de
        # teste sem `closed` termina o stream sem simular morte -> nao emite dead.
        if getattr(client, "closed", False) and self._sessions.get(name) is sess:
            sess["state"] = "dead"
            self._sessions.pop(name, None)
            PushPreviewSource._sources.pop(name, None)
            espalhar(StateEvent(session=name, state="dead"))

    async def send_prompt(self, name: str, text: str) -> str:
        """Envia o prompt como `turn/start` no app-server — NAO digitando no pane do tmux.

        MEDIDO (probe contra codex-cli 0.144.6, docs/codex-app-server-contract.md): a TUI
        `codex --remote` RENDERIZA um turno iniciado por outro cliente do mesmo app-server. O
        commit que passou a digitar no tmux partiu do diagnostico inverso -- o que faltava nao era
        a TUI mostrar, era o BACKEND ENXERGAR (sem `thread/resume` ele nao recebe turn/*). Com a
        assinatura no lugar (_subscribe), `turn/start` deixa terminal + celular + rollout como uma
        conversa so, e o caminho Claude (terminal_input) volta a nao ser tocado por Codex nenhum.

        Modelo, esforço e modo são herdados da thread: reenviar valores locais sobrescreveria
        uma mudança mais recente feita pelo terminal.
        """
        client = await self.ensure_running(name)
        if client is None or not await self.deliverable(name):
            return "deferred"
        sess = self._sessions[name]
        params: dict = {"threadId": sess["thread_id"],
                        "input": await self._user_input(name, text)}
        if sess.get("headless"):
            # Sem TUI ninguém aplica o /permissions: o approvalPolicy vai a cada turno (é o
            # único eixo que o turn/start aplica de verdade; o sandbox é do processo).
            meta = await asyncio.to_thread(codex_sessions.load, name) or {}
            params["approvalPolicy"] = sem_terminal.politica(meta.get("permission_mode"))[0]
        try:
            result = await client.request("turn/start", params)
            sess["turn_id"] = (result.get("turn") or {}).get("id")
        except Exception:
            # app-server morto/timeout: NAO engolir -- o caller (api/_send_one_codex, drain) trata
            # "deferred" reenfileirando, entao a msg nao se perde silenciosamente.
            _log.exception("codex turn/start falhou name=%s", name)
            return "deferred"
        # Marca in_progress AQUI (nao so esperar o turn/started chegar no loop de notifications):
        # o drain roda dentro desse mesmo loop, entao um turn/started concorrente pode nao ser
        # processado a tempo -- sem isto, deliverable() ficaria True e o drain mandaria todas as
        # entradas pendentes back-to-back (ver test_drain_stops_after_first_delivery).
        #
        sess["in_progress"] = True
        sess["turn_state_known"] = bool(sess.get("turn_id"))
        sess["state_revision"] = sess.get("state_revision", 0) + 1
        sess["in_progress_since"] = time.monotonic()
        return "sent"

    async def interrupt(self, name: str) -> bool:
        """Interrompe o turno em voo via `turn/interrupt` (threadId+turnId).

        Pelo app-server, nao por Escape no pane: o interrupt vale igual venha do celular ou do
        terminal, e nao depende de heuristica de TUI. Sem turno em voo (turn_id None) e no-op
        seguro -- mandar interrupt de turno morto nao ajuda ninguem.
        """
        if await self.ensure_running(name) is None:
            return False
        try:
            turn_id = await self._active_turn_id(name)
        except Exception:
            _log.exception("codex: não foi possível recuperar o turno para interromper name=%s", name)
            return False
        sess = self._sessions.get(name)
        if sess is None or not turn_id:
            return False
        try:
            await sess["client"].request("turn/interrupt",
                                          {"threadId": sess["thread_id"], "turnId": turn_id})
        except Exception:
            _log.exception("codex turn/interrupt falhou name=%s", name)
            return False
        return True

    def _marcador_diz_ocioso(self, name: str) -> bool:
        """O marcador de estado desta sessao diz que nao ha turno aberto? Ausente = nao sabe (False:
        so o que se prova destrava)."""
        meta = codex_sessions.load(name) or {}
        rollout = meta.get("rollout_path")
        if not rollout:
            return False
        m = hook_state.get_state(session_key(rollout))
        return bool(m and m[0] == "idle")

    async def deliverable(self, name: str) -> bool:
        # Predicado BARATO (nao spawna): so olha o in_progress cacheado. Sessao nao anexada = nada
        # em andamento -> True (send_prompt e quem chama ensure_running e realmente entrega).
        sess = self._sessions.get(name)
        if sess is None:
            return True
        from .questions import pending
        if pending(sess.get("client"), sess.get("thread_id", "")) is not None:
            return False
        if not sess["in_progress"]:
            return True
        # O marcador do hook e uma fonte INDEPENDENTE do nosso estado em memoria, e quem o escreve e
        # o proprio Codex. Turno fechado la = nao ha turno aqui, ponto. Sem isto, um `turn/completed`
        # perdido (app-server retomado no meio, SSE caido) deixava a sessao ASSINADA "ocupada" pra
        # sempre: todo envio virava fila e a fila nunca drenava — medido em 30/08/2026, com a lista
        # mostrando a sessao ociosa e o /input respondendo "sessao ocupada" no mesmo instante. O
        # escape por tempo logo abaixo nao cobria isso: ele so vale pra sessao nao assinada.
        # So DESTRAVA: marcador "working" nao e usado pra segurar nada (quem segura e o in_progress).
        if self._marcador_diz_ocioso(name):
            sess["in_progress"] = False
            return True
        # Quem LIMPA in_progress e o turn/completed -- e ele so chega pra quem assinou a thread.
        # Numa sessao que nunca assinou, in_progress ficaria True pra sempre: deliverable() eterno
        # False, todo envio virando "deferred" e a fila crescendo calada. Expira por TEMPO, com log,
        # em vez de bloquear pra sempre. Nao vale pra sessao assinada: la o turn/completed manda.
        if not sess.get("subscribed"):
            since = sess.get("in_progress_since") or 0.0
            if time.monotonic() - since > self.UNSUBSCRIBED_TURN_TTL:
                _log.warning("codex: turno sem confirmacao ha %.0fs numa sessao NAO assinada "
                             "name=%s — liberando envio (sem rastreio de turno)",
                             self.UNSUBSCRIBED_TURN_TTL, name)
                sess["in_progress"] = False
                return True
        return False

    async def drain(self, name: str, path: str) -> int:
        sess = self._sessions.get(name)
        async with self.delivery_lock(name):
            if self._sessions.get(name) is not sess:
                return 0
            return await self._drain(name, path)

    async def _drain(self, name: str, path: str) -> int:
        """Entrega a fila duravel (PromptQueue keyed por nome) via send_prompt (TUI no tmux). Sem
        tty/overlay como no Claude: claim-1-envia-1, para no primeiro `deferred` (turno em curso).
        Retorna quantas entregou. `path` (rollout) mantido por assinatura do Protocol; nao usado.

        IMPORTANT 2: PromptQueue.load/claim_undelivered/set_delivered fazem I/O de arquivo SINCRONO
        com lock -- chamados direto numa corrotina bloqueariam o event loop (que serve o SSE de
        outras sessoes). O pool de envio evita disputar com funcionalidades secundárias."""
        q = PromptQueue(name)
        if not any(e.get("delivered") is False for e in await send_thread(q.load)):
            return 0
        sent = 0
        while True:
            claimed = await send_thread(q.claim_undelivered, limit=1)
            if not claimed:
                return sent
            entry = claimed[0]
            try:
                result = await self.send_prompt(name, entry["text"])
            except Exception:
                _log.exception("codex drain: falha ao entregar entry=%s name=%s", entry.get("id"), name)
                # CRITICAL: claim_undelivered ja marcou delivered=True (otimista). send_prompt
                # levantou (app-server morto/timeout/RuntimeError do JSON-RPC) -- sem reverter, a
                # entrada fica delivered=True pra sempre (nunca reenviada, bolha "queued-" eterna).
                # Mesmo tratamento do branch "deferred" abaixo.
                try:
                    await send_thread(q.set_delivered, entry["id"], False)
                except OSError:
                    pass
                return sent
            if result != "sent":
                # turno em curso / sessao indisponivel: reverte (nada foi enviado) e espera o proximo idle.
                try:
                    await send_thread(q.set_delivered, entry["id"], False)
                except OSError:
                    pass
                return sent
            sent += 1

    async def read_rate_limits(self, name: str) -> Optional[dict]:
        """Le os limites de uso da conta Codex via `account/rateLimits/read` (Task B). Devolve o
        `RateLimitSnapshot` cru (limitId/limitName/primary/secondary/credits/individualLimit/
        planType/rateLimitReachedType) ou None se a sessao nao tem client vivo/sidecar (mesmo
        contrato de ensure_running) ou se o app-server recusar o pedido -- nunca levanta, o
        endpoint trata None como "sem dado" em vez de derrubar a request."""
        try:
            client = await self.ensure_running(name)
        except Exception:
            _log.exception("codex read_rate_limits: ensure_running falhou name=%s", name)
            return None
        if client is None:
            return None
        try:
            result = await client.request("account/rateLimits/read", {})
        except Exception:
            _log.exception("codex read_rate_limits: request falhou name=%s", name)
            return None
        snapshot = result.get("rateLimits")
        if snapshot and snapshot.get("limitId") in (None, "codex"):
            # O cabeçalho usa a cota da conta, não a última cota específica de outro modelo.
            self._sessions[name]["rate_limits"] = snapshot
        return snapshot

    async def list_models(self, name: str) -> list[dict]:
        """Lista os modelos disponiveis pra sessao via `model/list` (Task C), normalizados:
        {model, displayName, description, efforts: [{value, description}], defaultEffort}.
        Filtra hidden=True (schema 0.141.0: `Model.hidden`). Mesmo contrato de nao-levantar de
        read_rate_limits -- lista vazia se a sessao nao tem client vivo/sidecar ou o app-server
        recusar o pedido."""
        try:
            client = await self.ensure_running(name)
        except Exception:
            _log.exception("codex list_models: ensure_running falhou name=%s", name)
            return []
        if client is None:
            return []
        try:
            result = await client.request("model/list", {})
        except Exception:
            _log.exception("codex list_models: request falhou name=%s", name)
            return []
        return [
            {
                "model": m.get("model"),
                "displayName": m.get("displayName"),
                "description": m.get("description"),
                "efforts": [
                    {"value": e.get("reasoningEffort"), "description": e.get("description")}
                    for e in (m.get("supportedReasoningEfforts") or [])
                ],
                "defaultEffort": m.get("defaultReasoningEffort"),
            }
            for m in (result.get("data") or [])
            if not m.get("hidden")
        ]

    async def set_model(self, name: str, model: Optional[str], effort: Optional[str]) -> None:
        """Atualiza a thread viva; futuros turnos herdam inclusive mudanças feitas no terminal."""
        client = await self.ensure_running(name)
        if client is None:
            raise RuntimeError("Sessão Codex indisponível")
        sess = self._sessions[name]
        await client.request("thread/settings/update", {
            "threadId": sess["thread_id"], "model": model, "effort": effort,
        })
        sess["model"], sess["effort"] = model, effort
        sess["default_effort"] = effort
        codex_sessions.update_model(name, model, effort)

    @staticmethod
    def _restore_turn(sess: dict, thread: dict, *, include_turns: bool = True) -> None:
        status = (thread.get("status") or {}).get("type")
        if status not in {"active", "idle"}:
            return
        sess["state"] = "working" if status == "active" else "idle"
        sess["in_progress"] = status == "active"
        if status == "idle":
            sess["turn_id"] = None
            sess.pop("codex_buffering", None)
            sess.pop("codex_response_started", None)
        elif include_turns:
            sess["turn_id"] = next((t.get("id") for t in reversed(thread.get("turns") or [])
                                    if t.get("status") == "inProgress"), None)
        sess["turn_state_known"] = not sess["in_progress"] or bool(sess.get("turn_id"))
        if sess["in_progress"]:
            sess["in_progress_since"] = time.monotonic()

    async def read_settings(self, name: str, *, include_turns: bool = False) -> dict:
        client = await self.ensure_running(name)
        if client is None:
            raise RuntimeError("Sessão Codex indisponível")
        sess = self._sessions[name]
        revision = sess.get("settings_revision", 0)
        state_revision = sess.get("state_revision", 0)
        result = await client.request("thread/read", {"threadId": sess["thread_id"], "includeTurns": include_turns})
        thread = result.get("thread") or {}
        # Uma notification recebida durante a leitura é mais recente que esse retrato.
        if state_revision == sess.get("state_revision", 0):
            self._restore_turn(sess, thread, include_turns=include_turns)
        if revision == sess.get("settings_revision", 0):
            if thread.get("model"):
                sess["model"] = thread["model"]
            if "reasoningEffort" in thread:
                sess["effort"] = sess["default_effort"] = thread["reasoningEffort"]
        # thread/read não informa o modo; o último turno pode anteceder uma troca pelo terminal.
        return {**self.current_model(name), "mode": sess.get("mode")}

    async def set_mode(self, name: str, mode: str) -> dict:
        if mode not in {"default", "plan"}:
            raise ValueError("Modo Codex inválido")
        current = await self.read_settings(name)
        if not current["model"]:
            raise RuntimeError("Modelo atual indisponível")
        sess = self._sessions[name]
        await sess["client"].request("thread/settings/update", {
            "threadId": sess["thread_id"], "collaborationMode": {"mode": mode, "settings": {
                "model": current["model"], "reasoning_effort": current["effort"],
                "developer_instructions": None,
            }},
        })
        sess["mode"] = mode
        return {**current, "mode": mode}

    async def compact(self, name: str) -> None:
        client = await self.ensure_running(name)
        if client is None:
            raise ValueError("Sessão Codex indisponível.")
        await self.read_settings(name)
        sess = self._sessions[name]
        if not sess.get("turn_state_known") or self._question_state(name, sess).state != "idle":
            raise ValueError("Espere o Codex terminar e responda às perguntas antes de compactar.")
        revision = sess.get("state_revision", 0)
        await client.request("thread/compact/start", {"threadId": sess["thread_id"]})
        # O RPC aceita antes dos eventos; impeça outro envio nesse intervalo.
        if sess.get("state_revision", 0) == revision:
            sess.update(in_progress=True, turn_state_known=False,
                        in_progress_since=time.monotonic(), state="working")

    async def list_skills(self, name: str) -> list[dict]:
        from .chat_controls import skills_do_catalogo
        client = await self.ensure_running(name)
        if client is None:
            raise RuntimeError("Sessão Codex indisponível")
        meta = codex_sessions.load(name) or {}
        params = {"cwds": [meta["cwd"]]} if meta.get("cwd") else {}
        return skills_do_catalogo(await client.request("skills/list", params))

    async def _user_input(self, name: str, text: str) -> list[dict]:
        inputs = [{"type": "text", "text": text}]
        if text.lstrip().startswith("/"):
            command = text.lstrip().split()[0][1:]
            skill = next((s for s in await self.list_skills(name) if s["name"] == command), None)
            if skill:
                inputs.append({"type": "skill", "name": skill["native_name"], "path": skill["path"]})
        return inputs

    async def steer(self, name: str, text: str, *, turn_id: str | None = None) -> None:
        if not text.strip():
            raise ValueError("A orientação não pode estar vazia")
        client = await self.ensure_running(name)
        sess = self._sessions.get(name) or {}
        expected = turn_id or await self._active_turn_id(name)
        if client is None or not expected or not sess.get("in_progress"):
            raise RuntimeError("Não há turno em andamento para orientar")
        await client.request("turn/steer", {
            "threadId": sess["thread_id"], "expectedTurnId": expected,
            "input": await self._user_input(name, text),
        })

    async def steer_queue(self, name: str, *, entry_id: str | None = None) -> list[str]:
        sess = self._sessions.get(name)
        async with self.delivery_lock(name):
            if self._sessions.get(name) is not sess:
                raise RuntimeError("A sessão mudou antes de orientar a fila")
            return await self._steer_queue(name, entry_id=entry_id)

    async def _steer_queue(self, name: str, *, entry_id: str | None = None) -> list[str]:
        await self.ensure_running(name)
        sess = self._sessions.get(name) or {}
        turn_id = await self._active_turn_id(name)
        if not turn_id or not sess.get("in_progress"):
            raise RuntimeError("Não há turno em andamento para orientar")
        q = PromptQueue(name)
        sent: list[str] = []
        while claimed := await send_thread(q.claim_undelivered, limit=1, entry_id=entry_id):
            entry = claimed[0]
            try:
                await self.steer(name, entry["text"], turn_id=turn_id)
            except BaseException:
                await send_thread(q.set_delivered, entry["id"], False)
                raise
            sent.append(entry["id"])
            # Fora do rollback: uma falha de gravação não desfaz o RPC já aceito.
            try:
                await send_thread(q.set_delivered, entry["id"], True, steered=True)
            except OSError:
                _log.exception("orientacao aceita, mas recibo indisponivel name=%s entry=%s", name, entry["id"])
                return sent
        return sent

    async def _active_turn_id(self, name: str) -> str | None:
        sess = self._sessions.get(name)
        if sess is None or not sess.get("in_progress"):
            return None
        if not sess.get("turn_id"):
            await self.read_settings(name, include_turns=True)
            sess = self._sessions.get(name)
        return sess.get("turn_id") if sess else None

    def current_model(self, name: str) -> dict:
        """Modelo/effort pra DISPLAY (pill do front): a escolha explicita do usuario tem
        prioridade; sem escolha, cai pro default da thread (dict quente, populado no attach) --
        so entao {model: None, effort: None} pra sessao nunca vista."""
        sess = self._sessions.get(name)
        if sess is not None and (sess.get("model") or sess.get("effort")
                                  or sess.get("default_model") or sess.get("default_effort")):
            return {"model": sess.get("model") or sess.get("default_model"),
                    "effort": sess.get("effort") or sess.get("default_effort")}
        meta = codex_sessions.load(name)
        if meta is not None:
            return {"model": meta.get("model"), "effort": meta.get("effort")}
        return {"model": None, "effort": None}

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None,
                      permission_mode: str | None = None,
                      initial_prompt: str | None = None,
                      codex_home: str | None = None,
                      codex_account: str | None = None) -> list[str]:
        # Sessao Codex nasce como as outras: um comando no pane. O comando e o lancador, que sobe o
        # app-server e a TUI juntos (ver comando_do_lancador).
        # session_id nao entra: a identidade da conversa e o threadId, que so existe depois que a
        # TUI chama thread/start — quem grava isso no sidecar e o lancador.
        # model/effort NAO viram flag aqui (nem passam por model_args.args_de): o esforco do Codex
        # nao e flag do binario, e a traducao dos dois e do lancador. permission_mode e do Claude.
        # `validar` continua sendo a barreira — o comando vira UMA string executada por `$SHELL -c`
        # (tmux.py), e sem esta chamada o Codex seria o unico provider cujo id nao passa por ela.
        from app import model_args
        model, effort = model_args.validar("codex", model, effort)
        return comando_do_lancador(cwd, initial_prompt, model=model, effort=effort,
                                   codex_home=codex_home, codex_account=codex_account)

    def transcript_path(self, cwd: str, session_id: str) -> str:
        # O rollout path vem do thread/start (result.thread.path), gravado no sidecar -- nao ha como
        # derivar do cwd+id como no Claude. Nunca chamado no caminho Codex.
        raise NotImplementedError("Codex obtem o rollout path via thread/start, nao por derivacao")
