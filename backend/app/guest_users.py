# backend/app/guest_users.py
"""Convidados deste servidor: login próprio no Hangar que só enxerga as sessões dele.

Delimita a INTERFACE, não é fronteira de segurança: o convidado roda como o mesmo usuário do
sistema que o dono (mesma premissa do compartilhamento de sessão).
"""
from __future__ import annotations

import contextvars
from contextlib import contextmanager
import hashlib
import json
import logging
import os
import secrets
import tempfile
import threading
import time
from dataclasses import asdict, dataclass
from pathlib import Path

from app import atomico
from app.config import _backend_config_base
from app.share_life import session_life

_log = logging.getLogger(__name__)

_FILE = "guests.json"
_path_override: Path | None = None
_lock = threading.RLock()
_state: dict | None = None
# O `hangar-server` também grava (o dono da sessão que ele abre): relê quando o arquivo muda.
_state_sig: tuple | None = None
# Arquivo existe mas não foi lido: gravar por cima apagaria todos os convidados.
_unreadable = False
# ponytail: vida cacheada 2 s por nome; cada consulta é um fork do tmux.
_LIFE_TTL = 2.0
_life_cache: dict[str, tuple[float, str | None]] = {}

current: contextvars.ContextVar["Guest | None"] = contextvars.ContextVar("hangar_guest_user", default=None)


class GuestError(Exception):
    def __init__(self, reason: str):
        super().__init__(reason)
        self.reason = reason


@dataclass
class Guest:
    id: str
    name: str
    token_hash: str
    root: str
    sees_owner: bool = False
    owner_sees: bool = True


def _path() -> Path:
    return _path_override or Path(_backend_config_base()) / _FILE


