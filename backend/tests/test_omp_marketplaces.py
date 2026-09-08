"""Importação de catálogos sem instalar nem migrar plugins existentes."""
import json
import os
from pathlib import Path
import subprocess
from urllib.parse import urlunsplit

import pytest

from app.omp_plugin_sync import PluginSynchronizer
from tests.test_omp_plugin_sync import write_json, read_json, tree_snapshot

if os.environ.get("HANGAR_TEST_SANDBOX") == "1":
    from tests.test_omp_plugin_sync_runtime import runtime_home, git_http, _git


@pytest.fixture
def catalogs(tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(home / ".omp/agent"))
    monkeypatch.setenv("HOME", str(home))
    definitions = {}
    for name in ("catalog-alpha", "catalog-beta"):
        root = home / ".claude/plugins/marketplaces" / name
        write_json(root / ".claude-plugin/marketplace.json", {
            "name": name, "owner": {"name": "Fixture"}, "plugins": [],
        })
        definitions[name] = {"source": {"source": "directory", "path": str(root)},
                             "installLocation": str(root)}
    write_json(home / ".claude/plugins/known_marketplaces.json", definitions)
    return home, definitions


class CatalogCLI:
    def __init__(self, home, definitions, *, no_effect=False):
        self.registry = home / ".omp/marketplaces.json"
        self.catalogs = {value["source"].get("path", value["source"].get("url")): name for name, value in definitions.items()}
        self.calls = []
        self.no_effect = no_effect

    def __call__(self, args, **kwargs):
        assert args[:4] == ["omp", "plugin", "marketplace", "add"]
        self.calls.append(args)
        if not self.no_effect:
            uri = args[4]
            registry = read_json(self.registry) if self.registry.exists() else {"version": 1, "marketplaces": []}
            registry["marketplaces"].append({"name": self.catalogs[uri], "sourceType": "local", "sourceUri": uri})
            write_json(self.registry, registry)
        return subprocess.CompletedProcess(args, 0, "Catálogo adicionado\n", "")


def test_importa_catalogos_genericos_sem_repetir_ou_instalar_plugins(catalogs):
    home, definitions = catalogs
    native = CatalogCLI(home, definitions)
    sync = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native)
    assert sync.import_marketplaces()["errors"] == []
    assert {item["name"] for item in read_json(native.registry)["marketplaces"]} == set(definitions)
    before = tree_snapshot(home / ".omp")
    native.calls.clear()
    assert sync.import_marketplaces()["errors"] == []
    assert native.calls == []
    assert tree_snapshot(home / ".omp") == before
    assert not (home / ".omp/plugins/package.json").exists()


def test_catalogo_homonimo_com_outra_origem_e_preservado(catalogs):
    home, definitions = catalogs
    native = CatalogCLI(home, definitions)
    existing = {"name": "catalog-alpha", "sourceType": "git", "sourceUri": "https://other.example.invalid/catalog.git"}
    write_json(native.registry, {"version": 1, "marketplaces": [existing]})
    result = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native).import_marketplaces()
    registered = read_json(native.registry)["marketplaces"]
    assert registered[0] == existing
    assert len(registered) == 2
    assert any(item["action"] == "conflict" for item in result["items"])
    assert len(native.calls) == 1


def test_dry_run_de_marketplaces_nao_cria_estado(catalogs):
    home, definitions = catalogs
    native = CatalogCLI(home, definitions)
    before = tree_snapshot(home)
    result = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native).import_marketplaces(dry_run=True)
    assert result["mode"] == "read_only_local"
    assert len(result["items"]) == 2
    assert native.calls == []
    assert tree_snapshot(home) == before


def test_rc_zero_sem_catalogo_nao_e_sucesso(catalogs):
    home, definitions = catalogs
    native = CatalogCLI(home, definitions, no_effect=True)
    result = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native).import_marketplaces()
    assert result["errors"]
    assert not native.registry.exists()


def test_origem_com_segredo_nao_e_enviada_ao_cli(catalogs):
    home, definitions = catalogs
    synthetic_url = urlunsplit(("https", "user:secret@example.invalid", "/catalog.git", "", ""))
    definitions["catalog-alpha"]["source"] = {"source": "git", "url": synthetic_url}
    write_json(home / ".claude/plugins/known_marketplaces.json", definitions)
    native = CatalogCLI(home, definitions)
    result = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native).import_marketplaces()
    assert len(native.calls) == 1
    assert "secret" not in json.dumps(result)
    assert [entry["name"] for entry in read_json(native.registry)["marketplaces"]] == ["catalog-beta"]


