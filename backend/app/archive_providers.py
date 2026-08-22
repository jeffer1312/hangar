"""Onde Pi, Kimi e Codex guardam conversa MORTA -- o equivalente ao projects/ do Claude, que e o
unico que o app.archive conhecia.

Cada um guarda de um jeito, e nenhum guarda como o Claude:

- **Pi**: `~/.pi/agent/sessions/<slug-do-cwd>/<ts>_<uuid>.jsonl`. O slug NAO e reversivel pro cwd
  (barra vira traco), entao o cwd sai de dentro do arquivo, na 1a linha (`{"type":"session",...}`).
  Subagente escreve em `<stem>/<taskId>/run-N/session.jsonl` e nao e conversa -- fica de fora.
- **Kimi**: `~/.kimi-code/sessions/<wd_...>/<session_id>/agents/main/wire.jsonl`, e existe um
  `session_index.jsonl` no home que ja mapeia sessionId -> sessionDir + workDir. E a unica fonte
  aqui que da o cwd SEM abrir transcript nenhum.
- **Codex**: `~/.codex/sessions/<ano>/<mes>/<dia>/rollout-<ts>-<uuid>.jsonl`, sem agrupamento por
  pasta; o cwd vem do `session_meta` da 1a linha.

Resolver o caminho a partir do session_id e por BUSCA (glob), nao por montagem: so o Kimi tem
indice, e o nome do arquivo do Pi e do Codex carrega um timestamp que nao da pra recriar.
"""
import json
import logging
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional

from app.models import ChatEvent

_log = logging.getLogger("claude_pocket.archive_providers")

PROVIDERS = ("pi", "kimi", "codex")

_UUID_RE = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
_KIMI_SID_RE = re.compile(r"^session_[0-9a-fA-F-]{36}$")


@dataclass(frozen=True)
class Conversa:
    provider: str
    cwd: Optional[str]
    session_id: str
    path: Path
    mtime: float


def _mtime(p: Path) -> float:
    try:
        return p.stat().st_mtime
    except OSError:
        return 0.0


def _cwd_do_cabecalho(p: Path, campo: Callable[[dict], Optional[str]], max_linhas: int = 5) -> Optional[str]:
    """cwd lido das PRIMEIRAS linhas do transcript. Pi e Codex gravam na 1a, mas ler algumas a mais
    e barato e cobre um cabecalho que ganhe linha nova."""
    try:
        with open(p, encoding="utf-8", errors="replace") as fh:
            for _, linha in zip(range(max_linhas), fh):
                try:
                    obj = json.loads(linha)
                except (json.JSONDecodeError, ValueError):
                    continue
                if isinstance(obj, dict) and (c := campo(obj)):
                    return c
    except OSError:
        # Sem cwd a conversa e DESCARTADA da listagem (o agrupamento por pasta sai dele), entao uma
        # falha de leitura aqui nao deixa a linha errada: some com ela. Tem que aparecer no log.
        _log.warning("arquivo: nao consegui ler o cabecalho de %s", p, exc_info=True)
        return None
    return None


# ── Pi ────────────────────────────────────────────────────────────────────────
def _pi_conversas() -> list[Conversa]:
    from app.adapters.pi import sessions as pi_sessions
    raiz = pi_sessions.sessions_root()
    out: list[Conversa] = []
    try:
        pastas = [d for d in raiz.iterdir() if d.is_dir()]
    except OSError:
        return []
    for pasta in pastas:
        # glob raso: o transcript da conversa mora DIRETO no slug do cwd. O rglob pegaria tambem o
        # `<stem>/<taskId>/run-N/session.jsonl` dos subagentes, que nao e conversa.
        for f in pasta.glob("*.jsonl"):
            sid = f.stem.split("_", 1)[-1]
            if not _UUID_RE.match(sid):
                continue
            cwd = _cwd_do_cabecalho(f, lambda o: o.get("cwd") if o.get("type") == "session" else None)
            out.append(Conversa("pi", cwd, sid, f, _mtime(f)))
    return out


def _pi_jsonl(session_id: str) -> Optional[Path]:
    from app.adapters.pi import sessions as pi_sessions
    if not _UUID_RE.match(session_id):
        raise ValueError("session_id invalido")
    try:
        achados = sorted(pi_sessions.sessions_root().glob(f"*/*_{session_id}.jsonl"),
                         key=_mtime, reverse=True)
    except OSError:
        return None
    return achados[0] if achados else None


