"""Primeira chegada pelo Git: módulos reais antigos sobrevivem à troca dos arquivos no disco."""
from __future__ import annotations

import base64
from collections import Counter
import hashlib
import importlib
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import urllib.request
import zlib

import pytest

ROOT = Path(__file__).resolve().parents[2]
SOURCE_COMMIT = "06f73ac2098a348f505003cd665a67cab4911bcd"

# Fontes integrais exportadas do commit indicado, compactadas para não depender de Git no ensaio.
LEGACY_SOURCES = {
    "rust_bins": {
        "sha256": "93fc81a7bca76f5936b05bc0eb9a80c73d435df9efe846aa435f3b8be2bfc07c",
        "bytes": 2268,
        "zlib_base64": "eNp9Vt2K3DYYvfdTfDgEbDLxbAq9mXYKpQQKLd0lNy2EYGvtb2bE2pIiybO7peRdQi9KL3rVR5gX6/lke3ZCQudmZFnf/zlHfka3qr1j062Vc2s/hljfahMq95jleX5tOiYO8fSnJRsIb04fvcbqDQ5SZ+lHZfbKU9Ec0uJlYH9k36xo2WiVsU1ZZdm173jYkKKj8vr08cg9wbcabjWbyFRwaG1/UMQPrj/90+qoVnRk0yI+GYnfKk1OeUUqkB2jV6H8hgIPxD2O2qxBduuGOkVe6d/lH3VE1avTX2JeONXayMkm+bM0KB3I2KMlpOAZO7ej7jupqz1we4coqKz1KnJYR+X3HNeee1aBUWHHcHS0/VEPMLclJQdKPyjY85A1H9bV1IQ1Mmsq6We283agut6NcfRc16QHZ30kZYyNKmprQpbNe73d77XZL482LKuIgJMjp+Kh17eLlxs8ZlkNQ9ou5hWS/hlL9kVdGzUgaJnVb17fXOOMGGB7p3vZrjwHlMNFWaHPqCm8/epdlmUd76jmB27HqDC2wm2SYUkvv6Nba/tNRvh5RkmGXKVDcliUqKpD2pVqWw6hcCt5+K2+/qmcne606QRtheS1oRD9CpM41kBIekoRJBT9Qb9Yw1MgZILUxYReUJFXeM5J78R52txuKTcxBywCU56Xk1FCl8ZktnIQUbS3RppTzBFXcrZCVO2KyQY+z2ZTaPm5pW/ndyVScKh1BPhn09n8k66VTz4u+3XefEY/qEGbA8DjvWAoodRZsATJgmQCenvmIPjQ45QwaqFOsIOwRcn0wBtjBQTVOYAAo7pX3gAWRX6m+oaeh+3zMIU7/U1TzomhwpX9iCnZRLTnIT8PaPXUm1UaxlPhc2UysWwq7CZRb0MX2YOeEIogMeF5DPDiuLfUtKMX7DVULERmakSbJjb3tbc2Qp7AQM+Oo+7UHAUceD8KCRftsY5Ngz7B4V66BJlROKJBeAA9IqRUhR6n9FKUplxUx04qg7/O6lAtlUiO7Twnz6q/KETqc94K2K1k7dDM1FR4aVVSuzn6+1FJTy+rHcZOTUHmrRlm1QFDBZfWlM9qkst6PpTPaX0/x8DIf5Vx3SNrhhJROP17EaayfddIn2gcJnU0MhYOjqGrgIs2d5MgqojkX9Fcd68HHYV1Ij3VYCFW1ugWab2gV9VVOnN/AOmpwCshmChT1fODDjEUc/SzIHzpZXWv4yEJVJFfZJuX5SVrxMFnOXw753dBsHQm9MyuuKquvj7z+RNCTlPfbJeOP+mfdBhIlU7D4MLvQtpkmnZVewALA3rz1tEOnXVoLBWTwsqg0u2RZjbdIGk53yJzgNX/Tfoij0/040sK826Gw/XF1ZbQvJGBf365cQBhWlHFjh1QLi+bvY7kxr5vaAeFmTAdLbgOVJio93Y1RxGq3TzGgzVyF/YaGWs5n74NQKwjJ/KOQbTr4makTquKXg+AAeMOAGxbKzRc0qou75NBPRRzj1d0x4/bHp8M4BwuIQe1VrEQ0a4HmbjcyTs19nEr2lNm/wHxdPFG"
    },
    "rust_release": {
        "sha256": "dedf1b7d823c857788b7a4ab1b519dedd583290f5026d94eac9e8ef7e3b94bcf",
        "bytes": 17936,
        "zlib_base64": "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"
    }
}


