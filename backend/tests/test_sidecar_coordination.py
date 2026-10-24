"""Sidecars do Claude e do Codex gravados por dois processos (Python e `hangar-server`).

O Rust grava o sidecar do Claude (`patch_meta`, cano, `/clear`) e o do Codex sob a trava
`<nome>.lock` e troca o arquivo por tmp+replace. O Python tem de segurar a mesma trava no
ler-alterar-gravar e, no Windows, esperar a troca terminar em vez de ler "sem sidecar".
"""
import contextlib
import os
import pathlib
import threading
import time

import pytest

from app import atomico
from app.adapters.claude_headless import sessions as claude_sessions
from app.adapters.codex import sessions as codex_sessions


@pytest.fixture
def claude_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(claude_sessions, "_dir", lambda: tmp_path / "claude-headless")
    return tmp_path / "claude-headless"


@pytest.fixture
def codex_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(codex_sessions, "_dir", lambda: tmp_path / "codex-sessions")
    return tmp_path / "codex-sessions"


def _replace_in_progress(monkeypatch, failures: int):
    """A abertura para leitura levanta o `PermissionError` do Windows nas primeiras vezes, como um
    leitor que cai no meio do `replace` de outro processo."""
    real = atomico._open_shared
    calls = {"n": 0}

    def fake(path):
        if pathlib.Path(path).suffix == ".json":
            calls["n"] += 1
            if calls["n"] <= failures:
                raise PermissionError(32, "O arquivo já está sendo usado por outro processo")
        return real(path)

    monkeypatch.setattr(atomico, "_open_shared", fake)
    monkeypatch.setattr(atomico.time, "sleep", lambda _s: None)
    return calls


def test_read_waits_out_a_windows_replace(monkeypatch, tmp_path):
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    target = tmp_path / "a.json"
    target.write_bytes(b"{}")
    calls = _replace_in_progress(monkeypatch, 3)
    assert atomico.read(target) == b"{}"
    assert calls["n"] == 4


def test_read_gives_up_and_raises(monkeypatch, tmp_path):
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    target = tmp_path / "a.json"
    target.write_bytes(b"{}")
    _replace_in_progress(monkeypatch, 10**12)
    with pytest.raises(PermissionError):
        atomico.read(target)


def test_read_never_retries_on_posix(monkeypatch, tmp_path):
    monkeypatch.setattr(atomico, "_E_WINDOWS", False)
    real = pathlib.Path.read_bytes
    calls = {"n": 0}

    def fake(path):
        calls["n"] += 1
        if calls["n"] == 1:
            raise PermissionError(13, "sem permissão")
        return real(path)

    monkeypatch.setattr(pathlib.Path, "read_bytes", fake)
    target = tmp_path / "a.json"
    target.write_bytes(b"{}")
    with pytest.raises(PermissionError):
        atomico.read(target)
    assert calls["n"] == 1


def test_read_does_not_wait_for_a_missing_file(monkeypatch, tmp_path):
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    with pytest.raises(FileNotFoundError):
        atomico.read(tmp_path / "ausente.json")


@pytest.mark.parametrize("module", [claude_sessions, codex_sessions])
def test_load_during_a_windows_replace_is_not_a_missing_sidecar(module, claude_dir, codex_dir, monkeypatch):
    directory = module._dir()
    directory.mkdir(parents=True)
    (directory / "s.json").write_text('{"name": "s", "key": "k"}', encoding="utf-8")
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    _replace_in_progress(monkeypatch, 2)
    assert module.load("s") == {"name": "s", "key": "k"}


def _hold(lock_path):
    """A trava do outro escritor (o `hangar-server` usa a mesma), segurada até fechar."""
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    stack = contextlib.ExitStack()
    stack.enter_context(atomico.locked(lock_path))
    return stack


def test_claude_update_waits_for_the_other_writer(claude_dir):
    claude_sessions.save("s", "/tmp", "sid")
    handle = _hold(claude_dir / "s.lock")
    done = threading.Event()
    worker = threading.Thread(target=lambda: (claude_sessions.update("s", model="m"), done.set()))
    worker.start()
    try:
        assert not done.wait(0.3), "o update gravou sem a trava do outro escritor"
    finally:
        handle.close()
    assert done.wait(5)
    worker.join()
    assert claude_sessions.load("s")["model"] == "m"


def test_claude_update_keeps_what_the_other_writer_saved_under_the_lock(claude_dir):
    """O update relê o arquivo depois de pegar a trava: o campo gravado pelo outro lado fica."""
    claude_sessions.save("s", "/tmp", "sid")
    handle = _hold(claude_dir / "s.lock")
    worker = threading.Thread(target=lambda: claude_sessions.update("s", model="m"))
    worker.start()
    try:
        # O outro processo grava enquanto segura a trava.
        path = claude_dir / "s.json"
        import json
        meta = json.loads(path.read_text(encoding="utf-8"))
        path.write_text(json.dumps({**meta, "effort": "high"}), encoding="utf-8")
    finally:
        handle.close()
    worker.join(5)
    meta = claude_sessions.load("s")
    assert (meta["model"], meta["effort"]) == ("m", "high")


@pytest.mark.skipif(os.name != "nt", reason="compartilhamento de arquivo do Windows")
def test_the_reader_shares_deletion_like_the_rust_reader(tmp_path):
    """O leitor do Python aberto compartilha a exclusão, como o `std::fs::read` do Rust: é o que deixa
    a troca do `hangar-server` (rename com semântica POSIX) passar por cima dele (a outra metade da #145)."""
    target = tmp_path / "s.json"
    target.write_bytes(b"velho")
    with atomico._open_shared(target) as reader:
        os.remove(target)
        assert reader.read() == b"velho"
    assert not target.exists()


def test_the_lock_is_taken_soon_after_the_other_writer_releases(tmp_path):
    """Quem espera a trava a pega logo que o outro solta (o `LK_LOCK` do Windows tentava a cada 1 s)."""
    lock = tmp_path / "s.lock"
    held, release = threading.Event(), threading.Event()

    def holder():
        with atomico.locked(lock):
            held.set()
            release.wait(5)

    worker = threading.Thread(target=holder)
    worker.start()
    assert held.wait(5)
    threading.Timer(0.2, release.set).start()
    started = time.monotonic()
    with atomico.locked(lock):
        waited = time.monotonic() - started
    worker.join()
    assert 0.15 <= waited < 0.8, waited
