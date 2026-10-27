"""A troca de modelo e o envio de texto disputam a mesma trava entre processos."""
import os
from pathlib import Path

import pytest

from app import terminal_input


@pytest.mark.skipif(os.name == "nt", reason="A prova Unix usa flock; o Windows confere LockFileEx na suíte Rust.")
def test_model_and_prompt_writers_share_the_operating_system_lock(tmp_path, monkeypatch):
    import fcntl

    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    # SHA-1 conhecido de «abc», compartilhado com o contrato do escritor Rust.
    path = tmp_path / ".hangar/terminal-locks/a9993e364706816aba3e25717850c26c9cd0d89d.lock"
    path.parent.mkdir(parents=True)
    with path.open("a+b") as other:
        lock = terminal_input._send_lock("abc")
        with lock:
            with pytest.raises(BlockingIOError):
                fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(other, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(other, fcntl.LOCK_UN)


def test_a_failed_nonblocking_attempt_does_not_hold_the_local_lock(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    lock = terminal_input._send_lock("entrada")
    assert lock.acquire(blocking=False)
    try:
        assert lock.locked()
        assert not lock.acquire(blocking=False)
    finally:
        lock.release()
    assert not lock.locked()
    assert lock.acquire(timeout=1)
    lock.release()
