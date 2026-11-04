"""Reconciliação de plugins nativos entre contas Codex."""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import shutil
import re
import tomllib
from unittest.mock import AsyncMock

import pytest

from app.codex_contas import Account
from app import codex_contas as accounts
from app.codex_contas_sync import project_preferences
from app.codex_contas_plugins import sync_plugins
from app.codex_importador import CodexNativo


def _json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")


def _plugin(marketplace: Path, name: str, version: str = "1.0.0") -> Path:
    root = marketplace / "plugins" / name
    _json(root / ".codex-plugin/plugin.json", {
        "name": name,
        "version": version,
        "description": "Plugin temporário",
        "hooks": "./hooks/hooks.json",
    })
    _json(root / "hooks/hooks.json", {"hooks": {"SessionEnd": [{"hooks": [
        {"type": "command", "command": "echo plugin-hook"},
    ]}]}})
    (root / "skills" / f"{name}-skill").mkdir(parents=True, exist_ok=True)
    (root / "skills" / f"{name}-skill/SKILL.md").write_text(
        "---\nname: sample-skill\ndescription: teste\n---\nTeste.\n", encoding="utf-8",
    )
    return root


@pytest.fixture
def contas(tmp_path, monkeypatch):
    # No Windows o tmp_path vem com a caixa do login (`administrator`); a produção resolve e compara
    # com a caixa real do disco.
    tmp_path = tmp_path.resolve()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    home = tmp_path / "home"
    home.mkdir()
    source = Account("default", tmp_path / ".codex", True)
    target = Account("work", tmp_path / ".codex-work", False)
    source.home.mkdir()
    target.home.mkdir()
    marketplace = tmp_path / "marketplace"
    _json(marketplace / ".claude-plugin/marketplace.json", {
        "name": "accounts-local",
        "owner": {"name": "Hangar"},
        "plugins": [{"name": "sample", "source": "./plugins/sample"}],
    })
    _plugin(marketplace, "sample")
    return home, source, target, marketplace


def _entry(marketplace: Path, *, enabled: bool = True, version: str = "1.0.0") -> dict:
    return {
        "pluginId": "sample@accounts-local",
        "name": "sample",
        "marketplaceName": "accounts-local",
        "version": version,
        "installed": True,
        "enabled": enabled,
        "source": {"source": "local", "path": str(marketplace / "plugins/sample")},
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
        "installPolicy": "AVAILABLE",
        "authPolicy": "ON_INSTALL",
    }


