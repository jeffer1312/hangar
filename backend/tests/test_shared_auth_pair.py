"""Fixação por artefato; os bytes fictícios nunca são carregados como uma DLL."""
import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

from app import rust_bins, rust_release


def _publisher():
    path = Path(__file__).resolve().parents[2] / "scripts/pin-auth-pair.py"
    spec = importlib.util.spec_from_file_location("pin_auth_pair_test", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.publish


def test_pair_uses_only_chosen_worktree_and_preserves_open_immutable_library(tmp_path):
    publish = _publisher()
    first, second = tmp_path / "first", tmp_path / "second"
    for directory in (first, second):
        directory.mkdir()
        (directory / "hangar-server.exe").write_bytes(directory.name.encode())
        (directory / "hangar_auth.dll").write_bytes(("library-" + directory.name).encode())
        publish(directory / "hangar-server.exe", directory / "hangar_auth.dll")
    library, digest = rust_bins.auth_pair(first / "hangar-server.exe")
    assert library.parent.name == digest and library.is_relative_to(first)
    with library.open("rb") as old:
        (first / "hangar_auth.dll").write_bytes(b"new-library")
        (first / "hangar-server.exe").write_bytes(b"new-server")
        publish(first / "hangar-server.exe", first / "hangar_auth.dll")
        new_library, new_digest = rust_bins.auth_pair(first / "hangar-server.exe")
        assert new_digest != digest and new_library != library
        assert old.read() == b"library-first" and library.read_bytes() == b"library-first"
    (first / "hangar-server.auth.json").unlink()
    with pytest.raises(FileNotFoundError):
        rust_bins.auth_pair(first / "hangar-server.exe")
    assert rust_bins.auth_pair(second / "hangar-server.exe")[0].is_relative_to(second)


@pytest.mark.parametrize("change", ["abi", "store_version", "server_sha256", "library_sha256", "library"])
def test_pair_rejects_incompatible_metadata_without_a_different_library(tmp_path, change):
    server, library = tmp_path / "hangar-server", tmp_path / "libhangar_auth.so"
    server.write_bytes(b"server")
    library.write_bytes(b"library")
    _publisher()(server, library)
    path = server.with_suffix(".auth.json")
    value = json.loads(path.read_text())
    value[change] = 99 if change in {"abi", "store_version"} else "../other" if change == "library" else "00" * 32
    path.write_text(json.dumps(value))
    with pytest.raises((OSError, ValueError)):
        rust_bins.auth_pair(server)


def test_rollback_restores_metadata_with_the_server_and_keeps_both_library_hashes(tmp_path, monkeypatch):
    monkeypatch.setattr(rust_release, "platform_key", lambda: "linux-x86_64")
    server, source = tmp_path / "hangar-server", tmp_path / "libhangar_auth.so"
    server.write_bytes(b"old-server")
    source.write_bytes(b"old-library")
    _publisher()(server, source)
    old_library, _ = rust_bins.auth_pair(server)
    new_bytes = b"new-library"
    sha = hashlib.sha256(new_bytes).hexdigest()
    new_library = tmp_path / "auth" / sha / "libhangar_auth.so"
    meta = {"abi": 1, "store_version": 1, "server_sha256": hashlib.sha256(b"new-server").hexdigest(),
            "library_sha256": sha, "library": new_library.relative_to(tmp_path).as_posix()}
    hold = [(new_bytes, new_library, "hangar-auth", "fixture", None),
            (b"new-server", server, "hangar-server", "fixture", None),
            (json.dumps(meta).encode(), server.with_suffix(".auth.json"), "hangar-auth-pair", "fixture", None)]
    assert rust_release.install_held(hold) == []
    assert rust_bins.auth_pair(server)[0] == new_library
    assert rust_release.restore_previous(tmp_path) == []
    assert rust_bins.auth_pair(server)[0] == old_library
    assert new_library.read_bytes() == new_bytes and old_library.read_bytes() == b"old-library"


@pytest.fixture
def replacement(tmp_path, monkeypatch):
    monkeypatch.setattr(rust_release, "platform_key", lambda: "linux-x86_64")
    monkeypatch.setattr(rust_release, "_E_WINDOWS", False)
    server, source = tmp_path / "hangar-server", tmp_path / "libhangar_auth.so"
    server.write_bytes(b"old-server")
    source.write_bytes(b"old-library")
    _publisher()(server, source)
    metadata = server.with_suffix(".auth.json")
    old_metadata = metadata.read_bytes()
    old_library, _ = rust_bins.auth_pair(server)
    sha = hashlib.sha256(b"new-library").hexdigest()
    new_library = tmp_path / "auth" / sha / "libhangar_auth.so"
    new_metadata = json.dumps({"abi": 1, "store_version": 1,
        "server_sha256": hashlib.sha256(b"new-server").hexdigest(),
        "library_sha256": sha, "library": new_library.relative_to(tmp_path).as_posix()}).encode()
    hold = [(b"new-library", new_library, "hangar-auth", "fixture", None),
            (b"new-server", server, "hangar-server", "fixture", None),
            (new_metadata, metadata, "hangar-auth-pair", "fixture", None)]
    return server, metadata, old_library, new_library, old_metadata, new_metadata, hold


def test_failed_cano_install_does_not_interrupt_pair_publication(replacement, monkeypatch):
    server, metadata, _, library, old_metadata, _, hold = replacement
    cano = server.with_name("hangar-cano")
    cano.write_bytes(b"old-cano")
    install = rust_release._install
    def fault(data, target):
        if target == cano:
            raise OSError("cano refused")
        if target == server:
            assert rust_release._previous(server).read_bytes() == b"old-server"
            assert rust_release._previous(metadata).read_bytes() == old_metadata
        return install(data, target)
    monkeypatch.setattr(rust_release, "_install", fault)
    warnings = rust_release.install_held([*hold, (b"new-cano", cano, "hangar-cano", "fixture", None)])
    assert len(warnings) == 1 and "hangar-cano" in warnings[0]
    assert server.read_bytes() == b"new-server" and cano.read_bytes() == b"old-cano"
    assert rust_bins.auth_pair(server)[0] == library


@pytest.mark.parametrize("held", [False, True])
def test_failed_cano_download_keeps_the_pair_independent(replacement, monkeypatch, held):
    server, _, _, library, _, _, _ = replacement
    cano = server.with_name("hangar-cano")
    cano.write_bytes(b"old-cano")
    contents = {"server": b"new-server", "auth": b"new-library", "cano": b"new-cano"}
    files = {f"linux-x86_64/{name}": {"name": asset, "sha256": hashlib.sha256(contents[asset]).hexdigest()}
             for name, asset in (("hangar-server", "server"), ("hangar-auth", "auth"), ("hangar-cano", "cano"))}
    def get(url, deadline):
        asset = url.rsplit("/", 1)[1]
        if asset == "cano":
            raise OSError("cano download refused")
        return contents[asset]
    monkeypatch.setattr(rust_release, "_get", get)
    pending = [] if held else None
    warnings = rust_release.fetch(dest=server.parent, release=("https://fixture.invalid", None,
                                {"commit": "fixture", "files": files}), hold=pending)
    assert len(warnings) == 1 and "hangar-cano" in warnings[0]
    if held:
        assert server.read_bytes() == b"old-server"
        assert rust_release.install_held(pending) == []
    assert server.read_bytes() == b"new-server" and rust_bins.auth_pair(server)[0] == library
    assert cano.read_bytes() == b"old-cano"


def test_metadata_backup_failure_aborts_before_changing_the_pair(replacement, monkeypatch):
    server, metadata, library, _, old_metadata, _, hold = replacement
    keep = rust_release._keep_previous
    def fault(target, name):
        return "metadata backup refused" if target == metadata else keep(target, name)
    monkeypatch.setattr(rust_release, "_keep_previous", fault)
    assert rust_release.install_held(hold) == ["metadata backup refused"]
    assert server.read_bytes() == b"old-server" and metadata.read_bytes() == old_metadata
    assert rust_bins.auth_pair(server)[0] == library
    assert not rust_release._previous(server).exists() and not rust_release._previous(metadata).exists()


@pytest.mark.parametrize("preserve", [False, True])
def test_failed_metadata_write_rolls_back_the_whole_pair(replacement, monkeypatch, preserve):
    server, metadata, old_library, new_library, old_metadata, new_metadata, hold = replacement
    install = rust_release._install
    def fault(data, target):
        if target == metadata and data == new_metadata:
            raise OSError("new metadata refused")
        return install(data, target)
    monkeypatch.setattr(rust_release, "_install", fault)
    warnings = rust_release.install_held(hold, preserve_previous=preserve)
    assert len(warnings) == 1 and "hangar-auth-pair" in warnings[0]
    assert server.read_bytes() == b"old-server" and metadata.read_bytes() == old_metadata
    assert rust_bins.auth_pair(server)[0] == old_library
    assert new_library.read_bytes() == b"new-library"
    assert not rust_release._previous(server).exists() and not rust_release._previous(metadata).exists()


def test_partial_restore_failure_returns_to_the_current_coherent_pair(replacement, monkeypatch):
    server, metadata, _, new_library, old_metadata, new_metadata, hold = replacement
    assert rust_release.install_held(hold) == []
    install = rust_release._install
    def fault(data, target):
        if target == metadata and data == old_metadata:
            raise OSError("old metadata refused")
        return install(data, target)
    monkeypatch.setattr(rust_release, "_install", fault)
    warnings = rust_release.restore_previous(server.parent)
    assert len(warnings) == 1 and "desfazer falhou" in warnings[0]
    assert server.read_bytes() == b"new-server" and metadata.read_bytes() == new_metadata
    assert rust_bins.auth_pair(server)[0] == new_library
    assert rust_release._previous(server).read_bytes() == b"old-server"
    assert rust_release._previous(metadata).read_bytes() == old_metadata


def test_failed_undo_is_explicit_and_keeps_the_recovery_copies(replacement, monkeypatch):
    server, metadata, _, new_library, old_metadata, new_metadata, hold = replacement
    install = rust_release._install
    def fault(data, target):
        if (target == metadata and data == new_metadata) or (target == server and data == b"old-server"):
            raise OSError("injected persistent refusal")
        return install(data, target)
    monkeypatch.setattr(rust_release, "_install", fault)
    warnings = rust_release.install_held(hold)
    assert any("hangar-auth-pair" in warning for warning in warnings)
    assert any("desfazer falhou" in warning for warning in warnings)
    assert server.read_bytes() == b"new-server" and metadata.read_bytes() == old_metadata
    with pytest.raises(ValueError):
        rust_bins.auth_pair(server)
    assert rust_release._previous(server).read_bytes() == b"old-server"
    assert rust_release._previous(metadata).read_bytes() == old_metadata
    assert new_library.read_bytes() == b"new-library"
