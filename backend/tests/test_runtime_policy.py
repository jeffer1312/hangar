import asyncio

import pytest

from app import runtime_policy


def test_policy_failure_is_visible_and_redacted(monkeypatch):
    from app import diag
    records = []
    monkeypatch.setattr(diag, "registrar", lambda *args, **kwargs: records.append((args, kwargs)))
    async def scenario():
        result = await runtime_policy.execute("unsupported", {"text":"private-input", "token":"private-secret"}, {"provider":"claude"})
        assert result["ok"] is False
        assert result["error_type"] == "ValueError"
    asyncio.run(scenario())
    assert "private-input" not in repr(records)
    assert "private-secret" not in repr(records)


def test_metadata_patch_has_a_strict_catalog(monkeypatch):
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "update", lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("escrita indevida")))
    with pytest.raises(ValueError):
        runtime_policy.run("session.patch_meta", {"key":"changed"}, {"provider":"codex", "name":"session"})


@pytest.mark.parametrize("kind", ["prepare_prompt", "format_status", "skill_catalog", "answer_body", "quota",
                                  "session.marker", "diag.error", "last_usage", "reload_stamp", "unknown_private"])
def test_services_that_moved_to_rust_are_gone(kind):
    # O ator Rust roda estes por conta própria; o Python não responde mais.
    for provider in ("claude", "codex"):
        with pytest.raises(ValueError):
            runtime_policy.run(kind, {"text": "Olá", "catalog": {}, "questions": [], "answers": []}, {"provider": provider})


def test_native_message_has_journal_before_uds(monkeypatch):
    from app import api, uds_messaging
    validated = []
    monkeypatch.setattr(uds_messaging, "socket_da_sessao", lambda *args: "fake.sock")
    monkeypatch.setattr(api, "_classe_modo", lambda *args: "prompting")
    def send(*args, **kwargs):
        assert validated == [True]
        assert kwargs["msg_id"]
        raise OSError("write sem prova")
    monkeypatch.setattr(uds_messaging, "enviar", send)
    result = runtime_policy.native_message({"text":"[de: source] Olá"}, {"key":"key", "name":"session",
        "session_id":"sid", "operation_id":"op", "validate":lambda: validated.append(True)})
    assert result["outcome"] == "unknown"


def test_quota_windows_drops_per_model_windows(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from app import cotas
    runtime_policy._quota_cache.clear()
    account = SimpleNamespace(provedor="claude", id="claude:" + str(tmp_path), model_dump=lambda: {"janelas": [
        {"rotulo": "5h", "pct": 42, "reset_ts": None, "por_modelo": False},
        {"rotulo": "7d", "pct": 9, "reset_ts": None, "por_modelo": True}]})
    monkeypatch.setattr(cotas, "listar_cotas", lambda: [account])
    assert runtime_policy.quota_windows(str(tmp_path)) == [{"rotulo": "5h", "pct": 42, "reset_ts": None, "por_modelo": False}]
    # Sem cache próprio: uma segunda chamada lê de novo.
    account.model_dump = lambda: {"janelas": [{"rotulo": "5h", "pct": 50, "reset_ts": None}]}
    assert runtime_policy.quota_windows(str(tmp_path))[0]["pct"] == 50
    runtime_policy._quota_cache.clear()


def _codex_patch(tmp_path, monkeypatch, sidecar_thread):
    import json
    from app.adapters.codex import sessions
    writes = []
    state = tmp_path / "state.json"
    # Vista que o Rust grava antes de chamar o serviço (control_view com service_tier no topo).
    state.write_text(json.dumps({"runtime_state":{"view":{"thread_id":"t1", "service_tier":"priority"}}}))
    monkeypatch.setattr(sessions, "load", lambda name: {"key":"k", "thread_id":sidecar_thread})
    monkeypatch.setattr(sessions, "update", lambda name, **fields: writes.append(fields) or fields)
    result = runtime_policy.run("session.patch_meta", {"service_tier":"priority"},
        {"provider":"codex", "name":"session", "key":"k", "validate":lambda: None, "state_path":str(state)})
    return result, writes


def test_codex_patch_accepts_service_tier_from_rust(tmp_path, monkeypatch):
    result, writes = _codex_patch(tmp_path, monkeypatch, "t1")
    assert result == {"updated": True}
    assert writes == [{"service_tier":"priority"}]


def test_codex_service_tier_skips_sidecar_of_another_thread(tmp_path, monkeypatch):
    result, writes = _codex_patch(tmp_path, monkeypatch, "t2")
    assert result == {"updated": False, "stale": True}
    assert writes == []


def _fake_codex(tmp_path, monkeypatch):
    directory = tmp_path / "bin"
    directory.mkdir()
    exe = directory / "codex"
    exe.write_text("#!/bin/sh\n")
    exe.chmod(0o755)
    monkeypatch.setenv("PATH", str(directory))
    return exe


def test_launch_env_da_sessao_codex(monkeypatch, tmp_path):
    from pathlib import Path
    exe = _fake_codex(tmp_path, monkeypatch)
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "codex-sessions")
    monkeypatch.setenv("TMUX", "x")
    monkeypatch.setenv("TMUX_PANE", "%1")
    meta = {"name": "cx", "key": "k" * 32, "codex_account": "default", "jev": False, "cwd": str(tmp_path)}
    out = runtime_policy.run("launch_env", {}, {"provider": "codex", **meta})
    env = out["env"]
    assert env["CP_SESSION_KEY"] == env["HANGAR_CANO_KEY"] == "k" * 32
    assert env["HANGAR_CANO_OWNER"] == str(Path.home())
    assert "TMUX" not in env and "TMUX_PANE" not in env and "CODEX_HOME" in env
    assert Path(out["program"][0]).name.startswith("codex") and out["program"][1:3] == ["app-server", "--stdio"]
    assert out["program"][0] == str(exe), "caminho resolvido: o Rust não procura o codex"
    assert 'sandbox_mode="danger-full-access"' in out["program"], "os -c do modo da sessão"
    assert out["cano_extra"] == {}