class FakeNative:
    states: dict[str, dict] = {}
    calls: list[tuple[str, str, object]] = []
    accounts: list[object | None] = []

    def __init__(self, home: Path, codex_home: Path, binario: str = "codex", **kwargs):
        self.home = Path(home)
        self.codex_home = Path(codex_home)
        self.accounts.append(kwargs.get("account"))
        self.state = self.states.setdefault(str(self.codex_home), {"plugins": [], "marketplaces": []})

    async def plugins_instalados(self) -> list[dict]:
        return copy.deepcopy(self.state["plugins"])

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return None

    async def request(self, method: str, params: dict):
        if method == "config/batchWrite":
            for edit in params["edits"]:
                key = json.loads("["+edit["keyPath"].replace('\".\"','\",\"')+"]")[2]
                for entry in self.state["hooks"]["data"]:
                    for hook in entry["hooks"]:
                        if hook["key"] == key:
                            assert hook["currentHash"] == edit["value"]
                            hook["trustStatus"] = "trusted"
            assert params["reloadUserConfig"] is True
            return {}
        assert method == "hooks/list"
        return copy.deepcopy(self.state.get("hooks", {"data": []}))

    async def cli(self, args: list[str], *, esperado: str = "") -> dict:
        self.calls.append((str(self.codex_home), "cli", list(args)))
        if args[:3] == ["plugin", "marketplace", "remove"]:
            self.state.pop("orphan", None)
            return {"marketplaceName": args[3]}
        if args[:3] == ["plugin", "marketplace", "add"] and self.state.get("orphan"):
            from app.codex_importador import CodexNativoErro
            raise CodexNativoErro("falhou", data={"stderr": "Error: marketplace 'x' is already "
                                                            "added from a different source; remove it"})
        if args[:4] == ["plugin", "marketplace", "list", "--json"]:
            config_path = self.codex_home / "config.toml"
            config = tomllib.loads(config_path.read_text(encoding="utf-8")) if config_path.is_file() else {}
            for name, item in (config.get("marketplaces") or {}).items():
                if not any(existing.get("name") == name for existing in self.state["marketplaces"]):
                    self.state["marketplaces"].append({
                        "name": name,
                        "root": item.get("source"),
                        "marketplaceSource": {
                            "sourceType": item.get("source_type"), "source": item.get("source"),
                        },
                    })
            return {"marketplaces": copy.deepcopy(self.state["marketplaces"])}
        if args[:3] == ["plugin", "marketplace", "add"]:
            source = args[3]
            manifest = Path(source) / ".claude-plugin/marketplace.json"
            name = json.loads(manifest.read_text(encoding="utf-8")).get("name", Path(source).name)
            for item in self.state["marketplaces"]:
                if item["name"] == name:
                    return {"marketplaceName": name, "installedRoot": source, "alreadyAdded": True}
            self.state["marketplaces"].append({
                "name": name,
                "root": source,
                "marketplaceSource": {"sourceType": "local", "source": source},
            })
            return {"marketplaceName": name, "installedRoot": source, "alreadyAdded": False}
        raise AssertionError(args)

    async def instalar_plugin(self, plugin_id: str) -> dict:
        self.calls.append((str(self.codex_home), "install", plugin_id))
        if self.state.get("install_error"):
            raise RuntimeError("falha simulada")
        marketplace = next(item for item in self.state["marketplaces"]
                           if item["name"] == plugin_id.rsplit("@", 1)[1])
        source = Path(marketplace["marketplaceSource"]["source"])
        root = source / "plugins" / plugin_id.split("@", 1)[0]
        installed_path = self.codex_home / "plugins/cache" / marketplace["name"] / plugin_id.split("@", 1)[0] / "1.0.0"
        name, marketplace_name = plugin_id.split("@", 1)
        manifest = json.loads((root / ".codex-plugin/plugin.json").read_text(encoding="utf-8"))
        entry = {
            "pluginId": plugin_id, "name": name, "marketplaceName": marketplace_name,
            "version": manifest["version"], "installed": True, "enabled": True,
            "source": {"source": "local", "path": str(root)},
            "marketplaceSource": copy.deepcopy(marketplace["marketplaceSource"]),
            "installPolicy": "AVAILABLE", "authPolicy": "ON_INSTALL",
        }
        entry["source"] = {"source": "local", "path": str(root)}
        entry["marketplaceSource"] = copy.deepcopy(marketplace["marketplaceSource"])
        self.state["plugins"] = [item for item in self.state["plugins"] if item["pluginId"] != plugin_id]
        self.state["plugins"].append(entry)
        return {"pluginId": plugin_id, "installedPath": str(installed_path), "version": "1.0.0"}


async def test_codex_nativo_expoe_inventario_de_marketplaces_validado(tmp_path):
    native = CodexNativo(tmp_path, tmp_path / ".codex")
    native.cli = AsyncMock(return_value={"marketplaces": [{
        "name": "accounts-local",
        "root": str(tmp_path / "marketplace"),
        "marketplaceSource": {"sourceType": "local", "source": str(tmp_path / "marketplace")},
    }]})

    result = await native.marketplaces_instalados()

    assert result[0]["name"] == "accounts-local"
    native.cli.assert_awaited_once_with(["plugin", "marketplace", "list", "--json"])


def test_heranca_de_preferencias_deixa_plugins_para_sync_nativo():
    result = project_preferences({
        "model": "fable",
        "marketplaces": {"accounts-local": {"source_type": "local"}},
        "plugins": {"sample@accounts-local": {"enabled": True}},
    })

    assert result == {"model": "fable", "cli_auth_credentials_store": "file"}


