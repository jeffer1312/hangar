"""Reconciliação com inventários em disco e CLI OMP substituído na fronteira."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
from urllib.parse import urlunsplit

import pytest

from app.omp_plugin_sync import PluginSynchronizer


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def write_claude_inventory(
    home: Path,
    install_root: Path,
    *,
    repository: str,
    revision: str,
    package_name: str,
    plugin_id: str = "comet@catalog",
    subdir: str = "",
    enabled: bool = True,
    scope: str = "user",
) -> Path:
    """Atualiza um plugin sem apagar os demais registros do fixture."""
    claude_dir = home / ".claude"
    plugin_name, marketplace = plugin_id.rsplit("@", 1)
    market_root = claude_dir / "plugins" / "marketplaces" / marketplace
    registry_path = claude_dir / "plugins" / "installed_plugins.json"
    registry = read_json(registry_path) if registry_path.exists() else {"version": 2, "plugins": {}}
    package = read_json(install_root / "package.json")
    assert package["name"] == package_name
    entry = {
        "scope": scope,
        "installPath": str(install_root),
        "version": package["version"],
        "gitCommitSha": revision,
        "installedAt": "2026-09-01T00:00:00.000Z",
        "lastUpdated": "2026-09-01T00:00:00.000Z",
    }
    if scope != "user":
        entry["projectPath"] = str(home / "project")
    registry["plugins"][plugin_id] = [entry]
    write_json(registry_path, registry)
    known_path = claude_dir / "plugins" / "known_marketplaces.json"
    known = read_json(known_path) if known_path.exists() else {}
    known[marketplace] = {
        "source": {"source": "directory", "path": str(market_root)},
        "installLocation": str(market_root),
        "lastUpdated": "2026-09-01T00:00:00.000Z",
    }
    write_json(known_path, known)
    catalog_path = market_root / ".claude-plugin" / "marketplace.json"
    catalog = read_json(catalog_path) if catalog_path.exists() else {
        "name": marketplace,
        "owner": {"name": "Fixture"},
        "plugins": [],
    }
    source = {"source": "git-subdir" if subdir else "url", "url": repository, "sha": revision}
    if subdir:
        source["path"] = subdir
    catalog["plugins"] = [item for item in catalog["plugins"] if item["name"] != plugin_name]
    catalog["plugins"].append({"name": plugin_name, "source": source, "version": package["version"]})
    write_json(catalog_path, catalog)
    settings_path = claude_dir / "settings.json"
    settings = read_json(settings_path) if settings_path.exists() else {}
    settings.setdefault("enabledPlugins", {})[plugin_id] = enabled
    write_json(settings_path, settings)
    return claude_dir


def git(path: Path, *args: str) -> str:
    environment = {
        key: os.environ[key]
        for key in ("PATH", "SYSTEMROOT", "WINDIR", "LANG", "HOME", "USERPROFILE")
        if key in os.environ
    }
    environment.update({
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_AUTHOR_NAME": "Fixture",
        "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
        "GIT_COMMITTER_NAME": "Fixture",
        "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
    })
    return subprocess.run(
        ["git", "-c", "core.hooksPath=" + os.devnull, *args],
        cwd=path, env=environment, capture_output=True, text=True,
        encoding="utf-8", errors="replace", check=True, timeout=15,
    ).stdout.strip()


def make_repository(path: Path, *, name: str = "@fixture/comet", subdir: str = "") -> str:
    path.mkdir(parents=True)
    git(path, "init", "--quiet")
    package_root = path / subdir
    write_json(package_root / "package.json", {
        "name": name, "version": "1.0.0", "type": "module",
        "omp": {"extensions": ["extension.ts"], "features": {"extra": {"description": "Extra"}}},
    })
    (package_root / "extension.ts").write_text("export default function () {}\n", encoding="utf-8")
    write_json(package_root / ".claude-plugin" / "plugin.json", {"name": "comet", "version": "1.0.0"})
    git(path, "add", ".")
    git(path, "commit", "--quiet", "-m", "fixture inicial")
    return git(path, "rev-parse", "HEAD")


class NativeStore:
    """Substitui somente o processo externo; o reconciliador lê arquivos e Git reais."""

    def __init__(self, home: Path):
        self.home = home
        self.root = home / ".omp" / "plugins"
        self.repositories: dict[str, Path] = {}
        self.calls: list[list[str]] = []
        self.failure: tuple[str, str] | None = None
        self.list_payload: str | None = None
        self.list_transform = None
        self.after_install = None
        self.install_entered: threading.Event | None = None
        self.install_release: threading.Event | None = None
        self._mutex = threading.Lock()
        write_json(self.root / "package.json", {"name": "omp-plugins", "private": True, "dependencies": {}})
        write_json(self.root / "omp-plugins.lock.json", {"plugins": {}, "settings": {}})
        write_json(self.root / "installed_plugins.json", {"version": 2, "plugins": {}})

    @property
    def package(self) -> dict:
        return read_json(self.root / "package.json")

    @property
    def lock(self) -> dict:
        return read_json(self.root / "omp-plugins.lock.json")

    @property
    def mutations(self) -> list[list[str]]:
        return [call for call in self.calls if call[2] != "list"]

    def path(self, name: str) -> Path:
        return self.root / "node_modules" / name

    def seed(
        self, repository: str, revision: str, *, enabled: bool = True,
        features: list[str] | None = None, settings: dict | None = None,
    ) -> str:
        checkout = self.repositories[repository.removeprefix("git+")]
        manifest = json.loads(git(checkout, "show", f"{revision}:package.json"))
        name = manifest["name"]
        target = self.path(name)
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            shutil.rmtree(target)
        git(checkout, "clone", "--quiet", "--no-hardlinks", str(checkout), str(target))
        git(target, "checkout", "--quiet", "--detach", revision)
        git(target, "remote", "set-url", "origin", repository.removeprefix("git+"))
        package, lock = self.package, self.lock
        package["dependencies"][name] = f"{repository}#{revision}"
        lock["plugins"][name] = {
            "version": manifest["version"], "enabledFeatures": features, "enabled": enabled,
        }
        if settings is not None:
            lock["settings"][name] = settings
        write_json(self.root / "package.json", package)
        write_json(self.root / "omp-plugins.lock.json", lock)
        return name

    def remove(self, name: str) -> None:
        package, lock = self.package, self.lock
        package["dependencies"].pop(name, None)
        lock["plugins"].pop(name, None)
        lock["settings"].pop(name, None)
        write_json(self.root / "package.json", package)
        write_json(self.root / "omp-plugins.lock.json", lock)
        if self.path(name).exists():
            shutil.rmtree(self.path(name))

    def inventory(self) -> dict:
        npm = []
        for name in self.package["dependencies"]:
            manifest = read_json(self.path(name) / "package.json")
            runtime = self.lock["plugins"][name]
            npm.append({
                "name": name, "version": manifest["version"], "path": str(self.path(name)),
                "manifest": manifest.get("omp", manifest.get("pi")),
                "enabledFeatures": runtime["enabledFeatures"], "enabled": runtime["enabled"],
            })
        marketplace = [
            {"id": plugin_id, "scope": "user", "entries": entries}
            for plugin_id, entries in read_json(self.root / "installed_plugins.json")["plugins"].items()
        ]
        return {"npm": npm, "marketplace": marketplace}

    def __call__(self, args: list[str], *, cwd: Path, env: dict[str, str], timeout: float):
        assert Path(env["HOME"]).resolve() == self.home.resolve()
        assert Path(cwd).resolve().is_relative_to(self.home.resolve())
        assert Path(args[0]).name == "omp"
        assert args[1] == "plugin" and args[-1] == "--json"
        assert timeout > 0
        action = args[2]
        assert action in {"list", "install", "enable", "disable", "uninstall"}
        with self._mutex:
            self.calls.append(args.copy())
        if self.failure and self.failure[0] == action:
            kind = self.failure[1]
            if kind == "timeout":
                raise subprocess.TimeoutExpired(args, timeout)
            if kind == "error":
                return subprocess.CompletedProcess(args, 1, "", "falha sintética")
            if kind == "no-effect":
                return subprocess.CompletedProcess(args, 0, '{"success":true}', "")
        if action == "list":
            assert len(args) == 4
            if self.list_payload is not None:
                return subprocess.CompletedProcess(args, 0, self.list_payload, "")
            payload = self.inventory()
            if self.list_transform:
                payload = self.list_transform(payload)
            return subprocess.CompletedProcess(args, 0, json.dumps(payload), "")
        assert len(args) == 5
        if action == "install":
            if self.install_entered:
                self.install_entered.set()
                assert self.install_release is not None and self.install_release.wait(10)
            match = re.fullmatch(r"(.+)#([0-9a-f]{40})(?:\[([^\]]*)\])?", args[3])
            assert match, args[3]
            repository, revision, selected = match.groups()
            features = None if selected is None else selected.split(",") if selected else []
            name = self.seed(repository, revision, features=features)
            if self.after_install:
                self.after_install()
            payload = next(item for item in self.inventory()["npm"] if item["name"] == name)
        elif action == "uninstall":
            self.remove(args[3])
            payload = {"success": True}
        else:
            lock = self.lock
            lock["plugins"][args[3]]["enabled"] = action == "enable"
            write_json(self.root / "omp-plugins.lock.json", lock)
            payload = {"success": True}
        return subprocess.CompletedProcess(args, 0, json.dumps(payload), "")


@dataclass
class Scenario:
    home: Path
    repository_path: Path
    repository: str
    revision: str
    install_root: Path
    claude_dir: Path
    native: NativeStore
    name: str = "@fixture/comet"
    plugin_id: str = "comet@catalog"

    def reconcile(self, **kwargs) -> dict:
        return PluginSynchronizer(home=self.home, claude_dir=self.claude_dir, runner=self.native).reconcile(**kwargs)

    def source(self, *, enabled: bool = True, scope: str = "user", subdir: str = "") -> None:
        write_claude_inventory(
            self.home, self.install_root, repository=self.repository, revision=self.revision,
            package_name=self.name, plugin_id=self.plugin_id, enabled=enabled, scope=scope, subdir=subdir,
        )

    def remove_source(self) -> None:
        path = self.claude_dir / "plugins" / "installed_plugins.json"
        registry = read_json(path)
        registry["plugins"].pop(self.plugin_id)
        write_json(path, registry)

    def advance(self) -> str:
        (self.repository_path / "extension.ts").write_text("export default function () { return 2; }\n", encoding="utf-8")
        git(self.repository_path, "add", "extension.ts")
        git(self.repository_path, "commit", "--quiet", "-m", "segunda revisão, mesma versão")
        self.revision = git(self.repository_path, "rev-parse", "HEAD")
        git(self.install_root, "fetch", "--quiet", str(self.repository_path), self.revision)
        git(self.install_root, "checkout", "--quiet", "--detach", self.revision)
        self.source()
        return self.revision


@pytest.fixture
def scenario(tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir()
    for key in tuple(os.environ):
        if key.startswith("GIT_"):
            monkeypatch.delenv(key)
    for key, value in {
        "HOME": home, "USERPROFILE": home, "CLAUDE_CONFIG_DIR": home / ".claude",
        "PI_CODING_AGENT_DIR": home / ".omp" / "agent",
        "XDG_CONFIG_HOME": home / ".config", "XDG_DATA_HOME": home / ".local" / "share",
        "XDG_CACHE_HOME": home / ".cache", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_TERMINAL_PROMPT": "0",
    }.items():
        monkeypatch.setenv(key, str(value))
    repository_path = home / "repositories" / "comet"
    revision = make_repository(repository_path)
    repository = "https://git.example.invalid/team/comet.git"
    install_root = home / ".claude" / "plugins" / "cache" / "catalog" / "comet" / revision
    install_root.parent.mkdir(parents=True)
    git(repository_path, "clone", "--quiet", "--no-hardlinks", str(repository_path), str(install_root))
    git(install_root, "remote", "set-url", "origin", repository)
    claude_dir = write_claude_inventory(
        home, install_root, repository=repository, revision=revision, package_name="@fixture/comet",
    )
    native = NativeStore(home)
    native.repositories[repository] = repository_path
    return Scenario(home, repository_path, repository, revision, install_root, claude_dir, native)


def tree_snapshot(root: Path) -> dict:
    return {
        str(path.relative_to(root)): ("link", str(path.readlink())) if path.is_symlink()
        else ("dir",) if path.is_dir() else ("file", path.read_bytes())
        for path in root.rglob("*")
    }


def test_instala_pacote_pelo_nome_npm_e_segunda_passagem_nao_muta(scenario):
    first = scenario.reconcile()
    assert first["complete_inventory"] is True and first["errors"] == []
    dependencies = scenario.native.package["dependencies"]
    assert set(dependencies) == {scenario.name}
    assert dependencies[scenario.name].removeprefix("git+") == f"{scenario.repository}#{scenario.revision}"
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is True
    assert [call[2] for call in scenario.native.mutations] == ["install"]
    assert scenario.native.mutations[0][3].removeprefix("git+") == f"{scenario.repository}#{scenario.revision}"
    scenario.native.calls.clear()
    second = scenario.reconcile()
    assert second["errors"] == []
    assert scenario.native.mutations == []


def test_adota_instalacao_com_prova_sem_reinstalar_e_passa_a_acompanhar(scenario):
    scenario.native.seed(scenario.repository, scenario.revision)
    assert scenario.reconcile()["errors"] == []
    assert scenario.native.mutations == []
    scenario.source(enabled=False)
    scenario.reconcile()
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is False
    assert [call[2] for call in scenario.native.mutations] == ["disable"]


def test_equivalencia_de_url_preserva_protocolo_e_porta_padrao(scenario):
    equivalent = "https://GIT.EXAMPLE.INVALID:443/team/comet.git"
    scenario.native.repositories[equivalent] = scenario.repository_path
    scenario.native.seed(equivalent, scenario.revision)
    scenario.reconcile()
    assert scenario.native.mutations == []
    scenario.source(enabled=False)
    scenario.reconcile()
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is False


@pytest.mark.parametrize("other", [
    "http://git.example.invalid/team/comet.git",
    "https://other.example.invalid/team/comet.git",
    "https://git.example.invalid:8443/team/comet.git",
    "https://git.example.invalid/other/comet.git",
    "https://git.example.invalid/team/Comet.git",
])
def test_colisao_de_nome_nao_funde_origens_distintas(scenario, other):
    scenario.native.repositories[other] = scenario.repository_path
    scenario.native.seed(other, scenario.revision)
    before = tree_snapshot(scenario.native.root)
    scenario.reconcile()
    scenario.source(enabled=False)
    scenario.reconcile()
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


@pytest.mark.parametrize("missing", ["source-git", "native-git"])
def test_origem_sem_prova_nao_instala_nem_adota(scenario, missing):
    if missing == "source-git":
        shutil.rmtree(scenario.install_root / ".git")
    else:
        scenario.native.seed(scenario.repository, scenario.revision)
        shutil.rmtree(scenario.native.path(scenario.name) / ".git")
    before = tree_snapshot(scenario.native.root)
    scenario.reconcile()
    scenario.source(enabled=False)
    scenario.reconcile()
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


def test_git_subdir_nao_vira_instalacao_da_raiz_sem_gerenciador_preservavel(scenario):
    nested_repo = scenario.claude_dir / "plugins" / "cache" / "catalog" / "monorepo" / "fixture"
    revision = make_repository(nested_repo, subdir="packages/comet")
    git(nested_repo, "remote", "add", "origin", scenario.repository)
    scenario.install_root = nested_repo / "packages" / "comet"
    scenario.revision = revision
    scenario.source(subdir="packages/comet")
    scenario.reconcile()
    assert scenario.native.package["dependencies"] == {}
    assert scenario.native.mutations == []


@pytest.mark.parametrize("scope", ["project", "local"])
def test_escopo_do_claude_nao_e_promovido_a_global(scenario, scope):
    scenario.source(scope=scope)
    scenario.reconcile()
    assert scenario.native.package["dependencies"] == {}
    assert scenario.native.mutations == []


def test_atualiza_sha_mesmo_quando_a_versao_nao_muda(scenario):
    scenario.reconcile()
    old_revision = scenario.revision
    new_revision = scenario.advance()
    scenario.native.calls.clear()
    result = scenario.reconcile()
    assert result["errors"] == [] and new_revision != old_revision
    assert read_json(scenario.native.path(scenario.name) / "package.json")["version"] == "1.0.0"
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == new_revision
    assert [call[2] for call in scenario.native.mutations] == ["install"]
    assert scenario.native.mutations[0][3].removeprefix("git+") == f"{scenario.repository}#{new_revision}"


def test_desativacao_guarda_atualizacao_pendente_ate_reativar(scenario):
    scenario.reconcile()
    original = scenario.revision
    scenario.source(enabled=False)
    scenario.reconcile()
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is False
    scenario.advance()
    scenario.source(enabled=False)
    scenario.native.calls.clear()
    scenario.reconcile()
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == original
    assert scenario.native.mutations == []
    scenario.source(enabled=True)
    result = scenario.reconcile()
    assert result["errors"] == []
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is True


def test_reativa_sem_reinstalar_quando_sha_e_o_mesmo(scenario):
    scenario.reconcile()
    scenario.source(enabled=False)
    scenario.reconcile()
    scenario.native.calls.clear()
    scenario.source(enabled=True)
    scenario.reconcile()
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is True
    assert [call[2] for call in scenario.native.mutations] == ["enable"]


def test_remove_apenas_pacote_gerenciado_apos_inventario_completo(scenario):
    scenario.reconcile()
    scenario.remove_source()
    scenario.native.calls.clear()
    result = scenario.reconcile()
    assert result["complete_inventory"] is True and result["errors"] == []
    assert scenario.name not in scenario.native.package["dependencies"]
    assert not scenario.native.path(scenario.name).exists()
    assert [call[2:4] for call in scenario.native.mutations] == [["uninstall", scenario.name]]


@pytest.mark.parametrize("change", ["enabled", "features", "settings", "removed"])
def test_edicao_manual_retira_autoridade_do_sync(scenario, change):
    scenario.reconcile()
    if change == "removed":
        scenario.native.remove(scenario.name)
    else:
        lock = scenario.native.lock
        if change == "enabled":
            lock["plugins"][scenario.name]["enabled"] = False
        elif change == "features":
            lock["plugins"][scenario.name]["enabledFeatures"] = ["extra"]
        else:
            lock["settings"][scenario.name] = {"endpoint": "https://custom.example.invalid"}
        write_json(scenario.native.root / "omp-plugins.lock.json", lock)
    before = tree_snapshot(scenario.native.root)
    scenario.native.calls.clear()
    scenario.reconcile()
    scenario.advance()
    scenario.reconcile()
    scenario.remove_source()
    scenario.reconcile()
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


@pytest.mark.parametrize("features", [None, [], ["extra"]])
def test_adocao_e_update_preservam_selecao_de_features_e_settings(scenario, features):
    settings = {"endpoint": "https://custom.example.invalid", "attempts": 2}
    scenario.native.seed(scenario.repository, scenario.revision, features=features, settings=settings)
    scenario.reconcile()
    scenario.advance()
    scenario.native.calls.clear()
    scenario.reconcile()
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision
    assert scenario.native.lock["plugins"][scenario.name]["enabledFeatures"] == features
    assert scenario.native.lock["settings"][scenario.name] == settings
    suffix = "" if features is None else "[" + ",".join(features) + "]"
    assert [call[3].removeprefix("git+") for call in scenario.native.mutations if call[2] == "install"] == [f"{scenario.repository}#{scenario.revision}{suffix}"]


@pytest.mark.parametrize("invalid", ["json", "shape", "entry", "duplicate"])
def test_registro_claude_invalido_nao_autoriza_remocao(scenario, invalid):
    scenario.reconcile()
    path = scenario.claude_dir / "plugins" / "installed_plugins.json"
    if invalid == "json":
        path.write_text("{", encoding="utf-8")
    elif invalid == "shape":
        write_json(path, {"version": 2, "plugins": []})
    elif invalid == "duplicate":
        path.write_text('{"version":2,"plugins":{},"plugins":{}}', encoding="utf-8")
    else:
        write_json(path, {"version": 2, "plugins": {"broken@catalog": [{"scope": "user"}]}})
    scenario.native.calls.clear()
    before = tree_snapshot(scenario.native.root)
    result = scenario.reconcile()
    assert result["complete_inventory"] is False
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


@pytest.mark.parametrize("payload", ["{", "null", '{"npm":[],"marketplace":{}}', '{"npm":[{"name":"@fixture/comet"}],"marketplace":[]}'])
def test_cli_com_inventario_invalido_nao_autoriza_remocao(scenario, payload):
    scenario.reconcile()
    scenario.remove_source()
    scenario.native.list_payload = payload
    scenario.native.calls.clear()
    before = tree_snapshot(scenario.native.root)
    result = scenario.reconcile()
    assert result["errors"]
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


def test_cli_e_registro_local_divergentes_nao_autorizam_remocao(scenario):
    scenario.reconcile()
    scenario.remove_source()
    scenario.native.list_transform = lambda payload: {**payload, "npm": []}
    scenario.native.calls.clear()
    before = tree_snapshot(scenario.native.root)
    result = scenario.reconcile()
    assert result["errors"]
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


@pytest.mark.parametrize("action,kind", [("list", "error"), ("list", "timeout"), ("install", "error"), ("install", "timeout"), ("install", "no-effect")])
def test_falha_cli_nao_confirma_instalacao_e_permite_nova_tentativa(scenario, action, kind):
    scenario.native.failure = (action, kind)
    result = scenario.reconcile()
    assert result["errors"]
    assert scenario.name not in scenario.native.package["dependencies"]
    scenario.native.failure = None
    scenario.native.calls.clear()
    assert scenario.reconcile()["errors"] == []
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision
    assert [call[2] for call in scenario.native.mutations] == ["install"]


def test_desfazer_que_falha_nao_esconde_a_causa_original(scenario, monkeypatch):
    from app import omp_plugin_sync as m
    armed = {"on": False}
    scenario.native.failure = ("install", "no-effect")
    scenario.native.after_install = lambda: armed.__setitem__("on", True)
    original = m.PluginSynchronizer._native

    def native(self, *, dry_run, stop=None):
        if armed["on"] and dry_run:
            raise m.InventoryError("falha sintética do desfazer")
        return original(self, dry_run=dry_run, stop=stop)

    monkeypatch.setattr(m.PluginSynchronizer, "_native", native)
    result = scenario.reconcile()
    assert result["errors"] == ["Efeito nativo não corresponde à ação solicitada"]


def test_digest_so_rele_arquivo_quando_a_assinatura_muda(tmp_path, monkeypatch):
    from app import omp_plugin_sync as m
    root = tmp_path / "plugin"
    (root / "sub").mkdir(parents=True)
    (root / "a.txt").write_bytes(b"a")
    (root / "sub" / "b.txt").write_bytes(b"b")
    reads = []
    real = Path.read_bytes
    monkeypatch.setattr(Path, "read_bytes", lambda self: reads.append(self) or real(self))
    first = m._digest(root)
    assert len(reads) == 2
    assert m._digest(root) == first and len(reads) == 2
    (root / "a.txt").write_bytes(b"aa")
    assert m._digest(root) != first and len(reads) == 4


@pytest.mark.parametrize("failure", ["error", "timeout", "no-effect"])
def test_falha_de_update_preserva_instalacao_e_pin_confirmados(scenario, failure):
    scenario.native.seed(scenario.repository, scenario.revision, features=["extra"], settings={"key": "preservar"})
    scenario.reconcile()
    before = tree_snapshot(scenario.native.root)
    original = scenario.revision
    scenario.advance()
    scenario.native.failure = ("install", failure)
    result = scenario.reconcile()
    assert result["errors"]
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == original
    assert tree_snapshot(scenario.native.root) == before
    scenario.native.failure = None
    assert scenario.reconcile()["errors"] == []
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision


def test_interrupcao_apos_install_nao_confirmado_nao_da_posse_para_remover(scenario):
    stopped = threading.Event()
    scenario.native.after_install = stopped.set
    scenario.reconcile(stop_requested=stopped.is_set)
    assert scenario.name in scenario.native.package["dependencies"]
    scenario.remove_source()
    scenario.native.after_install = None
    scenario.native.calls.clear()
    scenario.reconcile()
    assert scenario.name in scenario.native.package["dependencies"]
    assert scenario.native.mutations == []


def test_parada_antes_da_primeira_acao_nao_muta(scenario):
    before = tree_snapshot(scenario.native.root)
    scenario.reconcile(stop_requested=lambda: True)
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


@pytest.mark.parametrize("native_root_exists", [True, False])
def test_dry_run_planeja_por_registros_sem_cli_subprocesso_ou_escrita(scenario, monkeypatch, native_root_exists):
    def forbidden_process(*args, **kwargs):
        pytest.fail("dry_run tentou iniciar um subprocesso")

    if not native_root_exists:
        shutil.rmtree(scenario.home / ".omp")

    before = tree_snapshot(scenario.home)
    with monkeypatch.context() as context:
        context.setattr(subprocess, "run", forbidden_process)
        context.setattr(subprocess, "Popen", forbidden_process)
        result = scenario.reconcile(dry_run=True)
    assert result["mode"] == "read_only_local"
    assert result["items"]
    assert scenario.native.calls == []
    assert tree_snapshot(scenario.home) == before


def test_duas_threads_concorrentes_instalam_uma_unica_vez(scenario):
    entered, release, second_started = threading.Event(), threading.Event(), threading.Event()
    scenario.native.install_entered = entered
    scenario.native.install_release = release

    def second_reconcile():
        second_started.set()
        return scenario.reconcile()

    with ThreadPoolExecutor(max_workers=2) as pool:
        first = pool.submit(scenario.reconcile)
        try:
            assert entered.wait(10)
            second = pool.submit(second_reconcile)
            assert second_started.wait(10)
        finally:
            release.set()
        results = [first.result(timeout=15), second.result(timeout=15)]
    assert all(result["errors"] == [] for result in results)
    assert [call[2] for call in scenario.native.mutations] == ["install"]
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision


def test_registro_marketplace_existente_nao_e_convertido_em_npm(scenario):
    write_json(scenario.native.root / "installed_plugins.json", {
        "version": 2,
        "plugins": {scenario.plugin_id: [{
            "scope": "user", "installPath": str(scenario.install_root),
            "version": "1.0.0", "gitCommitSha": scenario.revision,
            "installedAt": "2026-09-01T00:00:00.000Z",
            "lastUpdated": "2026-09-01T00:00:00.000Z",
        }]},
    })
    before = tree_snapshot(scenario.native.root)
    scenario.reconcile()
    assert scenario.native.package["dependencies"] == {}
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


@pytest.mark.parametrize("registry", ["package.json", "omp-plugins.lock.json", "installed_plugins.json"])
def test_registro_nativo_invalido_nao_autoriza_escrita(scenario, registry):
    (scenario.native.root / registry).write_text("{", encoding="utf-8")
    scenario.native.list_payload = '{"npm":[],"marketplace":[]}'
    before = tree_snapshot(scenario.native.root)
    result = scenario.reconcile()
    assert result["errors"]
    assert scenario.native.mutations == []
    assert tree_snapshot(scenario.native.root) == before


def test_manifest_sem_omp_ou_pi_nao_e_promovido_por_ter_nome_npm(scenario):
    path = scenario.install_root / "package.json"
    manifest = read_json(path)
    manifest.pop("omp")
    write_json(path, manifest)
    scenario.reconcile()
    assert scenario.native.package["dependencies"] == {}
    assert scenario.native.mutations == []


@pytest.mark.parametrize("escape", ["manifest", "extension"])
def test_arquivo_de_pacote_fora_da_raiz_nao_e_promovido(scenario, escape):
    outside = scenario.home / "outside"
    outside.mkdir()
    if escape == "manifest":
        manifest_path = scenario.install_root / "package.json"
        external = outside / "package.json"
        external.write_bytes(manifest_path.read_bytes())
        manifest_path.unlink()
        manifest_path.symlink_to(external)
    else:
        external = outside / "extension.ts"
        external.write_text("export default function () {}\n", encoding="utf-8")
        manifest = read_json(scenario.install_root / "package.json")
        manifest["omp"]["extensions"] = [str(external)]
        write_json(scenario.install_root / "package.json", manifest)
    before = tree_snapshot(outside)
    scenario.reconcile()
    assert scenario.native.package["dependencies"] == {}
    assert scenario.native.mutations == []
    assert tree_snapshot(outside) == before


def test_plugin_sem_sha_nao_invalida_outros_registros_validos(scenario):
    path = scenario.claude_dir / "plugins/installed_plugins.json"
    registry = read_json(path)
    registry["plugins"]["without-pin@catalog"] = [{"scope": "user", "installPath": str(scenario.home / "without-pin")}]
    write_json(path, registry)
    result = scenario.reconcile()
    assert result["complete_inventory"] is True
    assert git(scenario.native.path(scenario.name), "rev-parse", "HEAD") == scenario.revision
    assert any(item["identity"] == "without-pin@catalog" and item["action"] == "diagnostic" for item in result["items"])


def test_adocao_preserva_desativacao_nativa_ate_mudanca_explicita_no_claude(scenario):
    scenario.native.seed(scenario.repository, scenario.revision, enabled=False)
    scenario.reconcile()
    scenario.reconcile()
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is False
    scenario.advance()
    assert scenario.reconcile()["errors"] == []
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is False
    scenario.source(enabled=False)
    scenario.reconcile()
    scenario.source(enabled=True)
    scenario.reconcile()
    assert scenario.native.lock["plugins"][scenario.name]["enabled"] is True


def test_plugin_claude_inativo_nao_da_posse_da_instalacao_manual(scenario):
    scenario.native.seed(scenario.repository, scenario.revision)
    scenario.source(enabled=False)
    scenario.reconcile()
    scenario.remove_source()
    scenario.reconcile()
    assert scenario.name in scenario.native.package["dependencies"]
    assert scenario.native.mutations == []


@pytest.mark.parametrize("field,value", [
    ("source.id", None), ("source.name", "other"), ("source.origin", []),
    ("source.url", "https://other.example.invalid/repo.git"), ("source.revision", "bad"),
    ("source.enabled", 1), ("status", "unknown"), ("native", None),
    ("native.proof", None), ("native.enabled", "true"), ("native.features", [3]),
    ("native.settings", []), ("native.digest", "bad"), ("native.path", "/other/project"),
    ("native.spec", "https://other.example.invalid/repo.git#" + "a" * 40),
])
def test_ledger_invalido_nao_confere_autoridade(scenario, field, value):
    scenario.reconcile()
    ledger_path = scenario.home / ".hangar/omp-plugin-sync.json"
    ledger = read_json(ledger_path)
    target = ledger["items"][scenario.name]
    parts = field.split(".")
    for part in parts[:-1]:
        target = target[part]
    target[parts[-1]] = value
    write_json(ledger_path, ledger)
    source_path = scenario.claude_dir / "plugins/installed_plugins.json"
    registry = read_json(source_path)
    registry["plugins"][scenario.plugin_id][0].pop("gitCommitSha")
    write_json(source_path, registry)
    before = ledger_path.read_bytes(), tree_snapshot(scenario.native.root)
    scenario.native.calls.clear()
    result = scenario.reconcile()
    assert result["complete_inventory"] is False
    assert result["errors"]
    assert scenario.native.calls == []
    assert (ledger_path.read_bytes(), tree_snapshot(scenario.native.root)) == before


def test_ledger_invalido_posterior_impede_remocao_anterior(scenario):
    scenario.reconcile()
    ledger_path = scenario.home / ".hangar/omp-plugin-sync.json"
    ledger = read_json(ledger_path)
    later = json.loads(json.dumps(ledger["items"][scenario.name]))
    later["source"].pop("id")
    ledger["items"]["zz-invalid"] = later
    write_json(ledger_path, ledger)
    scenario.remove_source()
    before = ledger_path.read_bytes(), tree_snapshot(scenario.native.root)
    scenario.native.calls.clear()
    result = scenario.reconcile()
    assert result["complete_inventory"] is False and result["errors"]
    assert scenario.native.calls == []
    assert (ledger_path.read_bytes(), tree_snapshot(scenario.native.root)) == before


def add_candidate(scenario, alias, package_name):
    root = scenario.home / "candidates" / alias
    revision = make_repository(root, name=package_name)
    origin = f"https://{alias}.example.invalid/group/plugin.git"
    git(root, "remote", "add", "origin", origin)
    scenario.native.repositories[origin] = root
    write_claude_inventory(scenario.home, root, repository=origin, revision=revision,
                          package_name=package_name, plugin_id=alias + "@catalog")


@pytest.mark.parametrize("reverse,third", [(False, False), (True, False), (False, True)])
def test_colisao_entre_fontes_nao_escolhe_primeira_ou_terceira(scenario, reverse, third):
    add_candidate(scenario, "second", scenario.name)
    if third:
        add_candidate(scenario, "third", scenario.name)
    add_candidate(scenario, "independent", "independent")
    path = scenario.claude_dir / "plugins/installed_plugins.json"
    if reverse:
        registry = read_json(path)
        registry["plugins"] = dict(reversed(list(registry["plugins"].items())))
        write_json(path, registry)
    result = scenario.reconcile()
    assert result["errors"] == []
    assert set(scenario.native.package["dependencies"]) == {"independent"}
    assert any(item.get("action") == "diagnostic" and item.get("name") == scenario.name for item in result["items"])


def test_colisao_preserva_instalacao_e_ledger_mesmo_sem_origem_anterior(scenario):
    scenario.reconcile()
    scenario.remove_source()
    add_candidate(scenario, "second", scenario.name)
    add_candidate(scenario, "third", scenario.name)
    ledger = scenario.home / ".hangar/omp-plugin-sync.json"
    before = ledger.read_bytes(), tree_snapshot(scenario.native.root)
    scenario.native.calls.clear()
    scenario.reconcile()
    assert scenario.native.mutations == []
    assert (ledger.read_bytes(), tree_snapshot(scenario.native.root)) == before


@pytest.mark.parametrize("dry_run", [True, False])
def test_erro_de_git_nao_transcreve_credencial_no_diagnostico(scenario, dry_run):
    marker = "CREDENCIAL_SINTETICA_NAO_PUBLICAR"
    synthetic_url = urlunsplit(("https", f"user:{marker}@example.invalid", "/repo.git", "", ""))
    (scenario.install_root / ".git/config").write_text(
        f"url={synthetic_url}\n", encoding="utf-8")
    before = tree_snapshot(scenario.home)
    result = scenario.reconcile(dry_run=dry_run)
    assert marker not in json.dumps(result)
    assert any(item["identity"] == scenario.plugin_id and item.get("reason") for item in result["items"])
    assert scenario.native.mutations == []
    if dry_run:
        assert tree_snapshot(scenario.home) == before
