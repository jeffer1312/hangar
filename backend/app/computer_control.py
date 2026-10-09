"""Liga, desliga e configura o MCP `hangar-computer-control` (opera um desktop Windows por objetivo).

A entrada mora em `mcpServers` do `~/.claude.json` principal e do `.claude.json` de cada conta. O
espelho das contas só acrescenta, nunca apaga, então quem desliga precisa tirar de cada arquivo. Só
a chave desse MCP é tocada: o resto do arquivo é estado do CLI daquela conta.
"""
import json
import logging
import os
import re
import shlex
import shutil
import socket
import subprocess
import urllib.error
import urllib.request
import uuid
from pathlib import Path

from app import atomico
from app.config import list_config_dirs

_log = logging.getLogger("hangar.computer_control")

NAME = "hangar-computer-control"
REPO = "jeffer1312/hangar-computer-control"
PRESET_URL = "http://127.0.0.1:8317/v1/chat/completions"
EFFORTS = ("", "low", "medium", "high")


def _install_dir() -> Path:
    return Path.home() / ".hangar" / "computer-control"


def _package_targets() -> Path:
    return _install_dir() / "targets"


def _package_exe() -> Path:
    return _install_dir() / "windows-agent.exe"


def _package_binary() -> Path:
    return _package_exe() if os.name == "nt" else _install_dir() / NAME


class ComputerControlError(Exception):
    def __init__(self, status: int, code: str, msg: str, **params):
        super().__init__(msg)
        self.status, self.code, self.msg, self.params = status, code, msg, params


def _main_file() -> Path:
    return Path.home() / ".claude.json"


def _config_files() -> list[Path]:
    """`~/.claude.json` e o `.claude.json` de cada conta que já tem um (conta nunca aberta ganha
    a entrada pelo espelho na primeira reconciliação)."""
    default_dir = (Path.home() / ".claude").resolve()
    files = [_main_file()]
    for c in list_config_dirs():
        d = Path(c.path)
        if d.resolve() != default_dir and (d / ".claude.json").is_file():
            files.append(d / ".claude.json")
    return files


def _read(p: Path) -> dict:
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as e:
        error = e.strerror if isinstance(e, OSError) else "JSON ou encoding inválido"
        raise ComputerControlError(500, "erro_computer_control_read", f"não consegui ler {p}: {error}",
                                   file=str(p), error=error) from e
    if not isinstance(data, dict):
        raise ComputerControlError(500, "erro_computer_control_read", f"{p} não é um objeto JSON",
                                   file=str(p), error="")
    return data


def _write(p: Path, data: dict) -> None:
    tmp = p.with_name(f"{p.name}.hangar-novo.{os.getpid()}.{uuid.uuid4().hex[:8]}")
    tmp.write_text(json.dumps(data, indent=2), encoding="utf-8")
    atomico.substituir(tmp, p)


def _entry(p: Path) -> dict | None:
    servers = _read(p).get("mcpServers") or {}
    if not isinstance(servers, dict):
        raise ComputerControlError(500, "erro_computer_control_read", f"mcpServers em {p} não é um objeto JSON",
                                   file=str(p), error="mcpServers inválido")
    e = servers.get(NAME)
    return e if isinstance(e, dict) else None


def _parked_file() -> Path:
    return Path.home() / ".hangar" / "computer-control.json"


def _known_entry() -> dict | None:
    """A entrada ativa ou, desligado, a que foi guardada ao desligar: religar não pode voltar sem
    modelo, esforço e chaves."""
    active = _entry(_main_file())
    if active is not None:
        return active
    parked = _read(_parked_file())
    return parked or None


def _park(entry: dict) -> None:
    p = _parked_file()
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_name(f"{p.name}.{os.getpid()}.{uuid.uuid4().hex[:8]}")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)   # tem chave: só o dono lê
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump(entry, fh, indent=2)
    atomico.substituir(tmp, p)


def _cliproxy_keys() -> list[str]:
    """`api-keys` do ~/.cli-proxy-api/config.yaml. Lido à mão: é uma lista simples de strings e o
    backend não tem biblioteca de YAML."""
    try:
        lines = (Path.home() / ".cli-proxy-api" / "config.yaml").read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    keys: list[str] = []
    inside = False
    for ln in lines:
        if re.match(r"^api-keys\s*:", ln):
            inside = True
            continue
        if inside:
            item = re.match(r"^\s*-\s*['\"]?([^'\"#\s]+)", ln)
            if item:
                keys.append(item.group(1))
            elif ln.strip() and not ln.startswith((" ", "\t", "-")):
                break
    return keys