@pytest.fixture
def fake_native(monkeypatch, contas):
    from app import codex_contas_plugins as module
    FakeNative.states = {}
    FakeNative.calls = []
    FakeNative.accounts = []
    _, source, target, _ = contas
    FakeNative.states[str(source.home)] = {"plugins": [], "marketplaces": []}
    FakeNative.states[str(target.home)] = {"plugins": [], "marketplaces": []}
    monkeypatch.setattr(module, "_NATIVO", FakeNative)
    async def no_config(account, choices, issues):
        state = FakeNative.states[str(account.home)]
        for item in state["plugins"]:
            if item["pluginId"] in choices:
                item["enabled"] = choices[item["pluginId"]]
    monkeypatch.setattr(module, "_set_enabled", no_config)

    async def edit_config(path, backups, work_dir, nativo, preparar, **kwargs):
        current = tomllib.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}
        edits, confirm = preparar(current)
        for edit in edits:
            parts = [json.loads(part) for part in re.findall(r'"(?:[^"\\]|\\.)*"', edit["keyPath"])]
            target = current
            for part in parts[:-1]:
                target = target.setdefault(part, {})
            target[parts[-1]] = edit["value"]
        lines = []
        for name, values in (current.get("marketplaces") or {}).items():
            lines.append(f'[marketplaces.{json.dumps(name)}]')
            lines.extend(f'{key} = {json.dumps(value)}' for key, value in values.items())
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        confirm()

    monkeypatch.setattr(module, "editar_config", edit_config)
    return FakeNative


async def test_cli_nativo_recebe_a_conta_para_isolar_o_ambiente(contas, fake_native):
    _, source, target, _ = contas

    await sync_plugins(source, target, {})

    assert FakeNative.accounts[:2] == [source, target]


async def test_instala_somente_plugins_da_fonte_e_preserva_exclusivos(contas, fake_native):
    home, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(marketplace),
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
    }]
    source_state["plugins"] = [_entry(marketplace, enabled=False)]
    target_state["plugins"] = [{
        "pluginId": "exclusive@other", "name": "exclusive", "marketplaceName": "other",
        "version": "9.0.0", "installed": True, "enabled": True,
        "source": {"source": "local", "path": str(home / "exclusive")},
        "marketplaceSource": {"sourceType": "local", "source": str(home / "other")},
    }]
    target_state["marketplaces"] = [{
        "name": "other", "root": str(home / "other"),
        "marketplaceSource": {"sourceType": "local", "source": str(home / "other")},
    }]

    result = await sync_plugins(source, target, {})

    assert not result["issues"], result
    assert {item["pluginId"] for item in target_state["plugins"]} == {
        "sample@accounts-local", "exclusive@other",
    }
    assert next(item for item in target_state["plugins"]
                if item["pluginId"] == "sample@accounts-local")["enabled"] is False
    assert not any(call[2] == "exclusive@other" for call in FakeNative.calls if call[1] == "install")


async def test_clone_orfao_do_marketplace_e_removido_e_readicionado(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(marketplace),
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
    }]
    source_state["plugins"] = [_entry(marketplace)]
    FakeNative.states[str(target.home)]["orphan"] = True

    result = await sync_plugins(source, target, {})

    assert not result["issues"], result
    comandos = [call[2][:3] for call in FakeNative.calls
                if call[0] == str(target.home) and call[1] == "cli" and call[2][2] != "list"]
    assert comandos == [["plugin", "marketplace", "add"], ["plugin", "marketplace", "remove"],
                        ["plugin", "marketplace", "add"]]


