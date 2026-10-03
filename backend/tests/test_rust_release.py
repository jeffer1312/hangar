# backend/tests/test_rust_release.py
"""Download do hangar-server e do hangar-cano da release `server-latest`, contra um HTTP falso."""
import hashlib
import http.client
import json
import os
import socket
import threading
import types
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

import pytest

from app import diag, rust_release

SERVER = b"#!/bin/sh\necho server\n"
CANO = b"#!/bin/sh\necho cano\n"
REAL_PLATFORM_KEY = rust_release.platform_key


@pytest.fixture(autouse=True)
def _ambiente(monkeypatch):
    for var in ("http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"):
        monkeypatch.delenv(var, raising=False)
    monkeypatch.setattr(rust_release, "platform_key", lambda: "linux-x86_64")
    monkeypatch.setattr(rust_release, "_E_WINDOWS", False)


@pytest.fixture
def events(monkeypatch):
    got = []
    monkeypatch.setattr(diag, "registrar",
                        lambda evento, nivel="ok", **campos: got.append((evento, nivel, campos)))
    return got


@pytest.fixture
def release(tmp_path):
    """Pasta servida como a release. Devolve (url, pasta, caminhos pedidos)."""
    pasta = tmp_path / "release"
    pasta.mkdir()
    pedidos = []

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(pasta), **kwargs)

        def do_GET(self):
            pedidos.append(self.path)
            super().do_GET()

        def log_message(self, *args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{httpd.server_address[1]}", pasta, pedidos
    httpd.shutdown()
    httpd.server_close()


def _publish(pasta, binaries: dict[str, bytes], plat="linux-x86_64", wrong_sha=()):
    files = {}
    for name, data in binaries.items():
        asset = f"{name}-{plat}{'.exe' if plat.startswith('windows') else ''}"
        (pasta / asset).write_bytes(data)
        sha = "0" * 64 if name in wrong_sha else hashlib.sha256(data).hexdigest()
        files[f"{plat}/{name}"] = {"name": asset, "sha256": sha}
    (pasta / "server-latest.json").write_text(json.dumps({"commit": "abc", "files": files}))


def test_downloads_both_checked_and_executable(release, tmp_path):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    dest = tmp_path / "bin"
    assert rust_release.fetch(url, dest) == []
    assert (dest / "hangar-server").read_bytes() == SERVER
    assert (dest / "hangar-cano").read_bytes() == CANO
    assert os.access(dest / "hangar-server", os.X_OK)
    assert not [p for p in dest.iterdir() if p.name.endswith(".tmp")]


def test_same_release_again_downloads_only_the_manifest(release, tmp_path):
    url, pasta, pedidos = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    dest = tmp_path / "bin"
    rust_release.fetch(url, dest)
    pedidos.clear()
    assert rust_release.fetch(url, dest) == []
    assert pedidos == ["/server-latest.json"]


def test_wrong_sha_keeps_the_binary_that_was_there(release, tmp_path, events):
    url, pasta, _ = release
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server").write_bytes(b"velho")
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, wrong_sha={"hangar-server"})
    avisos = rust_release.fetch(url, dest)
    assert len(avisos) == 1 and "hangar-server" in avisos[0] and "sha256" in avisos[0]
    assert (dest / "hangar-server").read_bytes() == b"velho"
    assert (dest / "hangar-cano").read_bytes() == CANO
    assert ("hangar_server.baixar", "erro", {"etapa": "sha256", "detalhe": "hangar-server"}) in events


def test_release_offline_is_a_warning_never_an_exception(tmp_path, events):
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]                   # porta fechada ao sair do with
    avisos = rust_release.fetch(f"http://127.0.0.1:{port}", tmp_path / "bin")
    assert len(avisos) == 1 and "manifesto" in avisos[0]
    assert any(e[0] == "hangar_server.baixar" and e[2].get("etapa") == "manifesto" for e in events)


def test_body_cut_mid_download_is_a_warning_and_keeps_the_old_binary(release, tmp_path, monkeypatch, events):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server").write_bytes(b"velho")
    real_get = rust_release._get

    def get(u, timeout):
        if u.endswith("hangar-server-linux-x86_64"):
            raise http.client.IncompleteRead(b"par", 10)
        return real_get(u, timeout)

    monkeypatch.setattr(rust_release, "_get", get)
    avisos = rust_release.fetch(url, dest)
    assert len(avisos) == 1 and "download" in avisos[0]
    assert (dest / "hangar-server").read_bytes() == b"velho"
    assert (dest / "hangar-cano").read_bytes() == CANO
    assert any(e[2].get("etapa") == "download" for e in events)


