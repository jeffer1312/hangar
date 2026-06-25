from unittest.mock import MagicMock, patch
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
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "cc", "-l", "--", "echo hi"]


def test_send_keys_named_key():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "Enter")
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "cc", "Enter"]


def test_capture_pane_returns_stdout():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="screen", returncode=0)) as run:
        assert tmux.capture_pane("cc") == "screen"
    assert run.call_args[0][0][:2] == ["tmux", "capture-pane"]