async def test_marketplace_declarado_no_config_nunca_e_removido(contas, fake_native, monkeypatch):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(marketplace),
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
    }]
    source_state["plugins"] = [_entry(marketplace)]
    FakeNative.states[str(target.home)]["orphan"] = True
    (target.home / "config.toml").write_text(
        '[marketplaces.accounts-local]\nsource_type = "git"\nsource = "https://x/y"\n', encoding="utf-8")

    async def listagem_sem_ele(native):
        return []

    monkeypatch.setattr("app.codex_contas_plugins._marketplaces", listagem_sem_ele)

    result = await sync_plugins(source, target, {})

    assert any(issue["code"] == "codex_account_plugin_marketplace_add_failed" for issue in result["issues"])
    assert not any(call[2][:3] == ["plugin", "marketplace", "remove"] for call in FakeNative.calls)


async def test_plugin_remote_ja_instalado_nao_exige_marketplace_homonimo(contas, fake_native):
    _, source, target, _ = contas
    remote = {
        "pluginId": "plugin-management@openai-curated-remote",
        "name": "plugin-management",
        "marketplaceName": "openai-curated-remote",
        "version": "0.1.0",
        "installed": True,
        "enabled": True,
        "source": {"source": "remote", "id": "plugin_connector_1p_example"},
        "installPolicy": "INSTALLED_BY_DEFAULT",
        "authPolicy": "ON_USE",
    }
    FakeNative.states[str(source.home)]["marketplaces"] = [{
        "name": "openai-curated", "root": str(source.home / ".tmp/plugins"),
    }]
    FakeNative.states[str(source.home)]["plugins"] = [copy.deepcopy(remote)]
    FakeNative.states[str(target.home)]["marketplaces"] = [{
        "name": "openai-curated", "root": str(target.home / ".tmp/plugins"),
    }]
    FakeNative.states[str(target.home)]["plugins"] = [copy.deepcopy(remote)]

    result = await sync_plugins(source, target, {})

    assert not result["issues"], result
    assert result["manifest"]["plugins"][remote["pluginId"]]["origem"] == {
        "type": "remote", "source": "plugin_connector_1p_example",
    }
    assert not any(call[1] == "install" for call in FakeNative.calls)


async def test_marketplace_embutido_ausente_e_materializado_na_conta_destino(contas, fake_native):
    _, source, target, _ = contas
    source_root = source.home / ".tmp/bundled-marketplaces/openai-bundled"
    _json(source_root / ".agents/plugins/marketplace.json", {
        "name": "openai-bundled", "interface": {},
        "plugins": [
            {"name": "browser", "source": {"source": "local", "path": "./plugins/browser"}},
            {"name": "latex", "source": {"source": "local", "path": "./plugins/latex"}},
        ],
    })
    _plugin(source_root, "browser", "26.1.0")
    _plugin(source_root, "latex", "99.0.0")
    source_entry = {
        "pluginId": "browser@openai-bundled", "name": "browser",
        "marketplaceName": "openai-bundled", "version": "26.1.0",
        "installed": True, "enabled": True,
        "source": {"source": "local", "path": str(source_root / "plugins/browser")},
        "marketplaceSource": {"sourceType": "local", "source": str(source_root)},
        "installPolicy": "AVAILABLE", "authPolicy": "ON_INSTALL",
    }
    FakeNative.states[str(source.home)]["marketplaces"] = [{
        "name": "openai-bundled", "root": str(source_root),
        "marketplaceSource": {"sourceType": "local", "source": str(source_root)},
    }]
    FakeNative.states[str(source.home)]["plugins"] = [source_entry]

    result = await sync_plugins(source, target, {})

    target_root = target.home / ".tmp/bundled-marketplaces/openai-bundled"
    assert not result["issues"], result
    assert (target_root / ".agents/plugins/marketplace.json").is_file()
    assert (target_root / "plugins/browser/.codex-plugin/plugin.json").is_file()
    assert not (target_root / "plugins/latex").exists()
    # Lido como TOML: no Windows a barra invertida vem escapada no texto cru.
    config = tomllib.loads((target.home / "config.toml").read_text(encoding="utf-8"))
    assert config["marketplaces"]["openai-bundled"]["source"] == str(target_root)
    assert any(item["pluginId"] == "browser@openai-bundled"
               for item in FakeNative.states[str(target.home)]["plugins"])