@pytest.mark.skipif(os.environ.get("HANGAR_TEST_SANDBOX") != "1", reason="CLI real exige sandbox")
def test_cli_real_importa_catalogo_local_sem_instalar_plugins(catalogs, monkeypatch):
    home, _ = catalogs
    binary = os.environ["OMP_TEST_BIN"]
    bin_dir = home / "bin"
    bin_dir.mkdir()
    (bin_dir / "omp").symlink_to(binary)
    monkeypatch.setenv("PATH", str(bin_dir) + os.pathsep + os.environ["PATH"])
    sync = PluginSynchronizer(home=home, claude_dir=home / ".claude")
    result = sync.import_marketplaces()
    assert result["errors"] == [], result
    registry = home / ".omp/marketplaces.json"
    before = registry.read_bytes()
    assert {entry["name"] for entry in read_json(registry)["marketplaces"]} == {"catalog-alpha", "catalog-beta"}
    assert sync.import_marketplaces()["errors"] == []
    assert registry.read_bytes() == before
    assert not (home / ".omp/plugins/installed_plugins.json").exists()


@pytest.mark.skipif(os.environ.get("HANGAR_TEST_SANDBOX") != "1", reason="CLI real exige sandbox")
def test_cli_real_importa_catalogo_git_pela_origem_original(runtime_home, git_http):
    home = runtime_home
    bare, uri = git_http
    source = home / "catalog-source"
    source.mkdir()
    write_json(source / ".claude-plugin/marketplace.json", {
        "name": "remote-catalog", "owner": {"name": "Fixture"}, "plugins": [],
    })
    _git(source, "init", "--initial-branch=main")
    _git(source, "add", ".claude-plugin/marketplace.json")
    _git(source, "commit", "-m", "Catálogo sintético")
    _git(source, "push", str(bare), "HEAD:refs/heads/main")
    _git(bare, "update-server-info")
    write_json(home / ".claude/plugins/known_marketplaces.json", {
        "remote-catalog": {"source": {"source": "git", "url": uri}, "installLocation": str(source)},
    })
    from app.omp_plugin_sync import _run
    diagnostics = []
    def native(args, **kwargs):
        completed = _run(args, **kwargs)
        diagnostics.append(completed.stderr)
        return completed
    result = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native).import_marketplaces()
    assert result["errors"] == [], {"report": result, "cli": diagnostics}
    entry = read_json(home / ".omp/marketplaces.json")["marketplaces"][0]
    assert entry["name"] == "remote-catalog"
    assert entry["sourceUri"] == uri
    assert read_json(Path(entry["catalogPath"]))["name"] == "remote-catalog"