def test_launch_env_reads_the_mode_the_rust_recorded(monkeypatch, tmp_path):
    import json
    from app.adapters.codex import sessions
    _fake_codex(tmp_path, monkeypatch)
    folder = tmp_path / "codex-sessions"
    folder.mkdir()
    monkeypatch.setattr(sessions, "_dir", lambda: folder)
    (folder / "cx.json").write_text(json.dumps({"name": "cx", "key": "k" * 32, "headless": True, "permission_mode": "Ask for approval"}))
    meta = {"name": "cx", "key": "k" * 32, "codex_account": "default", "jev": False, "cwd": str(tmp_path), "permission_mode": "Full Access"}
    out = runtime_policy.run("launch_env", {}, {"provider": "codex", **meta})
    assert 'sandbox_mode="read-only"' in out["program"], "trocar o sandbox sobe com o modo novo, não o da abertura"


def test_codex_patch_meta_accepts_the_permission_mode(tmp_path, monkeypatch):
    import json
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    (tmp_path / "cx.json").write_text(json.dumps({"name": "cx", "key": "k", "headless": True}))
    state = tmp_path / "state.json"
    state.write_text(json.dumps({"runtime_state": {"view": {"permission_mode": "Ask for approval"}}}))
    assert runtime_policy.run("session.patch_meta", {"permission_mode": "Ask for approval"}, {"provider": "codex", "name": "cx",
        "key": "k", "validate": lambda: None, "state_path": str(state)}) == {"updated": True}
    assert sessions.load("cx")["permission_mode"] == "Ask for approval"


def test_launch_env_without_codex_is_a_code(monkeypatch, tmp_path):
    monkeypatch.setenv("PATH", str(tmp_path))
    meta = {"name": "cx", "key": "k" * 32, "cwd": str(tmp_path)}
    assert runtime_policy.run("launch_env", {}, {"provider": "codex", **meta}) == {"error": "codex_ausente"}


def test_patch_meta_records_the_cano_rust_launched(tmp_path, monkeypatch):
    import json
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    (tmp_path / "cx.json").write_text(json.dumps({"name": "cx", "key": "k", "headless": True}))
    state = tmp_path / "state.json"
    state.write_text(json.dumps({"runtime_state": {"view": {}}}))
    cano = {"pid": 5, "escuta": "unix:/x", "token": "t", "ts": 1.0, "versao": 2}
    assert runtime_policy.run("session.patch_meta", {"cano": cano}, {"provider": "codex", "name": "cx", "key": "k",
        "validate": lambda: None, "state_path": str(state)}) == {"updated": True}
    assert sessions.load("cx")["cano"] == cano


def test_clear_cano_only_for_the_same_pid_and_never_recreates(tmp_path, monkeypatch):
    import json
    from app.adapters.codex import sessions
    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path)
    path = tmp_path / "cx.json"
    path.write_text(json.dumps({"name": "cx", "key": "k", "headless": True, "cano": {"pid": 5}}))
    meta = {"provider": "codex", "name": "cx", "key": "k", "validate": lambda: None}
    assert runtime_policy.run("session.clear_cano", {"pid": 6}, meta) == {"cleared": False}
    assert sessions.load("cx")["cano"] == {"pid": 5}, "outro processo já foi gravado: fica"
    assert runtime_policy.run("session.clear_cano", {"pid": 5}, meta) == {"cleared": True}
    assert sessions.load("cx")["cano"] is None
    path.unlink()
    assert runtime_policy.run("session.clear_cano", {"pid": 5}, meta) == {"cleared": False}
    assert not path.exists(), "arquivo apagado não volta"