async def test_origem_homonima_preserva_destino_e_registra_issue(contas, fake_native):
    _, source, target, marketplace = contas
    other = marketplace.parent / "other-marketplace"
    _plugin(other, "sample", "9.0.0")
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(marketplace),
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
    }]
    source_state["plugins"] = [_entry(marketplace)]
    target_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(other),
        "marketplaceSource": {"sourceType": "local", "source": str(other)},
    }]
    target_state["plugins"] = [{**_entry(other), "version": "9.0.0",
                                 "source": {"source": "local", "path": str(other / "plugins/sample")}}]

    result = await sync_plugins(source, target, {})

    assert any(issue["code"] == "codex_account_plugin_origin_conflict" for issue in result["issues"])
    assert target_state["plugins"][0]["version"] == "9.0.0"
    assert not any(call[1] == "install" for call in FakeNative.calls)


async def test_remocao_da_fonte_desabilita_apenas_plugin_gerenciado(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = []
    source_state["plugins"] = []
    target_state["plugins"] = [_entry(marketplace, enabled=True)]
    previous = {"plugins": {"sample@accounts-local": {
        "pluginId": "sample@accounts-local", "marketplace": "accounts-local",
        "version": "1.0.0", "enabled": True,
        # Forma que o manifesto grava: normcase minúscula no Windows.
        "origem": {"type": "local", "source": os.path.normcase(str(marketplace))},
    }}}

    result = await sync_plugins(source, target, previous)

    assert not result["issues"], result
    assert not target_state["plugins"][0]["enabled"]
    assert target_state["plugins"][0]["installed"] is True


async def test_falha_de_instalacao_preserva_manifesto_anterior(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(marketplace),
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
    }]
    source_state["plugins"] = [_entry(marketplace)]
    target_state["install_error"] = True
    previous = {"plugins": {"sample@accounts-local": {
        "pluginId": "sample@accounts-local", "version": "0.9.0",
        "enabled": True, "marketplace": "accounts-local",
    }}}

    result = await sync_plugins(source, target, previous)

    assert any(issue["code"] == "codex_account_plugin_install_failed" for issue in result["issues"])
    assert result["manifest"]["plugins"]["sample@accounts-local"]["version"] == "0.9.0"


async def test_segunda_rodada_e_idempotente_e_nao_compartilha_plugins_data(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(marketplace),
        "marketplaceSource": {"sourceType": "local", "source": str(marketplace)},
    }]
    source_state["plugins"] = [_entry(marketplace)]
    sentinel = target.home / "plugins/data/sentinel"
    sentinel.parent.mkdir(parents=True)
    sentinel.write_text("destino", encoding="utf-8")

    primeira = await sync_plugins(source, target, {})
    calls_after_first = [call for call in FakeNative.calls if call[1] == "install"]
    segunda = await sync_plugins(source, target, primeira["manifest"])

    assert not primeira["issues"], primeira
    assert not segunda["issues"], segunda
    assert len([call for call in FakeNative.calls if call[1] == "install"]) == len(calls_after_first) == 1
    assert sentinel.read_text(encoding="utf-8") == "destino"


