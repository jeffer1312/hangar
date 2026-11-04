"""orq_identity.identity: sidecar sem terminal antes do tmux; no tmux, o alvo resolve a sessão."""
import json
import shutil
import subprocess
import uuid

import pytest

from orq_contract_client import identity as orq_identity


@pytest.fixture
def home(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    return tmp_path


def _spy_mux(tmp_path):
    marker = tmp_path / "mux-called"
    spy = tmp_path / "spy-tmux"
    spy.write_text(f"#!/bin/sh\ntouch {marker}\nexit 1\n")
    spy.chmod(0o755)
    return spy, marker


@pytest.mark.parametrize("directory,provider", [("claude-headless", "claude"), ("codex-sessions", "codex")])
def test_headless_sidecar_key_resolves_without_tmux(home, tmp_path, directory, provider):
    side = home / ".hangar" / directory
    side.mkdir(parents=True)
    (side / "worker.json").write_text(json.dumps({"key": "k-1", "provider": provider}))
    spy, marker = _spy_mux(tmp_path)
    assert orq_identity.identity("worker", mux_bin=str(spy)) == f"{provider}:k-1"
    assert not marker.exists()


@pytest.mark.skipif(not shutil.which("tmux"), reason="tmux not installed")
def test_tmux_session_resolves_on_an_isolated_server(home, tmp_path):
    sock = f"orq-id-{uuid.uuid4().hex[:8]}"
    mux = tmp_path / "mux"
    mux.write_text(f'#!/bin/sh\nexec tmux -L {sock} -f /dev/null "$@"\n')
    mux.chmod(0o755)
    subprocess.run([str(mux), "new-session", "-d", "-s", "worker", "sleep 60"], check=True)
    try:
        got = orq_identity.identity("worker", mux_bin=str(mux))
        assert got.startswith("tmux:") and got.split(":")[3].startswith("$"), got
        with pytest.raises(orq_identity.IdentityUnavailable):
            orq_identity.identity("absent", mux_bin=str(mux))
    finally:
        subprocess.run([str(mux), "kill-session", "-t", "=worker"], capture_output=True)
