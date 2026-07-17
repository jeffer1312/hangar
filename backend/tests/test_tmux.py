import shutil
import subprocess
import uuid
from unittest.mock import MagicMock, patch

import pytest

from app import tmux


def test_list_sessions_parses_output():
    fake = MagicMock(stdout="cc\t/home/u/p\nweb\t/home/u/w\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake) as run:
        out = tmux.list_sessions()
    assert out == [
        {"name": "cc", "cwd": "/home/u/p"},
        {"name": "web", "cwd": "/home/u/w"},
    ]
    args = run.call_args[0][0]
    assert args[:2] == ["tmux", "list-sessions"]


def test_list_sessions_empty_when_no_server():
    fake = MagicMock(stdout="", returncode=1, stderr="no server running")
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_sessions() == []


def test_send_keys_literal_uses_dashdash():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "echo hi", literal=True)
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "-l", "--", "echo hi"]


def test_send_keys_named_key():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "Enter")
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "Enter"]


def test_capture_pane_returns_stdout():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="screen", returncode=0)) as run:
        assert tmux.capture_pane("cc") == "screen"
    assert run.call_args[0][0][:2] == ["tmux", "capture-pane"]


@pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux nao instalado no ambiente")
def test_has_session_is_exact_against_real_tmux():
    # SEMANTICA REAL do tmux (nao mock): sem o `=`, o `-t` resolve exact -> fnmatch -> PREFIX match,
    # entao `has_session("X")` respondia VIVO por causa da IRMA "X-2" — e o /input dava "entregue"
    # digitando num pane que nao existe. Os outros testes stubam has_session, por isso nenhum via.
    # Socket proprio (-L) e sessao de nome aleatorio: nao encosta no tmux/sessoes do usuario.
    sock = f"cp-test-{uuid.uuid4().hex[:8]}"
    base = f"pocket-{uuid.uuid4().hex[:6]}"

    def tmux_on_sock(args, **_kw):
        # injeta o `-L <socket>` logo apos o "tmux" -> o has_session real roda contra ESTE servidor
        return subprocess.run(["tmux", "-L", sock, *args[1:]], capture_output=True, text=True)

    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", f"{base}-2", "sleep 60"],
                   capture_output=True, text=True)
    try:
        with patch.object(tmux, "RUN", tmux_on_sock):
            assert tmux.has_session(f"{base}-2") is True   # exata e viva
            assert tmux.has_session(base) is False         # NUNCA existiu (so a irma "-2") -> prefix mentia
            assert tmux.has_session(base[:-3]) is False    # prefixo puro
    finally:
        # kill-SESSION (alvo exato), nunca kill-server: um `-L` esquecido num kill-server derruba o
        # servidor tmux DEFAULT e com ele todas as sessoes do usuario. Matar a unica sessao ja encerra
        # este servidor sozinho, e um socket orfao vazio e inofensivo — nao vale o risco do atalho.
        subprocess.run(["tmux", "-L", sock, "kill-session", "-t", f"={base}-2"],
                       capture_output=True, text=True)


def test_pane_target_uses_exact_session_form():
    # Nome NUMERICO (0/1/2) nao pode virar indice de window -> `=NAME:` forca match exato de sessao.
    assert tmux._pane_target("0") == "=0:"
    assert tmux._pane_target("cc") == "=cc:"


def test_capture_pane_targets_exact_session():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="", returncode=0)) as run:
        tmux.capture_pane("0")
    assert "=0:" in run.call_args[0][0]


def test_pane_pid_targets_exact_session():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="540144\n", returncode=0)) as run:
        assert tmux.pane_pid("0") == 540144
    assert "=0:" in run.call_args[0][0]


class _CP:
    returncode = 0
    stdout = ""
    stderr = ""


def test_new_session_forwards_explicit_config_dir():
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x", config_dir="/home/u/.claude-clean")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-clean" in captured["args"]


def test_new_session_falls_back_to_backend_config_dir(monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/home/u/.claude-work")
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-work" in captured["args"]


def test_scope_prefix_empty_without_runtime_dir(monkeypatch):
    # Sem XDG_RUNTIME_DIR (host nao-systemd) -> spawn direto, sem wrap.
    monkeypatch.delenv("XDG_RUNTIME_DIR", raising=False)
    assert tmux._scope_prefix() == []


def test_scope_prefix_wraps_when_systemd_available(monkeypatch):
    # Com runtime dir + systemd-run -> tmux nasce em scope proprio (fora do cgroup do backend).
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    assert tmux._scope_prefix()[:3] == ["systemd-run", "--user", "--scope"]


def test_new_session_passes_wayland_display_from_env(monkeypatch):
    monkeypatch.setenv("WAYLAND_DISPLAY", "wayland-7")
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "WAYLAND_DISPLAY=wayland-7" in captured["args"]


def test_new_session_detects_wayland_socket(monkeypatch, tmp_path):
    # Backend como servico systemd nao tem WAYLAND_DISPLAY -> detecta o socket no runtime dir
    # (ignorando o .lock). Sem isto, wl-paste no pane falha e o paste de imagem no claude morre.
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    (tmp_path / "wayland-1").touch()
    (tmp_path / "wayland-1.lock").touch()
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "WAYLAND_DISPLAY=wayland-1" in captured["args"]


def test_new_session_skips_wayland_without_socket(monkeypatch, tmp_path):
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert not any(str(a).startswith("WAYLAND_DISPLAY=") for a in captured["args"])


def test_new_session_execs_command_so_claude_owns_tty(monkeypatch):
    # O comando vai prefixado com `exec`: o tmux roda via `fish -c`, e sem exec o fish ficaria como
    # dono do tty e o send-keys nao chegaria no claude. Com exec, o fish vira o claude.
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert captured["args"][-1] == "exec claude --session-id x"