@pytest.mark.skipif(os.environ.get("HANGAR_TEST_SANDBOX") != "1", reason="CLI real exige sandbox")
@pytest.mark.parametrize("layout", [
    "custom-agent", "xdg-absent", "xdg-default", "xdg-custom", "config-root",
    "profile-legacy", "profile-xdg", "canonical-default", "xdg-double-slash",
])
def test_layout_global_coincide_com_cli_real(catalogs, monkeypatch, layout):
    home, _ = catalogs
    for variable in ("OMP_PROFILE", "PI_PROFILE", "PI_CONFIG_DIR", "PI_CODING_AGENT_DIR"):
        monkeypatch.delenv(variable, raising=False)
    data = home / ".local/share"
    monkeypatch.setenv("XDG_DATA_HOME", str(data))
    expected = home / ".omp"
    if layout in {"custom-agent", "xdg-custom"}:
        monkeypatch.setenv("PI_CODING_AGENT_DIR", str(home / "custom-agent"))
    if layout in {"xdg-default", "xdg-custom", "profile-legacy", "canonical-default"}:
        (data / "omp").mkdir(parents=True)
    if layout == "xdg-default":
        monkeypatch.setenv("PI_CODING_AGENT_DIR", str(home / ".omp/agent"))
        expected = data / "omp"
    elif layout == "config-root":
        monkeypatch.setenv("PI_CONFIG_DIR", ".omp-alt")
        expected = home / ".omp-alt"
    elif layout in {"profile-legacy", "profile-xdg"}:
        monkeypatch.setenv("OMP_PROFILE", "review")
        monkeypatch.setenv("PI_CODING_AGENT_DIR", str(home / "ignored-agent"))
        expected = home / ".omp/profiles/review"
        if layout == "profile-xdg":
            expected = data / "omp/profiles/review"
            expected.mkdir(parents=True)
    elif layout == "canonical-default":
        monkeypatch.setenv("OMP_PROFILE", "")
        monkeypatch.setenv("PI_PROFILE", "review")
        monkeypatch.setenv("PI_CODING_AGENT_DIR", str(home / ".omp/profiles/review/agent"))
        expected = data / "omp"
    elif layout == "xdg-double-slash":
        (data / "omp").mkdir(parents=True)
        monkeypatch.setenv("XDG_DATA_HOME", "//" + str(data).lstrip("/"))
        expected = data / "omp"
    private_bin = home / "bin"
    private_bin.mkdir()
    (private_bin / "omp").symlink_to(os.environ["OMP_TEST_BIN"])
    monkeypatch.setenv("PATH", str(private_bin) + os.pathsep + os.environ["PATH"])
    sync = PluginSynchronizer(home=home, claude_dir=home / ".claude")
    result = sync.import_marketplaces()
    assert result["errors"] == [], result
    assert sync.native_root == expected / "plugins"
    registry = expected / "marketplaces.json"
    assert {entry["name"] for entry in read_json(registry)["marketplaces"]} == {"catalog-alpha", "catalog-beta"}
    before = registry.read_bytes()
    assert sync.import_marketplaces()["errors"] == []
    assert registry.read_bytes() == before


def test_perfil_invalido_vira_diagnostico_sem_efeitos(catalogs, monkeypatch):
    home, definitions = catalogs
    monkeypatch.setenv("OMP_PROFILE", "../invalid")
    native = CatalogCLI(home, definitions)
    before = tree_snapshot(home)
    sync = PluginSynchronizer(home=home, claude_dir=home / ".claude", runner=native)
    result = sync.import_marketplaces()
    assert result["errors"]
    assert native.calls == []
    assert tree_snapshot(home) == before


@pytest.mark.skipif(os.environ.get("HANGAR_TEST_SANDBOX") != "1", reason="CLI real exige sandbox")
def test_ciclo_real_de_plugin_usa_raiz_xdg(runtime_home, git_http):
    from tests.test_omp_plugin_sync import make_repository, write_claude_inventory
    home = runtime_home
    bare, uri = git_http
    data = Path(os.environ["XDG_DATA_HOME"]) / "omp"
    data.mkdir(parents=True)
    source = home / "source"
    revision = make_repository(source, name="layout-fixture")
    _git(source, "remote", "add", "origin", uri)
    _git(source, "push", str(bare), "HEAD:refs/heads/main")
    _git(bare, "update-server-info")
    claude_dir = write_claude_inventory(home, source, repository=uri, revision=revision, package_name="layout-fixture")
    sync = PluginSynchronizer(home=home, claude_dir=claude_dir)
    assert sync.reconcile()["errors"] == []
    installed = data / "plugins/node_modules/layout-fixture"
    assert read_json(installed / "package.json")["name"] == "layout-fixture"
    changed = "export default function () {}\\n// Nova revisão sintética.\\n".replace("\\n", "\n")
    (source / "extension.ts").write_text(changed, encoding="utf-8")
    _git(source, "add", "extension.ts")
    _git(source, "commit", "-m", "Atualização sintética")
    revision = _git(source, "rev-parse", "HEAD")
    _git(source, "push", str(bare), "HEAD:refs/heads/main")
    _git(bare, "update-server-info")
    write_claude_inventory(home, source, repository=uri, revision=revision, package_name="layout-fixture")
    assert sync.reconcile()["errors"] == []
    assert (installed / "extension.ts").read_text(encoding="utf-8") == changed
    write_json(claude_dir / "plugins/installed_plugins.json", {"version": 2, "plugins": {}})
    assert sync.reconcile()["errors"] == []
    assert not installed.exists()