def _hash(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def _sig() -> tuple | None:
    try:
        st = _path().stat()
    except OSError:
        return None
    return st.st_mtime_ns, st.st_size, st.st_ino


def _reset() -> None:
    global _state, _unreadable
    with _lock:
        _state = None
        _unreadable = False
        _life_cache.clear()


def _load() -> dict:
    global _state, _unreadable, _state_sig
    with _lock:
        if _state is None or _sig() != _state_sig:
            _state_sig, _unreadable = _sig(), False
            try:
                raw = json.loads(atomico.read(_path()).decode("utf-8"))
                _state = {"guests": {g["id"]: Guest(**g) for g in raw.get("guests", [])},
                          "sessions": dict(raw.get("sessions", {}))}
            except FileNotFoundError:
                _state = {"guests": {}, "sessions": {}}
            except (OSError, ValueError, TypeError, KeyError) as e:
                # Ilegível: ninguém entra como convidado, o dono segue normal e nada grava por cima.
                _log.warning("[guests] %s ilegivel; sem convidados e sem gravar ate corrigir: %s",
                             _path(), e)
                _state = {"guests": {}, "sessions": {}}
                _unreadable = True
                if isinstance(e, OSError):
                    # Leitura que falhou no meio da troca (Windows): a próxima chamada tenta de novo.
                    _state_sig = None
        return _state


def _save() -> None:
    st = _load()
    if _unreadable:
        raise GuestError("arquivo_ilegivel")
    destino = _path()
    destino.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(destino.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump({"guests": [asdict(g) for g in st["guests"].values()],
                       "sessions": st["sessions"]}, fh, indent=2)
        # 0600 antes do rename: o arquivo nunca fica visível com permissão aberta.
        try:
            os.chmod(tmp, 0o600)
        except OSError:
            pass
        atomico.substituir(tmp, destino)
        global _state_sig
        _state_sig = _sig()
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


@contextmanager
def _writing():
    """Toda gravação: a mesma `guests.lock` do Rust, relendo o arquivo dentro dela."""
    _path().parent.mkdir(parents=True, exist_ok=True)
    with _lock, atomico.locked(_path().with_name("guests.lock")):
        yield _load()


def _valid_root(root: str) -> str:
    real = os.path.realpath(os.path.expanduser(root))
    if not os.path.isdir(real):
        raise GuestError("pasta_inexistente")
    return real


def create(name: str, root: str, sees_owner: bool, owner_sees: bool) -> tuple[Guest, str]:
    real = _valid_root(root)
    token = secrets.token_urlsafe(32)
    g = Guest(id=secrets.token_hex(8), name=name.strip(), token_hash=_hash(token), root=real,
              sees_owner=sees_owner, owner_sees=owner_sees)
    with _writing() as st:
        st["guests"][g.id] = g
        _save()
    return g, token


def update(gid: str, root: str, sees_owner: bool, owner_sees: bool) -> Guest:
    real = _valid_root(root)
    with _writing() as st:
        g = st["guests"].get(gid)
        if g is None:
            raise GuestError("convidado_inexistente")
        g.root, g.sees_owner, g.owner_sees = real, sees_owner, owner_sees
        _save()
        return g


def delete(gid: str) -> None:
    with _writing() as st:
        if st["guests"].pop(gid, None) is None:
            raise GuestError("convidado_inexistente")
        # As sessões que ele deixou voltam a ser do dono.
        st["sessions"] = {n: e for n, e in st["sessions"].items() if e["guest"] != gid}
        _save()


def lookup_token(token: str) -> Guest | None:
    if not token:
        return None
    h = _hash(token)
    return next((g for g in _load()["guests"].values()
                 if secrets.compare_digest(g.token_hash, h)), None)


def _life(name: str) -> str | None:
    now = time.monotonic()
    hit = _life_cache.get(name)
    if hit and now - hit[0] < _LIFE_TTL:
        return hit[1]
    life = session_life(name)
    _life_cache[name] = (now, life)
    return life


def claim(session: str, gid: str) -> None:
    # Logo após criar, o tmux pode ainda não responder a vida; tenta por até ~0,6 s.
    life = None
    for _ in range(3):
        life = session_life(session)
        if life is not None:
            break
        time.sleep(0.2)
    # ponytail: sem poda; entrada de sessão morta é ignorada pela vida diferente. Podar se o arquivo pesar.
    with _writing() as st:
        st["sessions"][session] = {"guest": gid, "life": life}
        _save()
        _life_cache.pop(session, None)


def set_life(session: str, life: str | None) -> None:
    # Trocar terminal <-> sem terminal muda a vida; sem isto a sessão deixaria de ser do convidado.
    if life is None:
        return
    with _writing() as st:
        entry = st["sessions"].get(session)
        if entry is None:
            return
        entry["life"] = life
        _save()
        _life_cache.pop(session, None)


def has_claims() -> bool:
    return bool(_load()["sessions"])


def owner_of(session: str) -> Guest | None:
    st = _load()
    entry = st["sessions"].get(session)
    if not entry:
        return None
    guest = st["guests"].get(entry["guest"])
    if guest is None:
        return None
    life = _life(session)
    if entry["life"] is None and life is not None:
        # Leitura nunca falha por causa da escrita: a vida adotada fica na memória e o disco
        # tenta de novo na próxima gravação.
        try:
            with _writing() as fresh:
                if cur := fresh["sessions"].get(session):
                    cur["life"] = life
                _save()
        except (OSError, GuestError) as e:
            entry["life"] = life
            _log.warning("[guests] nao gravou a vida de %s: %s", session, e)
        return guest
    # Vida desconhecida (tmux sem responder) mantém o dono registrado; só OUTRA vida desfaz.
    return guest if life is None or life == entry["life"] else None


def owner_name(session: str) -> str | None:
    g = owner_of(session)
    return g.name if g else None


def visible_to(guest: Guest | None, session: str) -> bool:
    owner = owner_of(session)
    if guest is None:
        return owner is None or owner.owner_sees
    if owner is None:
        return guest.sees_owner
    return owner.id == guest.id


def filter_visible(guest: Guest | None, items: list, name_of) -> list:
    if guest is None and not has_claims():
        return list(items)
    return [x for x in items if visible_to(guest, name_of(x))]


def inside_root(guest: Guest, path: str) -> bool:
    real = Path(os.path.realpath(os.path.expanduser(path)))
    return real.is_relative_to(guest.root)
