"""Uma posse por chave durante cliente do cano, fila e recuperação."""
from __future__ import annotations

import asyncio
import contextvars
import copy
import errno
import json
import logging
import os
import sys
import re
import threading
import time
import uuid
from contextlib import asynccontextmanager, contextmanager, suppress
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

from app import runtime_queue

_log = logging.getLogger("hangar.runtime")

_current = None
_lifecycle = contextvars.ContextVar("runtime_lifecycle", default=None)
# Modo do processo: `pending` (Rust esperado, ainda sem desfecho), `rust` ou `python`.
_initial_mode = "python"
# Uma partida do Rust leva até 20 s (linha de pronto + saúde); passou disso, `runtime_starting`.
PENDING_WAIT_S = 30.0
_mode_bypass = contextvars.ContextVar("runtime_mode_bypass", default=False)


def current():
    return _current


def ensure():
    global _current
    if _current is None:
        _current = RuntimeCoordinator()
    return _current


def expect_rust(expected=True):
    """O processo sobe com o hangar-server: as sessões migradas esperam o desfecho dele."""
    global _initial_mode
    _initial_mode = "pending" if expected else "python"


def refuse_python_client(name, provider="claude", *, headless=True):
    """Com o Rust esperado ou dono, as sessões que ele anuncia no modo `headless` são dele: cliente Python
    nelas é defeito, inclusive para relançar (o relançamento é do Rust)."""
    if _current is not None and _current.mode in {"pending", "rust"} and _current.rust_owns(provider, headless):
        raise RuntimeError(f"cliente Python bloqueado em {name}: o Rust é o dono desta sessão")


def rust_owns_codex_terminal():
    return _current is not None and _current.mode != "python" and _current.rust_owns("codex", False)


def rust_codex_terminal(name, route, method="POST", body=b""):
    """Repasse num sentido, à ponte do Rust, do que entra pelo Python para um Codex com terminal dele
    (escrita, encerrar, troca de modo). `(status, corpo)`; `None` = não é dele (421), e o Python segue.
    Com o Rust dono, ponte sem endereço é 503 com código, nunca o Python assumindo."""
    if not rust_owns_codex_terminal():
        return None
    return _rust_bridge("codex", name, route, method, body)


def rust_claude(name, route, method="POST", body=b"", timeout=90, headers=None):
    """O mesmo repasse para a pergunta do Claude que entra pelo convite ou pelo Connect. Com o Rust de pé
    ou esperado, ponte fora é falha com código (503), nunca o Python respondendo; `None` só no 421.
    `headers`: os de quem pediu (cliente, pedido, origem), para o diário do Rust."""
    if _current is None or _current.mode == "python":
        return None
    return _rust_bridge("claude", name, route, method, body, timeout, headers)


def _rust_bridge(provider, name, route, method, body, timeout=90, headers=None):
    from app import list_bridge
    import http.client, urllib.parse
    endpoint = list_bridge.endpoint()
    status, raw, sent = 503, b"", False
    if endpoint is not None:
        conn = http.client.HTTPConnection(endpoint[0], timeout=timeout)
        try:
            conn.request(method, f"/__hangar_server/{provider}/{urllib.parse.quote(name, safe='')}/{route}", body=body or None,
                         headers={**(headers or {}), "content-type": "application/json", "x-hangar-internal": endpoint[1]})
            sent = True
            response = conn.getresponse()
            status, raw = response.status, response.read()
        except (OSError, http.client.HTTPException):  # recusa, prazo e conexão caída
            if sent and method not in ("GET", "HEAD"):
                # O pedido inteiro saiu: a escrita pode ter sido entregue, e repetir duplicaria.
                return 504, json.dumps({"detail": {"code": f"{provider}_bridge_uncertain", "params": {},
                    "msg": "O Rust recebeu o pedido e não respondeu; confira o resultado antes de repetir."}}).encode()
        finally:
            conn.close()
        if status == 421:
            return None
    with suppress(ValueError):
        parsed = json.loads(raw)
        # Sucesso é qualquer JSON (o `/commands` é lista); erro precisa do envelope do Rust.
        if status < 300 or (isinstance(parsed, dict) and isinstance(parsed.get("detail"), dict)):
            return status, raw
    # Ponte fora, prazo ou corpo sem o envelope do Rust: falha com código, nunca exceção crua.
    return max(status, 502), json.dumps({"detail": {"code": f"{provider}_bridge_failed", "params": {},
        "msg": "O Rust não respondeu ao repasse; tente de novo."}}).encode()


class Phase(Enum):
    Python = "python"
    Rust = "rust"
    RecoveringPython = "recovering_python"


class WriterLease:
    def __init__(self, path):
        path = path if isinstance(path, Path) else Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
        self.file = os.fdopen(fd, "r+b", buffering=0)
        try:
            if sys.platform == "win32":
                import ctypes
                import msvcrt

                class Overlapped(ctypes.Structure):
                    _fields_ = [("internal", ctypes.c_size_t), ("internal_high", ctypes.c_size_t),
                                ("offset", ctypes.c_uint32), ("offset_high", ctypes.c_uint32),
                                ("event", ctypes.c_void_p)]

                lock = ctypes.WinDLL("kernel32", use_last_error=True).LockFileEx
                lock.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32,
                                 ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
                lock.restype = ctypes.c_int
                overlapped = Overlapped()
                if not lock(msvcrt.get_osfhandle(fd), 3, 0, 0xFFFFFFFF, 0xFFFFFFFF, ctypes.byref(overlapped)):
                    error = ctypes.get_last_error()
                    if error == 33:
                        raise BlockingIOError(errno.EAGAIN, "outro responsável possui a sessão")
                    raise ctypes.WinError(error)
            else:
                import fcntl
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BaseException:
            self.file.close()
            raise

    def close(self):
        self.file.close()

    @property
    def closed(self):
        return self.file.closed


@dataclass
class Binding:
    name: str
    key: str
    provider: str
    headless: bool
    meta: dict
    jsonl: str
    projection_dir: Path
    state_path: Path
    lock_path: Path
    generation: int

    def descriptor(self):
        return {"name": self.name, "key": self.key, "provider": self.provider,
                "headless": self.headless, "meta": copy.deepcopy(self.meta), "jsonl": self.jsonl,
                "projection_dir": str(self.projection_dir), "state_path": str(self.state_path),
                "lock_path": str(self.lock_path), "generation": self.generation}


@dataclass
class Slot:
    binding: Binding
    phase: Phase = Phase.RecoveringPython
    lease: WriterLease | None = None
    store: runtime_queue.QueueStore | None = None
    view: dict = field(default_factory=dict)
    reserve_state: dict = field(default_factory=dict)   # vista salva que hidrata o cliente da reserva
    active: int = 0
    frozen: bool = False
    guard: threading.Lock = field(default_factory=threading.Lock)
    lifecycle: asyncio.Lock = field(default_factory=asyncio.Lock)
    changed: asyncio.Event = field(default_factory=asyncio.Event)
    cache_valid: bool = False
    lifecycle_token: object | None = None
    terminal_serial: asyncio.Lock = field(default_factory=asyncio.Lock)
    awaiting_identity: bool = False
    change: dict | None = None          # administração em curso: alvo, avanço, vida, pedido de subida
    change_from_rust: bool = False      # fechada no Rust pela administração, ainda sem reabrir


def _open_command(descriptor):
    """Claude sem terminal abre administrado (pasta do arquivo da sessão): o Rust religa o processo que cai."""
    if descriptor["provider"] == "claude" and descriptor["headless"]:
        from app.adapters.claude_headless import sessions
        descriptor = {**descriptor, "sidecar_dir": str(sessions._dir())}
    return {"kind":"open", "descriptor":descriptor}


def _clock():
    return {"monotonic_s": time.monotonic(), "epoch_s": time.time()}


def _cano_alive(meta) -> bool:
    from app.procinfo import pid_vivo
    pid = (meta.get("cano") or {}).get("pid")
    return pid is not None and pid_vivo(int(pid))


def _raise_site(exc):
    """`arquivo:linha função` de onde a exceção nasceu: aponta a frase sem copiar a mensagem."""
    tb, frame = exc.__traceback__, None
    while tb is not None:
        frame, tb = (tb.tb_frame.f_code, tb.tb_lineno), tb.tb_next
    return f"{Path(frame[0].co_filename).name}:{frame[1]} {frame[0].co_name}" if frame else ""


def failure_reason(exc: BaseException) -> dict:
    """Tipo, motivo e ponto do `raise` de uma falha do runtime para o diário. Só falhas do caminho
    Rust levam o detalhe: lá a mensagem é código e frase fixa. As do Python podem embutir texto da
    sessão; o `raise_site` diz qual delas foi sem copiar a mensagem."""
    from_rust = getattr(exc, "_hangar_rust", False) or type(exc).__name__ in {"RustOpError", "RustCacheInvalid"}
    plain = from_rust or getattr(exc, "safe_detail", False) or isinstance(exc, (TimeoutError, ConnectionError))
    code = getattr(exc, "code", "") if from_rust else ""
    return {"codigo": code or type(exc).__name__, "detalhe": str(exc)[:200] if plain else "", "raise_site": _raise_site(exc)}


# Erro do terminal que o próprio Rust resolve, sem reabrir; a vista segue valendo para operar.
_TERMINAL_PRE_EFFECT_ERRORS = frozenset({"terminal_facts", "receipt_scan"})
# Respostas normais do Rust ao pedido (botão velho, sessão ocupada, entrada inválida): não são
# defeito: sobem como erro sem ir ao diário de falha.
_ANSWER_CODES = frozenset({
    "claude_command", "codex_command", "lifecycle_required", "operation_reused", "input_text",
    "queue_busy", "queue_entry", "steer_unknown", "policy_refused", "erro_permissao_ocupada", "erro_modo_desconhecido",
    "erro_codex_reiniciando"})
_BIRTH_POLL_S = 0.25
_INITIALIZE_WAIT_S = 185.0   # teto do `initialize` no Rust (180 s) com folga
_RESYNC_WAIT_S = 5.0     # o canal de eventos oscilou: a vista volta pelo snapshot em instantes
_ACTIVE_WAIT_S = 5.0     # gravação da fila Python em curso na passagem ao Rust: leva milissegundos
# O ator sumiu do Rust (morreu ou nunca abriu nesta instância): a sessão reabre.
_ACTOR_GONE_CODES = frozenset({"runtime_binding", "runtime_closed", "runtime_panic"})
_BACKGROUND_KINDS = frozenset({"snapshot", "drain", "confirm", "queue"})


class TransferInProgress(RuntimeError):
    """A posse está passando entre Python e Rust: nada pode gravar na fila agora."""


class RustCacheInvalid(RuntimeError):
    """O Python perdeu a cópia do estado da sessão no Rust; nada foi enviado."""


class RuntimeStarting(RuntimeError):
    """O Rust caiu ou está subindo e não voltou dentro de `PENDING_WAIT_S`."""
    code = "runtime_starting"
    safe_detail = True


def _codex_mode(name):
    """`None` sem sessão Codex com esse nome; senão se ela é sem terminal."""
    from app.adapters.codex import sessions as codex_sessions
    meta = codex_sessions.load(name)
    return None if meta is None else bool(meta.get("headless"))


def _headless_provider(name):
    """Provedor da sessão sem terminal pelo arquivo dela: o do Codex mora em pasta própria."""
    from app.adapters.codex import sessions as codex_sessions
    return "codex" if (codex_sessions.load(name) or {}).get("headless") else "claude"