def _ssh_hosts() -> list[str]:
    """Os `Host` do ~/.ssh/config (sem os coringas): o que a pessoa já usa em `ssh <host>`."""
    try:
        lines = (Path.home() / ".ssh" / "config").read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    hosts: list[str] = []
    for ln in lines:
        item = re.match(r"^\s*Host\s+(.+)$", ln, re.IGNORECASE)
        if item:
            hosts += [h for h in item.group(1).split() if not any(c in h for c in "*?!")]
    return list(dict.fromkeys(hosts))


_HOST_RE = re.compile(r"[A-Za-z0-9_][A-Za-z0-9._@-]*")


def _check_host(host: str) -> str:
    host = host.strip()
    if not _HOST_RE.fullmatch(host):   # nunca começa com '-': o valor vai pro argv do ssh
        raise ComputerControlError(400, "erro_computer_control_target_host", "informe o host SSH (sem espaços)")
    return host


def test_host(host: str, proxy_command: str = "") -> dict:
    """Entra por SSH sem senha e roda `echo ok`: é o que o agente vai precisar fazer."""
    host = _check_host(host)
    # accept-new: máquina nova não tem host key conhecida e o teste é justamente o primeiro contato.
    # Chave que MUDOU continua recusada.
    ssh = shutil.which("ssh")
    if not ssh:
        return {"ok": False, "detail": "ssh não encontrado neste servidor"}
    argv = [ssh, "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o", "StrictHostKeyChecking=accept-new"]
    if proxy_command.strip():
        argv += ["-o", f"ProxyCommand={proxy_command.strip()}"]
    argv += ["--", host, "echo", "ok"]
    try:
        r = subprocess.run(argv, capture_output=True, text=True, errors="replace", timeout=20,
                           stdin=subprocess.DEVNULL)
    except FileNotFoundError:
        return {"ok": False, "detail": "ssh não encontrado neste servidor"}
    except subprocess.TimeoutExpired:
        return {"ok": False, "detail": "sem resposta em 20 s"}
    ok = r.returncode == 0 and "ok" in r.stdout
    return {"ok": ok, "detail": "" if ok else ((r.stderr or r.stdout).strip()[-400:] or f"saiu com {r.returncode}")}


def _ssh_identity(host: str) -> tuple[str, str]:
    """(usuário, chave pública) que o `ssh` usaria pra esse host, pela resolução do próprio `ssh -G`."""
    ssh = shutil.which("ssh")
    if not ssh:
        return "", ""
    try:
        r = subprocess.run([ssh, "-G", "--", host], capture_output=True, text=True, errors="replace", timeout=10,
                           stdin=subprocess.DEVNULL)
    except (OSError, subprocess.TimeoutExpired):
        return "", ""
    user, key = "", ""
    for ln in r.stdout.splitlines():
        k, _, v = ln.partition(" ")
        if k == "user" and not user:
            user = v.strip()
        elif k == "identityfile" and not key:
            pub = Path(os.path.expanduser(v.strip()) + ".pub")
            if pub.is_file():
                key = pub.read_text(encoding="utf-8", errors="replace").strip()
    return user, key


_SETUP_PROMPT = """Configure este Windows para ser controlado pelo Hangar (MCP hangar-computer-control) a partir de outra máquina, por SSH com chave. Faça na ordem, confira cada passo e pare pra me perguntar se algo não bater.

1. Rode tudo num PowerShell elevado (como Administrador). Se não estiver elevado, me peça pra abrir um.
2. OpenSSH Server instalado, ligado e iniciando sozinho:
   Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
   Set-Service -Name sshd -StartupType Automatic; Start-Service sshd
3. Porta 22 liberada no firewall, se ainda não houver regra de entrada pra ela:
   New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
4. Autorize esta chave pública pro usuário {user_line}:
   {key}
   - Usuário administrador: a chave vai em C:\\ProgramData\\ssh\\administrators_authorized_keys, com permissão só pra Administradores e SYSTEM (use os SIDs, que valem em Windows de qualquer idioma):
     icacls.exe C:\\ProgramData\\ssh\\administrators_authorized_keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'
   - Usuário comum: em C:\\Users\\<usuário>\\.ssh\\authorized_keys.
   Acrescente sem apagar as chaves que já estiverem lá.
5. Esse usuário precisa ser administrador (o agente sobe como tarefa agendada elevada) e ficar logado numa sessão gráfica ATIVA: tela desbloqueada e sessão RDP não desconectada. Pra deixar a sessão ativa sem cliente RDP: tscon <id> /dest:console.
6. Confira: o serviço sshd está Running, a regra de firewall existe e o arquivo de chaves contém a chave acima.

Não mude outras configurações de segurança além dessas. No fim, me diga o nome desta máquina na rede (ou o IP) e o usuário, pra eu cadastrar no Hangar."""


