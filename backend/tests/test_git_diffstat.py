"""Testes de git_diffstat/_parse_numstat: o "+N -M" do card (working tree vs HEAD)."""
from app.git_ops import _parse_numstat, git_diffstat


def test_numstat_soma_added_removed():
    out = "10\t2\ta.py\n3\t0\tdir/b.py\n"
    assert _parse_numstat(out) == {"added": 13, "removed": 2}


def test_numstat_binario_conta_zero():
    # Binario vem "-\t-\t<path>" (o numero nao existe) -> ignora, nao soma nem quebra.
    out = "5\t1\ta.py\n-\t-\timg.png\n"
    assert _parse_numstat(out) == {"added": 5, "removed": 1}


def test_numstat_vazio_e_linha_malformada():
    assert _parse_numstat("") == {"added": 0, "removed": 0}
    assert _parse_numstat("lixo sem tabs\n7\tx\n") == {"added": 0, "removed": 0}


def test_git_diffstat_nao_repo(tmp_path):
    # sem .git -> None sem nem forkar git.
    assert git_diffstat(str(tmp_path)) is None
    assert git_diffstat(None) is None


def test_git_diffstat_timeout_vira_none(tmp_path, monkeypatch):
    # _run levanta GitError no timeout (repo enorme/NFS); TEM que virar None — nunca deixar subir
    # (senao 500 no /api/sessions + morte do SSE de TODAS as sessoes).
    (tmp_path / ".git").mkdir()
    from app import git_ops

    def boom(*a, **k):
        raise git_ops.GitError(504, "git timeout")

    monkeypatch.setattr(git_ops, "_run", boom)
    git_ops._diffstat_cache.clear()
    assert git_ops.git_diffstat(str(tmp_path)) is None


def test_git_diffstat_usa_timeout_proprio_2s(tmp_path, monkeypatch):
    # timeout PROPRIO (2s), nao o _TIMEOUT global de 20s — mesma regra do git_summary (watchdog).
    (tmp_path / ".git").mkdir()
    from app import git_ops

    seen = {}

    def fake_run(cwd, *a, timeout=None, **k):
        seen["timeout"] = timeout
        return type("P", (), {"returncode": 0, "stdout": "4\t1\ta.py\n", "stderr": ""})()

    monkeypatch.setattr(git_ops, "_run", fake_run)
    git_ops._diffstat_cache.clear()
    assert git_ops.git_diffstat(str(tmp_path)) == {"added": 4, "removed": 1}
    assert seen["timeout"] == git_ops._SUMMARY_TIMEOUT == 2.0


def test_git_diffstat_cache_ttl_curto(tmp_path, monkeypatch):
    # Resultado bom cacheado por 3s: sem isto seria um fork de `git diff` POR SESSAO POR POLL (2s).
    (tmp_path / ".git").mkdir()
    from app import git_ops

    clock = [1000.0]
    monkeypatch.setattr(git_ops.time, "monotonic", lambda: clock[0])
    calls = {"n": 0}

    def fake_run(cwd, *a, timeout=None, **k):
        calls["n"] += 1
        return type("P", (), {"returncode": 0, "stdout": "1\t0\ta.py\n", "stderr": ""})()

    monkeypatch.setattr(git_ops, "_run", fake_run)
    git_ops._diffstat_cache.clear()

    assert git_ops.git_diffstat(str(tmp_path)) == {"added": 1, "removed": 0}
    clock[0] = 1002.0                        # +2s: dentro do TTL -> cache, sem re-fork
    assert git_ops.git_diffstat(str(tmp_path)) == {"added": 1, "removed": 0}
    assert calls["n"] == 1
    clock[0] = 1004.0                        # +4s: expirou -> re-forka
    assert git_ops.git_diffstat(str(tmp_path)) == {"added": 1, "removed": 0}
    assert calls["n"] == 2


def test_git_diffstat_timeout_cache_negativo_longo(tmp_path, monkeypatch):
    # Timeout (git pendurado) cacheado por 30s, nao 3s — senao o fork lento se repagava a cada poll.
    (tmp_path / ".git").mkdir()
    from app import git_ops

    clock = [1000.0]
    monkeypatch.setattr(git_ops.time, "monotonic", lambda: clock[0])
    calls = {"n": 0}

    def boom(cwd, *a, timeout=None, **k):
        calls["n"] += 1
        raise git_ops.GitError(504, "git timeout")

    monkeypatch.setattr(git_ops, "_run", boom)
    git_ops._diffstat_cache.clear()

    assert git_ops.git_diffstat(str(tmp_path)) is None
    clock[0] = 1005.0                        # +5s: dentro do TTL negativo -> cache
    assert git_ops.git_diffstat(str(tmp_path)) is None
    assert calls["n"] == 1
    clock[0] = 1035.0                        # +35s: expirou -> re-tenta
    assert git_ops.git_diffstat(str(tmp_path)) is None
    assert calls["n"] == 2


def test_git_diffstat_repo_sem_commits_sem_spam(tmp_path, monkeypatch, caplog):
    # Repo sem commits: HEAD nao resolve ("ambiguous argument 'HEAD'") — caso ESPERADO, vira None
    # SEM warning (senao todo repo novo poluía o log a cada poll). TTL curto: o 1o commit liga.
    (tmp_path / ".git").mkdir()
    from app import git_ops
    import logging

    def sem_head(cwd, *a, timeout=None, **k):
        return type("P", (), {"returncode": 128, "stdout": "",
                              "stderr": "fatal: ambiguous argument 'HEAD': unknown revision"})()

    monkeypatch.setattr(git_ops, "_run", sem_head)
    git_ops._diffstat_cache.clear()
    with caplog.at_level(logging.WARNING, logger="claude_pocket.git_ops"):
        assert git_ops.git_diffstat(str(tmp_path)) is None
    assert not [r for r in caplog.records if r.levelno >= logging.WARNING]


def test_git_diffstat_returncode_nao_zero_avisa(tmp_path, monkeypatch, caplog):
    # returncode!=0 que NAO e "sem HEAD" (transiente de verdade): falha APARECE (warning) e o
    # TTL segue curto (3s) — volta rapido quando normaliza.
    (tmp_path / ".git").mkdir()
    from app import git_ops
    import logging

    def rc1(cwd, *a, timeout=None, **k):
        return type("P", (), {"returncode": 1, "stdout": "", "stderr": "something broke"})()

    monkeypatch.setattr(git_ops, "_run", rc1)
    git_ops._diffstat_cache.clear()
    with caplog.at_level(logging.WARNING, logger="claude_pocket.git_ops"):
        assert git_ops.git_diffstat(str(tmp_path)) is None
    assert any("git_diffstat" in r.getMessage() for r in caplog.records)


def test_sessioninfo_serializa_campos_diff():
    from app.models import SessionInfo
    s = SessionInfo(name="x", git_added=128, git_removed=24)
    d = s.model_dump()
    assert d["git_added"] == 128 and d["git_removed"] == 24


def test_sessioninfo_diff_default_none():
    from app.models import SessionInfo
    d = SessionInfo(name="x").model_dump()
    assert d["git_added"] is None and d["git_removed"] is None