def _registration_failed(event, name, exc, *, rust_dead=False):
    """`rust_dead`: abertura no Rust com o processo dele já morto; só aí conexão caída ou recusada
    é queda. Com ele vivo, conexão caída, prazo ou resposta inválida são falha dele."""
    from app import diag
    reason = failure_reason(exc)
    if rust_dead and isinstance(exc, ConnectionError):
        # Queda do Rust durante a abertura: o próximo Rust ou a retomada pelo Python decide.
        _log.warning("abertura de %s interrompida pela queda do Rust (%s)", name, reason["codigo"])
        diag.registrar("runtime.reopen_interrupted", "aviso", sessao=name, etapa=event, **reason)
        return
    _log.error("%s: sessão %s (%s: %s) em %s", event, name, reason["codigo"], reason["detalhe"] or "-", reason["raise_site"] or "?")
    diag.registrar(event, "erro", sessao=name, **reason)


class RuntimeCoordinator:
    def __init__(self, transport=None, legacy=None):
        self.transport, self.legacy = transport, legacy
        self.instance = getattr(transport, "instance", None)
        self.slots: dict[str, Slot] = {}
        self.names: dict[str, str] = {}
        self.loop = None
        self.events_task = None
        self.refreshing = {}
        self.legacy_active = set()
        self.registration_locks = {}
        self.adoption_task = None
        self.rebindings = {}
        self.rebind_times = {}
        self.rebind_capped = set()
        self.drains = {}
        self.mode = _initial_mode
        self.mode_hooks = {}        # "rust"/"python" -> corrotina que o lifespan registra
        self._hooks_ran = set()
        self._settling = False      # entrando num modo: as sessões ainda abrindo ou voltando
        self._mode_event = None
        self.python_left = None     # ligado quando o Python solta as sessões ao Rust que volta
        # Sessões cuja recusa de fundo por cache inválido já foi ao diário nesta sequência.
        self._cache_invalid_logged: set[str] = set()
        # Antes da saúde do Rust vale o que se espera dele; depois, o que ele anuncia.
        self._owns = {("claude", True), ("claude", False), ("codex", True), ("codex", False)}

    def rust_owns(self, provider, headless):
        return (provider, headless) in self._owns

    def _set_mode(self, mode, *, settling=False):
        self.mode, self._settling = mode, settling
        event, self._mode_event = self._mode_event, None
        if event is not None:
            event.set()

    async def await_mode(self):
        """Espera o desfecho do Rust (subida, queda 1–2) até `PENDING_WAIT_S`; devolve o modo."""
        deadline = time.monotonic() + PENDING_WAIT_S
        while (self.mode == "pending" or self._settling) and not _mode_bypass.get():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeStarting("runtime Rust ainda subindo; tente de novo em instantes")
            if self._mode_event is None:
                self._mode_event = asyncio.Event()
            try:
                await asyncio.wait_for(self._mode_event.wait(), remaining)
            except TimeoutError:
                pass
        return self.mode

    async def _run_hook(self, mode):
        hook = self.mode_hooks.get(mode)
        if hook is None or mode in self._hooks_ran:
            return
        self._hooks_ran.add(mode)
        try:
            await hook()
        except Exception as exc:
            from app import diag
            diag.registrar("runtime.mode_hook_failed", "erro", etapa=mode, **failure_reason(exc))

    async def enter_pending(self):
        """O Rust caiu (queda 1 ou 2): as sessões ficam com ele, sem dono por segundos, até o
        próximo subir e abri-las de novo. Nada volta ao Python aqui."""
        self._set_mode("pending")       # antes da limpeza: ninguém usa o transporte que morreu
        await self.close_events()
        self.transport, self.instance = None, None
        for slot in tuple(self.slots.values()):
            if slot.phase == Phase.Rust:
                slot.cache_valid = False
                self._signal(slot)

    async def enter_python(self, containment=None):
        """O Rust desistiu: o Python é o dono até o backend reiniciar. Cada sessão do Rust é
        retomada uma vez; depois roda o que o lifespan faria sem o Rust."""
        self._set_mode("python", settling=True)
        self.python_left = asyncio.Event()
        token = _mode_bypass.set(True)
        try:
            for slot in tuple(self.slots.values()):
                if (self.names.get(slot.binding.name) != slot.binding.key or slot.phase == Phase.Python
                        or slot.awaiting_identity):
                    continue
                try:
                    await self.recover(slot.binding.name, confirmed_dead=True, containment=containment)
                except Exception as exc:
                    # Uma sessão que não volta fica suspensa sozinha; a porta e as outras seguem.
                    _registration_failed("runtime.recover_failed", slot.binding.name, exc)
            await self._run_hook("python")      # registra e religa antes de liberar quem espera
        finally:
            _mode_bypass.reset(token)
            self._set_mode("python")

    async def leave_python(self):
        """Religar o Rust: cada sessão sai do Python pela passagem normal (operação em curso termina,
        `quiesce`, trava solta) e fica como numa queda. Exceção = quem chama devolve tudo ao Python."""
        adapters = self.legacy.adapters if self.legacy is not None else {}
        codex = adapters.get("codex")
        if any(sess["client"].tem_processo_proprio for sess in getattr(codex, "_sessions", {}).values()):
            raise RuntimeError("Codex com processo do Python: soltar o cliente mataria o processo")
        self._set_mode("pending")
        self._hooks_ran.discard("python")       # nova desistência religa os clientes de novo
        if self.python_left is not None:
            self.python_left.set()              # os SSE com estado do Python fecham e reconectam no Rust
        for slot in tuple(self.slots.values()):
            if self.names.get(slot.binding.name) != slot.binding.key or slot.phase != Phase.Python:
                continue
            async with self._barrier(slot):
                await asyncio.wait_for(self._wait_active(slot), _ACTIVE_WAIT_S)
                await self.legacy.quiesce(slot.binding.descriptor())
                with slot.guard:
                    if slot.lease is not None:
                        slot.lease.close()
                    slot.lease, slot.store, slot.phase, slot.cache_valid = None, None, Phase.Rust, False
            self._signal(slot)
        # Cliente sem registro (cano religado só para ver o estado): basta fechar a ligação.
        if (claude := adapters.get("claude")) is not None:
            claude.desligar_todas()
            claude._sessions.clear()
        for name in list(getattr(codex, "_sessions", ())):
            await codex.release_client(name)

    async def _close_ingress_of_incomplete_transfers(self):
        """O Rust novo nasce com as portas abertas: troca interrompida precisa delas fechadas até terminar."""
        from app import conversation_transfer
        for record in await asyncio.to_thread(conversation_transfer.list_incomplete):
            try:
                await self.ingress(record.name, True, held=True)
                conversation_transfer.hold_gate(record.name)
            except Exception as exc:
                _registration_failed("runtime.ingress_failed", record.name, exc, rust_dead=not self._rust_alive())

    async def _enter_rust(self):
        """Rust novo de pé: ele adota sozinho na subida o que é dele (canos vivos, fila pendente, terminais
        com fila); os registros daqui dessas sessões saem e quem esperava neles relê e acha o do Rust. Só
        então o modo vira `rust`."""
        token = _mode_bypass.set(True)
        cancelled = False
        try:
            await self._close_ingress_of_incomplete_transfers()
            for slot in tuple(self.slots.values()):
                name = slot.binding.name
                if self.names.get(name) != slot.binding.key or slot.phase != Phase.Rust:
                    continue
                self._signal(slot)
                self.slots.pop(slot.binding.key, None)
                self.names.pop(name, None)
        except asyncio.CancelledError:
            cancelled = True        # o Rust caiu no meio: quem decide o modo agora é a queda
            raise
        finally:
            _mode_bypass.reset(token)
            if not cancelled:
                self._set_mode("rust")
        await self._run_hook("rust")

    async def prepare_session(self, name, provider, *, launch=False):
        """`launch`: quem chama pode subir o processo (envio, acordar); leitura e parada nunca sobem."""
        if self.legacy is None:
            return self.managed_runtime(name)
        if provider == "claude" or self.rust_owns(provider, bool(await asyncio.to_thread(_codex_mode, name))):
            await self.await_mode()
        async with self.registration_locks.setdefault(name, asyncio.Lock()):
            binding = await asyncio.to_thread(self.legacy.binding, name, provider)
            if binding is None and provider == "claude":
                binding = await self._await_birth(name, provider)
            if binding is None:
                from app.runtime_terminal import outside_scope
                if self.managed_runtime(name) or provider == "claude" and not await asyncio.to_thread(outside_scope, name):
                    raise RuntimeError("vínculo gerenciado indisponível; escrita suspensa")
                return False
            if self.managed_queue(name) and self.slot(name).binding.key != binding.key:
                previous = self.slot(name)
                if previous.awaiting_identity:
                    # Registro em espera nunca teve posse, fila nem dono no Rust: não há o que soltar.
                    self.names.pop(name, None)
                else:
                    async with self.freeze(name):
                        if previous.phase != Phase.Python:
                            await self.detach(name, restore=False)
                        previous.lease.close()
                        previous.lease = None
                        previous.phase = Phase.RecoveringPython
                        self.names.pop(name, None)
            slot = self.slots.get(binding.key)
            if self._born_in_rust(binding) and (slot is None or slot.phase == Phase.Python):
                alive = await asyncio.to_thread(_cano_alive, binding.meta)
                # Registro Python de sessão Claude sem terminal nunca tem cliente no cano com o Rust
                # de pé (`refuse_python_client`): a fila passa direto ao Rust.
                if launch or alive:
                    if slot is None or await self._release_python_slot(name, slot):
                        await self._open_headless(name, binding, launch=launch)
                    return True
            if slot is None and self.transport is not None and binding.meta.get("terminal"):
                await self._open_terminal(name, binding)    # nasce no Rust, sem fase Python
                return True
            if slot is None:
                slot = await asyncio.to_thread(self.register, binding)
            if slot is None or not self.managed_runtime(name):
                return False
            if slot.awaiting_identity:
                async with self.freeze(name):
                    from app.runtime_process import reconcile_startup
                    from app.runtime_terminal import validate_binding
                    await asyncio.to_thread(reconcile_startup, allow_current=True)
                    await asyncio.to_thread(validate_binding, binding.descriptor())
                    if slot.store is not None or slot.lease is not None or slot.phase != Phase.RecoveringPython:
                        raise RuntimeError("registro aguardando identidade já possui responsável")
                    await self._restore(slot, reconnect=False)
                    slot.awaiting_identity = False
            if slot.frozen or slot.phase not in {Phase.Python, Phase.Rust}:
                raise RuntimeError("sessão em transferência; aguarde a confirmação")
            agent_changed = binding.meta.get("terminal") and any(binding.meta.get(field) != slot.binding.meta.get(field)
                for field in ("agent_pid", "agent_birth"))
            if binding.jsonl != slot.binding.jsonl or agent_changed:
                async def changed():
                    return None
                field = "session_id" if binding.provider == "claude" else "thread_id"
                await self.change(name, changed, advance=bool(agent_changed) or binding.meta.get(field) != slot.binding.meta.get(field), reopen=False)
                slot = self.slot(name)      # o pane renascido com outra conversa troca o registro
                binding = slot.binding
                await self._remember_terminal_customizations(binding)
            if slot.phase == Phase.Python:
                with slot.guard:
                    slot.binding.meta = binding.meta
                    descriptor = slot.binding.descriptor()
                    # Conta como operação em curso: a posse não troca de mãos durante o fsync.
                    slot.active += 1
                store = slot.store
                def persist_binding():
                    # O fsync espera o disco: fora do laço e fora da trava do slot.
                    with store._lock:
                        state = copy.deepcopy(store.state)
                        state["runtime_state"]["_binding"] = descriptor
                        store._persist(state)
                try:
                    await asyncio.to_thread(persist_binding)
                finally:
                    with slot.guard:
                        slot.active -= 1
                    self._signal(slot)
                if self.transport is not None and binding.meta.get("terminal"):
                    # Registro Python do terminal (vínculo pendente que provou a conversa): vai ao Rust.
                    await self._open_slot_in_rust(name, slot, launch=False)
            return True

    async def _open_terminal(self, name, binding):
        from app.runtime_terminal import validate_binding
        from app import diag
        from app.rust_server import RustOpError
        self.loop = asyncio.get_running_loop()
        descriptor, sent = binding.descriptor(), False
        try:
            await asyncio.to_thread(validate_binding, descriptor)
            await asyncio.to_thread(self._prepare_queue_file, binding)
            sent = True
            ready = await self._rpc(descriptor, _open_command(descriptor), uuid.uuid4().hex)
            self._check_opened(ready, descriptor)
        except Exception as exc:
            diag.registrar("runtime.open_failed", "erro", sessao=name, **failure_reason(exc))
            if sent and not isinstance(exc, RustOpError):
                # Resposta perdida ou recusada aqui: o Rust pode ter aberto, e ninguém o fecharia.
                await self._close_unconfirmed(name, descriptor)
            raise
        slot = Slot(binding=copy.deepcopy(binding), phase=Phase.Rust, view=ready["state"], cache_valid=True)
        runtime_queue.configure(self)
        self.slots[binding.key], self.names[name] = slot, binding.key
        self._signal(slot)
        return slot

    def _prepare_queue_file(self, binding):
        """Fila do terminal pronta para o Rust: importação única da fila antiga por nome, geração
        alinhada e vínculo gravado, com a trava tomada e solta antes do `open`."""
        from app.runtime_process import reconcile_startup
        reconcile_startup(allow_current=True)
        try:
            lease = WriterLease(binding.lock_path)
        except BlockingIOError:
            # O Rust já adotou a sessão e é dono da fila; o `open` dele reconhece a mesma vida (ou recusa a outra).
            from app import diag
            diag.registrar("runtime.queue_prepare_skipped", "aviso", sessao=binding.name, codigo="queue_lock_held")
            return
        try:
            rows = []
            projection = binding.projection_dir / f"{self._sanitize(binding.name)}.jsonl"
            if not binding.state_path.exists() and projection.exists():
                from app.runtime_terminal import import_legacy
                rows = import_legacy(self, binding, projection)
            store = runtime_queue.QueueStore(binding.state_path, binding.projection_dir,
                runtime_queue.initial_state(binding.key, binding.generation, binding.name, rows))
            state = copy.deepcopy(store.state)
            if state["generation"] != binding.generation:
                if binding.generation < state["generation"]:
                    raise ValueError("a geração não pode voltar para uma vida antiga")
                state["generation"] = binding.generation
                state["runtime_state"] = {key:value for key,value in state["runtime_state"].items() if key == "terminal_write_barrier"}
            state["runtime_state"]["_binding"] = binding.descriptor()
            store._persist(state)
        finally:
            lease.close()

    def _born_in_rust(self, binding):
        # Sem terminal só; o terminal tem caminho próprio mesmo quando o Rust é dono dele.
        return self.transport is not None and binding.headless and self.rust_owns(binding.provider, True)

    async def ensure_open(self, name, *, wait_initialized=False):
        """Sessão sem terminal aberta no Rust, que é o único cliente do cano: ele sobe o processo,
        com ambiente e sidecar montados por ele. Serializado por nome com o `prepare_session`."""
        if self.legacy is None or self.transport is None:
            raise RuntimeError("runtime Rust indisponível")
        provider = (self.slot(name).binding.provider if self.managed_queue(name)
                    else await asyncio.to_thread(_headless_provider, name))
        if not await self.prepare_session(name, provider, launch=True):
            raise RuntimeError("sessão sem terminal sem sidecar")
        slot = self.slot(name)
        if slot.phase != Phase.Rust:
            raise RuntimeError("sessão registrada fora do Rust")
        if wait_initialized:
            await self._await_initialized(slot)
        return slot

    async def _release_python_slot(self, name, slot):
        adapter = self.legacy.adapters[slot.binding.provider]
        if slot.binding.provider == "codex":
            # Cliente ligado antes de o Rust anunciar o Codex: só a ligação fecha, o processo segue.
            await adapter.release_client(name)
        elif name in adapter._sessions:
            raise RuntimeError("cliente Python ainda subindo nesta sessão; tente de novo")
        async with self.freeze(name):
            with slot.guard:
                if slot.phase == Phase.Rust:
                    return False        # uma administração que esperava a barreira já reabriu
                if slot.active or slot.phase != Phase.Python:
                    raise RuntimeError("fila da sessão em uso no Python")
                if slot.lease is not None:
                    slot.lease.close()
                slot.lease, slot.store = None, None
        self.slots.pop(slot.binding.key, None)
        self.names.pop(name, None)
        return True

    async def _open_headless(self, name, binding, *, launch=True):
        binding, ready = await self._launch_and_open(name, launch=launch, provider=binding.provider)
        slot = Slot(binding=copy.deepcopy(binding), phase=Phase.Rust, view=ready["state"], cache_valid=True)
        runtime_queue.configure(self)
        self.slots[binding.key], self.names[name] = slot, binding.key
        self._signal(slot)
        return slot

    async def _launch_and_open(self, name, *, launch=True, provider="claude"):
        """Abre no Rust, que sobe o processo do cano pelo spec da sessão se preciso (nunca com o gravado
        vivo e dele) e grava o sidecar."""
        if provider == "codex":
            return await self._open_codex(name, launch=launch)
        from app import diag
        from app.adapters.claude_headless import sessions as claude_sessions
        from app.rust_server import RustOpError
        binding = await asyncio.to_thread(self.legacy.binding, name, "claude")
        if binding is None or not binding.headless:
            raise RuntimeError("sessão sem sidecar")
        descriptor = binding.descriptor()
        command = {"kind":"open", "descriptor":{**descriptor, "sidecar_dir":str(claude_sessions._dir())}, "launch":bool(launch)}
        try:
            ready = await self._rpc(descriptor, command, uuid.uuid4().hex)
            self._check_opened(ready, descriptor)
            opened = await asyncio.to_thread(self.legacy.binding, name, "claude")
            if opened is None or opened.key != binding.key:
                raise RuntimeError("sidecar mudou durante a abertura da sessão")
        except Exception as exc:
            diag.registrar("runtime.open_failed", "erro", sessao=name, **failure_reason(exc))
            if not isinstance(exc, RustOpError):
                # Resposta perdida ou recusada aqui: o Rust pode ter aberto, e ninguém o fecharia.
                await self._close_unconfirmed(name, descriptor)
            raise
        return opened, ready

    async def _open_codex(self, name, *, launch):
        """Codex sem terminal: quem sobe o processo é o Rust (`launch`, só se o gravado não for dele);
        ambiente e arquivo da sessão também são dele (`codex_launch.rs`, `codex_sidecar.rs`)."""
        from app import diag
        from app.adapters.codex import sessions as codex_sessions
        from app.rust_server import RustOpError
        binding = await asyncio.to_thread(self.legacy.binding, name, "codex")
        if binding is None or not binding.headless:
            raise RuntimeError("sessão sem sidecar")
        descriptor = binding.descriptor()
        command = {"kind":"open", "descriptor":{**descriptor, "sidecar_dir":str(codex_sessions._dir())}, "launch":bool(launch)}
        # As políticas da subida conferem a posse do Rust pela chave já durante o `open`.
        slot = self.slots.get(binding.key)
        temporary = slot is None
        if temporary:
            slot = self.slots[binding.key] = Slot(binding=copy.deepcopy(binding), phase=Phase.Rust)
        with slot.guard:
            previous, slot.phase = slot.phase, Phase.Rust
            slot.binding.meta = copy.deepcopy(binding.meta)     # o ambiente sai do modo gravado agora
        try:
            ready = await self._rpc(descriptor, command, uuid.uuid4().hex)
            self._check_opened(ready, descriptor)
            # O Rust gravou o cano novo no arquivo da sessão: o registro passa a apontá-lo.
            opened = await asyncio.to_thread(self.legacy.binding, name, "codex")
            if opened is None or opened.key != binding.key:
                raise RuntimeError("sidecar mudou durante a abertura da sessão")
        except Exception as exc:
            diag.registrar("runtime.open_failed", "erro", sessao=name, **failure_reason(exc))
            if not isinstance(exc, RustOpError):
                # Resposta perdida ou recusada aqui: o Rust pode ter aberto, e ninguém o fecharia.
                await self._close_unconfirmed(name, descriptor)
            with slot.guard:
                if temporary and self.slots.get(binding.key) is slot:
                    self.slots.pop(binding.key, None)
                slot.phase = previous
            raise
        return opened, ready

    async def _await_initialized(self, slot, timeout=_INITIALIZE_WAIT_S):
        deadline = time.monotonic() + timeout
        while True:
            slot.changed.clear()
            view = (slot.view or {}).get("view") or {}
            state = view.get("public_state") or {}
            if view.get("initialized") is True:
                return
            if state.get("problema") == "headless_nao_subiu" or view.get("alive") is False:
                raise RuntimeError(state.get("problema_detalhe") or "a sessão não concluiu a inicialização")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError("a sessão não concluiu a inicialização a tempo")
            try:
                await asyncio.wait_for(slot.changed.wait(), min(remaining, 1.0))
            except TimeoutError:
                pass

    async def _await_birth(self, name, provider):
        # Sessão com terminal recém-criada não é vínculo perdido: o envio espera o agente provar a
        # conversa, como a espera da TUI fazia antes. Suspensão fica para a vida que já tinha vínculo.
        from app import runtime_terminal
        after = 0
        if self.managed_queue(name):
            after = (self.slot(name).binding.meta.get("terminal") or {}).get("created") or 0
        waited = False
        while await asyncio.to_thread(runtime_terminal.being_born, name, after):
            waited = True
            await asyncio.sleep(_BIRTH_POLL_S)
            binding = await asyncio.to_thread(self.legacy.binding, name, provider)
            if binding is not None:
                return binding
        if waited:
            from app import diag
            diag.registrar("runtime.birth_wait_expired", "aviso", sessao=name)
        return None

    async def start_sessions(self, adapters):
        from app.runtime_process import reconcile_startup
        await asyncio.to_thread(reconcile_startup)
        from app.runtime_adapter import LegacyBridge
        self.loop = asyncio.get_running_loop()
        self.legacy = LegacyBridge(self, adapters)
        # Com o Rust esperado as sessões Claude esperam por ele: nada de trava, fila ou cliente
        # Python antes do desfecho. O que o Rust não anuncia como dele fica com o Python.
        if self.mode == "python":
            await self.register_claude_sessions()
        else:
            await self._register_durable_terminals(owned=False)
        if self.mode == "python" or not self.rust_owns("codex", True):
            from app.adapters.codex import sessions as codex_sessions
            await self._prepare_listed("codex", await asyncio.to_thread(codex_sessions.list_all))

    async def _prepare_listed(self, provider, metas):
        for meta in metas:
            if meta.get("headless"):
                try:
                    await self.prepare_session(meta["name"], provider)
                except Exception as exc:
                    _registration_failed("runtime.registration_failed", meta["name"], exc)

    async def register_claude_sessions(self):
        """Sessões Claude do estado durável. Python dono: registra todas nele. Rust de pé: ele adota
        sozinho as com e sem terminal na subida."""
        if self.transport is not None:
            return
        await self._register_durable_terminals(owned=None)
        from app.adapters.claude_headless import sessions as claude_sessions
        await self._prepare_listed("claude", await asyncio.to_thread(claude_sessions.list_all))

    async def _register_durable_terminals(self, *, owned):
        """Registros de terminal do estado durável da fila: `owned` True só os de provedor que o Rust
        atende, False só os do Python, None todos."""
        from app.pqueue import _queue_dir
        terminals = []
        for path in (_queue_dir() / "runtime").glob("*.json"):
            try:
                state = await asyncio.to_thread(lambda: json.loads(path.read_bytes()))
            except (OSError, ValueError) as exc:
                _registration_failed("runtime.registration_failed", path.stem, exc)
                continue
            descriptor = state.get("runtime_state", {}).get("_binding")
            if owned is not None and descriptor and self.rust_owns(descriptor.get("provider"), False) != owned:
                continue
            if descriptor and not descriptor.get("headless") and descriptor.get("key") not in self.slots:
                values = {**descriptor, "generation":state["generation"]}
                for field in ("projection_dir", "state_path", "lock_path"):
                    values[field] = Path(values[field])
                binding = Binding(**values)
                if binding.meta.get("terminal"):
                    from app.runtime_terminal import resolve_binding
                    fresh = await asyncio.to_thread(resolve_binding, binding.name, binding)
                    if fresh is None or fresh.key != binding.key:
                        self.slots[binding.key] = Slot(binding=binding)
                        if fresh is None:
                            from app import diag, tmux
                            exists = await asyncio.to_thread(tmux.sessao_existe, binding.name)
                            born = await asyncio.to_thread(tmux.session_created, binding.name) if exists else 0.0
                            # Sessão tmux de outra vida com o mesmo nome (fechada e recriada, até em
                            # outro provedor): o registro morto não reserva o nome, senão a nova fica sem dados.
                            recorded = binding.meta["terminal"].get("created")
                            if born and recorded and born != recorded:
                                diag.registrar("runtime.stale_terminal_record", "ok", sessao=binding.name)
                                continue
                            self.slots[binding.key].awaiting_identity = True
                            self.names.setdefault(binding.name, binding.key)
                            # Sem sessão tmux com o nome é sessão fechada (o estado da fila fica no
                            # disco); falha é haver sessão sem vínculo provado, ou o tmux não responder.
                            if exists is not False:
                                diag.registrar("runtime.registration_failed", "erro", sessao=binding.name, codigo="terminal_binding")
                            continue
                    else:
                        fresh.generation += int(fresh.jsonl != binding.jsonl)
                        fresh.meta["terminal"]["generation"] = fresh.generation
                    binding = fresh
                try:
                    await asyncio.to_thread(self.register, binding)
                except Exception as exc:
                    _registration_failed("runtime.registration_failed", binding.name, exc)
                    continue
                if binding.meta.get("terminal"):
                    terminals.append(binding)
        return terminals

    async def native_receipt(self, message_id, status):
        # Com o Rust de pé os recados da fila saem pela caixa de entrada dele, e é lá que o recibo volta;
        # ler o arquivo de estado aqui, enquanto o Rust o troca, falharia no Windows.
        if self.mode != "python":
            return False
        for slot in tuple(self.slots.values()):
            if self.names.get(slot.binding.name) != slot.binding.key:
                continue
            state = await asyncio.to_thread(lambda: json.loads(slot.binding.state_path.read_bytes()))
            for operation_id, operation in state["operations"].items():
                if operation["payload"].get("kind") not in {"input", "steer"}:
                    continue
                root_id = operation.get("entry_id") or operation_id
                if slot.binding.meta.get("terminal"):
                    result = operation.get("result") or {}
                    delivery = result.get("payload") or {}
                    if (delivery.get("native") is not True or delivery.get("message_id") != message_id
                            or operation["payload"].get("payload", {}).get("_terminal_generation") != slot.binding.generation):
                        continue
                expected = str(uuid.uuid5(uuid.NAMESPACE_URL, "hangar:" + slot.binding.key + ":" + root_id))
                if message_id != expected:
                    continue
                disposition = "accepted" if status in {"delivered", "released", ""} else "rejected" if status in {"rejected", "refused"} else "unknown"
                await self.op(slot.binding.name, {"kind":"queue", "action":{"kind":"finish", "id":root_id,
                    "status":disposition, "result":{"operation_id":root_id, "disposition":disposition,
                        "payload":{"native_status":status}}}}, "native-receipt:" + message_id + ":" + status)
                if disposition == "accepted":
                    await self.op(slot.binding.name, {"kind":"confirm"}, uuid.uuid4().hex)
                return True
        return False

    def configure_transport(self, transport, owns=None):
        if self.events_task is not None and not self.events_task.done():
            raise RuntimeError("leitor privado anterior ainda ativo")
        if owns is not None:
            self._owns = {(item["provider"], item["headless"]) for item in owns}
        self.transport, self.instance = transport, transport.instance
        self.loop = asyncio.get_running_loop()
        self.events_task = self.loop.create_task(self._events(transport, transport.instance))
        if self.legacy is not None:
            self._set_mode("rust", settling=True)
            self.adoption_task = self.loop.create_task(self._enter_rust())
        else:
            self._set_mode("rust")

    async def close_events(self):
        tasks = [task for task in [self.events_task, self.adoption_task, *self.refreshing.values(), *self.rebindings.values()] if task is not None]
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        self.events_task = None
        self.refreshing.clear()

    async def refresh_snapshot(self, name):
        from app.runtime_adapter import apply_event
        slot = self.slot(name)
        descriptor, instance = slot.binding.descriptor(), self.instance
        data = await self._rpc(descriptor, {"kind":"snapshot"}, uuid.uuid4().hex)
        if (self.instance != instance or slot.binding.generation != descriptor["generation"]
                or slot.phase != Phase.Rust):
            return False
        event = {"key":descriptor["key"], "generation":descriptor["generation"], "revision":data.get("revision"), "channel":"snapshot", "data":data}
        valid = apply_event(slot, event)
        self._signal(slot)
        return valid

    def _refresh(self, slot):
        key = slot.binding.key
        if key in self.refreshing and not self.refreshing[key].done():
            return
        async def refresh():
            try:
                await self.refresh_snapshot(slot.binding.name)
            except Exception as exc:
                slot.cache_valid = False
                self._signal(slot)
                from app import diag
                diag.registrar("runtime.refresh_failed", "erro", sessao=slot.binding.name, **failure_reason(exc))
        self.refreshing[key] = asyncio.create_task(refresh())

    def request_drain(self, name, kind="drain"):
        slot = self.slots.get(self.names.get(name, ""))
        if slot is None or slot.binding.key in self.drains and not self.drains[slot.binding.key].done():
            return
        async def drain():
            try:
                await self.op(name, {"kind":kind}, uuid.uuid4().hex)
            except RuntimeStarting:
                return          # o Rust novo drena ao abrir a sessão; a vista não está errada
            except Exception as exc:
                slot.cache_valid = False
                self._signal(slot)
                if isinstance(exc, RustCacheInvalid):
                    return      # o `op` já registrou a recusa, uma vez por sequência
                from app import diag
                diag.registrar("runtime.drain_failed", "erro", sessao=name, **failure_reason(exc))
        self.drains[slot.binding.key] = self.loop.create_task(drain())

    async def _events(self, transport, instance):
        from app.runtime_adapter import apply_event
        delay = 0.25
        while self.transport is transport and self.instance == instance:
            try:
                async for event in transport.events():
                    if self.transport is not transport or self.instance != instance:
                        return
                    if not isinstance(event, dict) or not isinstance(event.get("key"), str):
                        raise ValueError("evento privado inválido")
                    slot = self.slots.get(event["key"])
                    if slot is None or slot.phase != Phase.Rust:
                        continue
                    if type(event.get("generation")) is int and event["generation"] > slot.binding.generation:
                        self._follow_rust_life(slot, event)
                    if type(event.get("generation")) is int and event["generation"] != slot.binding.generation:
                        continue
                    previous = slot.view.get("revision", -1)
                    if not apply_event(slot, event) or event.get("channel") == "problem":
                        # `problem` também pode ser cosmético (política que falhou): o snapshot diz
                        # se o ator está mesmo em erro, e some com a faixa se não estiver.
                        slot.cache_valid = False
                        self._refresh(slot)
                    elif event.get("revision", -1) > previous or event.get("channel") == "snapshot":
                        if event["channel"] == "rate":
                            from app.live_rate import first_response_report, live_rate, rate_report
                            if (first := first_response_report(event["data"])) is not None:
                                live_rate(slot.binding.name).first_response(*first)
                            elif (report := rate_report(event["data"])) is not None:
                                live_rate(slot.binding.name).close(*report)
                            else:
                                from app import diag
                                diag.registrar("runtime.rate_invalid", "aviso", sessao=slot.binding.name)
                    self._signal(slot)
                    if event.get("channel") in {"view", "snapshot"}:
                        conversation = (slot.view.get("view") or {}).get("conversation")
                        field = "session_id" if slot.binding.provider == "claude" else "thread_id"
                        if conversation and conversation != slot.binding.meta.get(field):
                            self._rebind(slot, conversation)
                    if slot.binding.meta.get("terminal") and slot.view.get("error"):
                        self.request_drain(slot.binding.name, "confirm" if slot.view["error"] == "receipt_scan" else "drain")
                    delay = 0.25
                raise RuntimeError("stream privado encerrado sem aviso")
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                from app import diag
                diag.registrar("runtime.events_interrupted", "aviso", ms=int(delay * 1000), **failure_reason(exc))
                for slot in tuple(self.slots.values()):
                    if slot.phase == Phase.Rust:
                        slot.cache_valid = False
                        self._signal(slot)
            await asyncio.sleep(delay)
            delay = min(5.0, delay * 2)

    async def _remember_terminal_customizations(self, binding):
        settings = binding.meta.get("claude_settings")
        if not binding.meta.get("terminal") or not settings:
            return
        from app import claude_customizations, diag
        try:
            await asyncio.to_thread(claude_customizations.remember, binding.meta["session_id"], settings)
        except claude_customizations.CustomizationsError as exc:
            diag.registrar("claude.customizations_not_saved", "aviso", sessao=binding.name, codigo=exc.code)

    def _follow_rust_life(self, slot, event):
        """O Rust avança a vida sozinho (`/clear`, conversa nova, reabertura em bypass). O Python
        segue a geração dele; com a velha, fechar, renomear e trocar de conta seriam recusados."""
        data = event.get("data") if isinstance(event.get("data"), dict) else {}
        conversation = data.get("conversation") if event.get("channel") in {"reset", "view"} else (data.get("view") or {}).get("conversation")
        with slot.guard:
            binding = copy.deepcopy(slot.binding)
            binding.generation = event["generation"]
            terminal = binding.meta.get("terminal")
            if isinstance(terminal, dict):
                terminal["generation"] = binding.generation
            if binding.provider == "claude" and isinstance(conversation, str) and conversation:
                binding.meta["session_id"] = conversation
                if isinstance(terminal, dict):
                    terminal["conversation"] = conversation
                if binding.jsonl:
                    binding.jsonl = str(Path(binding.jsonl).with_name(f"{conversation}.jsonl"))
            slot.binding, slot.cache_valid = binding, False
        from app import diag
        diag.registrar("runtime.followed_rust_life", "ok", sessao=binding.name)
        self._refresh(slot)
        if isinstance(binding.meta.get("terminal"), dict) and self.loop is not None:
            self.loop.create_task(self._follow_terminal_pane(slot, binding.generation))

    async def _follow_terminal_pane(self, slot, generation):
        """Reabertura em bypass troca o pane e o processo: relê o vínculo mantendo a chave da sessão."""
        from app.runtime_terminal import pending_binding, resolve_binding
        def resolve():
            pending = pending_binding(slot.binding.name, slot.binding)
            if pending is None:
                return None
            pending.meta.pop("terminal", None)
            return resolve_binding(pending.name, pending)
        try:
            fresh = await asyncio.to_thread(resolve)
        except Exception as exc:
            from app import diag
            diag.registrar("runtime.follow_pane_failed", "aviso", sessao=slot.binding.name, **failure_reason(exc))
            return
        with slot.guard:
            if (fresh is None or fresh.key != slot.binding.key or slot.binding.generation != generation
                    or fresh.meta.get("session_id") != slot.binding.meta.get("session_id")):
                return
            fresh.generation = generation
            fresh.meta["terminal"]["generation"] = generation
            slot.binding = fresh

    def _rebind(self, slot, conversation):
        key = slot.binding.key
        # Thread nova do Codex é do Rust (sidecar e vida dele): só o Claude religa por aqui.
        if slot.binding.provider != "claude" or key in self.rebindings and not self.rebindings[key].done():
            return
        async def rebind():
            async def changed():
                return None
            from app import diag
            from app.adapters.claude_headless import sessions as claude_sessions
            # Religar relê o arquivo da sessão: antes de a conversa chegar nele, a vida nova nasceria sem
            # ela e abriria outra. A vista volta quando o patch grava, e aí religa.
            current = await asyncio.to_thread(claude_sessions.load, slot.binding.name)
            if (current or {}).get("session_id") != conversation:
                return
            # Conversa que muda a cada vida religaria sem parar: teto de 3 religações por minuto.
            now = time.monotonic()
            recent = [t for t in self.rebind_times.get(key, ()) if now - t < 60]
            if len(recent) >= 3:
                if key not in self.rebind_capped:
                    self.rebind_capped.add(key)
                    diag.registrar("runtime.rebind_loop", "erro", sessao=slot.binding.name, codigo="rebind_loop")
                return
            self.rebind_capped.discard(key)
            self.rebind_times[key] = recent + [now]
            try:
                async with self._ingress_closed(slot.binding.name):
                    slot.frozen = True
                    await self.change(slot.binding.name, changed)
                await self._remember_terminal_customizations(slot.binding)
            except Exception as exc:
                slot.cache_valid = False
                self._signal(slot)
                diag.registrar("runtime.rebind_failed", "erro", sessao=slot.binding.name, **failure_reason(exc))
        self.rebindings[key] = asyncio.create_task(rebind())

    def register(self, binding: Binding):
        global _current
        from app.runtime_process import reconcile_startup
        reconcile_startup(allow_current=True)
        if binding.provider not in {"claude", "codex"} or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", binding.key):
            raise ValueError("binding inválido para o runtime")
        if old := self.slots.get(binding.key):
            with old.guard:
                if old.binding.lock_path != binding.lock_path or old.binding.state_path != binding.state_path:
                    raise ValueError("a chave não pode trocar os arquivos de posse")
                identity_changed = (old.binding.name != binding.name or old.binding.headless != binding.headless
                    or old.binding.provider != binding.provider or old.binding.jsonl != binding.jsonl)
                if identity_changed and (not old.frozen or old.active or old.phase != Phase.Python):
                    raise RuntimeError("mudança de binding exige parada e posse Python")
                if old.binding.generation != binding.generation:
                    if not old.frozen or old.active or old.phase != Phase.Python:
                        raise RuntimeError("mudança de vida exige a barreira de lifecycle")
                    if binding.generation <= old.binding.generation:
                        raise ValueError("a geração não pode voltar para uma vida antiga")
                    state = copy.deepcopy(old.store.state)
                    state["generation"] = binding.generation
                    old.store._persist(state)
                    old.store.ensure_projection()
                self.names.pop(old.binding.name, None)
                old.binding = copy.deepcopy(binding)
                self.names[binding.name] = binding.key
            return old
        terminal = binding.provider == "claude" and isinstance(binding.meta.get("terminal"), dict)
        if not binding.headless and not terminal and not binding.state_path.exists():
            return None
        lease = WriterLease(binding.lock_path)
        slot = Slot(binding=copy.deepcopy(binding), lease=lease)
        self.slots[binding.key], self.names[binding.name] = slot, binding.key
        _current = self
        runtime_queue.configure(self)
        try:
            projection = binding.projection_dir / f"{self._sanitize(binding.name)}.jsonl"
            rows = []
            if terminal:
                from app.runtime_terminal import validate_binding, import_legacy
                if self.legacy is not None:
                    validate_binding(binding.descriptor())
                if not binding.state_path.exists() and projection.exists():
                    rows = import_legacy(self, binding, projection)
            if not binding.state_path.exists() and projection.exists() and not terminal:
                for raw in projection.read_text(encoding="utf-8").splitlines():
                    row = json.loads(raw)
                    if not isinstance(row, dict):
                        raise ValueError("entrada Legacy da fila inválida")
                    rows.append(row)
            slot.store = runtime_queue.QueueStore(binding.state_path, binding.projection_dir,
                runtime_queue.initial_state(binding.key, binding.generation, binding.name, rows))
            if slot.store.state["generation"] != binding.generation:
                if binding.generation < slot.store.state["generation"]:
                    raise ValueError("a geração não pode voltar para uma vida antiga")
                state = copy.deepcopy(slot.store.state)
                state["generation"] = binding.generation
                if terminal:
                    state["runtime_state"] = {key:value for key,value in state["runtime_state"].items() if key == "terminal_write_barrier"}
                slot.store._persist(state)
            slot.store.exec(binding.generation, "recover:" + uuid.uuid4().hex, _clock(), {"kind": "recover"})
            state = copy.deepcopy(slot.store.state)
            state["runtime_state"]["_binding"] = binding.descriptor()
            slot.store._persist(state)
            slot.phase = Phase.Python
            return slot
        except BaseException:
            # O slot bloqueado permanece visível; não se importa JSONL por cima de estado inválido.
            lease.close()
            slot.lease = None
            raise

    @staticmethod
    def _sanitize(name):
        return re.sub(r"[^A-Za-z0-9_.-]", "-", name)

    def slot(self, name):
        key = self.names.get(name)
        if key is None:
            raise ValueError("sessão sem registro de runtime")
        return self.slots[key]

    def managed_runtime(self, name):
        key = self.names.get(name)
        return bool(key and (self.slots[key].binding.headless or
            self.slots[key].binding.provider == "claude" and (isinstance(self.slots[key].binding.meta.get("terminal"), dict)
                or self.slots[key].binding.meta.get("pending_terminal"))))

    def terminal_in_rust(self, name):
        """O terminal Claude da sessão está aberto no Rust: o clique e a interface dos mods são dele."""
        slot = self.slots.get(self.names.get(name, ""))
        return bool(slot is not None and slot.phase == Phase.Rust and not slot.binding.headless
            and slot.binding.provider == "claude" and isinstance(slot.binding.meta.get("terminal"), dict))

    def managed_queue(self, name):
        return name in self.names

    def legacy_allowed(self, key, generation):
        slot = self.slots.get(key)
        if slot is None:
            return True
        with slot.guard:
            return (slot.phase == Phase.Python and (not slot.frozen or self.in_lifecycle(slot)) and slot.binding.generation == generation
                    and slot.lease is not None and not slot.lease.closed)

    @staticmethod
    def in_lifecycle(slot):
        return slot.lifecycle_token is not None and _lifecycle.get() is slot.lifecycle_token

    def _signal(self, slot):
        if self.loop is not None and self.loop.is_running():
            self.loop.call_soon_threadsafe(slot.changed.set)

    def settle_before_queue(self, name):
        """Fila síncrona (thread) de sessão Claude espera o desfecho do Rust ANTES do portão: com o
        `slot.active` preso nessa espera, a passagem ao Rust, que espera o `active` zerar, nunca
        acontece. O que o Rust não atende é do Python; a escrita da reserva e a administração já passaram por ele."""
        if self.mode != "pending" and not self._settling or _mode_bypass.get() or self.loop is None:
            return
        slot = self.slots.get(self.names.get(name, ""))
        from app.runtime_terminal import _writer
        # Dentro da barreira (renomear, fechar) quem fecha o modo espera a mesma barreira.
        if slot is None or not self.rust_owns(slot.binding.provider, slot.binding.headless) or _writer.get() is not None or self.in_lifecycle(slot):
            return
        try:
            if asyncio.get_running_loop() is self.loop:
                return      # no próprio laço não há como esperar; o `queue_rpc` decide, como antes
        except RuntimeError:
            pass
        from app.runtime_adapter import run_sync
        run_sync(self.await_mode, self.loop)

    @contextmanager
    def queue_gate(self, name):
        if not self.managed_queue(name):
            yield None
            return
        slot = self.slot(name)
        with slot.guard:
            if (slot.frozen and not self.in_lifecycle(slot)) or slot.phase not in {Phase.Python, Phase.Rust}:
                raise TransferInProgress("sessão em transferência; aguarde a posse ser confirmada")
            if slot.phase == Phase.Python and (slot.lease is None or slot.lease.closed):
                raise RuntimeError("reserva sem posse da sessão")
            slot.active += 1
            route = (slot, slot.phase, slot.binding.descriptor())
        try:
            yield route
        finally:
            with slot.guard:
                slot.active -= 1
            self._signal(slot)

    def queue_rpc(self, route, call_id, clock, action):
        slot, phase, descriptor = route
        if phase == Phase.Python:
            if descriptor["meta"].get("terminal") and not self.in_lifecycle(slot):
                from app.runtime_terminal import _writer
                if _writer.get() is None:
                    from app.runtime_adapter import run_sync
                    return run_sync(lambda:self.op(descriptor["name"], {"kind":"queue", "action":action}, call_id), self.loop)
            # A rota já conta em `slot.active` (queue_gate): a posse não muda até ela sair, e a
            # gravação é serializada dentro do QueueStore, fora da trava que o laço de eventos usa.
            return slot.store.exec(descriptor["generation"], call_id, _clock(), action)
        if self.loop is None or not self.loop.is_running():
            raise RuntimeError("loop do runtime indisponível")
        try:
            running = asyncio.get_running_loop()
        except RuntimeError:
            running = None
        if running is self.loop:
            raise RuntimeError("fila síncrona chamada no loop do servidor")
        async def after_mode():
            await self.await_mode()
            return await self._rpc(descriptor, {"kind": "queue", "action": action}, call_id)
        future = asyncio.run_coroutine_threadsafe(after_mode(), self.loop)
        try:
            # O prazo da fila só conta depois do desfecho do Rust (queda ou subida).
            return future.result(timeout=35 + PENDING_WAIT_S)
        except BaseException:
            future.cancel()
            raise

    def commit_python_state(self, route, state):
        slot, phase, descriptor = route
        if phase != Phase.Python:
            raise RuntimeError("a fila pertence ao Rust")
        if state["owner_key"] != descriptor["key"] or state["generation"] != descriptor["generation"]:
            raise ValueError("estado não pertence à vida atual")
        with slot.store._lock:
            slot.store._persist(copy.deepcopy(state))
            slot.store.ensure_projection()

    async def shutdown(self):
        # O Rust sai com o backend e o sistema solta as travas dele; os canos seguem vivos. Do
        # Python só se esperam as gravações da fila em curso antes de soltar as travas.
        for slot in tuple(self.slots.values()):
            if self.names.get(slot.binding.name) != slot.binding.key or slot.phase != Phase.Python:
                continue
            async with self._barrier(slot):
                try:
                    # Sem reabrir: o Rust sai junto com o backend. Falha não pode impedir soltar as travas.
                    await self.ingress(slot.binding.name, True)
                except Exception:
                    _log.warning("porta do Rust não fechou no desligamento de %s", slot.binding.name, exc_info=True)
                with slot.guard:
                    slot.frozen = True
                await self._wait_active(slot)
        self.close_python_leases()

    async def _wait_active(self, slot):
        while True:
            with slot.guard:
                if slot.active == 0:
                    return
                slot.changed.clear()
            await slot.changed.wait()

    @asynccontextmanager
    async def _barrier(self, slot):
        self.loop = asyncio.get_running_loop()
        if self.in_lifecycle(slot):
            yield
        else:
            async with slot.lifecycle:
                slot.lifecycle_token = object()
                token = _lifecycle.set(slot.lifecycle_token)
                try:
                    yield
                finally:
                    slot.lifecycle_token = None
                    _lifecycle.reset(token)

    async def _rpc(self, descriptor, command, operation_id):
        if self.transport is None or not self.instance:
            raise RuntimeError("IPC do runtime indisponível")
        from app.rust_server import RustOpError
        try:
            result = await self.transport.op(descriptor, command, operation_id, _clock())
        except (RustOpError, ConnectionRefusedError):
            raise       # o Rust respondeu, ou o pedido nem saiu: não há efeito desconhecido
        except Exception as exc:
            # Sem resposta do Rust depois do pedido (queda, prazo, resposta inválida): efeito desconhecido.
            exc._transport_lost = True
            raise
        if descriptor["meta"].get("terminal") and command["kind"] == "drain":
            result = {**result, "sent":int((result.get("reply") or {}).get("disposition") == "accepted")}
        return result

    def _rust_failed(self, slot):
        view = slot.view or {}
        error = view.get("error")
        if slot.binding.meta.get("terminal"):
            return bool(error) and error not in _TERMINAL_PRE_EFFECT_ERRORS
        return bool(error) or (view.get("view") or {}).get("alive") is False

    async def _reopen_if_failed(self, name):
        """Rust com a sessão em erro (ator morto, cano caído, entrega incerta): uma reabertura no
        Rust antes da operação. Nunca passa a sessão ao Python. O Claude sem terminal o próprio Rust
        reabre (`request_reopen`, religação do cano): reabrir daqui seria um segundo dono."""
        slot = self.slots.get(self.names.get(name, ""))
        if slot is None or slot.phase != Phase.Rust or self.in_lifecycle(slot):
            return
        if slot.binding.headless and slot.binding.provider == "claude":
            return
        if not slot.cache_valid or self._rust_failed(slot):
            try:
                await self.refresh_snapshot(name)
            except Exception as exc:
                if getattr(exc, "code", "") not in _ACTOR_GONE_CODES:
                    return          # sem resposta do Rust: quem decide é a espera da vista
                with slot.guard:
                    slot.view = {**(slot.view or {}), "error": exc.code, "problem": f"{exc.code}: ator do runtime ausente"}
                    slot.cache_valid = False
        if self._rust_failed(slot):
            await self._reopen(name, slot)

    async def _reopen(self, name, slot):
        from app import diag
        async with self.freeze(name):
            # Outra operação pode ter reaberto enquanto esta esperava a barreira.
            if self.slots.get(self.names.get(name, "")) is not slot or slot.phase != Phase.Rust or not self._rust_failed(slot):
                return
            previous = (slot.view or {}).get("error")
            try:
                closed = await self._rpc(slot.binding.descriptor(), {"kind":"close"}, uuid.uuid4().hex)
                if closed.get("closed") is not True:
                    raise RuntimeError("Rust não confirmou o fechamento da sessão")
                if slot.binding.meta.get("terminal"):
                    from app.runtime_terminal import resolve_binding
                    binding = await asyncio.to_thread(resolve_binding, name, slot.binding)
                    if binding is None or binding.key != slot.binding.key or binding.generation != slot.binding.generation:
                        raise RuntimeError("vínculo da sessão mudou durante a reabertura")
                    descriptor = binding.descriptor()
                    ready = await self._rpc(descriptor, _open_command(descriptor), uuid.uuid4().hex)
                    self._check_opened(ready, descriptor)
                else:
                    binding, ready = await self._launch_and_open(name, launch=True, provider=slot.binding.provider)
                    if binding.key != slot.binding.key or binding.generation != slot.binding.generation:
                        raise RuntimeError("vínculo da sessão mudou durante a reabertura")
            except Exception as exc:
                diag.registrar("runtime.reopen_failed", "erro", sessao=name, etapa=str(previous or ""), **failure_reason(exc))
                raise
            with slot.guard:
                slot.binding = copy.deepcopy(binding)
                slot.view = ready["state"]
                slot.cache_valid = True
            diag.registrar("runtime.reopened", "aviso", sessao=name, etapa=str(previous or ""))
        self._signal(slot)

    async def _await_resync(self, name, kind):
        slot = self.slots.get(self.names.get(name, ""))
        if slot is None or slot.phase != Phase.Rust or slot.cache_valid:
            return
        if slot.binding.meta.get("terminal") and slot.view.get("error") in _TERMINAL_PRE_EFFECT_ERRORS:
            return      # o próprio Rust resolve; o `_op_once` deixa passar
        self._refresh(slot)
        deadline = time.monotonic() + _RESYNC_WAIT_S
        while not slot.cache_valid and slot.phase == Phase.Rust:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return          # o `_op_once` responde `RustCacheInvalid` com a frase
            slot.changed.clear()
            if slot.cache_valid:
                return
            try:
                await asyncio.wait_for(slot.changed.wait(), min(remaining, 0.5))
            except TimeoutError:
                pass

    async def op(self, name, command, operation_id):
        """Uma tentativa no dono da sessão. Erro do Rust sobe com o código; antes, uma reabertura
        no Rust se a sessão estiver em erro. Envio sem resposta do Rust fica incerto, nunca falho:
        a entrada pode estar na fila durável."""
        self.loop = asyncio.get_running_loop()
        owner = self.slots.get(self.names.get(name, ""))
        if (self.mode == "pending" or self._settling) and (
                self.rust_owns(owner.binding.provider, owner.binding.headless) if owner is not None
                else (codex := await asyncio.to_thread(_codex_mode, name)) is None or self.rust_owns("codex", codex)):
            await self.await_mode()
        if self.legacy is not None and self.managed_queue(name):
            slot = self.slot(name)
            if (slot.binding.meta.get("terminal") or slot.binding.meta.get("pending_terminal")) and command["kind"] != "queue" and not self.in_lifecycle(slot):
                await self.prepare_session(name, "claude")
        if (command["kind"] == "submit" and command["text"].split()[0:1] == ["/clear"]
                and self.managed_queue(name) and self.slot(name).binding.meta.get("terminal")
                and not self.in_lifecycle(self.slot(name))):
            async def clear():
                async with self.freeze(name):
                    result = await self.op(name, command, operation_id)
                    # Só o /clear cujo Enter pode ter saído (o que ergueu a trava): o que parou antes não
                    # rodou e não leva a fila junto.
                    if (result.get("disposition") in {"accepted", "unknown"}
                            and (result.get("payload") or {}).get("preserve_binding") is True):
                        from app.runtime_terminal import BindingChanged
                        try:
                            await self.op(name, {"kind":"queue", "action":{"kind":"clear"}}, uuid.uuid4().hex)
                        except BindingChanged as exc:
                            # Em geral a conversa já trocou e a troca do vínculo esvazia a fila; o resto
                            # (sessão morta) fica no diário.
                            from app import diag
                            diag.registrar("runtime.clear_queue_skipped", "aviso", sessao=name, **failure_reason(exc))
                    return result
            task = asyncio.create_task(clear())
            try:
                return await asyncio.shield(task)
            except asyncio.CancelledError:
                await task
                raise
        if command.get("kind") not in _BACKGROUND_KINDS:
            # Só operação de alguém (envio, controle, histórico): drenagem de fundo reabrindo a cada
            # evento viraria laço enquanto o erro persiste.
            await self._reopen_if_failed(name)
        if command.get("kind") not in _BACKGROUND_KINDS | {"ensure_projection"}:
            await self._await_resync(name, command.get("kind"))
        transport = self.transport
        try:
            result = await self._op_once(name, command, operation_id)
            self._cache_invalid_logged.discard(name)
            return result
        except Exception as exc:
            if not getattr(exc, "_hangar_rust", False) or getattr(exc, "code", "") in _ANSWER_CODES:
                raise
            from app import diag
            # Fundo recusado por cache inválido se repete a cada troca de estado até alguém reabrir a
            # sessão: vai ao diário uma vez por sequência; a operação que der certo zera.
            background_refusal = isinstance(exc, RustCacheInvalid) and command.get("kind") in _BACKGROUND_KINDS
            if not background_refusal or name not in self._cache_invalid_logged:
                diag.registrar("runtime.rust_op_failed", "erro", sessao=name, etapa=str(command.get("kind")), **failure_reason(exc))
            if background_refusal:
                self._cache_invalid_logged.add(name)
            if getattr(exc, "_transport_lost", False):
                slot = self.slots.get(self.names.get(name, ""))
                if slot is not None:
                    slot.cache_valid = False        # a próxima operação relê o estado do Rust
                    self._signal(slot)
            if command.get("kind") == "submit" and getattr(exc, "_transport_lost", False):
                repeated = await self._repeat_after_crash(name, command, operation_id, transport)
                if repeated is not None:
                    return repeated
                diag.registrar("runtime.send_uncertain", "aviso", sessao=name, **failure_reason(exc))
                return {"operation_id":operation_id, "disposition":"unknown", "payload":{"transport_lost":True}}
            raise

    async def _repeat_after_crash(self, name, command, operation_id, transport):
        """Envio sem resposta porque o Rust morreu: a entrada pode já estar na fila dele. Espera o
        Rust novo e repete o MESMO `operation_id` uma vez (ele não duplica); senão, incerto."""
        alive = getattr(transport, "alive", True)
        if (alive() if callable(alive) else alive) or transport is None:
            return None
        deadline = time.monotonic() + PENDING_WAIT_S
        while self.mode != "python" and (self.transport is transport or self.mode == "pending" or self._settling):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            if self._mode_event is None:
                self._mode_event = asyncio.Event()
            try:
                await asyncio.wait_for(self._mode_event.wait(), remaining)
            except TimeoutError:
                pass
        if self.mode != "rust":
            return None         # o Python assumiu a porta: a fila durável dele decide
        try:
            if not self.managed_queue(name):
                # O registro do Claude sem terminal saiu na volta do Rust, que o reabre sozinho: é achado lá.
                await self.prepare_session(name, await asyncio.to_thread(_headless_provider, name), launch=True)
            return await self._op_once(name, command, operation_id)
        except Exception as again:
            if getattr(again, "_transport_lost", False):
                return None
            raise

    async def _op_once(self, name, command, operation_id):
        with self.queue_gate(name) as route:
            if route is None:
                raise RuntimeError("sessão sem responsável gerenciado")
            slot, phase, descriptor = route
            if phase == Phase.Rust:
                try:
                    # O ator terminal segue aceitando nesse erro: entrada vira linha na fila durável.
                    maintenance = (slot.binding.meta.get("terminal")
                        and slot.view.get("error") in _TERMINAL_PRE_EFFECT_ERRORS)
                    if command["kind"] not in {"snapshot", "ensure_projection"} and not slot.cache_valid and not maintenance:
                        raise RustCacheInvalid("estado do runtime indisponível; aguarde a reposição")
                    return await self._rpc(descriptor, command, operation_id)
                except Exception as exc:
                    exc._hangar_rust = True     # só falha do caminho Rust entra na contagem
                    raise
            if self.legacy is None:
                raise RuntimeError("serviço da reserva indisponível")
            if descriptor["meta"].get("terminal"):
                from app import runtime_terminal
                async with slot.terminal_serial:
                    if slot.binding.descriptor() != descriptor:
                        raise RuntimeError("binding mudou durante a espera")
                    await asyncio.to_thread(runtime_terminal.validate_binding, descriptor)
                    task = asyncio.create_task(self.legacy.op(descriptor, command, operation_id))
                    try:
                        return await asyncio.shield(task)
                    except asyncio.CancelledError:
                        await task
                        raise
            return await self.legacy.op(descriptor, command, operation_id)

    async def _restore(self, slot, *, reconnect=True):
        if slot.lease is None or slot.lease.closed:
            slot.lease = WriterLease(slot.binding.lock_path)
        binding = slot.binding
        def open_store():
            store = runtime_queue.QueueStore(binding.state_path, binding.projection_dir,
                runtime_queue.initial_state(binding.key, binding.generation, binding.name, []))
            store.exec(binding.generation, "recover:" + uuid.uuid4().hex, _clock(), {"kind": "recover"})
            return store
        slot.store = await asyncio.to_thread(open_store)
        recovered_view = slot.store.state.get("runtime_state", {}).get("view") or {}
        slot.reserve_state = {"runtime_state":copy.deepcopy(recovered_view)} if recovered_view else slot.reserve_state
        if self.legacy is None:
            raise RuntimeError("serviço da reserva indisponível")
        try:
            ready = await self.legacy.reconnect(slot.binding.descriptor(), slot.reserve_state) if reconnect else {"hydrated":True}
            if ready.get("hydrated") is not True:
                raise RuntimeError("reserva não restaurou o snapshot")
        except Exception as exc:
            # Com terminal, quem resolve é o `recover`: aposenta o registro e o próximo
            # prepare_session refaz a sessão do estado durável, com o pane que existir então.
            if slot.binding.meta.get("terminal"):
                raise
            # A trava e a fila já são do Python: sem cano para religar (morreu com a sessão), ela
            # fica estacionada e o próximo envio a sobe de novo, como o adapter antigo. Ficar em
            # "recuperando" recusava toda operação daquela sessão até o backend reiniciar.
            from app import diag
            diag.registrar("runtime.restore_sem_cliente", "erro", sessao=slot.binding.name, **failure_reason(exc))
        with slot.guard:
            slot.phase = Phase.Python

    async def detach(self, name, *, restore=True, kill=False):
        """Fecha no Rust. `kill`: o processo da sessão morre junto; a resposta diz se o Rust o matou."""
        slot = self.slot(name)
        async with self._barrier(slot):
            if slot.phase == Phase.Python:
                return None
            with slot.guard:
                slot.phase = Phase.RecoveringPython
            await self._wait_active(slot)
            try:
                reply = await self._rpc(slot.binding.descriptor(), {"kind": "close", **({"kill": True} if kill else {})}, uuid.uuid4().hex)
                if reply.get("closed") is not True:
                    raise RuntimeError("Rust não confirmou a liberação da sessão")
            except BaseException:
                # O Rust não soltou: ele continua dono, e a sessão não fica presa em transferência.
                with slot.guard:
                    slot.phase = Phase.Rust
                raise
            await self._restore(slot, reconnect=restore)
            return reply

    async def recover(self, name, confirmed_dead: bool, containment=None):
        slot = self.slot(name)
        async with self._barrier(slot):
            if not confirmed_dead or self._rust_alive():
                raise RuntimeError("morte do Rust não foi confirmada; a reserva permanece bloqueada")
            if slot.binding.meta.get("terminal"):
                proof = getattr(containment or self.transport, "containment_clean", None)
                if proof is None or proof() is not True:
                    raise RuntimeError("fim dos descendentes Rust não comprovado; escrita suspensa")
            with slot.guard:
                slot.phase = Phase.RecoveringPython
            await self._wait_active(slot)
            try:
                await self._restore(slot)
            except BaseException:
                # Rust morto e contido: o registro sai, e o próximo prepare_session refaz a sessão a
                # partir do estado durável. Sem isto o nome ficava preso em recuperação até o restart.
                with slot.guard:
                    try:
                        if slot.lease is not None:
                            slot.lease.close()
                    except Exception:
                        _log.warning("trava de escrita não fechou ao aposentar a sessão", exc_info=True)
                    slot.lease = None
                    if self.names.get(name) == slot.binding.key:
                        self.names.pop(name, None)
                    self.slots.pop(slot.binding.key, None)
                raise

    async def ingress(self, name, closed, *, held=False):
        """Fecha/abre a porta de escrita do Rust para a sessão `name`. Vai direto pelo transporte:
        `op` passa por queue_gate/freeze e travaria dentro do próprio freeze. `held`: fechamento da
        troca de conversa, que pode durar indefinidamente; o Rust recusa na hora em vez de esperar, e
        a reabertura correspondente leva o mesmo `held`."""
        if self.transport is None:
            return
        # O Rust trata `ingress` antes de procurar a entrada: serve qualquer chave, até de nome sem registro.
        descriptor = {"key": name, "generation": 0, "meta": {}}
        command = {"kind": "ingress", "name": name, "closed": closed}
        if held:
            command["held"] = True
        await self._rpc(descriptor, command, uuid.uuid4().hex)

    def ingress_sync(self, name, closed, *, held=False):
        if self.transport is None:
            return
        from app.runtime_adapter import run_sync
        run_sync(lambda: self.ingress(name, closed, held=held), self.loop)

    @asynccontextmanager
    async def _ingress_closed(self, *names):
        """Cada fechamento é contado no Rust: abre exatamente os que este bloco fechou."""
        closed = []
        try:
            for name in names:
                await self.close_ingress(name)
                closed.append(name)
            yield
        finally:
            # Cada reabertura na sua tentativa: uma falha não deixa as outras portas fechadas. Não
            # levanta: o bloco já fez o trabalho, e a porta presa vai ao diário com o código.
            for name in closed:
                try:
                    await self.ingress(name, False)
                except Exception as exc:
                    from app import diag
                    diag.registrar("runtime.ingress_reopen_failed", "erro", sessao=name, **failure_reason(exc))

    async def close_ingress(self, name, *, held=False):
        """Fecha a porta; se quem espera for cancelado, o pedido já enviado termina (o transporte o
        protege) e o fechamento que ninguém vai abrir é desfeito."""
        task = asyncio.ensure_future(self.ingress(name, True, held=held))
        try:
            await asyncio.shield(task)
        except asyncio.CancelledError:
            try:
                await task
            except Exception:
                pass            # o fechamento não chegou ao Rust: nada a desfazer
            else:
                try:
                    await self.ingress(name, False, held=held)
                except Exception:
                    _log.warning("porta do Rust ficou fechada após cancelamento de %s", name, exc_info=True)
            raise

    @asynccontextmanager
    async def freeze(self, name):
        slot = self.slot(name)
        async with self._barrier(slot):
            # Regra: quem congela uma sessão fecha antes a porta do Rust.
            async with self._ingress_closed(name):
                with slot.guard:
                    slot.frozen = True
                try:
                    await self._wait_active(slot)
                    yield slot.binding
                finally:
                    with slot.guard:
                        slot.frozen = False

    def close_python_leases(self):
        global _current
        for slot in self.slots.values():
            with slot.guard:
                if slot.active and slot.lease is not None:
                    raise RuntimeError("persistência ainda em curso; posse conservada")
                if slot.lease is not None:
                    slot.lease.close()
        if _current is self:
            _current = None
        if runtime_queue._coordinator is self:
            runtime_queue.configure(None)

    async def retire_waiting(self, name):
        """Nome reaproveitado por uma vida nova: o registro que esperava a identidade antiga sai."""
        async with self.registration_locks.setdefault(name, asyncio.Lock()):
            key = self.names.get(name)
            if key is not None and self.slots[key].awaiting_identity:
                self.names.pop(name, None)

    async def change(self, name, action, *, advance=True, remove=False, reopen=True, stopped=False, kill=False):
        """Administração da sessão. Do Rust: barreira → `close` (a trava e a fila voltam ao Python,
        sem cliente no cano) → ação → nova vida gravada na fila → `open` no Rust, salvo `stopped`
        ou sem processo para abrir. `reopen` só vale para o caminho Python."""
        if not self.managed_queue(name):
            return await action()
        slot = self.slot(name)
        if self.in_lifecycle(slot):
            if slot.phase == Phase.Rust and slot.change is not None:
                # Ação aninhada depois de reaberta (a volta atrás de uma troca de conta): fecha antes.
                await self._close_for_change(name, slot)
            return await action()
        if remove and slot.awaiting_identity:
            # Registro em espera nunca teve posse nem dono no Rust: fechar só solta o nome.
            await self.retire_waiting(name)
            return await action()

        async def perform():
            async with self.freeze(name):
                if self.slots.get(self.names.get(name, "")) is not slot:
                    # Outro caminho trocou o registro enquanto esta esperava a barreira.
                    raise RuntimeError("registro da sessão mudou durante a espera; tente de novo")
                from_rust, closed = slot.phase == Phase.Rust, None
                if from_rust:
                    closed = await self._close_for_change(name, slot, kill=kill)
                elif remove and self.legacy is not None:
                    # Fechar não escreve na conversa: basta esperar os escritores, mesmo com vínculo mudado.
                    await self.legacy.quiesce({**slot.binding.descriptor(), "removed":True})
                try:
                    try:
                        from app.runtime_terminal import terminal_life
                        life = None if remove else await asyncio.to_thread(terminal_life, slot.binding)
                        slot.change = {"advance":advance, "life":life, "from_rust":from_rust, "relaunch":False,
                                       "killed":bool(closed and closed.get("killed") is True)}
                        if remove and slot.binding.meta.get("terminal") and slot.binding.meta.get("claude_settings") and self.legacy is not None:
                            current = await asyncio.to_thread(self.legacy.binding, name, slot.binding.provider)
                            await self._remember_terminal_customizations(current or slot.binding)
                        result = await action()
                    except Exception:
                        if slot.change_from_rust and slot.phase == Phase.Python and not remove:
                            # A ação falhou com a sessão fechada no Rust: volta a ela na vida de antes.
                            await self._reopen_after_change(name, slot, launch=False)
                        raise
                    await self._wait_active(slot)
                    if remove:
                        if self.legacy is not None and not from_rust:
                            # Removida, a sessão não tem mais vínculo a conferir: só se esperam os escritores.
                            await self.legacy.quiesce({**slot.binding.descriptor(), "removed":True})
                        with slot.guard:
                            slot.lease.close()
                            slot.lease = None
                            self.names.pop(slot.binding.name, None)
                            self.slots.pop(slot.binding.key, None)
                        return result, None
                    if slot.phase == Phase.Rust:
                        return result, None         # a ação já reabriu no Rust (troca de conta)
                    relaunch = slot.change["relaunch"]
                    try:
                        current = await self._commit_change(name, slot)
                    except Exception:
                        if slot.change_from_rust and slot.phase == Phase.Python and self.slots.get(slot.binding.key) is slot:
                            await self._reopen_after_change(name, slot, launch=False)
                        raise
                    if not current.change_from_rust:
                        return result, current
                    try:
                        if not stopped:
                            await self._reopen_after_change(current.binding.name, current, launch=relaunch)
                    finally:
                        current.change_from_rust = False
                    return result, None
                finally:
                    slot.change, slot.change_from_rust = None, False

        task = asyncio.create_task(perform())
        try:
            result, current = await asyncio.shield(task)
        except asyncio.CancelledError:
            await task
            raise
        if reopen and not remove and current is not None and self.managed_runtime(current.binding.name):
            await self.prepare_session(current.binding.name, current.binding.provider)
        return result

    async def _close_for_change(self, name, slot, kill=False):
        """Fecha no Rust e devolve a trava e a fila ao Python sem cliente no cano. A vista do Rust,
        relida agora, fica guardada: é ela que diz se a sessão estava ociosa (transferência)."""
        try:
            if not await self.refresh_snapshot(name):
                slot.cache_valid = False
        except Exception as exc:
            slot.cache_valid = False        # quem lê a ociosidade recusa por estado desconhecido
            from app import diag
            diag.registrar("runtime.refresh_failed", "aviso", sessao=name, **failure_reason(exc))
        reply = await self.detach(name, restore=False, **({"kill": True} if kill else {}))
        slot.change_from_rust = True
        return reply

    async def _commit_change(self, name, slot):
        """Grava a nova vida na fila (nome, conversa, geração) sob a trava do Python. Devolve o
        registro que segue: o mesmo, ou um novo quando o pane renascido roda outra conversa."""
        from app.runtime_terminal import reborn_binding, pending_binding
        change = slot.change
        advance, life = change["advance"], change["life"]
        binding = await asyncio.to_thread(self.legacy.binding, name, slot.binding.provider)
        if life is not None and (binding is None or binding.key != slot.binding.key):
            # Vida de terminal nascida com a sessão congelada é obra da ação: herda a chave.
            binding = await asyncio.to_thread(reborn_binding, name, slot.binding, life) or binding
        if binding is None:
            binding = (await asyncio.to_thread(pending_binding, name, slot.binding)
                if slot.binding.provider == "claude" and slot.binding.headless else None)
            if binding is None:
                binding = copy.deepcopy(slot.binding)
                binding.name, binding.headless = name, False
                binding.meta = {**binding.meta, "headless":False, "cano":None}
        if binding.key != slot.binding.key:
            if not (binding.meta.get("terminal") and binding.meta.get("session_id") != slot.binding.meta.get("session_id")):
                raise RuntimeError("mudança de modo não pode trocar a chave durável")
            # O pane novo roda outra conversa: a fila da antiga fica com ela, e o vínculo novo
            # nasce com a fila da própria conversa.
            with slot.guard:
                slot.lease.close()
                slot.lease, slot.store = None, None
                if self.names.get(slot.binding.name) == slot.binding.key:
                    self.names.pop(slot.binding.name, None)
                self.slots.pop(slot.binding.key, None)
            fresh = await asyncio.to_thread(self.register, binding)
            fresh.change_from_rust = slot.change_from_rust
            return fresh
        if self.legacy is not None and not change["from_rust"]:
            await self.legacy.quiesce(binding.descriptor())
        def commit():
            # Grava com fsync: fora do laço de eventos.
            with slot.guard:
                # Só conversa nova esvazia a fila: a conta nova muda o caminho do transcript, não a conversa.
                if slot.binding.meta.get("terminal") and binding.meta.get("session_id") != slot.binding.meta.get("session_id"):
                    slot.store.exec(slot.binding.generation, "clear:" + uuid.uuid4().hex, _clock(), {"kind":"clear"})
                binding.generation = slot.binding.generation + int(advance)
                if isinstance(binding.meta.get("terminal"), dict):
                    binding.meta["terminal"]["generation"] = binding.generation
                state = copy.deepcopy(slot.store.state)
                if advance:
                    state["runtime_state"] = {key:value for key,value in state["runtime_state"].items() if key == "terminal_write_barrier"}
                state["runtime_state"]["_binding"] = binding.descriptor()
                slot.store._persist(state)
            self.register(binding)
        await asyncio.to_thread(commit)
        slot.view, slot.cache_valid = {}, False
        self._signal(slot)
        return slot

    async def _reopen_after_change(self, name, slot, *, launch):
        """Abre no Rust o que a administração deixou com o Python. Sem processo vivo e sem pedido
        de subida, ou com o terminal ainda sem prova da conversa, fica o registro sem cliente."""
        from app import diag
        binding = slot.binding
        if not binding.meta.get("terminal"):
            if not self._born_in_rust(binding):
                return
            if name in self.legacy.adapters[binding.provider]._sessions:
                # Cliente religado no boot: segue pela adoção até a Task 5.
                diag.registrar("runtime.reopen_skipped", "aviso", sessao=name, codigo="python_client")
                return
            meta = binding.meta
            if binding.provider == "codex":
                # O Rust pode ter subido outro processo nesta vida: vale o cano gravado no arquivo.
                fresh = await asyncio.to_thread(self.legacy.binding, name, "codex")
                if fresh is not None and fresh.key == binding.key:
                    meta = fresh.meta
            if not launch and not await asyncio.to_thread(_cano_alive, meta):
                # Processo parado: fica como sessão parada, e o próximo envio a sobe no Rust.
                diag.registrar("runtime.reopen_skipped", "aviso", sessao=name, codigo="cano_parado")
                return
        try:
            await self._open_slot_in_rust(name, slot, launch=launch)
        except Exception as exc:
            # A ação já aconteceu: ela não vira erro. A sessão fica no registro sem cliente, a
            # falha na faixa (sem terminal) e no diário, e o próximo envio tenta abrir de novo
            # (com terminal, a próxima operação recusa com o motivo).
            diag.registrar("runtime.reopen_failed", "erro", sessao=name, etapa="change", **failure_reason(exc))

    async def reopen_in_change(self, name, *, wait_initialized=False):
        """`ensure_running` dentro da barreira (troca de conta, restauração da transferência):
        grava a vida nova e abre no Rust já, com a conta pedida e, se pedido, a espera do
        `initialize`; a falha sobe para quem desfaz a troca."""
        slot = self.slot(name)
        if not self.in_lifecycle(slot):
            raise RuntimeError("reabertura fora da barreira da sessão")
        if slot.phase != Phase.Rust:
            if slot.phase != Phase.Python or slot.lease is None:
                raise RuntimeError("sessão sem registro para reabrir")
            if name in self.legacy.adapters[slot.binding.provider]._sessions:
                raise RuntimeError("cliente Python ainda ligado nesta sessão")
            if slot.change is not None:
                slot = await self._commit_change(name, slot)
            await self._open_slot_in_rust(slot.binding.name, slot, launch=True)
        if wait_initialized:
            await self._await_initialized(slot)
        return slot

    async def _open_slot_in_rust(self, name, slot, *, launch):
        """Solta a trava do Python e abre o registro no Rust; falhando, a trava volta ao Python."""
        from app import diag
        self.loop = asyncio.get_running_loop()
        headless = not slot.binding.meta.get("terminal")
        if not headless:
            from app.runtime_terminal import validate_binding
            await asyncio.to_thread(validate_binding, slot.binding.descriptor())
        deadline = self.loop.time() + _ACTIVE_WAIT_S
        while True:
            with slot.guard:
                if slot.phase != Phase.Python:
                    raise RuntimeError("sessão fora da posse Python")
                if not slot.active:
                    lease, slot.lease, slot.store = slot.lease, None, None
                    break
            # Quem está na fila Python sai em instantes; recusar aqui deixava a sessão no Python.
            if self.loop.time() >= deadline:
                raise RuntimeError("fila da sessão em uso no Python")
            try:
                await asyncio.wait_for(self._wait_active(slot), deadline - self.loop.time())
            except TimeoutError:
                pass
        if lease is not None:
            lease.close()
        try:
            if headless:
                binding, ready = await self._launch_and_open(name, launch=launch, provider=slot.binding.provider)
                if binding.key != slot.binding.key:
                    raise RuntimeError("sidecar mudou durante a abertura da sessão")
            else:
                binding = copy.deepcopy(slot.binding)
                descriptor = binding.descriptor()
                ready = await self._rpc(descriptor, _open_command(descriptor), uuid.uuid4().hex)
                self._check_opened(ready, descriptor)
        except Exception as exc:
            if not headless:
                diag.registrar("runtime.open_failed", "erro", sessao=name, **failure_reason(exc))
            # O `open` pode ter chegado ao Rust (resposta perdida ou recusada aqui): fecha lá antes
            # de a trava voltar ao Python, como a adoção; sem confirmação, quem decide é a trava.
            await self._close_unconfirmed(name, slot.binding.descriptor())
            try:
                await self._restore(slot, reconnect=False)
            except Exception as restore_error:
                # A trava não voltou: o Rust ficou com a sessão. Ela fica com ele, de vista
                # inválida, e a próxima operação relê o estado (ou reabre) lá.
                diag.registrar("runtime.restore_failed", "erro", sessao=name, **failure_reason(restore_error))
                with slot.guard:
                    slot.phase, slot.cache_valid = Phase.Rust, False
            raise
        with slot.guard:
            slot.binding = copy.deepcopy(binding)
            slot.view, slot.cache_valid, slot.phase = ready["state"], True, Phase.Rust
            slot.change_from_rust = False
        self._signal(slot)

    async def _close_unconfirmed(self, name, descriptor):
        """Depois de um `open` sem resposta ou recusado aqui: fecha no Rust, que pode ter aberto. Rust
        morto que nem escuta não segura nada (a trava morre com o processo); fora isso, sem
        confirmação, quem decide é a trava, e o diário registra."""
        try:
            await self._rpc(descriptor, {"kind":"close"}, uuid.uuid4().hex)
        except Exception as exc:
            if isinstance(exc, ConnectionRefusedError) and not self._rust_alive():
                return
            from app import diag
            diag.registrar("runtime.close_unconfirmed", "erro", sessao=name, **failure_reason(exc))

    def _rust_alive(self):
        alive = getattr(self.transport, "alive", False)
        return alive() if callable(alive) else alive

    def _check_opened(self, ready, descriptor):
        if not (ready.get("opened") is True and isinstance(ready.get("state"), dict) and ready.get("instance") == self.instance
                and ready.get("key") == descriptor["key"] and ready.get("generation") == descriptor["generation"]):
            raise RuntimeError("abertura não corresponde à vida atual")

    def source_view(self, name):
        """Vista do Rust da sessão aberta nele, ou guardada no `close` da administração em curso."""
        slot = self.slots.get(self.names.get(name, ""))
        if slot is None or not (slot.phase == Phase.Rust or slot.change_from_rust):
            return None
        if not slot.cache_valid or not isinstance((slot.view or {}).get("view"), dict):
            raise RuntimeError("estado do runtime indisponível")
        return copy.deepcopy(slot.view["view"])

    async def lifecycle_call(self, name, method, arguments):
        if self.legacy is None:
            raise RuntimeError("serviço administrativo indisponível")
        adapter = self.legacy.adapters[self.slot(name).binding.provider]
        import inspect
        original = inspect.unwrap(getattr(adapter, method))
        params = {key:value for key,value in arguments.items() if key not in {"self", "name"}}
        async def action():
            if inspect.iscoroutinefunction(original):
                return await original(adapter, name, **params)
            return await asyncio.to_thread(original, adapter, name, **params)
        stopped = method in {"close_sync", "parar"}
        binding = self.slot(name).binding
        kill = method == "close_sync" and binding.provider == "codex" and binding.headless and self.rust_owns("codex", True)
        if kill:
            async def action():
                # O Rust matou o processo que ele subiu no `close`; sem a sessão aberta lá, o processo
                # não tem outro dono e o Python o encerra.
                if (self.slot(name).change or {}).get("killed"):
                    return adapter.forget_memory(name, preserve_preview=params.get("preserve_preview", False))
                return await asyncio.to_thread(original, adapter, name, **params)
        return await self.change(name, action, remove=False, reopen=not stopped, stopped=stopped, kill=kill)
