import json
import logging
import os
import re
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Optional

import yaml

from app import atomico
from app.models import CommandInfo

_log = logging.getLogger("hangar.commands")

# Fallback quando a CLI ainda nao foi sondada (ou a sonda falhou) e fonte de descricao em pt-BR e
# da marca destructive (a UI pede confirmacao antes de enviar). A lista de NOMES vem da CLI.
BUILTINS: list[dict] = [
    {"name": "clear", "description": "Limpa o histórico da conversa", "destructive": True},
    {"name": "compact", "description": "Resume e compacta o contexto", "destructive": True},
    {"name": "btw", "description": "Pergunta lateral, sem interromper a conversa", "argumentHint": "[pergunta]"},
    {"name": "context", "description": "Mostra o uso do contexto"},
    {"name": "model", "description": "Troca o modelo do Claude"},
    {"name": "effort", "description": "Ajusta o esforço de raciocínio"},
    {"name": "resume", "description": "Retoma uma conversa anterior"},
    {"name": "rewind", "description": "Volta a conversa a um ponto anterior"},
    {"name": "release-notes", "description": "Mostra as novidades da versão"},
    {"name": "help", "description": "Lista os comandos disponíveis"},
    {"name": "status", "description": "Mostra o estado da sessão e da conta"},
    {"name": "cost", "description": "Mostra o custo e o uso de tokens"},
    {"name": "export", "description": "Exporta a conversa"},
    {"name": "init", "description": "Cria um arquivo CLAUDE.md no projeto"},
    {"name": "agents", "description": "Gerencia subagentes"},
    {"name": "mcp", "description": "Gerencia servidores MCP"},
    {"name": "memory", "description": "Edita os arquivos de memória"},
    {"name": "vim", "description": "Ativa o modo de edição estilo vim"},
    {"name": "config", "description": "Abre as configurações"},
    {"name": "doctor", "description": "Verifica a saúde da instalação"},
    {"name": "quit", "description": "Encerra a sessão", "destructive": True},
    # A CLI em `-p` não informa os comandos que só a TUI tem: estes a sonda nunca traz.
    {"name": "login", "description": "Entra com a conta Anthropic"},
    {"name": "permissions", "description": "Gerencia as regras de permissão"},
    {"name": "hooks", "description": "Gerencia os hooks"},
    {"name": "plugin", "description": "Gerencia plugins e marketplaces"},
    {"name": "skills", "description": "Lista as skills disponíveis"},
    {"name": "theme", "description": "Troca o tema do terminal"},
    {"name": "statusline", "description": "Configura a linha de status"},
    {"name": "add-dir", "description": "Adiciona um diretório de trabalho", "argumentHint": "<caminho>"},
    {"name": "review", "description": "Revisa um pull request"},
]

# Captura so o bloco YAML entre os '---' do topo do markdown (tolera BOM e CRLF).
_FRONTMATTER_RE = re.compile(r"^﻿?---\s*\n(.*?)\n---\s*(?:\n|$)", re.DOTALL)


def _read_text(path: Path) -> str:
    # Defensivo: arquivo ilegível ou que sumiu no meio do scan -> sem frontmatter.
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


_YAML_LOADER = getattr(yaml, "CSafeLoader", yaml.SafeLoader)


def _parse_frontmatter(text: str) -> dict:
    # Extrai o frontmatter do topo. Qualquer erro de YAML -> {} (segue sem ele).
    m = _FRONTMATTER_RE.match(text)
    if not m:
        return {}
    try:
        data = yaml.load(m.group(1), Loader=_YAML_LOADER)
    except Exception:
        return {}
    return data if isinstance(data, dict) else {}


# path -> ((mtime_ns, size), frontmatter). A lista roda a cada abertura do menu e relia e
# reparseava centenas de SKILL.md que quase nunca mudam.
_fm_cache: dict[str, tuple[tuple[int, int], dict]] = {}


def _frontmatter_of(path: Path) -> dict:
    try:
        st = path.stat()
    except OSError:
        return {}
    chave = (st.st_mtime_ns, st.st_size)
    hit = _fm_cache.get(str(path))
    if hit and hit[0] == chave:
        return hit[1]
    fm = _parse_frontmatter(_read_text(path))
    _fm_cache[str(path)] = (chave, fm)
    return fm


def _clean(value) -> Optional[str]:
    # Colapsa descricao multi-linha do frontmatter numa unica linha enxuta.
    if not isinstance(value, str):
        return None
    s = " ".join(value.split())
    return s or None


