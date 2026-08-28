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


def test_projects_mtime_nao_revarre_dentro_do_ttl(tmp_path, monkeypatch):
    """A varredura recursiva custa ~55ms com 9.370 arquivos e roda em caminho quente (cotas por
    requisicao, statusline e previa por leitura de sidecar). Dentro do TTL ela nao pode acontecer."""
    cfg._mtime_cache.clear()
    d = _make_dir(tmp_path, ".claude-work", ts=100)
    relogio = [1000.0]
    monkeypatch.setattr(cfg.time, "monotonic", lambda: relogio[0])

    primeiro = cfg._projects_mtime(d)
    assert primeiro == 100

    novo = d / "projects" / "ws" / "b.jsonl"
    novo.write_text("", encoding="utf-8")
    os.utime(novo, (900, 900))
    assert cfg._projects_mtime(d) == 100, "dentro do TTL tem que devolver o valor guardado"

    relogio[0] += cfg._MTIME_TTL + 1
    assert cfg._projects_mtime(d) == 900, "passado o TTL, revarre e ve o arquivo novo"


def test_projects_mtime_nao_cacheia_a_falha(tmp_path, monkeypatch):
    """Engasgo de disco nao pode virar `mtime 0` congelado por 60s: o valor ordena a lista de
    contas, entao a que a pessoa acabou de usar afundaria pro fim sem nenhuma pista do porque."""
    cfg._mtime_cache.clear()
    d = _make_dir(tmp_path, ".claude-work", ts=100)
    vai_falhar = [True]
    original = cfg.Path.rglob

    def rglob(self, padrao):
        if vai_falhar[0]:
            vai_falhar[0] = False
            raise OSError("disco engasgou")
        return original(self, padrao)

    monkeypatch.setattr(cfg.Path, "rglob", rglob)

    assert cfg._projects_mtime(d) == 0.0
    assert str(d) not in cfg._mtime_cache
    assert cfg._projects_mtime(d) == 100, "a chamada seguinte tenta de novo, sem esperar o TTL"


def test_active_flag_matches_backend_config_dir(tmp_path, monkeypatch):
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.setattr(cfg.Path, "home", classmethod(lambda cls: tmp_path))
    work = _make_dir(tmp_path, ".claude-work")
    _make_dir(tmp_path, ".claude-clean")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(work))
    active = [c for c in cfg.list_config_dirs() if c.active]
    assert len(active) == 1 and active[0].path == str(work.resolve())
