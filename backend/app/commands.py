import json
import re
from pathlib import Path
from typing import Optional

import yaml

from app.models import CommandInfo

# Built-ins comuns do Claude Code. destructive marca os que apagam o contexto ou encerram a
# sessao (a UI pede confirmacao antes de enviar). Descricoes curtas em pt-BR, sem em-dash.
BUILTINS: list[dict] = [
    {"name": "clear", "description": "Limpa o histórico da conversa", "destructive": True},
    {"name": "compact", "description": "Resume e compacta o contexto", "destructive": True},
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
]

# Captura so o bloco YAML entre os '---' do topo do markdown (tolera BOM e CRLF).
_FRONTMATTER_RE = re.compile(r"^﻿?---\s*\n(.*?)\n---\s*(?:\n|$)", re.DOTALL)


def _read_text(path: Path) -> str:
    # Defensivo: arquivo ilegível ou que sumiu no meio do scan -> sem frontmatter.
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def _parse_frontmatter(text: str) -> dict:
    # Extrai o frontmatter do topo. Qualquer erro de YAML -> {} (segue sem ele).
    m = _FRONTMATTER_RE.match(text)
    if not m:
        return {}
    try:
        data = yaml.safe_load(m.group(1))
    except Exception:
        return {}
    return data if isinstance(data, dict) else {}


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
        fm = _parse_frontmatter(_read_text(md))
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
        fm = _parse_frontmatter(_read_text(skill_md))
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
                    fm = _parse_frontmatter(_read_text(md))
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
                    fm = _parse_frontmatter(_read_text(skill_md))
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


def list_commands(cwd: Optional[str]) -> list[CommandInfo]:
    """Built-ins + comandos/skills do projeto (cwd) + skills globais (~/.claude/skills)
    + skills/comandos de plugins instalados (~/.claude/plugins, namespaced '<plugin>:<nome>').

    Tolerante a diretorios ausentes: cada scan so roda se o diretorio existir. Dedupe por
    nome preservando a ordem de prioridade (built-in > projeto > global > plugin)."""
    raw: list[dict] = [{**b, "source": "builtin"} for b in BUILTINS]

    if cwd:
        base = Path(cwd)
        raw += _scan_project_commands(base / ".claude" / "commands")
        raw += _scan_skills(base / ".claude" / "skills")

    raw += _scan_skills(Path.home() / ".claude" / "skills")
    raw += _scan_plugins(Path.home() / ".claude" / "plugins")

    seen: set[str] = set()
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
