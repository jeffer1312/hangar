import os
import time

import pytest
from fastapi.testclient import TestClient

from app.config import resolve_scan_roots, settings
from app.fs import FsError, list_roots, scan_dir


def _set_roots(*paths):
    # CP_SCAN_ROOTS e uma string crua "a,b"; resolve_scan_roots faz expanduser+realpath.
    settings.scan_roots = ",".join(str(p) for p in paths)


def _real(p):
    return os.path.realpath(os.path.expanduser(str(p)))


# ── config: resolucao da allowlist ──────────────────────────────────────────
def test_resolve_scan_roots_expands_and_drops_missing(tmp_path):
    good = tmp_path / "pessoal"
    good.mkdir()
    _set_roots(good, tmp_path / "nao-existe")
    roots = resolve_scan_roots(settings)
    assert [str(r) for r in roots] == [_real(good)]


def test_resolve_scan_roots_dedupes(tmp_path):
    good = tmp_path / "pessoal"
    good.mkdir()
    _set_roots(good, good)
    assert len(resolve_scan_roots(settings)) == 1


# ── list_roots ──────────────────────────────────────────────────────────────
def test_list_roots_names_are_basenames(tmp_path):
    a = tmp_path / "pessoal"
    a.mkdir()
    b = tmp_path / "sistemas"
    b.mkdir()
    _set_roots(a, b)
    roots = list_roots()
    assert {r.name for r in roots} == {"pessoal", "sistemas"}
    assert {r.path for r in roots} == {_real(a), _real(b)}


# ── listing basico + flags + ordenacao ──────────────────────────────────────
def test_scan_lists_subdirs_with_flags_sorted_by_mtime(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    _set_roots(root)

    proj = root / "alpha"
    proj.mkdir()
    (proj / ".git").mkdir()
    (proj / "CLAUDE.md").write_text("x", encoding="utf-8")
    plain = root / "beta"
    plain.mkdir()
    (root / ".hidden").mkdir()                       # dot-dir escondido
    (root / "note.txt").write_text("x", encoding="utf-8")  # arquivo ignorado

    now = time.time()
    os.utime(proj, (now - 100, now - 100))
    os.utime(plain, (now, now))

    res = scan_dir(str(root))
    assert res.error is None
    assert [e.name for e in res.entries] == ["beta", "alpha"]  # mtime desc, dirs-only
    by = {e.name: e for e in res.entries}
    assert by["alpha"].is_git is True
    assert by["alpha"].has_claude_md is True
    assert by["beta"].is_git is False
    assert by["beta"].has_claude_md is False
    assert by["alpha"].path == _real(proj)


def test_scan_defaults_path_to_root_and_drills(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    _set_roots(root)
    proj = root / "alpha"
    proj.mkdir()
    (proj / "backend").mkdir()

    # default: path = root
    assert [e.name for e in scan_dir(str(root)).entries] == ["alpha"]
    # drill: path = subdir
    assert [e.name for e in scan_dir(str(root), str(proj)).entries] == ["backend"]


# ── seguranca: allowlist + escapes ──────────────────────────────────────────
def test_scan_root_not_allowed(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    other = tmp_path / "fora"
    other.mkdir()
    _set_roots(root)
    with pytest.raises(FsError) as ei:
        scan_dir(str(other))
    assert ei.value.status == 403


def test_scan_rejects_parent_escape(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    _set_roots(root)
    with pytest.raises(FsError) as ei:
        scan_dir(str(root), str(root / ".." / ".."))
    assert ei.value.status == 400


def test_scan_rejects_symlink_escape(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    outside = tmp_path / "segredo"
    outside.mkdir()
    link = root / "escape"
    link.symlink_to(outside, target_is_directory=True)
    _set_roots(root)
    # path aponta pra um symlink cujo realpath sai da raiz -> rejeitado
    with pytest.raises(FsError) as ei:
        scan_dir(str(root), str(link))
    assert ei.value.status == 400


def test_scan_hides_escaping_symlink_child(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    outside = tmp_path / "segredo"
    outside.mkdir()
    (root / "escape").symlink_to(outside, target_is_directory=True)
    (root / "real").mkdir()
    _set_roots(root)
    names = [e.name for e in scan_dir(str(root)).entries]
    assert "escape" not in names  # nunca lista um symlink que sai da raiz
    assert "real" in names


def test_scan_missing_path_404(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    _set_roots(root)
    with pytest.raises(FsError) as ei:
        scan_dir(str(root), str(root / "inexistente"))
    assert ei.value.status == 404


def test_scan_not_a_directory_400(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    f = root / "file.txt"
    f.write_text("x", encoding="utf-8")
    _set_roots(root)
    with pytest.raises(FsError) as ei:
        scan_dir(str(root), str(f))
    assert ei.value.status == 400


@pytest.mark.skipif(os.geteuid() == 0, reason="root ignora permissoes de diretorio")
def test_scan_permission_denied_soft(tmp_path):
    root = tmp_path / "pessoal"
    root.mkdir()
    locked = root / "locked"
    locked.mkdir()
    _set_roots(root)
    os.chmod(locked, 0)
    try:
        res = scan_dir(str(root), str(locked))
    finally:
        os.chmod(locked, 0o755)  # restaura pra limpeza do tmp_path
    assert res.entries == []
    assert res.error == "permission_denied"


# ── nivel HTTP: auth + traducao do status ───────────────────────────────────
def test_fs_routes_require_auth():
    settings.auth_token = "secret"
    from app.api import app
    c = TestClient(app)
    assert c.get("/api/fs/roots").status_code == 401
    assert c.get("/api/fs/scan", params={"root": "/x"}).status_code == 401


def test_fs_roots_route_lists(tmp_path):
    settings.auth_token = "secret"
    a = tmp_path / "pessoal"
    a.mkdir()
    _set_roots(a)
    from app.api import app
    c = TestClient(app)
    r = c.get("/api/fs/roots", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    assert r.json()[0]["name"] == "pessoal"


def test_fs_scan_route_translates_root_not_allowed(tmp_path):
    settings.auth_token = "secret"
    root = tmp_path / "pessoal"
    root.mkdir()
    _set_roots(root)
    from app.api import app
    c = TestClient(app)
    r = c.get(
        "/api/fs/scan",
        params={"root": str(tmp_path / "fora")},
        headers={"Authorization": "Bearer secret"},
    )
    assert r.status_code == 403
