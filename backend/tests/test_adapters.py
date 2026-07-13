import pytest

from app.adapters import get_adapter


def test_claude_adapter_registered():
    a = get_adapter("claude")
    assert a.provider == "claude"


def test_unknown_provider_raises():
    with pytest.raises(KeyError):
        get_adapter("nope")


def test_claude_adapter_spawn_command():
    # spawn_command devolve os argv do processo claude, sem reescrever a logica de sessao (a
    # tmux.new_session so aceita string -> o registry junta com " ".join()).
    a = get_adapter("claude")
    cmd = a.spawn_command("/tmp/proj", "abc-123")
    assert cmd == ["claude", "--session-id", "abc-123"]


def test_claude_adapter_transcript_path():
    from pathlib import Path
    from app.config import settings
    from app.registry import sanitize_cwd

    a = get_adapter("claude")
    p = a.transcript_path("/tmp/proj", "abc-123")
    assert p == str(Path(settings.projects_dir) / sanitize_cwd("/tmp/proj") / "abc-123.jsonl")