def windows_setup(host: str) -> dict:
    """Prompt pra colar num agente (Claude Code, Codex…) rodando no Windows: deixa a máquina pronta
    pra este controlador entrar por SSH sem senha."""
    host = _check_host(host) if host.strip() else "novo-windows"
    explicit_user = host.split("@")[0] if "@" in host else ""
    user, key = _ssh_identity(host)
    if not key:
        raise ComputerControlError(400, "erro_computer_control_no_ssh_key",
                                   "esta máquina não tem chave SSH: crie uma com ssh-keygen -t ed25519")
    user = explicit_user or user
    user_line = (f"{user} (é o usuário com que o Hangar vai entrar; se ele não existir aqui, use o administrador "
                 f"logado e me diga qual é)") if user else "que vai entrar por SSH (me diga qual é)"
    return {"prompt": _SETUP_PROMPT.format(user_line=user_line, key=key), "user": user}


def _cliproxy_running() -> bool:
    try:
        with socket.create_connection(("127.0.0.1", 8317), timeout=0.5):
            return True
    except OSError:
        return False


def _jev_from_settings() -> str:
    env = _read(Path.home() / ".claude" / "settings.json").get("env") or {}
    v = env.get("TYPESAFE_API_KEY") if isinstance(env, dict) else None
    return v if isinstance(v, str) else ""


def jev_key() -> str:
    env = (_known_entry() or {}).get("env") or {}
    if not isinstance(env, dict):
        raise ComputerControlError(500, "erro_computer_control_read", "env do MCP Windows não é um objeto JSON")
    key = env.get("TYPESAFE_API_KEY")
    return key if isinstance(key, str) and key else _jev_from_settings()


def save_jev_key(key: str) -> None:
    """Atualiza só a chave, sem ligar o MCP nem substituir ajustes de cada conta."""
    pending = []
    for path in _config_files():
        entry = _entry(path)
        if entry is not None:
            data = _read(path)
            env = entry.get("env") or {}
            if not isinstance(env, dict):
                raise ComputerControlError(500, "erro_computer_control_read", f"env em {path} não é um objeto JSON")
            data["mcpServers"][NAME] = {**entry, "env": {**env, "TYPESAFE_API_KEY": key}}
            pending.append((path, data))
    parked = _read(_parked_file())
    env = parked.get("env") or {}
    if not isinstance(env, dict):
        raise ComputerControlError(500, "erro_computer_control_read", "env do MCP Windows guardado não é um objeto JSON")
    if parked or not pending:
        _park({**parked, "env": {**env, "TYPESAFE_API_KEY": key}})
    for path, data in pending:
        _write(path, data)


def _tail(key: str) -> str:
    return key[-4:] if len(key) >= 8 else ""


def _local_binary(project: Path) -> Path:
    return project / "target" / "release" / (f"{NAME}.exe" if os.name == "nt" else NAME)


