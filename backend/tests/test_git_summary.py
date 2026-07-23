"""Testes de _parse_status_branch: parse do `git status --porcelain=v1 --branch`."""
from app.git_ops import _parse_status_branch, git_summary


def test_ahead_limpo():
    assert _parse_status_branch("## main...origin/main [ahead 5]\n") == {
        "dirty": 0, "ahead": 5, "behind": 0}


def test_ahead_behind_e_sujo():
    out = "## main...origin/main [ahead 2, behind 1]\n M a.py\n?? b\n"
    assert _parse_status_branch(out) == {"dirty": 2, "ahead": 2, "behind": 1}


def test_sincronizado():
    assert _parse_status_branch("## main...origin/main\n") == {
        "dirty": 0, "ahead": 0, "behind": 0}


def test_rename_conta_uma_linha():
    out = "## main...origin/main\nR  old -> new\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": 0, "behind": 0}


def test_sem_upstream():
    out = "## feature\n M a.py\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": None, "behind": None}


def test_detached():
    out = "## HEAD (no branch)\n M a.py\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": None, "behind": None}


def test_upstream_gone():
    assert _parse_status_branch("## main...origin/main [gone]\n") == {
        "dirty": 0, "ahead": None, "behind": None}


def test_sem_commits():
    out = "## No commits yet on main\n?? a\n"
    assert _parse_status_branch(out) == {"dirty": 1, "ahead": None, "behind": None}


def test_saida_vazia_ou_invalida():
    assert _parse_status_branch("") is None
    assert _parse_status_branch("lixo\n") is None


def test_git_summary_nao_repo(tmp_path):
    # sem .git -> None sem nem forkar git.
    assert git_summary(str(tmp_path)) is None
    assert git_summary(None) is None


def test_git_summary_timeout_vira_none(tmp_path, monkeypatch):
    # _run levanta GitError no timeout (repo enorme/NFS); git_summary TEM que virar None — nunca
    # deixar a exceção subir (senão 500 no /api/sessions + morte do SSE de TODAS as sessões).
    (tmp_path / ".git").mkdir()
    from app import git_ops

    def boom(*a, **k):
        raise git_ops.GitError(500, "timeout")

    monkeypatch.setattr(git_ops, "_run", boom)
    assert git_ops.git_summary(str(tmp_path)) is None


def test_git_summary_usa_timeout_proprio_2s(tmp_path, monkeypatch):
    # git_summary passa timeout PROPRIO (2s), nao o _TIMEOUT global de 20s (push/log): um git status
    # pendurado num NFS custava 20s de tick re-pago a cada poll = watchdog dos clientes estourando.
    (tmp_path / ".git").mkdir()
    from app import git_ops

    seen = {}

    def fake_run(cwd, *a, timeout=None, **k):
        seen["timeout"] = timeout
        return type("P", (), {"returncode": 0, "stdout": "## main...origin/main\n"})()

    monkeypatch.setattr(git_ops, "_run", fake_run)
    git_ops._summary_cache.clear()
    git_ops.git_summary(str(tmp_path))
    assert seen["timeout"] == git_ops._SUMMARY_TIMEOUT == 2.0


def test_git_summary_cache_negativo_mais_longo(tmp_path, monkeypatch):
    # None (timeout/erro) cacheado por _SUMMARY_TTL_NEG (30s), nao 3s: sem isto o git lento era
    # re-forkado a cada poll (3s) = a rajada de forks que estourava o watchdog. Resultado bom segue 3s.
    (tmp_path / ".git").mkdir()
    from app import git_ops

    clock = [1000.0]
    monkeypatch.setattr(git_ops.time, "monotonic", lambda: clock[0])
    calls = {"n": 0}

    def boom(cwd, *a, timeout=None, **k):
        calls["n"] += 1
        raise git_ops.GitError(504, "git timeout")

    monkeypatch.setattr(git_ops, "_run", boom)
    git_ops._summary_cache.clear()

    assert git_ops.git_summary(str(tmp_path)) is None
    clock[0] = 1005.0                       # +5s: dentro do TTL negativo de 30s -> cache, sem re-fork
    assert git_ops.git_summary(str(tmp_path)) is None
    assert calls["n"] == 1
    clock[0] = 1035.0                       # +35s: expirou -> re-tenta
    assert git_ops.git_summary(str(tmp_path)) is None
    assert calls["n"] == 2


def test_git_summary_returncode_nao_zero_usa_ttl_curto(tmp_path, monkeypatch):
    # returncode!=0 (ex. index.lock transitorio) NAO ganha o TTL negativo de 30s: badge some mas volta
    # em 3s quando o lock sai — senao repo SAO ficaria 30s sem badge silenciosamente. So o TIMEOUT (30s).
    (tmp_path / ".git").mkdir()
    from app import git_ops

    clock = [1000.0]
    monkeypatch.setattr(git_ops.time, "monotonic", lambda: clock[0])
    calls = {"n": 0}

    def rc1(cwd, *a, timeout=None, **k):
        calls["n"] += 1
        return type("P", (), {"returncode": 1, "stdout": ""})()

    monkeypatch.setattr(git_ops, "_run", rc1)
    git_ops._summary_cache.clear()

    assert git_ops.git_summary(str(tmp_path)) is None
    clock[0] = 1002.0                       # +2s: ainda no TTL curto (3s) -> cache
    assert git_ops.git_summary(str(tmp_path)) is None
    assert calls["n"] == 1
    clock[0] = 1004.0                       # +4s: TTL curto expirou (NAO 30s) -> re-tenta
    assert git_ops.git_summary(str(tmp_path)) is None
    assert calls["n"] == 2


def test_sessioninfo_serializa_campos_git():
    from app.models import SessionInfo
    s = SessionInfo(name="x", git_dirty=3, git_ahead=2, git_behind=0)
    d = s.model_dump()
    assert d["git_dirty"] == 3 and d["git_ahead"] == 2 and d["git_behind"] == 0


def test_sessioninfo_git_default_none():
    from app.models import SessionInfo
    d = SessionInfo(name="x").model_dump()
    assert d["git_dirty"] is None and d["git_ahead"] is None and d["git_behind"] is None