def _scan_project_commands(commands_dir: Path) -> list[dict]:
    # <cwd>/.claude/commands/*.md -> comandos do projeto (source 'skill').
    out: list[dict] = []
    if not commands_dir.is_dir():
        return out
    for md in sorted(commands_dir.glob("*.md")):
        if not md.is_file():
            continue
        fm = _frontmatter_of(md)
        name = _clean(fm.get("name")) or md.stem
        if not name:
            continue
        out.append({
            "name": name,
            "description": _clean(fm.get("description")),
            "argumentHint": _clean(fm.get("argument-hint") or fm.get("argumentHint")),
            "source": "skill",
        })
    return out


def _scan_skills(skills_dir: Path) -> list[dict]:
    # <dir>/<skill>/SKILL.md -> skills (source 'skill'). Nome vem do frontmatter ou da pasta.
    out: list[dict] = []
    if not skills_dir.is_dir():
        return out
    for sub in sorted(skills_dir.iterdir()):
        if not sub.is_dir():
            continue
        skill_md = sub / "SKILL.md"
        if not skill_md.is_file():
            continue
        fm = _frontmatter_of(skill_md)
        name = _clean(fm.get("name")) or sub.name
        if not name:
            continue
        out.append({
            "name": name,
            "description": _clean(fm.get("description")),
            "argumentHint": None,
            "source": "skill",
        })
    return out


def _scan_plugins(plugins_dir: Path) -> list[dict]:
    # ~/.claude/plugins/installed_plugins.json -> para cada plugin instalado, varre o seu
    # installPath: commands/*.md e skills/<nome>/SKILL.md. Nome vira '<plugin>:<nome>'
    # (namespaced, como o Claude Code invoca), fonte 'plugin'. Le so os instalados (dedup
    # de versao via manifest) em vez dos milhares de arquivos crus sob plugins/.
    manifest = plugins_dir / "installed_plugins.json"
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    plugins = data.get("plugins") if isinstance(data, dict) else None
    if not isinstance(plugins, dict):
        return []

    out: list[dict] = []
    for key, entries in plugins.items():
        plugin = key.split("@", 1)[0]
        if not isinstance(entries, list):
            continue
        for entry in entries:
            root = entry.get("installPath") if isinstance(entry, dict) else None
            if not root:
                continue
            base = Path(root)

            cmd_dir = base / "commands"
            if cmd_dir.is_dir():
                for md in sorted(cmd_dir.glob("*.md")):
                    if not md.is_file():
                        continue
                    fm = _frontmatter_of(md)
                    stem = _clean(fm.get("name")) or md.stem
                    if not stem:
                        continue
                    out.append({
                        "name": f"{plugin}:{stem}",
                        "description": _clean(fm.get("description")),
                        "argumentHint": _clean(fm.get("argument-hint") or fm.get("argumentHint")),
                        "source": "plugin",
                    })

            skills_dir = base / "skills"
            if skills_dir.is_dir():
                for sub in sorted(skills_dir.iterdir()):
                    if not sub.is_dir():
                        continue
                    skill_md = sub / "SKILL.md"
                    if not skill_md.is_file():
                        continue
                    fm = _frontmatter_of(skill_md)
                    stem = _clean(fm.get("name")) or sub.name
                    if not stem:
                        continue
                    out.append({
                        "name": f"{plugin}:{stem}",
                        "description": _clean(fm.get("description")),
                        "argumentHint": None,
                        "source": "plugin",
                    })
    return out


_SONDA_TETO_S = 120.0
_SONDA_RETRY_S = 600.0
_sonda_trava = threading.Lock()
_sonda_em_voo: set[str] = set()
_sonda_falhou_em: dict[str, float] = {}


def _cache_path() -> Path:
    return Path.home() / ".hangar" / "claude-slash-commands.json"


_cache_trava = threading.Lock()


def _chave_cli(config_dir: Optional[str]) -> Optional[str]:
    """Assinatura do que muda a lista: binario resolvido (versao, sem pagar `claude --version`) e o
    mtime do que instala comando — settings, plugins, skills e commands do config dir."""
    exe = shutil.which("claude")
    if not exe:
        _log.warning("commands: binario claude nao encontrado; lista fixa no lugar")
        return None
    try:
        real = Path(exe).resolve()
        st = real.stat()
    except OSError as e:
        _log.warning("commands: binario claude ilegivel (%s); lista fixa no lugar", e)
        return None
    base = Path(config_dir) if config_dir else Path.home() / ".claude"
    partes = [str(real), str(st.st_mtime_ns), str(st.st_size)]
    for rel in ("settings.json", "plugins/installed_plugins.json", "skills", "commands"):
        try:
            partes.append(str((base / rel).stat().st_mtime_ns))
        except OSError:
            partes.append("-")
    return "|".join(partes)


def _ler_cache() -> dict:
    path = _cache_path()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as e:
        _log.warning("commands: cache da lista ilegivel em %s (%s); sondando de novo", path, e)
        return {}
    return data if isinstance(data, dict) else {}