def _targets(project: Path) -> list[dict]:
    """Os `<nome>-agent.json` da pasta do projeto: cada um é uma máquina que o MCP alcança."""
    out = []
    for p in sorted(project.glob("*-agent.json")) if project.is_dir() else []:
        try:
            cfg = json.loads(p.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            cfg = {}
        cfg = cfg if isinstance(cfg, dict) else {}
        out.append({"name": p.name.removesuffix("-agent.json"), "path": str(p),
                    "transport": cfg.get("transport", "local"), "host": cfg.get("host", "")})
    return out


def _mode(entry: dict | None) -> str:
    command = (entry or {}).get("command")
    if command:
        path = Path(str(command))
        return "package" if path == _package_binary() or path.name in ("uvx", "uvx.exe") else "local"
    install = _read(_install_dir() / "install.json")
    if install.get("tag") and _package_exe().is_file():
        return "package"
    project = Path.home() / "Projetos" / NAME
    legacy_python = project / ".venv" / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    local = _local_binary(project).is_file() or ((project / "servidor_mcp.py").is_file() and legacy_python.is_file())
    return "local" if local else "package"


def _local_project(entry: dict | None) -> Path:
    if _mode(entry) == "local":
        command = Path(str((entry or {}).get("command") or ""))
        if command.name in (NAME, f"{NAME}.exe") and command.parent.name == "release" \
                and command.parent.parent.name == "target":
            return command.parents[2]
        args = (entry or {}).get("args") or []
        if args:
            return Path(args[0]).parent
    return Path.home() / "Projetos" / NAME


def _where_targets(entry: dict | None, project_dir: str = "") -> tuple[Path, Path]:
    """(pasta dos *-agent.json, windows-agent.exe) conforme o modo."""
    if _mode(entry) == "package":
        return _package_targets(), _package_exe()
    project = Path(project_dir).expanduser() if project_dir else _local_project(entry)
    return project, _package_exe()


def create_target(body: dict) -> dict:
    """Cria `<nome>-agent.json` na pasta dos alvos. SSH aponta pra um Windows na rede; local é o
    próprio Windows onde este Hangar roda."""
    project, agent_exe = _where_targets(_known_entry(), str(body.get("project_dir") or "").strip())
    if not project.is_dir():
        raise ComputerControlError(400, "erro_computer_control_dir",
                                   f"{project} não tem target/release/hangar-computer-control (rode cargo build --release)", dir=str(project))
    name = str(body.get("name") or "").strip()
    if not name and body.get("transport") != "local":
        # Sem nome, sai do host: "administrator@delphi-03" vira "delphi-03".
        name = re.sub(r"[^a-z0-9._-]+", "-", str(body.get("host") or "").split("@")[-1].lower()).strip("-._")[:41]
    if not re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,40}", name):
        raise ComputerControlError(400, "erro_computer_control_target_name",
                                   "nome do alvo: letras minúsculas, números, ponto, hífen ou sublinhado")
    path = project / f"{name}-agent.json"
    if path.exists():
        raise ComputerControlError(409, "erro_computer_control_target_exists", f"o alvo {name} já existe",
                                   name=name)
    cfg: dict
    if body.get("transport") == "local":
        if os.name != "nt":
            raise ComputerControlError(400, "erro_computer_control_local_only_windows",
                                       "este computador só pode ser alvo quando o Hangar roda no Windows")
        cfg = {"transport": "local", "command": [str(agent_exe)]}
    else:
        cfg = {"transport": "ssh", "host": _check_host(str(body.get("host") or "")), "agent_path": str(agent_exe)}
        proxy = str(body.get("proxy_command") or "").strip()
        if proxy:
            cfg["proxy_command"] = proxy
    timeout = body.get("request_timeout")
    if timeout:
        cfg["request_timeout"] = int(timeout)
    tmp = path.with_name(f"{path.name}.{os.getpid()}.{uuid.uuid4().hex[:8]}")
    tmp.write_text(json.dumps(cfg, indent=2) + "\n", encoding="utf-8")
    atomico.substituir(tmp, path)
    return state()


def state() -> dict:
    """O que a tela mostra. Chave nunca sai inteira: só se existe e os 4 últimos caracteres."""
    enabled = _entry(_main_file()) is not None
    entry = _known_entry()
    env = (entry or {}).get("env") or {}
    project = str(_local_project(entry))
    llm_key = env.get("LLM_PROXY_KEY", "")
    jev = env.get("TYPESAFE_API_KEY", "")
    jev_settings = _jev_from_settings()
    cliproxy = _cliproxy_keys()
    targets = _targets(_where_targets(entry)[0])
    agents = [t["path"] for t in targets]
    install = _read(_install_dir() / "install.json")
    agent_exe = _where_targets(entry)[1]
    return {
        # O exe sai DAQUI até pro alvo remoto (copiado na 1ª conexão): sem ele, nenhum alvo funciona.
        "agent_exe": {"path": str(agent_exe), "exists": agent_exe.is_file(),
                      "size": agent_exe.stat().st_size if agent_exe.is_file() else 0},
        "mode": _mode(entry),
        "installed_tag": install.get("tag", ""),
        "package_exists": bool(install.get("tag")) and _package_exe().is_file()
                          and (_package_binary().is_file() or bool(install.get("uvx"))),
        "targets": targets,
        "ssh_hosts": _ssh_hosts(),
        "local_available": os.name == "nt",
        "enabled": enabled,
        "project_dir": project,
        "agent_config": env.get("HCC_AGENT_CONFIG", agents[0] if agents else ""),
        "agent_configs": agents,
        "llm_url": env.get("LLM_PROXY_URL", PRESET_URL),
        "llm_model": env.get("LLM_MODEL", ""),
        "llm_effort": env.get("LLM_EFFORT", ""),
        "llm_key_set": bool(llm_key),
        "llm_key_tail": _tail(llm_key),
        "jev_key_set": bool(jev or jev_settings),
        "jev_key_tail": _tail(jev or jev_settings),
        "jev_key_from_settings": not jev and bool(jev_settings),
        "cliproxy": {"preset_url": PRESET_URL, "has_keys": bool(cliproxy),
                     "installed": shutil.which("cli-proxy-api") is not None, "running": _cliproxy_running(),
                     "key_is_cliproxy": bool(llm_key) and llm_key in cliproxy},
        "files": [{"path": str(p), "enabled": _entry(p) is not None} for p in _config_files()],
    }


