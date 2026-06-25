from pathlib import Path
from app.config import _default_projects_dir


def test_default_projects_dir_honors_claude_config_dir(monkeypatch):
    """The transcript dir must follow $CLAUDE_CONFIG_DIR, not a hardcoded ~/.claude —
    machines/users set CLAUDE_CONFIG_DIR to different locations."""
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/tmp/some-custom-config")
    assert _default_projects_dir() == Path("/tmp/some-custom-config/projects")


def test_default_projects_dir_falls_back_to_home(monkeypatch):
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    assert _default_projects_dir() == Path.home() / ".claude" / "projects"
