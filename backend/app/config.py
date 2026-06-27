import os
import socket
from pathlib import Path
from pydantic_settings import BaseSettings, SettingsConfigDict

_LOOPBACK = {"127.0.0.1", "localhost", "::1", "0.0.0.0", "auto"}


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
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("8.8.8.8", 80))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


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