async def test_fonte_de_plugin_invalida_preserva_destino(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = []
    source_state["plugins"] = [{**_entry(marketplace), "marketplaceSource": {
        "sourceType": "local",
    }}]
    target_state["plugins"] = [_entry(marketplace, version="0.9.0")]
    previous = {"plugins": {"sample@accounts-local": {"version": "0.9.0", "enabled": True}}}

    result = await sync_plugins(source, target, previous)

    assert result["issues"]
    assert result["manifest"]["plugins"]["sample@accounts-local"]["version"] == "0.9.0"
    assert target_state["plugins"][0]["version"] == "0.9.0"


async def test_marketplace_embutido_compara_identidade_e_nao_persiste_checkout(contas, fake_native):
    _, source, target, marketplace = contas
    source_root = marketplace / ".tmp/bundled-marketplaces/accounts-local"
    target_root = marketplace.parent / "target" / ".tmp/bundled-marketplaces/accounts-local"
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(source_root),
        "marketplaceSource": {"sourceType": "local", "source": str(source_root)},
    }]
    target_state["marketplaces"] = [{
        "name": "accounts-local", "root": str(target_root),
        "marketplaceSource": {"sourceType": "local", "source": str(target_root)},
    }]
    source_state["plugins"] = [_entry(source_root)]
    target_state["plugins"] = [_entry(target_root)]
    source_state["plugins"][0]["marketplaceSource"]["source"] = str(source_root)
    target_state["plugins"][0]["marketplaceSource"]["source"] = str(target_root)

    result = await sync_plugins(source, target, {})

    assert not result["issues"], result
    assert ".tmp/bundled-marketplaces" not in json.dumps(result["manifest"])
    source_state["plugins"] = []
    source_state["marketplaces"] = []
    removed = await sync_plugins(source, target, result["manifest"])
    assert not removed["issues"], removed
    assert target_state["plugins"][0]["enabled"] is False
    target_state["plugins"][0]["enabled"] = True
    legacy = {"plugins": {"sample@accounts-local": {
        "pluginId": "sample@accounts-local", "marketplace": "accounts-local",
        "version": "1.0.0", "enabled": True,
        "origem": {"type": "local", "source": str(source_root)},
    }}}
    migrated = await sync_plugins(source, target, legacy)
    assert not migrated["issues"], migrated
    assert target_state["plugins"][0]["enabled"] is False


