import os

import pytest

from app.config import resolve_scan_roots, settings


@pytest.fixture(autouse=True)
def _isola_runtime_config(tmp_path_factory, monkeypatch):
    # resolve_scan_roots agora le o override do runtime_config primeiro; sem isolar, um
    # runtime-config.json real da maquina de dev mudaria o resultado destes testes.
    from app import runtime_config as rc
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path_factory.mktemp("rc"))
    yield


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


def test_resolve_scan_roots_override_do_app_vence_o_env(tmp_path, monkeypatch):
    """scan_roots editavel pela tela (runtime_config): override nao-vazio GANHA do env;
    vazio volta ao CP_SCAN_ROOTS."""
    from app import runtime_config as rc
    monkeypatch.setattr(rc, "_backend_config_base", lambda: tmp_path / "rc")
    do_env = tmp_path / "do-env"
    do_env.mkdir()
    do_app = tmp_path / "do-app"
    do_app.mkdir()
    _set_roots(do_env)
    rc.aplicar({"scan_roots": str(do_app)})
    assert [str(r) for r in resolve_scan_roots(settings)] == [_real(do_app)]
    rc.aplicar({"scan_roots": ""})
    assert [str(r) for r in resolve_scan_roots(settings)] == [_real(do_env)]