def save(body: dict, *, installing: bool = False) -> dict:
    """Grava (ligado) ou remove (desligado) a entrada em todos os `.claude.json`. Chave vazia no
    pedido mantém a que já está gravada: a tela nunca recebeu a chave inteira pra devolver."""
    files = _config_files()
    if not body.get("enabled"):
        active = _entry(_main_file())
        if active is not None:
            _park(active)
        for p in files:
            data = _read(p)
            mcp = data.get("mcpServers")
            if isinstance(mcp, dict) and NAME in mcp:
                del mcp[NAME]
                _write(p, data)
        return state()

    if (body.get("mode") or _mode(_known_entry())) == "package":
        install = _read(_install_dir() / "install.json")
        known = _known_entry() or {}
        legacy_install = install.get("tag") and install.get("uvx") and _package_exe().is_file()
        if install.get("tag") and not install.get("uvx") and _package_binary().is_file():
            command, args = str(_package_binary()), []
        elif installing and legacy_install:
            command = install["uvx"]
            args = ["--from", f"git+https://github.com/{REPO}@{install['tag']}", NAME]
        elif Path(str(known.get("command") or "")).name in ("uvx", "uvx.exe"):
            command, args = known["command"], known.get("args") or []
        elif legacy_install:
            command = install["uvx"]
            args = ["--from", f"git+https://github.com/{REPO}@{install['tag']}", NAME]
        else:
            raise ComputerControlError(400, "erro_computer_control_not_installed",
                                       "instale a versão publicada antes de usar esse modo")
        agents_dir = _package_targets()
    else:
        project = Path(str(body.get("project_dir") or "").strip()).expanduser().resolve()
        binary = _local_binary(project)
        if not binary.is_file():
            raise ComputerControlError(400, "erro_computer_control_dir",
                                       f"{project} não tem target/release/hangar-computer-control (rode cargo build --release)", dir=str(project))
        command, args = str(binary), []
        agents_dir = project
    agent = str(body.get("agent_config") or "").strip()
    if not Path(agent).is_file():
        raise ComputerControlError(400, "erro_computer_control_agent", f"o arquivo {agent} não existe", file=agent)
    url = str(body.get("llm_url") or "").strip()
    if not re.match(r"^https?://", url):
        raise ComputerControlError(400, "erro_computer_control_url",
                                   "a URL do LLM precisa começar com http:// ou https://")
    effort = str(body.get("llm_effort") or "")
    if effort not in EFFORTS:
        raise ComputerControlError(400, "erro_computer_control_effort", f"esforço inválido: {effort}", effort=effort)

    previous = (_known_entry() or {}).get("env") or {}
    llm_key = str(body.get("llm_key") or "")
    if body.get("use_cliproxy_key"):
        keys = _cliproxy_keys()
        if not keys:
            raise ComputerControlError(400, "erro_computer_control_cliproxy_key",
                                       "o ~/.cli-proxy-api/config.yaml não tem nenhuma api-key")
        llm_key = previous.get("LLM_PROXY_KEY") if previous.get("LLM_PROXY_KEY") in keys else keys[0]
    llm_key = llm_key or previous.get("LLM_PROXY_KEY", "")
    jev = str(body.get("jev_key") or "") or previous.get("TYPESAFE_API_KEY", "") or _jev_from_settings()

    # HCC_AGENT_CONFIG é o alvo padrão; HCC_AGENTS_DIR, a pasta de onde o MCP tira os outros.
    managed = {"HCC_AGENT_CONFIG": agent, "HCC_AGENTS_DIR": str(agents_dir),
               "LLM_PROXY_URL": url,
               "LLM_MODEL": str(body.get("llm_model") or "").strip(), "LLM_EFFORT": effort,
               "LLM_PROXY_KEY": llm_key, "TYPESAFE_API_KEY": jev}
    # O resto do env é de quem montou o MCP e fica como está (o VIRTUAL_ENV vazio de propósito
    # impede herdar o venv do processo que abre a sessão). Das variáveis desta tela, vazia = ausente.
    env = {k: v for k, v in previous.items() if k not in managed and k != "PYTHONPATH"}
    env.update({k: v for k, v in managed.items() if v})
    entry = {"command": command, "args": args, "env": env}
    for p in files:
        data = _read(p)
        if not data and p != _main_file():
            continue
        current = data.get("mcpServers")
        servers: dict = current if isinstance(current, dict) else {}
        if servers.get(NAME) == entry:
            continue
        data["mcpServers"] = {**servers, NAME: entry}
        _write(p, data)
    return state()


