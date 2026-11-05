import asyncio

import os

import pytest

from app import procinfo

from app import registry as reg

def test_proxy_init_response_does_not_replace_fixed_route(tmp_path, monkeypatch):
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter, _Sessao
    from app.adapters.claude_headless import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "hl")
    meta = sessions.save("fixed", str(tmp_path), "11111111-1111-1111-1111-111111111111",
                         engine="proxy", engine_account="default", model="fixed/gpt-5.5")
    session = _Sessao("fixed", meta)
    asyncio.run(ClaudeHeadlessAdapter()._on_system(session, {"subtype": "init", "model": "gpt-5.5"}))
    assert session.model == "fixed/gpt-5.5"
    assert sessions.load("fixed")["engine_account"] == "default"

@pytest.mark.skipif(os.name != "posix",
                    reason="exercita o ramo /proc do _engine_of contra um /proc de mentira; no "
                           "Windows o despacho vai pro psutil e nao ha o que este caso testa")
def test_engine_of_le_o_cp_engine_do_proc(tmp_path, monkeypatch):
    # Mesmo truque do _config_dir_of: o env do processo VIVO é o registro autoritativo — um sidecar
    # em disco pode divergir do que está de fato rodando no pane.
    environ = tmp_path / "environ"
    environ.write_bytes(b"PATH=/usr/bin\x00CP_ENGINE=kimi\x00HOME=/home/x\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(environ))
    assert reg._engine_of(1234) == "kimi"

def test_engine_of_sem_a_marca_e_none(tmp_path, monkeypatch):
    environ = tmp_path / "environ"
    environ.write_bytes(b"PATH=/usr/bin\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(environ))
    assert reg._engine_of(1234) is None
