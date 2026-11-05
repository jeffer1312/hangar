"""Fixação por artefato; os bytes fictícios nunca são carregados como uma DLL."""
import importlib.util
import json
from pathlib import Path

import pytest

from app import rust_bins


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
