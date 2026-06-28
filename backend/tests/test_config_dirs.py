import os
from pathlib import Path
from app import config as cfg


def _make_dir(home: Path, name: str, *, login=True, projects=True, ts=None):
    d = home / name
    d.mkdir(parents=True, exist_ok=True)
    if login:
        (d / ".credentials.json").write_text("{}", encoding="utf-8")
    if projects:
        pj = d / "projects" / "ws"
        pj.mkdir(parents=True, exist_ok=True)
        f = pj / "a.jsonl"
        f.write_text("", encoding="utf-8")
        if ts:
            os.utime(f, (ts, ts))
    return d


def test_autoscan_finds_login_dirs_with_projects(tmp_path, monkeypatch):
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.setattr(cfg.Path, "home", classmethod(lambda cls: tmp_path))
    _make_dir(tmp_path, ".claude-work", ts=200)
    _make_dir(tmp_path, ".claude-clean", ts=100)
    _make_dir(tmp_path, ".claude-nologin", login=False)
    _make_dir(tmp_path, ".claude-noproj", projects=False)
    out = cfg.list_config_dirs()
    assert [c.label for c in out] == ["work", "clean"]  # recency: work(ts200) before clean(ts100)


def test_env_override_with_labels(tmp_path, monkeypatch):
    a = _make_dir(tmp_path, ".claude-work")
    b = _make_dir(tmp_path, ".claude-clean")
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", f"trabalho:{a},{b}")
    out = cfg.list_config_dirs()
    assert [(c.label, c.path) for c in out] == [("trabalho", str(a.resolve())), ("clean", str(b.resolve()))]


def test_active_flag_matches_backend_config_dir(tmp_path, monkeypatch):
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.setattr(cfg.Path, "home", classmethod(lambda cls: tmp_path))
    work = _make_dir(tmp_path, ".claude-work")
    _make_dir(tmp_path, ".claude-clean")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(work))
    active = [c for c in cfg.list_config_dirs() if c.active]
    assert len(active) == 1 and active[0].path == str(work.resolve())
