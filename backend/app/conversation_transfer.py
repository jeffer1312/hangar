"""Origem privada, etapas confirmadas e coordenação da troca de agente.

O registro é a verdade; índices não confirmam etapas. Dados privados dos processos
ficam no manifesto de runtime, separados da associação histórica.
"""
from __future__ import annotations

import hashlib
import json
import logging
import os
import tempfile
import threading
import uuid
from contextlib import asynccontextmanager, contextmanager
from contextvars import ContextVar
from dataclasses import asdict, dataclass, replace
from enum import StrEnum
from pathlib import Path

from app import atomico
from app.share_life import session_life


_log = logging.getLogger("hangar.transfer")


class TransferError(ValueError):
    """Erro público da troca; os parâmetros nunca carregam conversa/configuração."""

    def __init__(self, code: str, *, status: int = 409,
                 params: dict[str, str | int] | None = None):
        if not isinstance(code, str) or not code.startswith("session_transfer_") \
                or not all(c.islower() or c == "_" or c.isdigit() for c in code):
            raise ValueError("código de transferência inválido")
        if type(status) is not int or not 400 <= status <= 599:
            raise ValueError("status de transferência inválido")
        allowed = {"model", "effort", "account", "phase", "limit", "estimated"}
        if params is not None and (not isinstance(params, dict) or set(params) - allowed
                or any(type(v) not in (str, int) or
                       (isinstance(v, str) and (len(v) > 200 or "\n" in v or "\r" in v))
                       for v in params.values())):
            raise ValueError("parâmetros públicos de transferência inválidos")
        self.code, self.status, self.params = code, status, dict(params or {})
        super().__init__(code)


class TransferPhase(StrEnum):
    PREPARING = "preparing"
    SOURCE_STOPPED = "source_stopped"
    IMPORTED = "imported"
    PUBLISHING = "publishing"
    COMPLETE = "complete"
    RESTORING = "restoring"
    RESTORE_FAILED = "restore_failed"
    REJECTED = "rejected"
    ROLLED_BACK = "rolled_back"


@dataclass(frozen=True)
class ConversationSource:
    path: str
    provider: str
    digest: str
    selected_uuids: tuple[str, ...]


@dataclass(frozen=True)
class ImportBoundary:
    thread_id: str
    rollout_path: str
    min_offset: int
    imported_item_ids: tuple[str, ...]
    prefix_digest: str | None = None


@dataclass(frozen=True)
class TransferRecord:
    id: str
    name: str
    source_life: str
    phase: TransferPhase
    source: ConversationSource | None
    origin_meta: dict[str, object]
    destination_meta: dict[str, object] | None
    boundary: ImportBoundary | None
    error_code: str | None


_TERMINAL = {TransferPhase.COMPLETE, TransferPhase.REJECTED, TransferPhase.ROLLED_BACK}
# Somente escolhas, identidade e referências necessárias à retomada.
_META_FIELDS = {
    "name", "provider", "cwd", "jsonl", "session_id", "key", "config_dir", "headless",
    "engine", "model", "effort", "context_window", "permission_mode", "previous_non_plan",
    "subagent_model", "jev", "thread_id", "rollout_path", "codex_home", "codex_account",
    "lifecycle_id", "endpoint", "app_pid", "tui_pid", "launcher_pid", "pane_id", "pane_pid",
    "tool_output_token_limit", "transfer_id",
}
_lock = threading.RLock()
_UNSET = object()


def _base() -> Path:
    return Path.home() / ".hangar" / "conversation-transfers"


def _id(value: str) -> str:
    return str(uuid.UUID(value))


