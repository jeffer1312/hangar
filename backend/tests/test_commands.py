from unittest.mock import patch

from fastapi.testclient import TestClient

from app.commands import list_commands
from app.config import settings
from app.models import SessionInfo


def _names(cmds):
    return {c.name for c in cmds}


def _by_name(cmds):
    return {c.name: c for c in cmds}


def test_builtins_present(monkeypatch, tmp_path):
    # HOME vazio -> scan de skills globais nao acrescenta nada (resultado deterministico).
    monkeypatch.setenv("HOME", str(tmp_path))
    cmds = list_commands(None)
    required = {
        "clear", "compact", "context", "model", "effort", "resume", "rewind",
        "release-notes", "help", "status", "cost", "export", "init", "agents",
        "mcp", "memory", "vim", "config", "doctor",
    }
    assert required <= _names(cmds)
    by = _by_name(cmds)
    assert by["clear"].display == "/clear"
    assert by["clear"].source == "builtin"


def test_destructive_flags(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    by = _by_name(list_commands(None))
    assert by["clear"].destructive is True
    assert by["compact"].destructive is True
    assert by["quit"].destructive is True
    assert by["context"].destructive is False
    assert by["model"].destructive is False


def test_scan_project_commands_and_skills(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))

    cwd = tmp_path / "proj"
    cmd_dir = cwd / ".claude" / "commands"
    cmd_dir.mkdir(parents=True)
    (cmd_dir / "deploy.md").write_text(
        "---\n"
        "description: Sobe pra produção\n"
        "argument-hint: <ambiente>\n"
        "---\n\nfaz deploy\n",
        encoding="utf-8",
    )
    # comando sem frontmatter -> nome derivado do arquivo, sem dica de argumento
    (cmd_dir / "bare.md").write_text("sem frontmatter\n", encoding="utf-8")

    skills_dir = cwd / ".claude" / "skills"
    (skills_dir / "fixer").mkdir(parents=True)
    (skills_dir / "fixer" / "SKILL.md").write_text(
        "---\nname: fixer\ndescription: Conserta coisas\n---\n\ncorpo\n",
        encoding="utf-8",
    )

    by = _by_name(list_commands(str(cwd)))

    assert by["deploy"].source == "skill"
    assert by["deploy"].argumentHint == "<ambiente>"
    assert by["deploy"].description == "Sobe pra produção"
    assert by["deploy"].display == "/deploy"

    assert "bare" in by
    assert by["bare"].argumentHint is None

    assert by["fixer"].source == "skill"
    assert by["fixer"].description == "Conserta coisas"


def test_home_skills_scanned(monkeypatch, tmp_path):
    home = tmp_path / "home"
    skill = home / ".claude" / "skills" / "globber"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(
        "---\nname: globber\ndescription: skill global\n---\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("HOME", str(home))
    by = _by_name(list_commands(None))
    assert "globber" in by
    assert by["globber"].source == "skill"


def test_missing_dirs_skip(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    # cwd existe mas nao tem .claude -> so built-ins, sem crash
    cwd = tmp_path / "empty"
    cwd.mkdir()
    cmds = list_commands(str(cwd))
    assert _names(cmds) >= {"clear", "help"}
    assert all(c.source == "builtin" for c in cmds)


def test_dedupe_keeps_builtin_priority(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    # um comando de projeto chamado 'clear' nao deve sobrescrever o built-in destrutivo
    cwd = tmp_path / "proj"
    cmd_dir = cwd / ".claude" / "commands"
    cmd_dir.mkdir(parents=True)
    (cmd_dir / "clear.md").write_text("---\ndescription: outro\n---\n", encoding="utf-8")
    by = _by_name(list_commands(str(cwd)))
    assert by["clear"].source == "builtin"
    assert by["clear"].destructive is True


def test_commands_route(monkeypatch, tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    settings.auth_token = "secret"
    from app.api import app
    client = TestClient(app)
    with patch("app.api.registry.list",
               return_value=[SessionInfo(name="cc", cwd=str(tmp_path))]):
        r = client.get("/api/sessions/cc/commands", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    data = r.json()
    assert "clear" in {c["name"] for c in data}
    # chave camelCase preservada no contrato JSON
    assert "argumentHint" in data[0]


def test_commands_route_requires_auth():
    settings.auth_token = "secret"
    from app.api import app
    client = TestClient(app)
    assert client.get("/api/sessions/cc/commands").status_code == 401