def _source(name):
    entry = LEGACY_SOURCES[name]
    raw = zlib.decompress(base64.b64decode(entry["zlib_base64"]))
    assert len(raw) == entry["bytes"] and hashlib.sha256(raw).hexdigest() == entry["sha256"]
    return raw


def _run(tmp_path, scenario):
    root = tmp_path / "checkout"
    package = root / "backend/app"
    incoming = root / "incoming"
    package.mkdir(parents=True)
    incoming.mkdir()
    (package / "__init__.py").write_text("")
    for name in LEGACY_SOURCES:
        (package / f"{name}.py").write_bytes(_source(name))
        shutil.copyfile(ROOT / "backend/app" / f"{name}.py", incoming / f"{name}.py")
    shutil.copyfile(ROOT / "backend/app/atualizacoes.py", incoming / "atualizacoes.py")
    shutil.copyfile(ROOT / "backend/app/atomico.py", package / "atomico.py")
    (package / "diag.py").write_text(
        'events = []\n'
        'def registrar(event, level="ok", **fields): events.append((event, level, fields))\n'
        'def install_manifest(): return None\n'
        'def erro_campos(error): return {"erro_tipo": type(error).__name__}\n'
    )
    steps = root / "docs/atualizacoes"
    steps.mkdir(parents=True)
    (steps / "auth-pair-step.md").write_text(
        "---\nid: auth-pair-step\ntitulo: O código do par chegou\n---\nConteúdo da versão.\n")
    home = tmp_path / "home"
    home.mkdir()
    environment = {key: os.environ[key] for key in ("SYSTEMROOT", "WINDIR", "COMSPEC") if key in os.environ}
    environment.update(HOME=str(home), USERPROFILE=str(home), TMPDIR=str(tmp_path), PATH=str(tmp_path / "empty-path"),
        XDG_CONFIG_HOME=str(home / ".config"), XDG_DATA_HOME=str(home / ".local/share"),
        XDG_CACHE_HOME=str(home / ".cache"), CLAUDE_CONFIG_DIR=str(home / ".claude"),
        CODEX_HOME=str(home / ".codex"), PYTHONDONTWRITEBYTECODE="1", PYTHONNOUSERSITE="1",
        PYTHONPATH=str(root / "backend"), PYTHONIOENCODING="utf-8")
    completed = subprocess.run([sys.executable, __file__, "--worker", str(root), scenario],
                               cwd=root, env=environment, capture_output=True, text=True, timeout=20)
    assert completed.returncode == 0, completed.stdout + completed.stderr
    return json.loads(completed.stdout.splitlines()[-1])


def test_real_legacy_module_reference_downloads_the_selected_pair_and_rolls_back(tmp_path):
    result = _run(tmp_path, "legacy")
    assert result["same_release_module"] and result["same_bins_module"]
    assert result["reloads"] == ["app.rust_bins", "app.rust_release"]
    assert result["downloaded"] == 3 and result["manifest_reads"] == 0
    assert result["pair_checked"] and result["rollback_checked"]


@pytest.mark.parametrize("owner", ["missing", "0", "true"])
def test_without_update_owner_preloaded_legacy_modules_remain_untouched(tmp_path, owner):
    result = _run(tmp_path, "no-owner:" + owner)
    assert result["same_release_module"] and result["same_bins_module"]
    assert result["reloads"] == [] and result["downloaded"] == 0
    assert result["legacy_untouched"]


@pytest.mark.parametrize("initial", ["current", "bins-current", "release-current"])
def test_only_incomplete_modules_reload_and_current_selected_methods_survive(tmp_path, initial):
    result = _run(tmp_path, initial)
    expected = {"current": [], "bins-current": ["app.rust_release"],
                "release-current": ["app.rust_bins"]}[initial]
    assert result["reloads"] == expected and result["selected_methods_kept"]
    assert result["same_release_module"] and result["same_bins_module"]
    assert result["downloaded"] == 3 and result["pair_checked"]


@pytest.mark.parametrize("failure", ["fetch-failure", "rollback-failure"])
def test_fetch_and_rollback_failures_are_visible_after_the_legacy_handoff(tmp_path, failure):
    result = _run(tmp_path, failure)
    assert result["failure_visible"] and result["same_release_module"] and result["same_bins_module"]
    assert result["downloaded"] == 3 and result["manifest_reads"] == 0