def _hash(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def _thread_key(codex_home: str, thread_id: str) -> str:
    home = str(Path(codex_home).expanduser().resolve(strict=False))
    return json.dumps([home, thread_id], ensure_ascii=False)


def _record_path(transfer_id: str) -> Path:
    return _base() / f"{_id(transfer_id)}.json"


def _index_path(kind: str, key: str) -> Path:
    return _base() / "indexes" / f"{kind}-{_hash(key)}.json"


@contextmanager
def _locked():
    _base().mkdir(parents=True, exist_ok=True, mode=0o700)
    with _lock, (_base() / ".lock").open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            lock.seek(0)
            if not lock.read(1):
                lock.write(b"\0")
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            yield
        finally:
            if os.name == "nt":
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(lock, fcntl.LOCK_UN)


def _write_bytes(path: Path, raw: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, suffix=".tmp", delete=False) as tmp:
            temporary = Path(tmp.name)
            tmp.write(raw)
            tmp.flush()
            os.fsync(tmp.fileno())
        atomico.substituir(temporary, path)
        if os.name != "nt":
            # O rename precisa estar durável antes de publicar a referência ao snapshot.
            fd = os.open(path.parent, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _write_json(path: Path, value: object) -> None:
    _write_bytes(path, json.dumps(value, ensure_ascii=False).encode("utf-8"))


def _object(value: object, fields: set[str]) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError("registro de transferência com campos inválidos")
    return value


def _string(value: object) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError("registro de transferência com texto inválido")
    return value


def _strings(value: object) -> tuple[str, ...]:
    if not isinstance(value, (list, tuple)):
        raise ValueError("registro de transferência com lista inválida")
    return tuple(_string(item) for item in value)


def _metadata(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) - _META_FIELDS:
        raise ValueError("metadados de transferência contêm campos não permitidos")
    for key, item in value.items():
        if item is None:
            continue
        expected = (bool if key in {"headless", "jev"} else
                    int if key in {"context_window", "app_pid", "tui_pid", "launcher_pid", "pane_pid", "tool_output_token_limit"}
                    else str)
        if type(item) is not expected:
            raise ValueError(f"metadado de transferência inválido: {key}")
    return dict(value)


def _decode(value: object) -> TransferRecord:
    data = _object(value, {"id", "name", "source_life", "phase", "source", "origin_meta",
                           "destination_meta", "boundary", "error_code"})
    source = None
    if data["source"] is not None:
        s = _object(data["source"], {"path", "provider", "digest", "selected_uuids"})
        digest = _string(s["digest"])
        if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise ValueError("digest da origem inválido")
        if s["provider"] != "claude":
            raise ValueError("origem da transferência não é Claude")
        source = ConversationSource(_string(s["path"]), _string(s["provider"]), digest,
                                    _strings(s["selected_uuids"]))
    boundary = None
    if data["boundary"] is not None:
        if not isinstance(data["boundary"], dict):
            raise ValueError("fronteira de importação inválida")
        b = _object({"prefix_digest": None, **data["boundary"]},
                    {"thread_id", "rollout_path", "min_offset", "imported_item_ids", "prefix_digest"})
        prefix_digest = b["prefix_digest"]
        if prefix_digest is not None and (not isinstance(prefix_digest, str)
                or len(prefix_digest) != 64
                or any(c not in "0123456789abcdef" for c in prefix_digest)):
            raise ValueError("digest da fronteira inválido")
        if type(b["min_offset"]) is not int or b["min_offset"] < 0:
            raise ValueError("fronteira de importação inválida")
        boundary = ImportBoundary(_string(b["thread_id"]), _string(b["rollout_path"]),
                                  b["min_offset"], _strings(b["imported_item_ids"]), prefix_digest)
    error = None if data["error_code"] is None else _string(data["error_code"])
    destination = None if data["destination_meta"] is None else _metadata(data["destination_meta"])
    if boundary and destination and destination.get("thread_id") not in (None, boundary.thread_id):
        raise ValueError("thread da fronteira diverge do destino")
    return TransferRecord(_id(_string(data["id"])), _string(data["name"]),
                          _string(data["source_life"]), TransferPhase(data["phase"]), source,
                          _metadata(data["origin_meta"]),
                          destination, boundary, error)


def load_transfer(transfer_id: str) -> TransferRecord | None:
    path = _record_path(transfer_id)
    # Somente ausência de arquivo é ausência de registro; JSON null é corrupção.
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None
    record = _decode(json.loads(raw))
    if record.id != _id(transfer_id):
        raise ValueError("identidade do registro de transferência divergente")
    return record


def _index_ids(kind: str, key: str) -> list[str]:
    path = _index_path(kind, key)
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return []
    values = json.loads(raw)
    if not isinstance(values, list):
        raise ValueError("índice de transferência inválido")
    return [_id(_string(value)) for value in values]


def _append_index(kind: str, key: str, transfer_id: str) -> None:
    ids = _index_ids(kind, key)
    if transfer_id not in ids:
        _write_json(_index_path(kind, key), [*ids, transfer_id])


def _destination(record: TransferRecord) -> tuple[str, str] | None:
    meta = record.destination_meta or {}
    home = meta.get("codex_home")
    thread = meta.get("thread_id") or (record.boundary.thread_id if record.boundary else None)
    if isinstance(home, str) and home and isinstance(thread, str) and thread:
        return home, thread
    return None


def _save_locked(record: TransferRecord) -> None:
    # Índices aditivos antes do commit: uma interrupção deixa o registro anterior legível.
    # Um ponteiro novo só vale quando os campos do registro confirmam a associação.
    _append_index("session", record.name, record.id)
    _append_index("life", record.source_life, record.id)
    life = (record.destination_meta or {}).get("lifecycle_id")
    if isinstance(life, str) and life:
        _append_index("life", life, record.id)
    if destination := _destination(record):
        _append_index("thread", _thread_key(*destination), record.id)
    _write_json(_record_path(record.id), asdict(record))


def prepare_origin(origin_meta: dict[str, object]) -> dict[str, object]:
    meta = _metadata(origin_meta)
    if not meta.get("key"):
        meta["key"] = uuid.uuid4().hex
    return meta


def save_transfer(record: TransferRecord) -> None:
    record = _decode(asdict(record))
    with _locked():
        previous = load_transfer(record.id)
        if not record.origin_meta.get("key"):
            key = (previous.origin_meta.get("key") if previous else None) or uuid.uuid4().hex
            record = replace(record, origin_meta={**record.origin_meta, "key": key})
        elif previous and previous.origin_meta.get("key") != record.origin_meta["key"]:
            raise ValueError("a chave da origem da transferência mudou")
        if previous and previous.source_life != record.source_life:
            raise ValueError("a vida da origem da transferência mudou")
        _save_locked(record)
    if record.phase in _TERMINAL:
        release_gate(record.name)


def capture_snapshot(record: TransferRecord, source_path: str | Path,
                     selected_uuids: tuple[str, ...]) -> TransferRecord:
    """Copia a fonte depois da parada confirmada pelo coordenador, sem avançar a fase."""
    if record.phase != TransferPhase.SOURCE_STOPPED:
        raise ValueError("snapshot exige parada confirmada da origem")
    _strings(selected_uuids)
    raw = Path(source_path).read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    snapshot = _base() / "snapshots" / f"{_id(record.id)}-{digest}.jsonl"
    with _locked():
        previous = load_transfer(record.id)
        if previous and (previous.phase != TransferPhase.SOURCE_STOPPED
                         or previous.source_life != record.source_life or previous.name != record.name):
            raise ValueError("a etapa da transferência mudou antes do snapshot")
        record = replace(record, origin_meta=prepare_origin(previous.origin_meta if previous else record.origin_meta))
        _write_bytes(snapshot, raw)
        if hashlib.sha256(snapshot.read_bytes()).hexdigest() != digest:
            raise ValueError("snapshot da origem diverge da cópia")
        captured = replace(record, source=ConversationSource(str(snapshot), "claude", digest,
                                                            selected_uuids))
        _save_locked(_decode(asdict(captured)))
    return captured


def verify_source(source: ConversationSource) -> None:
    if hashlib.sha256(Path(source.path).read_bytes()).hexdigest() != source.digest:
        raise ValueError("snapshot da origem mudou")


def transfer_for_thread(codex_home: str, thread_id: str) -> TransferRecord | None:
    key = _thread_key(codex_home, thread_id)
    for transfer_id in reversed(_index_ids("thread", key)):
        record = load_transfer(transfer_id)
        destination = _destination(record) if record else None
        if destination and _thread_key(*destination) == key:
            return record
    return None


def _belongs(record: TransferRecord, life: str | None, meta: dict | None, source_path: str | None = None) -> bool:
    marked = meta and meta.get("transfer_id") == record.id
    if record.phase == TransferPhase.COMPLETE:
        destination = record.destination_meta or {}
        if not marked and (not life or life != destination.get("lifecycle_id")):
            return False
        # /clear preserva a chave da sessão, mas inicia outra conversa.
        if meta and meta.get("provider") == "codex":
            association = _destination(record)
            if not association or meta.get("thread_id") != association[1]:
                return False
            return (not meta.get("codex_home") or
                    _thread_key(meta["codex_home"], meta["thread_id"]) == _thread_key(*association))
        return True
    if marked:
        return True
    if life != record.source_life or life is None:
        return False
    origin_sid = record.origin_meta.get("session_id")
    if source_path:
        origin_path = record.origin_meta.get("jsonl")
        if origin_path and os.path.realpath(source_path) != os.path.realpath(origin_path):
            return False
        if not origin_path and origin_sid and Path(source_path).stem != origin_sid:
            return False
    if meta and origin_sid and meta.get("session_id") and meta["session_id"] != origin_sid:
        return False
    return True


def transfer_for_session(name: str, *, lifecycle_id=_UNSET, meta: dict | None = None,
                         source_path: str | None = None) -> TransferRecord | None:
    if meta is None:
        from app.adapters.claude_headless import sessions as hl_sessions
        from app.adapters.codex import sessions as cx_sessions
        meta = cx_sessions.load(name) or hl_sessions.load(name)
    life = session_life(name) if lifecycle_id is _UNSET else lifecycle_id
    parked = lifecycle_id is _UNSET and life is None and meta is None
    if meta and meta.get("transfer_id"):
        record = load_transfer(meta["transfer_id"])
        if record is None:
            raise ValueError("sidecar aponta para transferência ausente")
        if record.name == name and _belongs(record, life, meta, source_path):
            return record
    for transfer_id in reversed(_index_ids("session", name)):
        record = load_transfer(transfer_id)
        if record and record.name == name:
            association_life = (record.source_life if parked and record.phase not in _TERMINAL else life)
            if _belongs(record, association_life, meta, source_path):
                return record
    return None


def transfer_active(name: str) -> bool:
    record = transfer_for_session(name)
    return record is not None and record.phase not in _TERMINAL


def list_incomplete() -> list[TransferRecord]:
    try:
        paths = sorted(path for path in _base().iterdir() if path.suffix == ".json")
    except FileNotFoundError:
        return []
    records = [load_transfer(path.stem) for path in paths]
    return [record for record in records if record is not None and record.phase not in _TERMINAL]


_operation_locks: dict[str, threading.Lock] = {}
_ingress_counts: dict[str, int] = {}
# Ingressos que a requisição atual já segura: uma rota guardada que sobe para a troca exclusiva
# (modelo da conta ChatGPT fixa) não pode ser recusada pela própria entrada.
_held_ingress: ContextVar[frozenset[str]] = ContextVar("held_ingress", default=frozenset())


@contextmanager
def session_operation(name: str):
    with _lock:
        operation = _operation_locks.setdefault(name, threading.Lock())
        own = 1 if name in _held_ingress.get() else 0
        if _ingress_counts.get(name, 0) - own or not operation.acquire(blocking=False):
            raise TransferError("session_transfer_busy")
    try:
        yield
    finally:
        operation.release()


# Nomes cuja porta do Rust segue fechada porque a troca ficou em fase não terminal; o
# `save_transfer` terminal reabre.
_gate_held: set[str] = set()


def hold_gate(name: str) -> None:
    with _lock:
        _gate_held.add(name)


def release_gate(name: str) -> None:
    """Chamado de thread (save_transfer): reabre a porta que a troca deixou fechada."""
    with _lock:
        if name not in _gate_held:
            return
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    try:
        if coordinator is not None:
            coordinator.ingress_sync(name, False, held=True)
    except Exception:
        # O Rust segue fechado: o nome fica retido para a próxima chance de reabrir.
        _log.warning("porta do Rust não reabriu ao fim da troca de %s", name, exc_info=True)
        return
    with _lock:
        _gate_held.discard(name)


@asynccontextmanager
async def transfer_operation(name: str):
    """`session_operation` que também fecha a porta do Rust. Roda no laço (as rotas e os dois
    pontos da troca são corrotinas), então usa `await`. Troca não terminal ao sair mantém fechada."""
    import asyncio
    from app import runtime_coordinator
    with session_operation(name):
        coordinator = runtime_coordinator.current()
        own = False
        if coordinator is not None and coordinator.transport is not None and name not in _gate_held:
            try:
                await coordinator.close_ingress(name, held=True)
            except Exception:
                _log.warning("porta do Rust não fechou para a troca de %s", name, exc_info=True)
                raise TransferError("session_transfer_gate_unavailable", status=503) from None
            own = True
        ok = False
        try:
            yield
            ok = True
        finally:
            if own:
                try:
                    active = await asyncio.to_thread(transfer_active, name)
                except Exception:
                    active = True       # sem ler a fase não dá para soltar: a troca retém a porta
                if active:
                    hold_gate(name)
                else:
                    try:
                        await coordinator.ingress(name, False, held=True)
                    except Exception:
                        _log.warning("porta do Rust não reabriu para %s", name, exc_info=True)
                        hold_gate(name)
                        if ok:
                            raise TransferError("session_transfer_gate_unavailable", status=503) from None


@contextmanager
def session_ingress(name: str):
    """Ingressos coexistem; a troca exclusiva só começa depois de seus recibos."""
    with _lock:
        operation = _operation_locks.get(name)
        if operation is not None and operation.locked():
            raise TransferError("session_transfer_busy")
        _ingress_counts[name] = _ingress_counts.get(name, 0) + 1
    held = _held_ingress.get()
    _held_ingress.set(held | {name})
    try:
        yield
    finally:
        _held_ingress.set(held)
        with _lock:
            remaining = _ingress_counts[name] - 1
            if remaining:
                _ingress_counts[name] = remaining
            else:
                _ingress_counts.pop(name, None)


def require_available(name: str) -> None:
    if transfer_active(name):
        raise TransferError("session_transfer_busy")


def public_error(error: TransferError) -> dict:
    messages = {
        "session_transfer_source_changed": "A conversa mudou; abra a seleção de conta novamente.",
        "session_transfer_restore_failed": "A troca falhou e o Claude não voltou. Use Recarregar para tentar recuperar.",
        "session_transfer_busy": "A sessão está trocando de agente; tente novamente quando terminar.",
        "session_transfer_gate_unavailable": "A sessão está terminando uma escrita; tente de novo em instantes.",
        "session_transfer_context_budget_exceeded": "O histórico completo excede a capacidade disponível do modelo escolhido. Escolha outro modelo.",
        "session_transfer_model_capacity_unknown": "Não foi possível confirmar a capacidade de contexto do modelo escolhido.",
        "session_transfer_model_media_unsupported": "O modelo escolhido não aceita as imagens presentes na conversa.",
        "session_transfer_codex_version_unsupported": "A versão instalada do Codex não oferece suporte a esta transferência.",
        "session_transfer_login_required": "Entre na conta Codex escolhida antes de transferir a conversa.",
        "session_transfer_unknown_account": "A conta Codex escolhida não está cadastrada neste servidor.",
        "session_transfer_account_full": "A conta Codex escolhida está sem cota disponível.",
        "session_transfer_account_unavailable": "Não foi possível consultar a conta Codex escolhida.",
        "session_transfer_invalid_model_choice": "O modelo ou esforço escolhido não está disponível no catálogo desta conta.",
        "session_transfer_native_unavailable": "Não foi possível acessar o Codex instalado neste servidor.",
        "session_transfer_queue_pending": "Há mensagens na fila ou entregas ainda não confirmadas. Aguarde a confirmação antes de transferir.",
        "session_transfer_source_busy": "A sessão tem trabalho em execução ou uma pergunta ou permissão pendente.",
        "session_transfer_source_state_unknown": "Não foi possível confirmar que a sessão de origem está parada.",
        "session_transfer_invalid_permission_mode": "Não foi possível comprovar a permissão da origem ou a permissão anterior ao modo Plano.",
        "session_transfer_read_only": "Uma sessão protegida para somente leitura não pode ser transferida.",
        "session_transfer_source_item_too_large": "Um item da conversa excede o limite de transporte do Codex; a transferência integral foi recusada.",
        "session_transfer_source_media_missing": "Uma imagem necessária do histórico não está disponível na origem.",
        "session_transfer_unsupported_media_mime": "O formato de uma mídia da conversa não é aceito pelo Codex.",
        "session_transfer_invalid_media_schema": "Uma mídia da origem está incompleta ou tem formato inválido.",
        "session_transfer_invalid_source_json": "O histórico da origem contém um registro JSON inválido.",
        "session_transfer_invalid_source_schema": "O histórico da origem contém um registro de contexto inválido.",
        "session_transfer_source_partial_line": "O histórico da origem termina com um registro incompleto.",
        "session_transfer_source_unreadable": "Não foi possível ler o histórico da origem.",
        "session_transfer_no_source_context": "Não há contexto disponível na origem para transferir.",
        "session_transfer_no_source_leaf": "Não foi possível identificar a conversa ativa no histórico da origem.",
        "session_transfer_source_leaf_invalid": "A referência à conversa ativa na origem é inválida.",
        "session_transfer_source_parent_missing": "Falta um registro anterior necessário para reconstruir o histórico completo.",
        "session_transfer_source_cycle": "As referências do histórico da origem formam um ciclo inválido.",
        "session_transfer_source_tool_id_ambiguous": "Não foi possível associar com segurança uma chamada ao resultado histórico da ferramenta.",
        "session_transfer_unsupported_context": "A origem contém um tipo de contexto que ainda não pode ser preservado no Codex.",
        "session_transfer_unsupported_source_topology": "A organização do histórico da origem não permite reconstruir a conversa completa.",
        "session_transfer_native_config_changed": "A configuração do Codex mudou durante o preparo. Abra a seleção novamente.",
        "session_transfer_native_settings_mismatch": "As escolhas carregadas pelo Codex não correspondem ao destino confirmado.",
        "session_transfer_import_persistence_mismatch": "O histórico persistido no Codex diverge da origem; a transferência foi recusada.",
        "session_transfer_import_persistence_unconfirmed": "Não foi possível confirmar a gravação integral do histórico no Codex.",
        "session_transfer_native_import_failed": "O Codex recusou a importação do histórico.",
        "session_transfer_history_invalid": "O histórico da transferência está indisponível ou não passou na conferência de integridade.",
        "session_transfer_invalid_record": "O registro da transferência não corresponde à conversa solicitada.",
    }
    message = messages.get(error.code, "Não foi possível transferir a conversa.")
    if error.code == "session_transfer_restore_failed":
        reasons = {"session_transfer_source_changed": "A identidade da origem mudou.",
                   "session_transfer_process_identity_changed": "O processo agora pertence a outra execução.",
                   "session_transfer_process_identity_unknown": "Não foi possível confirmar o dono do processo.",
                   "session_transfer_source_not_stopped": "O processo da origem ainda não encerrou.",
                   "session_transfer_runtime_not_stopped": "O processo de destino ainda não encerrou.",
                   "session_transfer_import_not_stopped": "O importador ainda não encerrou.",
                   "session_transfer_import_cleanup_unconfirmed": "A queda interrompeu a confirmação dos processos filhos do importador.",
                   "session_transfer_runtime_cleanup_unconfirmed": "A queda interrompeu a confirmação dos processos filhos do destino."}
        message += " " + reasons.get(error.params.get("phase"), "Não foi possível confirmar o carregamento da origem.")
    return {"code": error.code, "msg": message,
            "params": error.params}


def _runtime_path(record: TransferRecord) -> Path:
    return _base() / "runtime" / f"{_id(record.id)}.json"


def _runtime(record: TransferRecord) -> dict:
    return json.loads(_runtime_path(record).read_text(encoding="utf-8"))


def prepare_runtime(record: TransferRecord, *, original: dict | None = None,
                    processes: dict | None = None) -> None:
    """Inicializa o manifesto privado; a prova isolada do importador pode omitir a origem física."""
    path = _runtime_path(record)
    if path.exists():
        raise TransferError("session_transfer_runtime_exists")
    _write_json(path, {"original": dict(original or {}), "processes": dict(processes or {})})


def _process_identity(pid: int) -> dict | None:
    from app import procinfo
    born, command = procinfo._proc_start_time(pid), procinfo._cmdline(pid)
    if born is None or not command:
        return None
    return {"started": born, "command": hashlib.sha256(command.encode()).hexdigest()}


def _processes(pid: int | None) -> dict[str, dict]:
    from app import procinfo
    if not pid:
        return {}
    result = {}
    for child in procinfo._descendant_pids(int(pid), procinfo._proc_children_map(max_age=0)):
        if not procinfo.pid_vivo(child):
            continue
        identity = _process_identity(child)
        if identity is None:
            raise TransferError("session_transfer_process_identity_unknown")
        result[str(child)] = identity
    return result


def _processes_stopped(processes: dict[str, dict]) -> bool:
    from app import procinfo
    # PID reaproveitado não é nosso e nunca recebe sinal para completar uma parada.
    for pid, identity in processes.items():
        if procinfo.pid_vivo(int(pid)):
            current = _process_identity(int(pid))
            if current is None or current.get("started") == identity.get("started"):
                return False
    return True


def record_import_process(record: TransferRecord, pid: int) -> None:
    private = _runtime(record)
    private["import_processes"] = {**private.get("import_processes", {}), **_processes(pid)}
    if str(pid) not in private["import_processes"]:
        raise TransferError("session_transfer_process_identity_unknown")
    private["import_pid"] = pid
    private["import_launching"] = False
    _write_json(_runtime_path(record), private)


def mark_import_starting(record: TransferRecord) -> None:
    private = _runtime(record)
    private["import_launching"] = True
    _write_json(_runtime_path(record), private)


def refresh_import_process(record: TransferRecord) -> None:
    from app import procinfo
    private = _runtime(record)
    pid = private.get("import_pid")
    if pid and procinfo.pid_vivo(pid):
        if _process_identity(pid) != private.get("import_processes", {}).get(str(pid)):
            raise TransferError("session_transfer_process_identity_changed")
        record_import_process(record, pid)
        private = _runtime(record)
        private["import_tree_final"] = True
        _write_json(_runtime_path(record), private)
    elif pid and not private.get("import_tree_final"):
        raise TransferError("session_transfer_import_cleanup_unconfirmed")


def confirm_import_exit(record: TransferRecord) -> None:
    private = _runtime(record)
    if private.get("import_launching"):
        raise TransferError("session_transfer_process_identity_unknown")
    if private.get("import_pid") and not private.get("import_tree_final"):
        raise TransferError("session_transfer_import_cleanup_unconfirmed")
    if not _processes_stopped(private.get("import_processes", {})):
        raise TransferError("session_transfer_import_not_stopped")
    private["import_stopped"] = True
    _write_json(_runtime_path(record), private)


async def wait_import_exit(record: TransferRecord, *, timeout: float = 5.0) -> None:
    import asyncio
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while True:
        try:
            await asyncio.to_thread(confirm_import_exit, record)
            return
        except TransferError as exc:
            if exc.code != "session_transfer_import_not_stopped" or loop.time() >= deadline:
                raise
        # O wait da raiz pode terminar antes dos filhos que já receberam a saída.
        await asyncio.sleep(min(0.05, max(0.0, deadline - loop.time())))


async def stop_import_process(record: TransferRecord) -> None:
    import asyncio
    import signal
    from app import procinfo
    from app.registry import _esperar_saida
    private = _runtime(record)
    if private.get("import_launching"):
        raise TransferError("session_transfer_process_identity_unknown")
    if private.get("import_stopped"):
        return
    processes = private.get("import_processes", {})
    root = private.get("import_pid")
    if root and procinfo.pid_vivo(root):
        if _process_identity(root) != processes.get(str(root)):
            raise TransferError("session_transfer_process_identity_changed")
        processes = {**processes, **await asyncio.to_thread(_processes, root)}
    if root and not private.get("import_tree_final"):
        raise TransferError("session_transfer_import_cleanup_unconfirmed")
    for pid, identity in reversed(list(processes.items())):
        if not procinfo.pid_vivo(int(pid)):
            continue
        if _process_identity(int(pid)) != identity:
            raise TransferError("session_transfer_process_identity_changed")
        try:
            os.kill(int(pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
    await asyncio.to_thread(_esperar_saida, [int(pid) for pid in processes])
    if not _processes_stopped(processes):
        raise TransferError("session_transfer_import_not_stopped")
    await asyncio.to_thread(confirm_import_exit, record)


def _map_permission(meta: dict) -> tuple[str, str]:
    current = meta.get("permission_mode")
    base = meta.get("previous_non_plan") if current == "plan" else current
    permission = {"bypassPermissions": "Full Access", "manual": "Ask for approval",
                  "default": "Ask for approval", "acceptEdits": "Approve for me"}.get(base)
    if permission is None:
        raise TransferError("session_transfer_invalid_permission_mode")
    return permission, "plan" if current == "plan" else "default"


def _resolve_target(credential_id: str):
    from app import codex_contas, cotas
    for listed in codex_contas.list_accounts():
        if f"codex:{listed.home.expanduser().resolve(strict=False)}" == credential_id:
            try:
                account = codex_contas.resolve_account(listed.id)
            except codex_contas.AccountError:
                raise TransferError("session_transfer_unknown_account", status=400) from None
            if account.home.resolve(strict=False) != listed.home.resolve(strict=False):
                break
            if cotas.id_conta_codex(account.home) != credential_id:
                raise TransferError("session_transfer_login_required")
            return account
    raise TransferError("session_transfer_unknown_account", status=400)


async def _check_target(account, model: str | None, effort: str | None) -> None:
    import asyncio
    from app import codex_models, codex_appserver, cotas
    from app.adapters.codex.transfer import SUPPORTED_VERSION
    try:
        version = await asyncio.to_thread(codex_appserver.versao)
    except Exception:
        raise TransferError("session_transfer_native_unavailable") from None
    if version != SUPPORTED_VERSION:
        raise TransferError("session_transfer_codex_version_unsupported")
    try:
        state, windows, _ = await asyncio.to_thread(cotas._ler_codex, account.home)
    except Exception:
        raise TransferError("session_transfer_account_unavailable") from None
    if state in {"sem_credencial", "expirada"}:
        raise TransferError("session_transfer_login_required")
    if state == "lida" and any(window.pct >= 99 for window in windows):
        raise TransferError("session_transfer_account_full")
    try:
        await asyncio.to_thread(codex_models.checar_escolha, model, effort, codex_home=account.home)
    except codex_models.CodexRecusado:
        raise TransferError("session_transfer_login_required") from None
    except ValueError:
        raise TransferError("session_transfer_invalid_model_choice") from None
    except Exception:
        raise TransferError("session_transfer_native_unavailable") from None


def _source_info(registry, name: str, source_life: str, source_jsonl: str):
    from app.adapters.claude_headless import sessions
    info = next((row for row in registry.list() if row.name == name), None)
    if (not info or info.provider != "claude" or info.engine or not info.tracked
            or not info.jsonl or session_life(name) != source_life
            or os.path.realpath(info.jsonl) != os.path.realpath(source_jsonl)):
        raise TransferError("session_transfer_source_changed")
    meta = sessions.load(name)
    if meta and Path(info.jsonl).stem != meta.get("session_id"):
        raise TransferError("session_transfer_source_changed")
    return info


def _runtime_source_view(name: str) -> dict | None:
    """Vista do Rust da origem (aberta nele ou guardada no fechamento da troca); None fora dele."""
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is None or getattr(coordinator, "transport", None) is None:
        return None
    try:
        return coordinator.source_view(name)
    except RuntimeError:
        raise TransferError("session_transfer_source_state_unknown") from None


async def _check_source_idle(registry, name: str, meta: dict) -> None:
    import asyncio
    from app import procinfo, tmux
    from app.adapters import get_adapter, CLAUDE_HEADLESS
    from app.pqueue import PromptQueue
    from app.askquestion import read_pending_askq as read_question
    queue = await asyncio.to_thread(PromptQueue(name).load)
    if any(not row.get("delivered") or not row.get("confirmed") for row in queue):
        raise TransferError("session_transfer_queue_pending")
    if meta["headless"]:
        view = _runtime_source_view(name)
        if view is not None:
            if view.get("alive") and (view.get("iniciando") or view.get("in_progress") or view.get("pending") or view.get("question")):
                raise TransferError("session_transfer_source_busy")
        else:
            hl = get_adapter(CLAUDE_HEADLESS)
            sess = await hl.ensure_running(name, so_reconectar=True)
            if sess and sess.vivo and (sess.iniciando or sess.in_progress or sess.pending or sess.question):
                raise TransferError("session_transfer_source_busy")
    else:
        pane = await asyncio.to_thread(registry._pane_of, name)
        if pane is None or pane.get("pid") != meta.get("pane_pid"):
            raise TransferError("session_transfer_source_changed")
        from app.registry import _pid_do_agente, provider_of_pane
        children = await asyncio.to_thread(procinfo._proc_children_map, max_age=0)
        if await asyncio.to_thread(provider_of_pane, pane["pid"], children) != "claude":
            raise TransferError("session_transfer_source_changed")
        agent = await asyncio.to_thread(_pid_do_agente, pane["pid"])
        if agent:
            if await asyncio.to_thread(procinfo._engine_of, agent):
                raise TransferError("session_transfer_source_changed")
            native = Path(meta["config_dir"]) / "sessions" / f"{agent}.json"
            try:
                state = json.loads(await asyncio.to_thread(native.read_text, encoding="utf-8"))
            except (OSError, ValueError):
                raise TransferError("session_transfer_source_state_unknown") from None
            if state.get("sessionId") != meta["session_id"] or state.get("pid") != agent:
                raise TransferError("session_transfer_source_changed")
            if state.get("status") != "idle":
                raise TransferError("session_transfer_source_busy")
    if await asyncio.to_thread(read_question, meta["jsonl"]):
        raise TransferError("session_transfer_source_busy")


async def transfer_claude_to_codex(registry, name: str, credential_id: str, source_life: str,
                                  model: str | None, effort: str | None, *, source_jsonl: str) -> dict:
    import asyncio
    from app.adapters import get_adapter, CLAUDE_HEADLESS
    from app.adapters.claude_headless import sessions as hl_sessions
    from app.adapters.codex import transfer as importer
    from app.claude_to_codex import convert_snapshot, ConversionError
    from app import terminal_input
    async with transfer_operation(name):
        require_available(name)
        await asyncio.to_thread(_source_info, registry, name, source_life, source_jsonl)
        account = await asyncio.to_thread(_resolve_target, credential_id)
        await _check_target(account, model, effort)
        hl = get_adapter(CLAUDE_HEADLESS)
        async with hl.delivery_lock(name):
            terminal_lock = terminal_input._send_lock(name)
            if not terminal_lock.acquire(blocking=False):
                raise TransferError("session_transfer_busy")
            record = None
            stop_attempted = False
            try:
                info = await asyncio.to_thread(_source_info, registry, name, source_life, source_jsonl)
                meta, private = await asyncio.to_thread(registry.transfer_origin, info)
                permission, collaboration = _map_permission(meta)
                await _check_source_idle(registry, name, meta)
                record = TransferRecord(str(uuid.uuid4()), name, source_life, TransferPhase.PREPARING,
                                        None, prepare_origin(meta), None, None, None)
                await asyncio.to_thread(prepare_runtime, record, **private)
                await asyncio.to_thread(save_transfer, record)
                # O estado nativo é relido imediatamente antes da parada; teclas humanas não usam nossas travas.
                await _check_source_idle(registry, name, meta)
                await asyncio.to_thread(_source_info, registry, name, source_life, source_jsonl)
                stop_attempted = True
                await registry.stop_transfer_source(record, private)
                record = replace(record, phase=TransferPhase.SOURCE_STOPPED)
                await asyncio.to_thread(save_transfer, record)
                record = await asyncio.to_thread(capture_snapshot, record, meta["jsonl"], ())
                context = await asyncio.to_thread(convert_snapshot, Path(record.source.path))
                record = replace(record, source=replace(record.source, selected_uuids=tuple(sorted(context.selected_uuids))))
                await asyncio.to_thread(save_transfer, record)
                prepared = await importer.prepare_import(account, meta["cwd"], context, model, effort,
                                                         permission, transfer_id=record.id,
                                                         collaboration_mode=collaboration)
                record = load_transfer(record.id)
                record = replace(record, phase=TransferPhase.IMPORTED,
                                 destination_meta={**record.destination_meta, "name": name, "provider": "codex",
                                    "key": record.origin_meta["key"], "headless": meta["headless"], "jev": bool(meta.get("jev")),
                                    "lifecycle_id": f"k:{record.origin_meta['key']}",
                                    "previous_non_plan": permission if collaboration == "plan" else None})
                await asyncio.to_thread(save_transfer, record)
                await get_adapter("codex").adopt_imported(name, prepared, record.destination_meta,
                                                          terminal=not meta["headless"])
                record = replace(record, phase=TransferPhase.PUBLISHING)
                await asyncio.to_thread(save_transfer, record)
                await registry.publish_transfer(record)
                record = replace(record, phase=TransferPhase.COMPLETE)
                await asyncio.to_thread(save_transfer, record)
                return {"ok": True, "provider": "codex", "conta": credential_id,
                        "model": prepared.model, "effort": prepared.effort, "transfer_id": record.id}
            except BaseException as exc:
                error = (exc if isinstance(exc, TransferError) else
                         TransferError(exc.code) if isinstance(exc, ConversionError) else
                         TransferError("session_transfer_failed"))
                if record is not None:
                    record = load_transfer(record.id) or record
                    if stop_attempted:
                        await _restore_transfer(registry, record, error.code)
                    else:
                        await asyncio.to_thread(save_transfer, replace(record, phase=TransferPhase.REJECTED,
                                                                     error_code=error.code))
                if isinstance(exc, asyncio.CancelledError):
                    raise
                raise error from None
            finally:
                terminal_lock.release()


async def _restore_transfer(registry, record: TransferRecord, cause: str) -> dict:
    import asyncio
    record = replace(record, phase=TransferPhase.RESTORING, error_code=cause)
    await asyncio.to_thread(save_transfer, record)
    try:
        await stop_import_process(record)
        await registry.restore_transfer_source(record, _runtime(record))
    except Exception as exc:
        reason = exc.code if isinstance(exc, TransferError) else "session_transfer_restore_unconfirmed"
        await asyncio.to_thread(save_transfer, replace(record, phase=TransferPhase.RESTORE_FAILED,
                                                       error_code=reason))
        raise TransferError("session_transfer_restore_failed", params={"phase": reason}) from None
    await asyncio.to_thread(save_transfer, replace(record, phase=TransferPhase.ROLLED_BACK))
    return {"ok": True, "provider": "claude", "transfer_id": record.id}


async def recover_transfer(registry, record: TransferRecord) -> dict:
    from app.adapters import get_adapter, CLAUDE_HEADLESS
    from app import terminal_input
    async with transfer_operation(record.name):
        async with get_adapter(CLAUDE_HEADLESS).delivery_lock(record.name):
            terminal_lock = terminal_input._send_lock(record.name)
            if not terminal_lock.acquire(blocking=False):
                raise TransferError("session_transfer_busy")
            try:
                current = load_transfer(record.id)
                if not current or current.phase in _TERMINAL:
                    raise TransferError("session_transfer_source_changed")
                # Um crash antes de COMPLETE sempre recupera a origem; nunca infere commit por PID/data.
                return await _restore_transfer(registry, current, current.error_code or "session_transfer_interrupted")
            finally:
                terminal_lock.release()
