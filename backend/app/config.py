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


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="CP_", env_file=".env")

    # Loopback by default: portable (exists on every machine) and safe (not exposed to
    # the network unless you opt in). Set CP_LAN_BIND_IP to your LAN IP for phone access.
    lan_bind_ip: str = "127.0.0.1"
    port: int = 8765
    auth_token: str = "change-me"
    projects_dir: Path = _default_projects_dir()
    poll_interval: float = 0.75
    front_port: int = 5173   # where the PWA is served (vite dev / Caddy) — used for QR pairing
    public_url: str = ""     # CP_PUBLIC_URL: overrides the auto-built pairing base URL


settings = Settings()


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