def test_unreadable_binary_there_is_downloaded_again(release, tmp_path, monkeypatch):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    dest = tmp_path / "bin"
    rust_release.fetch(url, dest)

    def nega(path):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(rust_release, "_sha256_file", nega)
    assert rust_release.fetch(url, dest) == []


def test_platform_missing_from_manifest_warns_per_binary(release, tmp_path):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="macos-aarch64")
    avisos = rust_release.fetch(url, tmp_path / "bin")
    assert len(avisos) == 2 and all("linux-x86_64" in a for a in avisos)


def test_machine_without_build_returns_none(monkeypatch, tmp_path, events):
    monkeypatch.setattr(rust_release, "platform_key", lambda: None)
    assert rust_release.fetch("http://127.0.0.1:1", tmp_path / "bin") is None
    assert ("hangar_server.baixar", "aviso", {"codigo": "sem_build"}) in events


@pytest.mark.parametrize("plat,machine,key", [
    ("linux", "x86_64", "linux-x86_64"),
    ("win32", "AMD64", "windows-x86_64"),
    ("darwin", "arm64", "macos-aarch64"),
    ("linux", "aarch64", None),
    ("darwin", "x86_64", None),
])
def test_platform_key(monkeypatch, plat, machine, key):
    # Troca os nomes do módulo, não o `sys`/`platform` globais (ver windows.md, os.name e pathlib).
    monkeypatch.setattr(rust_release, "sys", types.SimpleNamespace(platform=plat))
    monkeypatch.setattr(rust_release, "platform", types.SimpleNamespace(machine=lambda: machine))
    assert REAL_PLATFORM_KEY() == key


@pytest.fixture
def windows(monkeypatch, tmp_path):
    monkeypatch.setattr(rust_release, "platform_key", lambda: "windows-x86_64")
    monkeypatch.setattr(rust_release, "_E_WINDOWS", True)
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server.exe").write_bytes(b"rodando")
    return dest


def test_windows_moves_the_running_exe_aside_and_sweeps_it_next_time(release, windows):
    url, pasta, _ = release
    dest = windows
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")
    assert rust_release.fetch(url, dest) == []
    assert (dest / "hangar-server.exe").read_bytes() == SERVER
    assert (dest / "hangar-server.exe.old").read_bytes() == b"rodando"
    # Release nova: o resto da troca anterior sai na varredura, com outra caixa no nome.
    (dest / "hangar-server.exe.old").rename(dest / "HANGAR-SERVER.EXE.OLD")
    _publish(pasta, {"hangar-server": SERVER + b"#2", "hangar-cano": CANO}, plat="windows-x86_64")
    assert rust_release.fetch(url, dest) == []
    assert (dest / "hangar-server.exe").read_bytes() == SERVER + b"#2"
    assert sorted(p.name for p in dest.iterdir()) == [
        "hangar-cano.exe", "hangar-server.exe", "hangar-server.exe.old"]


def test_windows_stuck_old_yields_its_name(release, windows):
    url, pasta, _ = release
    dest = windows
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")
    # Diretório não sai com unlink: faz o papel do .old que ainda é a imagem de um processo vivo.
    stuck = dest / "hangar-server.exe.old"
    stuck.mkdir()
    assert rust_release.fetch(url, dest) == []
    assert stuck.is_dir()
    assert (dest / "hangar-server.exe.old-1").read_bytes() == b"rodando"
    assert (dest / "hangar-server.exe").read_bytes() == SERVER


def test_windows_failed_swap_puts_the_running_exe_back(release, windows, monkeypatch):
    url, pasta, _ = release
    dest = windows
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")

    def recusa(origem, destino):
        raise PermissionError(5, "Acesso negado")

    monkeypatch.setattr(rust_release.atomico, "substituir", recusa)
    avisos = rust_release.fetch(url, dest)
    assert len(avisos) == 2 and all("gravar" in a for a in avisos)
    assert (dest / "hangar-server.exe").read_bytes() == b"rodando"
    assert sorted(p.name for p in dest.iterdir()) == ["hangar-server.exe"]


