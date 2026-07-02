import os
import socket
from pathlib import Path
from pydantic import BaseModel
from pydantic_settings import BaseSettings, SettingsConfigDict

_LOOPBACK = {"127.0.0.1", "localhost", "::1", "0.0.0.0", "auto"}


class ConfigDirInfo(BaseModel):
    path: str
    label: str
    active: bool


def _backend_config_base() -> Path:
    return Path(os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude"))


def _label_for(path: Path) -> str:
    stripped = path.name.removeprefix(".claude-").removeprefix(".claude")
    return stripped or "default"


def _is_config_dir(p: Path) -> bool:
    return (p / ".credentials.json").is_file() and (p / "projects").is_dir()


def _projects_mtime(p: Path) -> float:
    # Varre recursivamente pra pegar o arquivo mais recente (nao so o dir imediato)
    try:
        return max((f.stat().st_mtime for f in (p / "projects").rglob("*") if f.is_file()), default=0.0)
    except OSError:
        return 0.0


def list_config_dirs() -> list[ConfigDirInfo]:
    """Config dirs do Claude pra escolher na criacao. Hibrido: CP_CLAUDE_CONFIG_DIRS ('label:path'
    por virgula) tem prioridade; senao auto-scan de ~/.claude* com login + projects/, label pelo
    basename, ordenado por recencia (backup/abandonado afundam)."""
    active_base = _backend_config_base().resolve()
    raw = os.environ.get("CP_CLAUDE_CONFIG_DIRS", "").strip()
    entries: list[tuple[str, Path]] = []
    if raw:
        for item in raw.split(","):
            item = item.strip()
            if not item:
                continue
            label, sep, path = item.partition(":")
            if not sep:
                path, label = label, ""
            p = Path(os.path.expanduser(path)).resolve()
            entries.append((label.strip() or _label_for(p), p))
    else:
        found = [p.resolve() for p in Path.home().glob(".claude*") if p.is_dir() and _is_config_dir(p)]
        found.sort(key=_projects_mtime, reverse=True)
        entries = [(_label_for(p), p) for p in found]
    out, seen = [], set()
    for label, p in entries:
        s = str(p)
        if s in seen:
            continue
        seen.add(s)
        out.append(ConfigDirInfo(path=s, label=label, active=(p == active_base)))
    return out


def _default_projects_dir() -> Path:
    # Claude Code writes transcripts under $CLAUDE_CONFIG_DIR/projects when that env
    # is set, else ~/.claude/projects. Don't hardcode — CLAUDE_CONFIG_DIR varies per
    # machine/user. CP_PROJECTS_DIR still overrides this when set.
    base = os.environ.get("CLAUDE_CONFIG_DIR") or (Path.home() / ".claude")
    return Path(base) / "projects"


# Allowlist padrao do scanner de pastas: estas raizes sao o PERIMETRO DE SEGURANCA da
# varredura. Editavel via CP_SCAN_ROOTS (lista separada por virgula, ~ expandido). Edicao
# das raizes dentro do app fica pra depois: por ora o env e a superficie editavel.
_DEFAULT_SCAN_ROOTS = "~/pessoal,~/sistemas"


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="CP_", env_file=".env")

    # Loopback by default: portable (exists on every machine) and safe (not exposed to
    # the network unless you opt in). Set CP_LAN_BIND_IP to your LAN IP for phone access.
    lan_bind_ip: str = "127.0.0.1"
    port: int = 8765
    auth_token: str = "change-me"
    projects_dir: Path = _default_projects_dir()
    # CP_SCAN_ROOTS: raizes que o fs-scanner pode listar (string crua; resolvida por
    # resolve_scan_roots). Mantida como str pra aceitar o formato "a,b" direto do env.
    scan_roots: str = _DEFAULT_SCAN_ROOTS
    reload: bool = False     # CP_RELOAD=1: uvicorn auto-reload no dev (NUNCA em prod). Default off.
    front_port: int = 5173   # where the PWA is served (vite dev / Caddy) — used for QR pairing
    public_url: str = ""     # CP_PUBLIC_URL: overrides the auto-built pairing base URL
    # Web Push (notificacao quando uma sessao fica awaiting_input). Par VAPID COMPARTILHADO entre os
    # servidores (single-user controla todos -> uma inscricao do celular serve os 3). Vazio = push
    # desligado (degrada gracioso: subscribe vira no-op). CP_VAPID_PUBLIC/PRIVATE/SUBJECT.
    vapid_public: str = ""
    vapid_private: str = ""
    vapid_subject: str = "mailto:claude-pocket@local"
    # Cloud sync hub (opt-in). CP_SYNC=1 turns THIS backend into the sync hub: it mounts /api/sync/*.
    # Stores only salt + auth verifier + ciphertext (zero-knowledge; tokens are encrypted client-side).
    sync: bool = False
    sync_bootstrap: str = ""        # CP_SYNC_BOOTSTRAP: one-time secret to gate first registration
    sync_data: Path = Path.home() / ".claude-pocket" / "sync-vault.json"
    sync_session_secret: str = ""   # CP_SYNC_SESSION_SECRET: HMAC key for the session cookie; empty -> random at boot
    sync_rate_max: int = 10         # CP_SYNC_RATE_MAX: failed sync logins allowed in the window before 429
    sync_rate_window: int = 300     # CP_SYNC_RATE_WINDOW: rate-limit window in seconds (default 5 min)
    # CP_FORWARDED_ALLOW_IPS: proxy IP(s) uvicorn trusts for X-Forwarded-For/-Proto, so the real client
    # IP (rate limiter) and the https scheme (cookie Secure) are seen behind a TLS proxy. "*" trusts any
    # upstream (only safe when nothing untrusted can reach the port directly).
    forwarded_allow_ips: str = "127.0.0.1"
    # CP_DEPLOY_SECRET: shared secret do webhook do GitHub (HMAC-SHA256). Vazio = endpoint de auto-deploy
    # desligado (retorna 404). O push na main dispara /api/deploy/github-webhook -> valida a assinatura ->
    # start (nao-bloqueante) da unit systemd 'claude-pocket-deploy.service' (pull + build + restart).
    deploy_secret: str = ""