def _gravar_cache(config_dir: Optional[str], assinatura: str, comandos: list[dict]) -> None:
    path = _cache_path()
    with _cache_trava:
        path.parent.mkdir(parents=True, exist_ok=True)
        data = _ler_cache()
        data[config_dir or ""] = {"assinatura": assinatura, "comandos": comandos, "em": time.time()}
        tmp = path.with_name(path.name + ".tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        atomico.substituir(tmp, path)


def _matar_arvore(proc: subprocess.Popen) -> None:
    # Hooks de SessionStart sao filhos do claude: matar so o pai deixa eles rodando.
    if proc.poll() is not None:
        return
    if os.name == "nt":
        exe = shutil.which("taskkill")
        if exe is None:
            _log.warning("commands: taskkill não encontrado; hooks filhos do pid=%s podem seguir vivos", proc.pid)
            proc.kill()
        else:
            try:
                r = subprocess.run([exe, "/T", "/F", "/PID", str(proc.pid)], capture_output=True, timeout=10)
            except (OSError, subprocess.TimeoutExpired):
                _log.warning("commands: taskkill falhou pid=%s", proc.pid, exc_info=True)
            else:
                # 128 = o processo já tinha saído: não é falha.
                if r.returncode not in (0, 128):
                    _log.warning("commands: taskkill rc=%s pid=%s: %s", r.returncode, proc.pid,
                                 (r.stderr or b"").decode(errors="replace").strip()[:200])
    else:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            try:
                proc.kill()
            except OSError:
                pass    # já saiu entre o poll e o kill: nada a matar
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass


def sondar_cli(config_dir: Optional[str]) -> list[dict]:
    """Lista de comandos da propria CLI via `control_request initialize`: a resposta traz nome,
    descricao e argumentHint, e nao gasta turno (o `system/init` so sai depois de um prompt).
    Levanta RuntimeError quando nao consegue."""
    exe = shutil.which("claude")
    if not exe:
        raise RuntimeError("binario claude nao encontrado")
    env = {k: v for k, v in os.environ.items() if k not in ("TMUX", "TMUX_PANE", "CP_SESSION_NAME", "CP_SESSION_KEY")}
    # Exportar CLAUDE_CONFIG_DIR=~/.claude nao e o mesmo que nao exportar (ver tmux._config_dir_padrao).
    if config_dir:
        env["CLAUDE_CONFIG_DIR"] = config_dir
    else:
        env.pop("CLAUDE_CONFIG_DIR", None)
    extra: dict = {"creationflags": 0x08000000} if os.name == "nt" else {"start_new_session": True}
    with tempfile.TemporaryDirectory(prefix="hangar-slash-", ignore_cleanup_errors=True) as cwd:
        proc = subprocess.Popen(
            [exe, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
             "--setting-sources", "user"],
            cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, **extra)
        carrasco = threading.Timer(_SONDA_TETO_S, _matar_arvore, (proc,))
        carrasco.start()
        try:
            assert proc.stdin and proc.stdout
            proc.stdin.write((json.dumps({"type": "control_request", "request_id": "hangar_slash",
                                          "request": {"subtype": "initialize"}}) + "\n").encode())
            proc.stdin.flush()
            for bruto in proc.stdout:
                try:
                    ev = json.loads(bruto)
                except ValueError:
                    continue
                r = ev.get("response") if isinstance(ev, dict) and ev.get("type") == "control_response" else None
                if not isinstance(r, dict) or r.get("request_id") != "hangar_slash":
                    continue
                if r.get("subtype") == "error":
                    raise RuntimeError(f"initialize recusado: {str(r.get('error'))[:200]}")
                comandos = (r.get("response") or {}).get("commands")
                validos = [c for c in comandos if isinstance(c, dict) and isinstance(c.get("name"), str)] \
                    if isinstance(comandos, list) else []
                if not validos:
                    raise RuntimeError("initialize sem lista de comandos")
                return validos
            raise RuntimeError(f"claude saiu sem responder ao initialize (rc={proc.poll()})")
        finally:
            carrasco.cancel()
            _matar_arvore(proc)


def _sondar_em_fundo(chave: str, config_dir: Optional[str]) -> None:
    try:
        inicio = time.monotonic()
        comandos = sondar_cli(config_dir)
        _gravar_cache(config_dir, chave.split("\0", 1)[1], comandos)
        _log.info("commands: CLI sondada (%d comandos, %.1fs) config_dir=%s", len(comandos),
                  time.monotonic() - inicio, config_dir or "~/.claude")
    except Exception as e:  # noqa: BLE001 — thread de fundo: erro escapando sumiria sem log
        _sonda_falhou_em[chave] = time.monotonic()
        _log.warning("commands: sonda da CLI falhou (lista fixa no lugar) config_dir=%s: %s",
                     config_dir or "~/.claude", e)
    finally:
        with _sonda_trava:
            _sonda_em_voo.discard(chave)


def comandos_da_cli(config_dir: Optional[str]) -> Optional[list[dict]]:
    """Lista cacheada por config_dir, valida enquanto a assinatura (binario + instalados) nao muda.
    Sem cache valido: dispara UMA sonda em fundo (~20s de subida da CLI) e devolve None, pro
    chamador servir a lista fixa enquanto isso."""
    assinatura = _chave_cli(config_dir)
    if assinatura is None:
        return None
    hit = _ler_cache().get(config_dir or "")
    if isinstance(hit, dict) and hit.get("assinatura") == assinatura and hit.get("comandos"):
        return hit["comandos"]
    chave = f"{config_dir or ''}\0{assinatura}"
    with _sonda_trava:
        falhou = _sonda_falhou_em.get(chave)
        if chave in _sonda_em_voo or (falhou and time.monotonic() - falhou < _SONDA_RETRY_S):
            return None
        _sonda_em_voo.add(chave)
    threading.Thread(target=_sondar_em_fundo, args=(chave, config_dir), daemon=True,
                     name="hangar-slash-sonda").start()
    return None


def sem_os_desligados(comandos: list[CommandInfo], claude_settings: Optional[dict]) -> list[CommandInfo]:
    """Tira o que a escolha da sessão desligou: a sonda e os scans são da conta, não da sessão."""
    if not claude_settings:
        return comandos
    escolha = claude_settings.get("enabledPlugins") or {}
    # O comando leva só o nome do plugin: o mesmo nome ligado noutro marketplace continua aparecendo.
    plugins = ({pid.split("@", 1)[0] for pid, on in escolha.items() if not on}
               - {pid.split("@", 1)[0] for pid, on in escolha.items() if on})
    skills = {nome for nome, estado in (claude_settings.get("skillOverrides") or {}).items() if estado == "off"}
    skills |= {regra[len("Skill("):-1].removesuffix(" *")
               for regra in (claude_settings.get("permissions") or {}).get("deny", [])}
    return [c for c in comandos
            if c.name not in skills and not (":" in c.name and c.name.split(":", 1)[0] in plugins)]


def list_commands(cwd: Optional[str], cli: Optional[list[dict]] = None,
                  excluir: frozenset[str] = frozenset(), com_tui: bool = True) -> list[CommandInfo]:
    """Nomes da CLI (`cli`, quando ja se sabe) ou, sem ela, built-ins fixos + scans. Descricao e
    argumentHint vem primeiro dos arquivos locais (frontmatter, pt-BR dos built-ins), depois da CLI;
    nome sem descricao entra mesmo assim. Comandos do projeto (cwd) sempre entram: a sonda roda
    fora de qualquer projeto. `com_tui` soma os built-ins fixos que a CLI nao informa (sessao com
    terminal); `excluir` tira nomes que nao tem como rodar nesta sessao."""
    raw: list[dict] = [{**b, "source": "builtin"} for b in BUILTINS]
    projeto: list[dict] = []
    if cwd:
        base = Path(cwd)
        projeto = _scan_project_commands(base / ".claude" / "commands") + _scan_skills(base / ".claude" / "skills")
        raw += projeto

    raw += _scan_project_commands(Path.home() / ".claude" / "commands")
    raw += _scan_skills(Path.home() / ".claude" / "skills")
    raw += _scan_plugins(Path.home() / ".claude" / "plugins")

    if cli is not None:
        local: dict[str, dict] = {}
        for item in raw:
            local.setdefault(item["name"], item)
        da_cli = []
        for c in cli:
            nome = c["name"]
            base_item = local.get(nome, {})
            da_cli.append({
                "name": nome,
                "description": base_item.get("description") or _clean(c.get("description")),
                "argumentHint": base_item.get("argumentHint") or _clean(c.get("argumentHint")),
                "source": base_item.get("source") or ("plugin" if ":" in nome else "builtin"),
                "destructive": base_item.get("destructive", False),
            })
        raw = da_cli + ([{**b, "source": "builtin"} for b in BUILTINS] if com_tui else []) + projeto

    seen: set[str] = set(excluir)
    out: list[CommandInfo] = []
    for item in raw:
        name = item["name"]
        if name in seen:
            continue
        seen.add(name)
        out.append(CommandInfo(
            name=name,
            display="/" + name,
            description=item.get("description"),
            argumentHint=item.get("argumentHint"),
            source=item.get("source", "builtin"),
            destructive=bool(item.get("destructive", False)),
        ))
    return out