def test_permission_error_outside_windows_is_a_warning(release, tmp_path, monkeypatch):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})

    def recusa(origem, destino):
        raise PermissionError(13, "Permission denied")

    monkeypatch.setattr(rust_release.atomico, "substituir", recusa)
    avisos = rust_release.fetch(url, tmp_path / "bin")
    assert len(avisos) == 2 and all("gravar" in a for a in avisos)


@pytest.mark.parametrize("result,argv,code", [
    ([], [], 0), (["x"], [], 1), (None, [], 2),
    (["x"], ["--never-fail"], 0), (None, ["--never-fail"], 0),
])
def test_cli_exit_codes(monkeypatch, result, argv, code):
    monkeypatch.setattr(rust_release, "fetch", lambda: result)
    assert rust_release.main(argv) == code


def test_trickling_asset_hits_the_deadline_and_keeps_the_old_binary(tmp_path, monkeypatch):
    """Servidor que pinga um byte por vez: o prazo total corta, senão o passo estoura os 600 s."""
    import time
    from http.server import BaseHTTPRequestHandler

    manifest = json.dumps({"commit": "abc", "files": {
        "linux-x86_64/hangar-server": {"name": "srv", "sha256": hashlib.sha256(SERVER).hexdigest()},
        "linux-x86_64/hangar-cano": {"name": "cano", "sha256": hashlib.sha256(CANO).hexdigest()},
    }}).encode()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            if self.path == "/server-latest.json":
                self.send_header("Content-Length", str(len(manifest)))
                self.end_headers()
                self.wfile.write(manifest)
                return
            self.send_header("Content-Length", "100000")
            self.end_headers()
            try:
                for _ in range(1000):
                    self.wfile.write(b"x")
                    self.wfile.flush()
                    time.sleep(0.05)
            except OSError:
                pass

        def log_message(self, *args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    monkeypatch.setattr(rust_release, "_ASSET_DEADLINE", 0.3)
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server").write_bytes(b"velho")
    try:
        started = time.monotonic()
        avisos = rust_release.fetch(f"http://127.0.0.1:{httpd.server_address[1]}", dest)
        assert time.monotonic() - started < 5
    finally:
        httpd.shutdown()
        httpd.server_close()
    assert len(avisos) == 2 and all("download" in a for a in avisos)
    assert (dest / "hangar-server").read_bytes() == b"velho"
    assert not (dest / "hangar-cano").exists()


def test_windows_without_a_free_old_name_is_a_warning_not_a_hang(release, windows, monkeypatch):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO}, plat="windows-x86_64")
    monkeypatch.setattr(rust_release, "_free", lambda path: False)   # lstat nega em todo candidato
    avisos = rust_release.fetch(url, windows)
    assert len(avisos) == 2 and all("gravar" in a for a in avisos)
    assert (windows / "hangar-server.exe").read_bytes() == b"rodando"


def test_main_survives_a_console_that_cannot_encode_the_message(monkeypatch):
    import io
    import sys
    saida = io.TextIOWrapper(io.BytesIO(), encoding="cp1252", errors="strict")
    monkeypatch.setattr(sys, "stdout", saida)
    monkeypatch.setattr(rust_release, "fetch", lambda: ["falhou em C:\\Users\\日本"])
    assert rust_release.main(["--never-fail"]) == 0
    saida.flush()
    assert b"?" in saida.buffer.getvalue()


def test_swap_logs_the_manifest_commit(release, tmp_path, events):
    url, pasta, _ = release
    _publish(pasta, {"hangar-server": SERVER, "hangar-cano": CANO})
    assert rust_release.fetch(url, tmp_path / "bin") == []
    assert ("hangar_server.baixar", "ok",
            {"codigo": "trocado", "detalhe": "hangar-server", "commit": "abc", "tag": None}) in events


def _branch(monkeypatch, stdout="", returncode=0, raises=None):
    def run(*args, **kwargs):
        if raises:
            raise raises
        return types.SimpleNamespace(stdout=stdout, returncode=returncode)
    monkeypatch.setattr(rust_release, "subprocess", types.SimpleNamespace(run=run))