def _get(url: str, timeout: int) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "hangar", "Accept": "application/vnd.github+json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        raise ComputerControlError(502, "erro_computer_control_release", f"o GitHub respondeu {e.code} em {url}",
                                   error=f"HTTP {e.code}")
    except (urllib.error.URLError, OSError) as e:
        raise ComputerControlError(502, "erro_computer_control_release", f"não consegui baixar {url}: {e}",
                                   error=str(e))


def _migrated(cfg: dict, exe: Path, binary: Path | None = None) -> dict:
    if cfg.get("transport") == "ssh" and str(cfg.get("agent_path", "")).endswith("windows-agent.exe"):
        return {**cfg, "agent_path": str(exe)}
    command = cfg.get("command")
    if isinstance(command, list) and cfg.get("transport", "local") == "local":
        if binary is not None and command:
            program = Path(str(command[0])).name
            script = command[1] if len(command) > 1 and re.fullmatch(r"python(?:\d+(?:\.\d+)?)?(?:\.exe)?", program) else command[0]
            if Path(str(script)).name == "linux_agent.py":
                return {**cfg, "command": [str(binary), "agent"]}
            # O wrapper sh fica: ele preenche HYPRLAND_INSTANCE_SIGNATURE, sem a qual o agente Rust não sobe.
            if len(command) >= 3 and program in {"sh", "bash"} and command[1] == "-c":
                wrapped = re.sub(r"exec\s+\S*python[\d.]*\s+\S*linux_agent\.py",
                                 lambda _: f"exec {shlex.quote(str(binary))} agent", str(command[2]), count=1)
                if wrapped != command[2]:
                    return {**cfg, "command": [*command[:2], wrapped, *command[3:]]}
        if command and str(command[0]).endswith("windows-agent.exe"):
            return {**cfg, "command": [str(exe), *command[1:]]}
    return cfg


