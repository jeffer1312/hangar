"""Ciclo real de plugins, permitido somente no runner isolado do Hangar."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
from threading import Thread
from urllib.parse import urlsplit

import pytest


if os.environ.get("HANGAR_TEST_SANDBOX") != "1":
    pytest.skip("Prova real exige o runner HANGAR_TEST_SANDBOX", allow_module_level=True)

from app.omp_plugin_sync import PluginSynchronizer
from tests.test_omp_plugin_sync import write_claude_inventory, write_json


PACKAGE_NAME = "hangar-comet-runtime-fixture"
PLUGIN_ID = "comet@catalog"
VERSION = "1.0.0"
FEATURES = ["optional"]
PLUGIN_SETTINGS = {"message": "Configuração preservada"}


def _require_sandbox() -> None:
    if os.environ.get("HANGAR_TEST_SANDBOX") != "1":
        raise RuntimeError("Execução real fora do runner isolado recusada")


def _run(args: list[str], cwd: Path, *, timeout: float = 60) -> str:
    _require_sandbox()
    completed = subprocess.run(
        args,
        cwd=cwd,
        env=dict(os.environ),
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        check=False,
    )
    assert completed.returncode == 0, {
        "args": args,
        "returncode": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
    }
    return completed.stdout


def _git(cwd: Path, *args: str) -> str:
    return _run(["git", *args], cwd).strip()


def _read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


@pytest.fixture
def runtime_home(tmp_path, monkeypatch):
    _require_sandbox()
    binary = os.environ.get("OMP_TEST_BIN") or shutil.which("omp")
    assert binary, "OMP ausente: forneça OMP_TEST_BIN ao runner"
    binary = Path(binary).resolve(strict=True)
    assert binary.is_file() and os.access(binary, os.X_OK)
    assert Path("/usr/bin/bun").is_file(), "Bun deve estar disponível, sem instalação pelo teste"
    assert shutil.which("git"), "Git deve estar disponível no runner"

    home = tmp_path / "home"
    home.mkdir(mode=0o700)
    private_bin = tmp_path / "bin"
    private_bin.mkdir(mode=0o700)
    (private_bin / "omp").symlink_to(binary)
    agent_dir = home / ".omp" / "agent"
    agent_dir.mkdir(parents=True)
    (agent_dir / "config.yml").write_text("setupVersion: 2\n", encoding="utf-8")
    temporary = home / "tmp"
    temporary.mkdir(mode=0o700)
    environment = {
        "PATH": f"{private_bin}:/usr/bin:/bin",
        "HOME": str(home),
        "USERPROFILE": str(home),
        "USER": "test",
        "LOGNAME": "test",
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TMPDIR": str(temporary),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local/share"),
        "XDG_STATE_HOME": str(home / ".local/state"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "CLAUDE_CONFIG_DIR": str(home / ".claude"),
        "PI_CODING_AGENT_DIR": str(agent_dir),
        "BUN_INSTALL": str(home / ".bun"),
        "BUN_INSTALL_CACHE_DIR": str(home / ".bun/install/cache"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_AUTHOR_NAME": "Fixture",
        "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture",
        "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
        "HANGAR_TEST_SANDBOX": "1",
    }
    # Credenciais, proxies e configuração Git do chamador não atravessam a prova.
    for key in tuple(os.environ):
        monkeypatch.delenv(key, raising=False)
    for key, value in environment.items():
        monkeypatch.setenv(key, value)
    monkeypatch.chdir(home)
    return home


class GitHTTPHandler(BaseHTTPRequestHandler):
    """Git Smart HTTP permite os clones rasos usados pelo importador nativo."""

    def _serve(self):
        request = urlsplit(self.path)
        environment = dict(os.environ, GIT_PROJECT_ROOT=str(self.server.git_root),
                           GIT_HTTP_EXPORT_ALL="1", PATH_INFO=request.path,
                           QUERY_STRING=request.query, REQUEST_METHOD=self.command,
                           CONTENT_TYPE=self.headers.get("Content-Type", ""),
                           CONTENT_LENGTH=self.headers.get("Content-Length", "0"),
                           HTTP_GIT_PROTOCOL=self.headers.get("Git-Protocol", ""))
        body = self.rfile.read(int(environment["CONTENT_LENGTH"]))
        response = subprocess.run(["git", "http-backend"], input=body, capture_output=True,
                                  env=environment, timeout=30)
        if response.returncode:
            self.send_error(500, "Git HTTP falhou")
            return
        headers, separator, content = response.stdout.partition(b"\r\n\r\n")
        if not separator:
            headers, separator, content = response.stdout.partition(b"\n\n")
        values = [line.split(":", 1) for line in headers.decode("latin-1").splitlines() if ":" in line]
        status = next((int(value.strip().split()[0]) for key, value in values if key.lower() == "status"), 200)
        self.send_response(status)
        for key, value in values:
            if key.lower() != "status":
                self.send_header(key, value.strip())
        self.end_headers()
        self.wfile.write(content)

    do_GET = _serve
    do_POST = _serve


@pytest.fixture
def git_http(runtime_home, tmp_path):
    _require_sandbox()
    served = tmp_path / "git-http"
    bare = served / "org" / "repo.git"
    bare.mkdir(parents=True, mode=0o700)
    _git(bare, "init", "--bare", "--initial-branch=main")
    server = ThreadingHTTPServer(("127.0.0.1", 0), GitHTTPHandler)
    server.git_root = served
    thread = Thread(target=server.serve_forever, name="plugin-fixture-http", daemon=True)
    thread.start()
    try:
        yield bare, f"http://127.0.0.1:{server.server_port}/org/repo.git"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive(), "Servidor HTTP próprio não encerrou"


def _publish(source: Path, bare: Path, content: bytes) -> str:
    (source / "commands" / "base.md").write_bytes(content)
    _git(source, "add", "package.json", "commands")
    _git(source, "-c", "core.hooksPath=/dev/null", "commit", "-m", "Recurso sintético")
    revision = _git(source, "rev-parse", "HEAD")
    _git(source, "push", str(bare), "HEAD:refs/heads/main")
    _git(bare, "update-server-info")
    return revision


def _reconcile(home: Path, claude_dir: Path, request, phase: str) -> dict:
    report = PluginSynchronizer(home=home, claude_dir=claude_dir).reconcile()
    request.node.add_report_section("call", phase, json.dumps(report, ensure_ascii=False, indent=2))
    assert report["complete_inventory"] is True, report
    assert report["errors"] == [], report
    return report


def _native_list(home: Path) -> dict:
    return json.loads(_run(["omp", "plugin", "list", "--json"], home))


def _installed(home: Path, repository: str, revision: str, content: bytes,
               *, enabled: bool, features: list[str] | None, settings: dict) -> Path:
    native = _native_list(home)
    assert native["marketplace"] == [], native
    assert [entry["name"] for entry in native["npm"]] == [PACKAGE_NAME], native
    plugin = native["npm"][0]
    root = home / ".omp" / "plugins"
    package_root = root / "node_modules" / PACKAGE_NAME
    assert Path(plugin["path"]).resolve() == package_root.resolve()
    assert package_root.resolve().is_relative_to(home.resolve())
    assert plugin["version"] == VERSION
    assert plugin["enabled"] is enabled
    assert plugin["enabledFeatures"] == features
    package = _read_json(package_root / "package.json")
    assert package["name"] == PACKAGE_NAME
    assert package["version"] == VERSION
    assert package["omp"]["commands"] == ["commands/base.md"]
    assert (package_root / "commands" / "base.md").read_bytes() == content
    assert (package_root / "commands" / "optional.md").read_bytes() == b"Optional fixture command.\n"

    manifest = _read_json(root / "package.json")
    dependency = manifest["dependencies"][PACKAGE_NAME]
    assert dependency.removeprefix("git+").split("#", 1) == [repository, revision]
    assert not manifest.get("trustedDependencies"), manifest
    # O pin vem do resolvedor nativo, não da versão textual do package.json.
    assert revision in (root / "bun.lock").read_text(encoding="utf-8")
    lock = _read_json(root / "omp-plugins.lock.json")
    assert lock["plugins"][PACKAGE_NAME] == {
        "version": VERSION, "enabledFeatures": features, "enabled": enabled,
    }
    assert lock["settings"].get(PACKAGE_NAME, {}) == settings
    return package_root


def _installation_snapshot(root: Path) -> dict:
    paths = [root, *root.rglob("*")]
    return {
        str(path.relative_to(root)): (
            path.lstat().st_ino,
            path.lstat().st_mtime_ns,
            path.lstat().st_ctime_ns,
            path.read_bytes() if path.is_file() else None,
        )
        for path in paths
    }


@pytest.mark.parametrize("preexisting_preferences", [False, True], ids=["fresh", "adopt-selection"])
def test_plugin_real_instala_atualiza_mesma_versao_preserva_preferencias_e_remove(
    runtime_home, git_http, request, preexisting_preferences,
):
    home = runtime_home
    bare, repository = git_http
    source = home / ".claude/plugins/cache/catalog/comet" / VERSION
    source.mkdir(parents=True)
    (source / "commands").mkdir()
    write_json(source / "package.json", {
        "name": PACKAGE_NAME,
        "version": VERSION,
        "omp": {
            "commands": ["commands/base.md"],
            "features": {
                "optional": {"default": False, "commands": ["commands/optional.md"]},
            },
            "settings": {"message": {"type": "string", "default": ""}},
        },
    })
    (source / "commands/optional.md").write_bytes(b"Optional fixture command.\n")
    _git(source, "init", "--initial-branch=main")
    _git(source, "remote", "add", "origin", repository)
    first_content = b"First fixture command.\n"
    first_revision = _publish(source, bare, first_content)
    claude_dir = write_claude_inventory(
        home, source, repository=repository, revision=first_revision,
        package_name=PACKAGE_NAME, plugin_id=PLUGIN_ID,
    )
    native_root = home / ".omp/plugins"
    lock_path = native_root / "omp-plugins.lock.json"
    features = FEATURES if preexisting_preferences else None
    plugin_settings = PLUGIN_SETTINGS if preexisting_preferences else {}
    adoption_snapshot = None
    if preexisting_preferences:
        _run(["omp", "plugin", "install", f"git+{repository}#{first_revision}[optional]", "--json"], home)
        # Preferências anteriores à adoção; alterar depois suspenderia a gestão.
        lock = _read_json(lock_path)
        lock["settings"][PACKAGE_NAME] = plugin_settings
        write_json(lock_path, lock)
        adopted = _installed(home, repository, first_revision, first_content,
                             enabled=True, features=features, settings=plugin_settings)
        adoption_snapshot = _installation_snapshot(adopted)

    _reconcile(home, claude_dir, request, "adopt" if preexisting_preferences else "install")
    installed = _installed(
        home, repository, first_revision, first_content,
        enabled=True, features=features, settings=plugin_settings,
    )

    if adoption_snapshot is not None:
        assert _installation_snapshot(installed) == adoption_snapshot
    before = _installation_snapshot(installed)
    native_files = [native_root / "package.json", native_root / "bun.lock", lock_path]
    native_before = {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in native_files}
    _reconcile(home, claude_dir, request, "idempotent")
    _installed(home, repository, first_revision, first_content,
               enabled=True, features=features, settings=plugin_settings)
    assert _installation_snapshot(installed) == before
    assert {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in native_files} == native_before

    next_content = b"Second fixture command, same package version.\n"
    next_revision = _publish(source, bare, next_content)
    assert next_revision != first_revision
    assert _git(source, "config", "--get", "remote.origin.url") == repository
    assert _git(bare, "rev-parse", "refs/heads/main") == next_revision
    write_claude_inventory(
        home, source, repository=repository, revision=next_revision,
        package_name=PACKAGE_NAME, plugin_id=PLUGIN_ID,
    )
    _reconcile(home, claude_dir, request, "update")
    installed = _installed(home, repository, next_revision, next_content,
                           enabled=True, features=features, settings=plugin_settings)
    assert first_revision not in (native_root / "bun.lock").read_text(encoding="utf-8")

    write_claude_inventory(
        home, source, repository=repository, revision=next_revision,
        package_name=PACKAGE_NAME, plugin_id=PLUGIN_ID, enabled=False,
    )
    before_disable = _installation_snapshot(installed)
    _reconcile(home, claude_dir, request, "disable")
    _installed(home, repository, next_revision, next_content,
               enabled=False, features=features, settings=plugin_settings)
    assert _installation_snapshot(installed) == before_disable

    inventory_path = claude_dir / "plugins/installed_plugins.json"
    inventory = _read_json(inventory_path)
    del inventory["plugins"][PLUGIN_ID]
    write_json(inventory_path, inventory)
    _reconcile(home, claude_dir, request, "uninstall")
    assert _native_list(home) == {"npm": [], "marketplace": []}
    assert not installed.exists()
    manifest = _read_json(native_root / "package.json")
    assert PACKAGE_NAME not in manifest.get("dependencies", {})
    assert not manifest.get("trustedDependencies"), manifest
    lock = _read_json(lock_path)
    assert PACKAGE_NAME not in lock["plugins"]
    assert PACKAGE_NAME not in lock["settings"]
    assert (source / "commands/base.md").read_bytes() == next_content
    assert _git(source, "rev-parse", "HEAD") == next_revision
