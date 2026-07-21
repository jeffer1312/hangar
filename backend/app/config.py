import os
import socket
from pathlib import Path
from pydantic import AliasChoices, BaseModel, Field
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
    model_config = SettingsConfigDict(env_prefix="CP_", env_file=".env", extra="ignore")

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
    # CP_SERVER_ID: id DESTA maquina no peers.json (mesmo que o cp-send usa). Vazio = pareamento
    # cross-server desligado; quando setado, vira o endereco de resposta 'srv::sessao' que o backend
    # remoto recebe pra registrar o vinculo reverso. Recado 1:1 cross-server segue so no cp-send.
    server_id: str = ""
    # Web Push (notificacao quando uma sessao fica awaiting_input). Par VAPID COMPARTILHADO entre os
    # servidores (single-user controla todos -> uma inscricao do celular serve os 3). Vazio = push
    # desligado (degrada gracioso: subscribe vira no-op). CP_VAPID_PUBLIC/PRIVATE/SUBJECT.
    vapid_public: str = ""
    vapid_private: str = ""
    vapid_subject: str = "mailto:claude-pocket@local"
    # Push extras (feature #2): "terminou" (working->idle apos turno longo) e "caiu" (dead). Cada um
    # com seu flag on/off; default ligado (mesmo padrao do awaiting, que nao tem flag proprio).
    notify_finished: bool = True   # CP_NOTIFY_FINISHED
    notify_dead: bool = True       # CP_NOTIFY_DEAD
    finish_min_seconds: int = 45   # CP_FINISH_MIN_SECONDS: debounce — turno mais curto que isso nao avisa
    # Watchdog de sessao travada (feature #7): "working" silencioso ha muito tempo (loop infinito de
    # ferramenta, subprocesso esperando stdin) nunca vira awaiting/finished/dead -> nunca pinga sozinho.
    # 300s de default: ha chamada de ferramenta longa legitima (build, teste) — muito baixo geraria
    # falso-positivo. CP_STALL_POLL_SECONDS: intervalo do watchdog que varre a lista.
    stall_seconds: int = 300         # CP_STALL_SECONDS
    stall_poll_seconds: int = 30     # CP_STALL_POLL_SECONDS
    # Auto-resume (feature #8, opt-in): quando uma sessao LIMITED tem fila nao-entregue e o reset
    # parseia num horario confiavel, arma um Timer pra drenar sozinha no reset. Default OFF -- e uma
    # acao UNATTENDED (manda prompt sem o usuario olhar); liga so quem quer. CP_AUTO_RESUME.
    auto_resume: bool = False
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
    # CP_EDITOR: binario do editor pro "abrir pasta no editor" (menu da sessao). So-desktop: abre na
    # maquina que roda o backend. Binario unico (sem args/shell) -> exec seguro com o cwd da sessao.
    editor: str = "code"
    # Kill-switch MESTRE (feature #12) pra qualquer acao autonoma sem o usuario olhar: encadeamento de
    # sessao (chain.py) e auto-resume (feature #8, stall_watch.py). Default ON (as duas features ja tem
    # seu proprio opt-in/config — isto e um portao ADICIONAL, nao substitui CP_AUTO_RESUME). CP_AUTOMATIONS=0
    # desliga tudo de uma vez (ex: antes de um teste manual, ou se uma automacao ficar barulhenta).
    automations: bool = True
    # Chave da Groq pra transcricao de audio (whisper-large-v3-turbo). Aceita CP_GROQ_API_KEY (padrao
    # do .env, com prefixo) OU GROQ_API_KEY (convencao do Groq/OpenAI SDK, ex: no Environment do systemd).
    # Vazio = transcricao desligada (o endpoint /transcribe responde 503). Ver docs/USAGE.md.
    groq_api_key: str = Field("", validation_alias=AliasChoices("CP_GROQ_API_KEY", "GROQ_API_KEY"))


settings = Settings()


def automations_enabled() -> bool:
    """Kill-switch mestre: True = automacoes desatendidas (encadeamento de sessao, auto-resume) podem
    disparar. Cada feature ainda mantem seu proprio flag por cima (ex: auto_resume exige ESTE + CP_AUTO_RESUME)."""
    return settings.automations


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
