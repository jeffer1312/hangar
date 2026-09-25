"""Máquina falsa para os testes da configuração compartilhada: uma casa em tmp com ~/.claude,
~/.codex, ~/.claude.json, preferências do Hangar e um repositório do Hangar próprio."""
import json
from pathlib import Path

from app.config_sync_paths import Roots


def prefs_path(roots: Roots) -> Path:
    return Path(roots.home).parent / "prefs.json"


def use_machine(monkeypatch, roots: Roots) -> None:
    """Faz o processo de teste 'ser' esta máquina: HOME, engines e runtime-config."""
    from app import runtime_config
    monkeypatch.setenv("HOME", roots.home)
    monkeypatch.delenv("CP_ENGINES_FILE", raising=False)
    monkeypatch.setattr(runtime_config, "_caminho", lambda: prefs_path(roots))


def make_machine(base: Path, name: str, *, full: bool = True) -> Roots:
    home = base / name / "home"
    hangar = base / name / "hangar"
    (hangar / "skills" / "orquestrar").mkdir(parents=True)
    (hangar / "skills" / "orquestrar" / "SKILL.md").write_text("skill do hangar\n")
    (hangar / "backend" / "hooks").mkdir(parents=True)
    for hook in ("state_hook.py", "guard_tmux.py"):
        (hangar / "backend" / "hooks" / hook).write_text("# hook do hangar\n")
    (hangar / "scripts").mkdir()
    (hangar / "scripts" / "statusline.js").write_text("// barra\n")
    roots = Roots(hangar=str(hangar), claude=str(home / ".claude"), codex=str(home / ".codex"),
                  home=str(home))
    Path(roots.claude).mkdir(parents=True)
    Path(roots.codex).mkdir(parents=True)
    if full:
        _fill(roots)
    return roots


def _fill(r: Roots) -> None:
    home, claude, codex, hangar = Path(r.home), Path(r.claude), Path(r.codex), Path(r.hangar)
    (claude / "CLAUDE.md").write_text("# regras\n")
    (claude / "CLAUDE.local.md").write_text("só desta máquina\n")
    (claude / "rules").mkdir()
    (claude / "rules" / "a.md").write_text("regra a\n")
    repo = home / "Projetos" / "skills" / "minha"
    (repo / ".venv").mkdir(parents=True)
    (repo / ".venv" / "lib.bin").write_bytes(b"x" * 10)
    (repo / "SKILL.md").write_text("skill em repo\n")
    (repo / "run.sh").write_text('#!/bin/sh\necho "${HOME}" {HOME}\n')
    (repo / "run.sh").chmod(0o755)
    (claude / "skills").mkdir()
    (claude / "skills" / "minha").symlink_to(repo)
    (claude / "skills" / "orquestrar").symlink_to(hangar / "skills" / "orquestrar")
    hooks = claude / "hooks"
    hooks.mkdir()
    (hooks / "lembrete.py").write_text(f"print('{home}/notas')\n")
    (hooks / "guard_tmux.py").symlink_to(hangar / "backend" / "hooks" / "guard_tmux.py")
    orca = home / ".orca" / "agent-hooks"
    orca.mkdir(parents=True)
    (orca / "claude-hook.sh").write_text("#!/bin/sh\n. ./lib.sh\n")
    (orca / "claude-hook.sh").chmod(0o755)
    (orca / "lib.sh").write_text("# lib\n")
    settings = {
        "model": "opus",
        "env": {"JIRA_TOKEN": "segredo-jira"},
        "enabledPlugins": {"ponytail@ponytail": True},
        "extraKnownMarketplaces": {
            "ponytail": {"source": {"source": "git", "url": "https://x/ponytail.git"}}},
        "statusLine": {"type": "command", "command":
                       f"'{home}/.local/share/fnm/v24/bin/node' '{hangar}/scripts/statusline.js'"},
        "hooks": {"PreToolUse": [
            {"hooks": [{"type": "command", "command":
                        f'"{hangar}/.venv/bin/python3" "{hangar}/backend/hooks/state_hook.py" || exit 0'}]},
            {"hooks": [{"type": "command", "command": f"python3 {hooks}/lembrete.py"}]},
            {"matcher": "Bash", "hooks": [{"type": "command",
                                           "command": f"/bin/sh '{orca}/claude-hook.sh'"}]},
        ]},
    }
    (claude / "settings.json").write_text(json.dumps(settings))
    (claude / "plugins").mkdir()
    (claude / "plugins" / "known_marketplaces.json").write_text(json.dumps(
        {"ponytail": {"source": {"source": "git", "url": "https://x/ponytail.git"}}}))
    (claude / ".credentials.json").write_text('{"token": "nao-pode-sair"}')
    (claude / "engines.json").write_text(json.dumps(
        {"kimi": {"base_url": "https://k", "api_key": "sk-kimi"}}))
    (home / ".claude.json").write_text(json.dumps({
        "oauthAccount": {"emailAddress": "ana@x"}, "userID": "u-ana",
        "mcpServers": {
            "hangar": {"type": "http", "url": "http://127.0.0.1:8765/mcp/"},
            "grafana": {"type": "http", "url": "https://g",
                        "headers": {"Authorization": "Bearer seg"}}}}))
    (codex / "AGENTS.md").symlink_to(claude / "CLAUDE.md")
    (codex / "auth.json").write_text('{"token": "nao-pode-sair"}')
    (codex / "config.toml").write_text(
        'model = "gpt-6"\n'
        '[projects."/x"]\ntrust_level = "trusted"\n'
        '[mcp_servers.hangar]\nurl = "http://127.0.0.1:8765/mcp/"\n'
        '[mcp_servers.docs]\ncommand = "npx"\n')
    prefs_path(r).write_text(json.dumps(
        {"elevenlabs_api_key": "el-key", "sync": True, "tts_max_chars": 900}))