# ── Kimi ──────────────────────────────────────────────────────────────────────
def _kimi_conversas() -> list[Conversa]:
    from app.adapters.kimi import sessions as kimi_sessions
    out: list[Conversa] = []
    try:
        with open(kimi_sessions.kimi_home() / "session_index.jsonl", encoding="utf-8") as fh:
            linhas = fh.readlines()
    except OSError:
        return []
    vistos: set[str] = set()
    # De tras pra frente: o indice e append-only e a MESMA sessao pode reaparecer; a entrada mais
    # recente e a que vale.
    for linha in reversed(linhas):
        try:
            o = json.loads(linha)
        except (json.JSONDecodeError, ValueError):
            continue
        if not isinstance(o, dict):
            continue
        sid, sdir = o.get("sessionId"), o.get("sessionDir")
        if not sid or not sdir or sid in vistos:
            continue
        vistos.add(sid)
        wire = Path(sdir) / "agents" / "main" / "wire.jsonl"
        if not wire.is_file():
            continue    # sessao aberta e nunca usada: a TUI so cria o wire no 1o prompt
        out.append(Conversa("kimi", o.get("workDir"), sid, wire, _mtime(wire)))
    return out


def _kimi_jsonl(session_id: str) -> Optional[Path]:
    if not _KIMI_SID_RE.match(session_id):
        raise ValueError("session_id invalido")
    for c in _kimi_conversas():
        if c.session_id == session_id:
            return c.path
    return None


# ── Codex ─────────────────────────────────────────────────────────────────────
def _codex_raiz() -> Path:
    return Path(os.environ.get("CODEX_HOME") or (Path.home() / ".codex")) / "sessions"


def _codex_cwd(obj: dict) -> Optional[str]:
    if obj.get("type") != "session_meta":
        return None
    payload = obj.get("payload")
    return payload.get("cwd") if isinstance(payload, dict) else None


def _codex_conversas() -> list[Conversa]:
    out: list[Conversa] = []
    try:
        arquivos = list(_codex_raiz().glob("*/*/*/rollout-*.jsonl"))
    except OSError:
        return []
    for f in arquivos:
        sid = f.stem[-36:]
        if not _UUID_RE.match(sid):
            continue
        out.append(Conversa("codex", _cwd_do_cabecalho(f, _codex_cwd), sid, f, _mtime(f)))
    return out


def _codex_jsonl(session_id: str) -> Optional[Path]:
    if not _UUID_RE.match(session_id):
        raise ValueError("session_id invalido")
    try:
        achados = sorted(_codex_raiz().glob(f"*/*/*/rollout-*-{session_id}.jsonl"),
                         key=_mtime, reverse=True)
    except OSError:
        return None
    return achados[0] if achados else None


# ── Fachada ───────────────────────────────────────────────────────────────────
_LISTAR = {"pi": _pi_conversas, "kimi": _kimi_conversas, "codex": _codex_conversas}
_RESOLVER = {"pi": _pi_jsonl, "kimi": _kimi_jsonl, "codex": _codex_jsonl}


def conversas() -> list[Conversa]:
    """Todas as conversas mortas de Pi, Kimi e Codex. Provider que falhar (layout mudou, home
    ausente) sai da lista com um log -- nunca derruba os outros nem o Arquivo do Claude."""
    out: list[Conversa] = []
    for nome, fn in _LISTAR.items():
        try:
            out += fn()
        except Exception:
            _log.warning("arquivo: falha ao varrer conversas de %s", nome, exc_info=True)
    return out


def jsonl_de(provider: str, session_id: str) -> Path:
    """Path do transcript. ValueError = session_id fora do formato daquele provider;
    FileNotFoundError = nao existe. Mesmos erros de archive.archive_jsonl."""
    fn = _RESOLVER.get(provider)
    if fn is None:
        raise ValueError("provider invalido")
    p = fn(session_id)
    if p is None:
        raise FileNotFoundError(session_id)
    return p


def parse_obj(provider: str, obj: dict) -> list[ChatEvent]:
    if provider == "pi":
        from app.adapters.pi import transcript as pi_transcript
        return pi_transcript.parse_obj(obj)
    if provider == "kimi":
        from app.adapters.kimi import transcript as kimi_transcript
        return kimi_transcript.parse_obj(obj)
    if provider == "codex":
        from app.adapters.codex import rollout as codex_rollout
        return codex_rollout.parse_rollout_obj(obj)
    raise ValueError("provider invalido")