async def test_versao_divergente_reexecuta_add_nativo_e_rele_inventario(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    market = {"name": "accounts-local", "root": str(marketplace),
              "marketplaceSource": {"sourceType": "local", "source": str(marketplace)}}
    source_state["marketplaces"] = [market]
    target_state["marketplaces"] = [market]
    source_state["plugins"] = [_entry(marketplace, version="1.0.0")]
    target_state["plugins"] = [_entry(marketplace, version="0.9.0")]

    result = await sync_plugins(source, target, {})

    assert not result["issues"], result
    assert target_state["plugins"][0]["version"] == "1.0.0"
    assert len([call for call in FakeNative.calls if call[1] == "install"]) == 1


@pytest.mark.parametrize("source_type", ["project","plugin"])
async def test_hooks_list_define_pendencia_atual_e_aprovacao_remove_pendencia(contas, fake_native, source_type):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    market = {"name": "accounts-local", "root": str(marketplace),
              "marketplaceSource": {"sourceType": "local", "source": str(marketplace)}}
    source_state["marketplaces"] = [market]
    target_state["marketplaces"] = [market]
    source_state["plugins"] = [_entry(marketplace)]
    target_state["plugins"] = [_entry(marketplace)]
    target_state["hooks"] = {"data": [{"hooks": [{
        "enabled": True, "trustStatus": "untrusted", "source":source_type,
        "key":"sample@accounts-local:stop:0:0", "currentHash":"sha256:sample",
    }]}]}

    pending = await sync_plugins(source, target, {})
    target_state["hooks"] = {"data": [{"hooks": [{
        "enabled": True, "trustStatus": "trusted", "source":source_type,
        "key":"sample@accounts-local:stop:0:0", "currentHash":"sha256:sample",
    }]}]}
    approved = await sync_plugins(source, target, pending["manifest"])

    assert pending["trust_pending"] is (source_type == "project")
    assert approved["trust_pending"] is False


async def test_remocao_nao_desabilita_id_com_origem_trocada(contas, fake_native):
    _, source, target, marketplace = contas
    other = marketplace.parent / "other"
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["plugins"] = []
    source_state["marketplaces"] = []
    target_state["plugins"] = [{**_entry(other), "enabled": True}]
    previous = {"plugins": {"sample@accounts-local": {
        "pluginId": "sample@accounts-local", "version": "1.0.0", "enabled": True,
        "origem": {"type": "local", "source": str(marketplace)},
    }}}

    result = await sync_plugins(source, target, previous)

    assert any(issue["code"] == "codex_account_plugin_origin_conflict" for issue in result["issues"])
    assert target_state["plugins"][0]["enabled"] is True


async def test_enabled_ausente_bloqueia_inventario_sem_reativar(contas, fake_native):
    _, source, target, marketplace = contas
    source_state = FakeNative.states[str(source.home)]
    target_state = FakeNative.states[str(target.home)]
    source_state["plugins"] = [{key: value for key, value in _entry(marketplace).items()
                                 if key != "enabled"}]
    target_state["plugins"] = [_entry(marketplace, enabled=False)]
    previous = {"plugins": {"sample@accounts-local": {
        "pluginId": "sample@accounts-local", "version": "1.0.0", "enabled": False,
        "origem": {"type": "local", "source": str(marketplace)},
    }}}

    result = await sync_plugins(source, target, previous)

    assert any(issue["code"] == "codex_account_plugin_inventory_failed" for issue in result["issues"])
    assert target_state["plugins"][0]["enabled"] is False


@pytest.mark.integration
@pytest.mark.skipif(
    os.environ.get("RUN_CODEX_INTEGRATION") != "1" or not shutil.which("codex"),
    reason="Exige RUN_CODEX_INTEGRATION=1 e Codex CLI; usa somente HOME temporária",
)
async def test_prova_real_instala_plugin_local_em_conta_alvo(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    source = Account("default", home / ".codex", True)
    target = Account("work", home / ".codex-work", False)
    source.home.mkdir()
    target.home.mkdir()
    marketplace = tmp_path / "marketplace"
    _json(marketplace / ".claude-plugin/marketplace.json", {
        "name": "accounts-local", "owner": {"name": "Hangar"},
        "plugins": [{"name": "sample", "source": "./plugins/sample"}],
    })
    _plugin(marketplace, "sample")
    async with CodexNativo(home, source.home) as native:
        await native.cli(["plugin", "marketplace", "add", str(marketplace), "--json"])
        await native.instalar_plugin("sample@accounts-local")

    result = await sync_plugins(source, target, {})

    assert not result["issues"], result
    assert result["trust_pending"] is True
    async with CodexNativo(home, target.home) as native:
        installed = await native.plugins_instalados()
    assert any(plugin["pluginId"] == "sample@accounts-local" for plugin in installed)


@pytest.mark.integration
@pytest.mark.skipif(
    os.environ.get("RUN_CODEX_INTEGRATION") != "1" or not shutil.which("codex"),
    reason="Exige RUN_CODEX_INTEGRATION=1 e Codex CLI; usa somente HOME temporária",
)
async def test_preparacao_de_conta_integra_plugins_sem_herdar_claude(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    for name in ("config", "data", "state", "cache"):
        monkeypatch.setenv(f"XDG_{name.upper()}_HOME", str(tmp_path / name))
    home = tmp_path
    source = Account("default", home / ".codex", True)
    target = Account("work", home / ".codex-work", False)
    source.home.mkdir()
    target.home.mkdir()
    marketplace = tmp_path / "marketplace"
    _json(marketplace / ".claude-plugin/marketplace.json", {
        "name": "accounts-local", "owner": {"name": "Hangar"},
        "plugins": [{"name": "sample", "source": "./plugins/sample"}],
    })
    _plugin(marketplace, "sample")
    async with CodexNativo(home, source.home) as native:
        await native.cli(["plugin", "marketplace", "add", str(marketplace), "--json"])
        await native.instalar_plugin("sample@accounts-local")

    from app.codex_contas_sync import prepare_account
    result = await prepare_account(target, force=True)

    assert result["status"] == "ready", result
    assert result["trust_pending"] is True
    async with CodexNativo(home, target.home) as native:
        installed = await native.plugins_instalados()
    assert any(plugin["pluginId"] == "sample@accounts-local" for plugin in installed)