def install() -> dict:
    try:
        release = json.loads(_get(f"https://api.github.com/repos/{REPO}/releases/latest", 15))
        tag = release["tag_name"]
        if not isinstance(tag, str) or not tag:
            raise ValueError("sem tag_name")
    except (ValueError, KeyError, TypeError):
        raise ComputerControlError(502, "erro_computer_control_release", "a release mais recente veio sem tag",
                                   error="sem tag_name")
    linux_asset = "hangar-computer-control-linux-x86_64"
    binary_release = any(isinstance(asset, dict) and asset.get("name") == linux_asset
                         for asset in release.get("assets", []))
    uvx = ""
    if not binary_release:
        uvx = shutil.which("uvx") or ""
        if not uvx:
            error = "a release ainda usa o pacote Python; instale uv neste servidor antes de atualizar"
            raise ComputerControlError(400, "erro_computer_control_release", error, error=error)
    before = _known_entry()
    local_before = _mode(before) == "local"
    exe = _package_exe()
    downloads = {exe: _get(f"https://github.com/{REPO}/releases/download/{tag}/windows-agent.exe", 120)}
    if binary_release and os.name != "nt":
        downloads[_package_binary()] = _get(f"https://github.com/{REPO}/releases/download/{tag}/{linux_asset}", 120)
    exe.parent.mkdir(parents=True, exist_ok=True)
    for path, content in downloads.items():
        tmp = path.with_name(f"{path.name}.{os.getpid()}.{uuid.uuid4().hex[:8]}")
        tmp.write_bytes(content)
        if path == _package_binary() and os.name != "nt":
            tmp.chmod(0o755)
        atomico.substituir(tmp, path)

    targets = _package_targets()
    targets.mkdir(parents=True, exist_ok=True)
    # Alvo que não deu pra ler não pode sumir calado da lista: volta no resultado pra tela avisar.
    puladas: list[str] = []
    if local_before:
        for p in _local_project(before).glob("*-agent.json"):
            dest = targets / p.name
            if dest.exists() or p.name == "exemplo-agent.json":   # modelo do repositório, não é máquina
                continue
            try:
                cfg = json.loads(p.read_text(encoding="utf-8"))
            except (OSError, ValueError) as e:
                _log.warning("computer-control: alvo %s não migrou: %s", p.name, e)
                puladas.append(p.name)
                continue
            if not isinstance(cfg, dict):
                puladas.append(p.name)
                continue
            dest.write_text(json.dumps(_migrated(cfg, exe, _package_binary() if binary_release else None), indent=2)
                            + "\n", encoding="utf-8")
    if binary_release:
        for path in sorted(targets.glob("*-agent.json")):
            try:
                cfg = json.loads(path.read_text(encoding="utf-8"))
                if not isinstance(cfg, dict):
                    raise TypeError("não é um objeto JSON")
            except (OSError, ValueError, TypeError) as error:
                _log.warning("computer-control: alvo %s não migrou: %s", path.name, error)
                puladas.append(path.name)
                continue
            migrated = _migrated(cfg, exe, _package_binary())
            if migrated != cfg:
                _write(path, migrated)
    _write(_install_dir() / "install.json", {"tag": tag, **({"uvx": uvx} if uvx else {})})

    s = state()
    s["migration_skipped"] = puladas
    s["legacy_package"] = not binary_release
    s["detail"] = "" if binary_release else "A release ainda não inclui o binário; instalado o pacote Python."
    default = Path(s["agent_config"]).name if s["agent_config"] else ""
    agent = targets / default if default and (targets / default).is_file() else None
    agent = agent or next(iter(sorted(targets.glob("*-agent.json"))), None)
    if agent is None:
        return s   # sem alvo ainda: a tela pede pra criar um antes de ligar
    ligado = save({"enabled": True, "mode": "package", "agent_config": str(agent), "llm_url": s["llm_url"],
                   "llm_model": s["llm_model"], "llm_effort": s["llm_effort"], "llm_key": None, "jev_key": None,
                   "use_cliproxy_key": s["cliproxy"]["key_is_cliproxy"]}, installing=True)
    ligado.update({key: s[key] for key in ("migration_skipped", "legacy_package", "detail")})
    return ligado


def list_models(url: str, key: str | None, use_saved_key: bool, use_cliproxy_key: bool) -> list[str]:
    """Modelos do endpoint (`GET .../v1/models` com Bearer), pra escolher em vez de digitar."""
    if not re.match(r"^https?://", url or ""):
        raise ComputerControlError(400, "erro_computer_control_url",
                                   "a URL do LLM precisa começar com http:// ou https://")
    if use_cliproxy_key:
        keys = _cliproxy_keys()
        key = keys[0] if keys else ""
    elif use_saved_key or not key:
        key = ((_known_entry() or {}).get("env") or {}).get("LLM_PROXY_KEY", "")
    base = re.sub(r"/chat/completions/?$", "", url.rstrip("/"))
    request = urllib.request.Request(f"{base}/models", headers={"Authorization": f"Bearer {key}"} if key else {})
    try:
        with urllib.request.urlopen(request, timeout=8) as r:
            data = json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        raise ComputerControlError(502, "erro_computer_control_models",
                                   f"o endpoint respondeu {e.code} ao listar modelos", error=f"HTTP {e.code}")
    except (urllib.error.URLError, OSError, ValueError) as e:
        raise ComputerControlError(502, "erro_computer_control_models", f"não consegui listar os modelos: {e}",
                                   error=str(e))
    items = data.get("data") if isinstance(data, dict) else None
    return sorted({i["id"] for i in items or [] if isinstance(i, dict) and isinstance(i.get("id"), str)})