def _worker(root, scenario):
    package = root / "backend/app"
    incoming = root / "incoming"
    initial_current_bins = scenario in {"current", "bins-current"}
    initial_current_release = scenario in {"current", "release-current"}
    if initial_current_bins:
        shutil.copyfile(incoming / "rust_bins.py", package / "rust_bins.py")
    if initial_current_release:
        shutil.copyfile(incoming / "rust_release.py", package / "rust_release.py")

    from app import rust_bins, rust_release
    bins_reference, release_reference = rust_bins, rust_release
    old_fetch = release_reference.fetch
    if not initial_current_bins:
        assert not hasattr(bins_reference, "auth_pair")
        assert hashlib.sha256((package / "rust_bins.py").read_bytes()).hexdigest() == LEGACY_SOURCES["rust_bins"]["sha256"]
    if not initial_current_release:
        assert release_reference.NAMES == ("hangar-server", "hangar-cano")
        assert hashlib.sha256((package / "rust_release.py").read_bytes()).hexdigest() == LEGACY_SOURCES["rust_release"]["sha256"]
    platform_key = release_reference.platform_key()
    assert platform_key is not None
    reloads = []
    reload_module = importlib.reload
    def observed_reload(module):
        reloads.append(module.__name__)
        return reload_module(module)
    importlib.reload = observed_reload

    chosen_fetch = None
    if initial_current_release:
        def chosen_fetch(*args, **kwargs):
            return old_fetch(*args, **kwargs)
        release_reference.fetch = chosen_fetch
    chosen_pair = None
    if initial_current_bins:
        auth_pair = bins_reference.auth_pair
        def chosen_pair(*args, **kwargs):
            return auth_pair(*args, **kwargs)
        bins_reference.auth_pair = chosen_pair

    # A troca representa só a chegada dos arquivos pelo pull; os módulos antigos seguem vivos.
    for name in ("rust_bins", "rust_release", "atualizacoes"):
        shutil.copyfile(incoming / f"{name}.py", package / f"{name}.py")
    assert "app.atualizacoes" not in sys.modules
    if scenario.startswith("no-owner:"):
        owner = scenario.split(":", 1)[1]
        if owner != "missing":
            os.environ["HANGAR_UPDATE_OWNS_RUST"] = owner
    else:
        os.environ["HANGAR_UPDATE_OWNS_RUST"] = "1"

    calls = []
    manifest_reads = []
    base_url = "https://selected-build.invalid/exact-release"
    suffix = ".exe" if platform_key.startswith("windows") else ""
    library_suffix = ".dll" if platform_key.startswith("windows") else ".dylib" if platform_key.startswith("macos") else ".so"
    payloads = {"hangar-server": b"server-from-selected-manifest", "hangar-cano": b"cano-from-selected-manifest",
                "hangar-auth": b"auth-library-from-selected-manifest"}
    entries = {name: {"name": name + "-selected" + (library_suffix if name == "hangar-auth" else suffix),
                      "sha256": hashlib.sha256(data).hexdigest()} for name, data in payloads.items()}
    manifest = {"commit": "selected-commit-fixture", "protocol": 94,
                "files": {f"{platform_key}/{name}": entry for name, entry in entries.items()}}
    manifest_before = json.loads(json.dumps(manifest))
    selected = (base_url, "server-selected", manifest)
    permitted_urls = {base_url + "/" + entry["name"]: name for name, entry in entries.items()}
    def download(url, timeout=None):
        if url.endswith("/server-latest.json"):
            manifest_reads.append(url)
            raise AssertionError("o manifesto escolhido não pode ser lido outra vez")
        assert url in permitted_urls, "rede fora do manifesto selecionado"
        name = permitted_urls[url]
        calls.append(url)
        if scenario == "fetch-failure" and name == "hangar-auth":
            raise OSError("download da biblioteca recusado pelo ensaio")
        return io.BytesIO(payloads[name])
    urllib.request.urlopen = download
    def forbidden(*args, **kwargs):
        raise AssertionError("processo ou rede não pertencente ao ensaio")
    subprocess.Popen = subprocess.run = subprocess.check_call = subprocess.check_output = forbidden
    socket.create_connection = socket.getaddrinfo = forbidden
    socket.socket.connect = socket.socket.connect_ex = forbidden

    from app import atualizacoes
    assert atualizacoes.REPO == root
    assert atualizacoes.aplicar_pendentes() == ["auth-pair-step"]
    same_release = release_reference is sys.modules["app.rust_release"]
    same_bins = bins_reference is sys.modules["app.rust_bins"]
    assert same_release and same_bins
    result = {"source_commit": SOURCE_COMMIT, "same_release_module": same_release, "same_bins_module": same_bins,
              "reloads": reloads, "downloaded": 0, "manifest_reads": 0}
    if scenario.startswith("no-owner:"):
        assert release_reference.fetch is old_fetch
        assert release_reference.NAMES == ("hangar-server", "hangar-cano") and not hasattr(bins_reference, "auth_pair")
        assert not calls and not reloads
        result["legacy_untouched"] = True
        return result

    assert "hangar-auth" in release_reference.NAMES and hasattr(bins_reference, "auth_pair")
    if not initial_current_release:
        assert release_reference.fetch is not old_fetch
    if chosen_fetch is not None:
        assert release_reference.fetch is chosen_fetch
    if chosen_pair is not None:
        assert bins_reference.auth_pair is chosen_pair
    # A segunda passagem não pode recarregar nem apagar métodos escolhidos de módulos atuais.
    before_reload = list(reloads)
    assert atualizacoes.aplicar_pendentes() == [] and reloads == before_reload
    result["selected_methods_kept"] = True
    destination = release_reference.bin_dir()
    destination.mkdir(parents=True)
    server, cano = destination / ("hangar-server" + suffix), destination / ("hangar-cano" + suffix)
    server.write_bytes(b"legacy-server-before-pull")
    cano.write_bytes(b"legacy-cano-before-pull")
    hold = []
    warnings = release_reference.fetch(release=selected, hold=hold)
    assert selected[2] is manifest and manifest == manifest_before
    assert Counter(calls) == Counter(permitted_urls.keys()) and len(calls) == 3
    assert not manifest_reads
    result["downloaded"] = len(calls)
    if scenario == "fetch-failure":
        assert warnings and any("hangar-auth" in warning and "download" in warning for warning in warnings)
        assert [item[2] for item in hold] == ["hangar-cano"]
        assert server.read_bytes() == b"legacy-server-before-pull" and cano.read_bytes() == b"legacy-cano-before-pull"
        assert not server.with_suffix(".auth.json").exists()
        result["failure_visible"] = True
        return result
    assert warnings == []
    artifacts = {item[2]: item for item in hold if item[2] in payloads}
    assert set(artifacts) == set(payloads) and len(hold) == 4
    for name, item in artifacts.items():
        assert item[0] == payloads[name] and hashlib.sha256(item[0]).hexdigest() == entries[name]["sha256"]
        assert item[3:] == ("selected-commit-fixture", "server-selected")
    assert release_reference.install_held(hold) == []
    library, digest = bins_reference.auth_pair(server)
    assert server.read_bytes() == payloads["hangar-server"] and cano.read_bytes() == payloads["hangar-cano"]
    assert library.read_bytes() == payloads["hangar-auth"] and digest == entries["hangar-auth"]["sha256"]
    result["pair_checked"] = True
    if scenario == "rollback-failure":
        install = release_reference._install
        def fail_old_server(data, target):
            if target == server and data == b"legacy-server-before-pull":
                raise OSError("devolução do servidor recusada pelo ensaio")
            return install(data, target)
        release_reference._install = fail_old_server
        warnings = release_reference.restore_previous()
        assert warnings and any("desfazer falhou" in warning for warning in warnings)
        assert server.read_bytes() == payloads["hangar-server"] and bins_reference.auth_pair(server)[0] == library
        assert release_reference._previous(server).read_bytes() == b"legacy-server-before-pull"
        assert len(calls) == 3 and not manifest_reads
        result["failure_visible"] = True
        return result
    assert release_reference.restore_previous() == []
    assert server.read_bytes() == b"legacy-server-before-pull" and cano.read_bytes() == b"legacy-cano-before-pull"
    assert not server.with_suffix(".auth.json").exists() and library.read_bytes() == payloads["hangar-auth"]
    assert len(calls) == 3 and not manifest_reads
    result["rollback_checked"] = True
    return result


if __name__ == "__main__" and sys.argv[1:2] == ["--worker"]:
    print(json.dumps(_worker(Path(sys.argv[2]), sys.argv[3])))