@pytest.mark.parametrize(("stdout", "returncode", "raises", "tag", "codigo"), [
    ("main\n", 0, None, "server-latest", None),
    ("master\n", 0, None, "server-latest", None),
    ("feature/x\n", 0, None, "server-feature-x", None),
    ("hangar-server-parte1\n", 0, None, "server-hangar-server-parte1", None),
    ("HEAD\n", 0, None, "server-latest", "head_solto"),
    ("", 128, None, "server-latest", "branch_ilegivel"),
    ("", 0, None, "server-latest", "branch_ilegivel"),
    ("", 0, FileNotFoundError("git"), "server-latest", "branch_ilegivel"),
    ("latest\n", 0, None, "server-latest", "branch_colide_com_main"),
])
def test_release_tag_follows_the_checkout_branch(monkeypatch, events, stdout, returncode, raises,
                                                 tag, codigo):
    _branch(monkeypatch, stdout, returncode, raises)
    assert rust_release.release_tag() == tag
    assert [e[2].get("codigo") for e in events] == ([codigo] if codigo else [])


def test_branch_without_release_falls_back_to_main_and_logs_it(release, tmp_path, monkeypatch, events):
    url, pasta, pedidos = release
    (pasta / "server-latest").mkdir()
    _publish(pasta / "server-latest", {"hangar-server": SERVER, "hangar-cano": CANO})
    monkeypatch.setattr(rust_release, "RELEASES_URL", url)
    monkeypatch.delenv("HANGAR_SERVER_RELEASE_URL", raising=False)
    _branch(monkeypatch, "feature/x\n")
    dest = tmp_path / "bin"
    dest.mkdir()
    (dest / "hangar-server").write_bytes(b"da branch")
    assert rust_release.fetch(dest=dest) == [
        "release server-feature-x ausente; mantive os binários instalados (hangar-server)",
        "release server-feature-x ausente; instalei os da main (hangar-cano)",
    ]
    assert pedidos[:2] == ["/server-feature-x/server-latest.json", "/server-latest/server-latest.json"]
    assert (dest / "hangar-server").read_bytes() == b"da branch"
    assert (dest / "hangar-cano").read_bytes() == CANO
    assert ("hangar_server.baixar", "aviso", {"codigo": "sem_release_da_branch", "tag": "server-feature-x"}) in events
    assert [e[2]["detalhe"] for e in events if e[2].get("codigo") == "trocado"] == ["hangar-cano"]
    assert all(e[2]["tag"] == "server-latest" for e in events if e[2].get("codigo") == "trocado")


def test_branch_fallback_whose_main_manifest_fails_logs_the_tag(release, tmp_path, monkeypatch, events):
    url, _, _ = release
    monkeypatch.setattr(rust_release, "RELEASES_URL", url)
    monkeypatch.delenv("HANGAR_SERVER_RELEASE_URL", raising=False)
    _branch(monkeypatch, "feature/x\n")
    avisos = rust_release.fetch(dest=tmp_path / "bin")
    assert len(avisos) == 1 and "manifesto" in avisos[0]
    assert any(e[2].get("etapa") == "manifesto" and e[2].get("tag") == "server-latest" for e in events)


def test_main_release_404_is_a_plain_warning(release, tmp_path, monkeypatch, events):
    url, _, pedidos = release
    monkeypatch.setattr(rust_release, "RELEASES_URL", url)
    monkeypatch.delenv("HANGAR_SERVER_RELEASE_URL", raising=False)
    _branch(monkeypatch, "main\n")
    avisos = rust_release.fetch(dest=tmp_path / "bin")
    assert len(avisos) == 1 and "manifesto" in avisos[0]
    assert pedidos == ["/server-latest/server-latest.json"]
    assert not any(e[2].get("codigo") == "sem_release_da_branch" for e in events)


def test_fetch_uses_the_branch_release_and_env_wins(monkeypatch, tmp_path):
    urls = []

    def get(url, deadline):
        urls.append(url)
        raise OSError("offline")
    monkeypatch.setattr(rust_release, "_get", get)
    monkeypatch.delenv("HANGAR_SERVER_RELEASE_URL", raising=False)
    _branch(monkeypatch, "feature/x\n")
    rust_release.fetch(dest=tmp_path / "bin")
    assert urls[-1] == f"{rust_release.RELEASES_URL}/server-feature-x/server-latest.json"

    monkeypatch.setenv("HANGAR_SERVER_RELEASE_URL", "http://127.0.0.1:1/x/")
    rust_release.fetch(dest=tmp_path / "bin")
    assert urls[-1] == "http://127.0.0.1:1/x/server-latest.json"