settings = Settings()


def resolve_scan_roots(s: "Settings") -> list[Path]:
    """Allowlist resolvida do scanner: cada entrada de CP_SCAN_ROOTS vira expanduser +
    realpath. Entradas inexistentes ou que nao sao diretorio sao descartadas (um typo
    nunca alarga o perimetro), e duplicatas (apos realpath) sao colapsadas. ESTA lista
    e a fronteira de seguranca: o fs-scan so lista dentro dela."""
    out: list[Path] = []
    seen: set[Path] = set()
    for entry in s.scan_roots.split(","):
        entry = entry.strip()
        if not entry:
            continue
        p = Path(os.path.realpath(os.path.expanduser(entry)))
        if p in seen or not p.is_dir():
            continue
        seen.add(p)
        out.append(p)
    return out


def detect_lan_ip() -> str:
    """Best-effort primary LAN IP. Opens a UDP socket toward a public address to find
    which local interface egress traffic would use — no packet is actually sent."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("8.8.8.8", 80))
            return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"


def resolve_bind_ip(s: "Settings") -> str:
    """The address uvicorn should bind. 'auto' -> the detected LAN IP."""
    return detect_lan_ip() if s.lan_bind_ip == "auto" else s.lan_bind_ip


def pairing_url(s: "Settings") -> str:
    """The URL a phone should open (QR target): the PWA front + the auth token.

    The phone reaches the PWA (vite/Caddy), not the API directly, so this points at the
    front. When the bind is loopback/auto/0.0.0.0 we substitute the detected LAN IP so the
    phone has something routable.
    """
    if s.public_url:
        base = s.public_url.rstrip("/")
    else:
        host = detect_lan_ip() if s.lan_bind_ip in _LOOPBACK else s.lan_bind_ip
        base = f"http://{host}:{s.front_port}"
    return f"{base}/?token={s.auth_token}"
