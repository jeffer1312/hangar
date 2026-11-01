"""Onde Pi, omp, Kimi e Codex guardam conversa MORTA -- o equivalente ao projects/ do Claude, que e
o unico que o app.archive conhecia.

Cada um guarda de um jeito, e nenhum guarda como o Claude:

- **Pi** (e o fork **omp**, mesmo formato, raiz propria via `pi_sessions.sessions_root`):
  `~/.pi/agent/sessions/<slug-do-cwd>/<ts>_<uuid>.jsonl`. O slug NAO e reversivel pro cwd
  (barra vira traco), entao o cwd sai de dentro do arquivo, na 1a linha (`{"type":"session",...}`).
  Subagente escreve em `<stem>/<taskId>/run-N/session.jsonl` (Pi) ou `<stem>/<Nome>.jsonl` (omp)
  e nao e conversa -- fica de fora do glob raso.
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
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional

from app import codex_contas

_log = logging.getLogger("hangar.archive_providers")

PROVIDERS = ("pi", "omp", "kimi", "codex")

# Publico: e o id que identifica uma conversa nestes providers, e quem retoma uma do Arquivo tem
# que validar pelo MESMO criterio (o retomar do Arquivo, no Rust). Duas definicoes de "id
# valido" faziam um id de 36 caracteres fora do formato passar numa e cair na outra, trocando a
# mensagem que explica o problema por um "caminho invalido" generico.
UUID_RE = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
_KIMI_SID_RE = re.compile(r"^session_[0-9a-fA-F-]{36}$")


@dataclass(frozen=True)
class Conversa:
    provider: str
    cwd: Optional[str]
    session_id: str
    path: Path
    mtime: float
    # Origem confiável do rollout Codex. `None` mantém fixtures e sidecars antigos compatíveis.
    codex_home: Optional[str] = None


def _mtime(p: Path) -> float:
    try:
        return p.stat().st_mtime
    except OSError:
        return 0.0


# O cabeçalho não muda depois de gravado, e o do Codex chega a centenas de KB: sem isto o índice
# relia todos os rollouts a cada passada. Chave com inode: arquivo recriado no mesmo caminho relê.
# Sem o extrator na chave: cada caminho é de um provider só, e o do Pi é um lambda novo por chamada.
# Entrada de arquivo apagado fica: cresce com o total de transcripts, não com o tempo.
_CWD_CACHE: dict[tuple[str, int], str] = {}


def _cwd_do_cabecalho(p: Path, campo: Callable[[dict], Optional[str]], max_linhas: int = 5) -> Optional[str]:
    """cwd lido das PRIMEIRAS linhas do transcript. Pi e Codex gravam na 1a, mas ler algumas a mais
    cobre um cabecalho que ganhe linha nova."""
    try:
        chave = (str(p), p.stat().st_ino)
    except OSError:
        chave = None
    if chave is not None and (cwd := _CWD_CACHE.get(chave)) is not None:
        return cwd
    cwd = _ler_cwd_do_cabecalho(p, campo, max_linhas)
    # Sem cwd não guarda: o cabeçalho pode ainda não ter sido escrito.
    if chave is not None and cwd is not None:
        _CWD_CACHE[chave] = cwd
    return cwd


def _ler_cwd_do_cabecalho(p: Path, campo: Callable[[dict], Optional[str]], max_linhas: int) -> Optional[str]:
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
def _pi_conversas(provider: str = "pi") -> list[Conversa]:
    from app.adapters.pi import sessions as pi_sessions
    raiz = pi_sessions.sessions_root(provider)
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
            if not UUID_RE.match(sid):
                continue
            cwd = _cwd_do_cabecalho(f, lambda o: o.get("cwd") if o.get("type") == "session" else None)
            out.append(Conversa(provider, cwd, sid, f, _mtime(f)))
    return out


def _pi_jsonl(session_id: str, provider: str = "pi") -> Optional[Path]:
    from app.adapters.pi import sessions as pi_sessions
    if not UUID_RE.match(session_id):
        raise ValueError("session_id invalido")
    try:
        achados = sorted(pi_sessions.sessions_root(provider).glob(f"*/*_{session_id}.jsonl"),
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
def _codex_raizes(home: Path | str) -> tuple[Path, Path]:
    base = Path(home).expanduser().absolute()
    return base / "sessions", base / "archived_sessions"


def _codex_contas(account_id: str | None = None) -> list[codex_contas.Account]:
    if account_id is not None:
        return [codex_contas.resolve_account(account_id)]
    return codex_contas.list_accounts()


def _codex_rollouts(home: Path | str) -> list[Path]:
    out: list[Path] = []
    vistos: set[Path] = set()
    for raiz in _codex_raizes(home):
        try:
            arquivos = raiz.rglob("rollout-*.jsonl") if raiz.is_dir() else ()
            for path in arquivos:
                try:
                    canonical = path.resolve(strict=True)
                except OSError:
                    continue
                if canonical in vistos:
                    continue
                vistos.add(canonical)
                out.append(path)
        except OSError:
            _log.warning("arquivo: nao consegui varrer raiz Codex %s", raiz, exc_info=True)
    return out


def _codex_cwd(obj: dict) -> Optional[str]:
    if obj.get("type") != "session_meta":
        return None
    payload = obj.get("payload")
    return payload.get("cwd") if isinstance(payload, dict) else None


def _codex_conversas(codex_account: str | None = None) -> list[Conversa]:
    out: list[Conversa] = []
    todas = codex_contas.list_accounts()
    for account in _codex_contas(codex_account):
        try:
            arquivos = _codex_rollouts(account.home)
        except OSError:
            _log.warning("arquivo: conta Codex ilegivel %s", account.id, exc_info=True)
            continue
        for f in arquivos:
            sid = f.stem[-36:]
            if not UUID_RE.match(sid):
                continue
            try:
                owner = codex_contas.account_for_rollout(f, todas)
            except codex_contas.AccountError:
                raise
            if owner is None or owner.id != account.id:
                continue
            from app.conversation_transfer import transfer_for_thread, TransferPhase
            transfer = transfer_for_thread(str(owner.home), sid)
            if transfer and transfer.phase != TransferPhase.COMPLETE:
                continue
            out.append(Conversa("codex", _cwd_do_cabecalho(f, _codex_cwd), sid, f, _mtime(f),
                                str(owner.home.resolve(strict=False))))
    return out


def _codex_jsonl(session_id: str, codex_account: str | None = None) -> Optional[Path]:
    if not UUID_RE.match(session_id):
        raise ValueError("session_id invalido")
    achados: list[tuple[Path, codex_contas.Account]] = []
    for account in _codex_contas(codex_account):
        for path in _codex_rollouts(account.home):
            if not path.name.endswith(f"-{session_id}.jsonl"):
                continue
            owner = codex_contas.account_for_rollout(path)
            if owner is not None and owner.id == account.id:
                achados.append((path, owner))
    if codex_account is not None and not achados:
        # O filtro escolhe a raiz, mas não pode transformar uma conversa existente noutra conta em
        # 404: a origem comprovada é um conflito explícito antes da retomada.
        for account in _codex_contas():
            if account.id == codex_account:
                continue
            for path in _codex_rollouts(account.home):
                if not path.name.endswith(f"-{session_id}.jsonl"):
                    continue
                owner = codex_contas.account_for_rollout(path)
                if owner is not None and owner.id != codex_account:
                    raise codex_contas.AccountError(
                        409,
                        "codex_account_archive_mismatch",
                        {"account_id": codex_account, "origin_account": owner.id},
                    )
    ids = {owner.id for _, owner in achados}
    if codex_account is None and len(ids) > 1:
        raise codex_contas.AccountError(
            409,
            "codex_account_ambiguous_rollout",
            {"session_id": session_id, "accounts": sorted(ids)},
        )
    achados.sort(key=lambda item: _mtime(item[0]), reverse=True)
    return achados[0][0] if achados else None


# ── Fachada ───────────────────────────────────────────────────────────────────
_LISTAR = {"pi": _pi_conversas, "omp": lambda: _pi_conversas("omp"),
           "kimi": _kimi_conversas, "codex": _codex_conversas}
_RESOLVER = {"pi": _pi_jsonl, "omp": lambda sid: _pi_jsonl(sid, "omp"),
             "kimi": _kimi_jsonl, "codex": _codex_jsonl}


def conversas(codex_account: str | None = None) -> list[Conversa]:
    """Todas as conversas mortas de Pi, Kimi e Codex. Provider que falhar (layout mudou, home
    ausente) sai da lista com um log -- nunca derruba os outros nem o Arquivo do Claude."""
    out: list[Conversa] = []
    for nome, fn in _LISTAR.items():
        try:
            out += _codex_conversas(codex_account) if nome == "codex" else fn()
        except Exception:
            _log.warning("arquivo: falha ao varrer conversas de %s", nome, exc_info=True)
    return out


def jsonl_de(provider: str, session_id: str, codex_account: str | None = None) -> Path:
    """Path do transcript. ValueError = session_id fora do formato daquele provider;
    FileNotFoundError = nao existe. Mesmos erros de archive.archive_jsonl."""
    fn = _RESOLVER.get(provider)
    if fn is None:
        raise ValueError("provider invalido")
    p = fn(session_id, codex_account=codex_account) if provider == "codex" else fn(session_id)
    if p is None:
        raise FileNotFoundError(session_id)
    return p
