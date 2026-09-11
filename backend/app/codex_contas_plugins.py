"""Replicação seletiva de plugins pelo gerenciador nativo do Codex."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tomllib
from urllib.parse import urlsplit

from app.codex_arquivos import editar_config
from app.codex_compat import adaptar_hooks_plugin, normalizar_hooks
from app.codex_contas import Account
from app.codex_importador import CodexNativo, CodexNativoErro


_NATIVO = CodexNativo
_REPO = Path(__file__).resolve().parents[2]
_BUNDLED_MARKERS = (".tmp", "bundled-marketplaces")
_MARKETPLACE_NAME = re.compile(r"^[A-Za-z0-9._-]+$")


def _issue(code: str, **params) -> dict:
    return {"code": code, "params": {key: str(value) for key, value in params.items()}}


def _manifest(previous: dict | None) -> dict:
    previous = previous if isinstance(previous, dict) else {}
    value = previous.get("manifest", previous)
    if not isinstance(value, dict):
        return {"marketplaces": {}, "plugins": {}}
    result = {
        "marketplaces": copy.deepcopy(value.get("marketplaces", {}))
        if isinstance(value.get("marketplaces", {}), dict) else {},
        "plugins": copy.deepcopy(value.get("plugins", {}))
        if isinstance(value.get("plugins", {}), dict) else {},
    }
    for collection in (result["marketplaces"], result["plugins"]):
        for item in collection.values():
            if not isinstance(item, dict):
                continue
            source = item.get("marketplaceSource")
            if isinstance(source, dict) and _is_bundled_path(source.get("source")):
                item["marketplaceSource"] = {
                    "sourceType": "bundled",
                    "name": _bundled_name(source.get("source"), item),
                }
            origin = item.get("origem")
            if isinstance(origin, dict) and origin.get("type") == "local" and _is_bundled_path(origin.get("source")):
                item["origem"] = {
                    "type": "bundled",
                    "source": _bundled_name(origin.get("source"), item),
                }
            if _is_bundled_path(item.get("root")):
                item.pop("root", None)
    return result


def _is_bundled_path(value: object) -> bool:
    if not isinstance(value, str):
        return False
    try:
        parts = Path(value).parts
    except (OSError, ValueError):
        return False
    return all(marker in parts for marker in _BUNDLED_MARKERS)


def _bundled_name(value: object, item: dict | None = None) -> str | None:
    if isinstance(item, dict):
        name = item.get("marketplace") or item.get("marketplaceName") or item.get("name")
        if isinstance(name, str) and name:
            return name
    if isinstance(value, str):
        parts = Path(value).parts
        try:
            index = parts.index("bundled-marketplaces")
        except ValueError:
            return None
        if index + 1 < len(parts):
            return parts[index + 1]
    return None


def _source_of(value: dict | None) -> tuple[str, str] | None:
    source = value.get("marketplaceSource") if isinstance(value, dict) else None
    if not isinstance(source, dict):
        source = value if isinstance(value, dict) else None
    if not isinstance(source, dict):
        return None
    kind = source.get("sourceType", source.get("source_type"))
    if kind == "bundled":
        name = source.get("name")
        if not isinstance(name, str) and isinstance(value, dict):
            name = value.get("marketplaceName") or value.get("name")
        return ("bundled", name) if isinstance(name, str) and name else None
    origin = source.get("source")
    if not isinstance(kind, str) or not isinstance(origin, str) or not origin:
        return None
    if _is_bundled_path(origin):
        name = value.get("marketplaceName") or value.get("name") if isinstance(value, dict) else None
        return "bundled", str(name or "")
    if kind in {"local", "directory"}:
        try:
            return "local", os.path.normcase(str(Path(origin).expanduser().resolve(strict=False)))
        except (OSError, RuntimeError):
            return None
    if kind in {"git", "github"}:
        if origin.startswith("git@") and ":" in origin:
            host, repo = origin[4:].split(":", 1)
        else:
            parsed = urlsplit(origin)
            host, repo = parsed.hostname, parsed.path.lstrip("/")
        if host and repo:
            return "git", f"{host.lower()}/{repo.rstrip('/').removesuffix('.git')}"
    return kind, origin


def _plugin_origin(plugin: dict | None) -> tuple[str, str] | None:
    origin = _source_of(plugin)
    if origin is not None or not isinstance(plugin, dict):
        return origin
    source = plugin.get("source")
    if isinstance(source, dict) and source.get("source") == "remote" \
            and isinstance(source.get("id"), str) and source["id"]:
        return "remote", source["id"]
    return None


def _private_dir(account: Account) -> Path:
    key = hashlib.sha256(str(account.home.expanduser().resolve()).encode()).hexdigest()
    return Path.home() / ".hangar" / "codex-contas" / key


def _config(account: Account) -> dict:
    path = account.home / "config.toml"
    if not path.is_file():
        return {}
    with path.open("rb") as file:
        data = tomllib.load(file)
    return data if isinstance(data, dict) else {}


async def _marketplaces(native) -> list[dict]:
    method = getattr(native, "marketplaces_instalados", None)
    if method is not None:
        result = await method()
    else:
        result = await native.cli(["plugin", "marketplace", "list", "--json"])
        result = result.get("marketplaces")
    if not isinstance(result, list) or any(not isinstance(item, dict) for item in result):
        raise CodexNativoErro("O Codex retornou um inventário de marketplaces inválido.")
    return result


def _plugins(items: list[dict]) -> dict[str, dict]:
    result = {}
    for item in items:
        plugin_id = item.get("pluginId")
        if not isinstance(plugin_id, str) or not plugin_id or "@" not in plugin_id:
            raise ValueError("inventário de plugin sem pluginId válido")
        if not isinstance(item.get("version"), str) or not item["version"]:
            raise ValueError("inventário de plugin sem versão válida")
        if type(item.get("enabled")) is not bool:
            raise ValueError("inventário de plugin sem habilitação válida")
        if plugin_id in result:
            raise ValueError("inventário de plugin ambíguo")
        result[plugin_id] = item
    return result


def _marketplace_map(items: list[dict]) -> dict[str, dict]:
    result = {}
    for item in items:
        name = item.get("name")
        if not isinstance(name, str) or not name:
            raise ValueError("inventário de marketplace sem nome válido")
        if name in result:
            raise ValueError("inventário de marketplace ambíguo")
        result[name] = item
    return result


def _marketplace_name(plugin_id: str, plugin: dict) -> str:
    value = plugin.get("marketplaceName")
    return value if isinstance(value, str) and value else plugin_id.rsplit("@", 1)[1]


def _plugin_name(plugin_id: str, plugin: dict) -> str:
    value = plugin.get("name")
    return value if isinstance(value, str) and value else plugin_id.split("@", 1)[0]


def _cache_path(account: Account, plugin: dict) -> Path:
    source = plugin.get("source")
    if isinstance(source, dict) and isinstance(source.get("path"), str):
        path = Path(source["path"])
        cache = account.home / "plugins" / "cache"
        try:
            if path.resolve(strict=False).is_relative_to(cache.resolve(strict=False)):
                return path
        except (OSError, RuntimeError):
            pass
    name = _plugin_name(str(plugin.get("pluginId", "plugin@unknown")), plugin)
    marketplace = _marketplace_name(str(plugin.get("pluginId", "plugin@unknown")), plugin)
    version = plugin.get("version") if isinstance(plugin.get("version"), str) else ""
    return account.home / "plugins" / "cache" / marketplace / name / version


def _bundled(entry: dict) -> bool:
    values = [entry.get("root")]
    source = entry.get("marketplaceSource")
    if isinstance(source, dict):
        values.append(source.get("source"))
    return any(isinstance(value, str) and all(marker in Path(value).parts for marker in _BUNDLED_MARKERS)
               for value in values)


def _marketplace_manifest(entry: dict) -> dict:
    name = entry.get("name")
    if _bundled(entry):
        return {"name": name, "marketplaceSource": {"sourceType": "bundled", "name": name}}
    source = entry.get("marketplaceSource")
    return {
        "name": name,
        "marketplaceSource": copy.deepcopy(source) if isinstance(source, dict) else {},
        "root": entry.get("root") if isinstance(entry.get("root"), str) else None,
    }


async def _set_bundled_marketplace(account: Account, name: str, root: Path,
                                   issues: list[dict]) -> bool:
    path = account.home / "config.toml"

    def preparar(current: dict):
        marketplaces = current.get("marketplaces", {})
        if not isinstance(marketplaces, dict):
            raise ValueError("marketplaces do config.toml são inválidos")
        desired = {"source_type": "local", "source": str(root)}
        actual = marketplaces.get(name)
        if actual == desired:
            return [], lambda: None
        prefix = ".".join(json.dumps(part, ensure_ascii=False) for part in ("marketplaces", name))
        edits = [{"keyPath": f'{prefix}.{json.dumps(key)}', "value": value,
                  "mergeStrategy": "replace"} for key, value in desired.items()]
        return edits, lambda: None

    try:
        await editar_config(path, _private_dir(account) / "backups", _private_dir(account),
                            _NATIVO, preparar)
        return True
    except (OSError, ValueError, RuntimeError, CodexNativoErro) as exc:
        issues.append(_issue("codex_account_plugin_config_failed", error=type(exc).__name__))
        return False


async def _materialize_bundled_marketplace(entry: dict, plugin: dict, source: Account,
                                           target: Account, issues: list[dict]) -> Path | None:
    name = entry.get("name")
    raw = entry.get("root")
    if not isinstance(raw, str):
        marketplace_source = entry.get("marketplaceSource")
        raw = marketplace_source.get("source") if isinstance(marketplace_source, dict) else None
    if not isinstance(name, str) or not _MARKETPLACE_NAME.fullmatch(name) or not isinstance(raw, str):
        issues.append(_issue("codex_account_plugin_marketplace_invalid", marketplace=name or "?"))
        return None
    try:
        source_root = Path(raw).resolve(strict=True)
        allowed = (source.home / ".tmp/bundled-marketplaces").resolve(strict=True)
        plugin_name = _plugin_name(str(plugin.get("pluginId", "plugin@unknown")), plugin)
        source_manifest = source_root / ".agents/plugins/marketplace.json"
        source_plugin = source_root / "plugins" / plugin_name
        if not source_root.is_dir() or not source_root.is_relative_to(allowed) \
                or not source_manifest.is_file() or not source_plugin.is_dir() \
                or any(path.is_symlink() for root in (source_manifest, source_plugin)
                       for path in ((root,) if root.is_file() else root.rglob("*"))):
            raise ValueError
        target_root = target.home / ".tmp/bundled-marketplaces" / name
        target_manifest = target_root / ".agents/plugins/marketplace.json"
        target_manifest.parent.mkdir(parents=True, exist_ok=True)
        if not target_manifest.is_file() or target_manifest.read_bytes() != source_manifest.read_bytes():
            shutil.copy2(source_manifest, target_manifest)
        target_plugin = target_root / "plugins" / plugin_name
        source_version = source_plugin / ".codex-plugin/plugin.json"
        target_version = target_plugin / ".codex-plugin/plugin.json"
        if not target_version.is_file() or target_version.read_bytes() != source_version.read_bytes():
            shutil.copytree(source_plugin, target_plugin, dirs_exist_ok=True)
    except (OSError, RuntimeError, ValueError):
        issues.append(_issue("codex_account_plugin_marketplace_unavailable", marketplace=name))
        return None
    if not await _set_bundled_marketplace(target, name, target_root, issues):
        return None
    return target_root


async def _ensure_marketplace(native, entry: dict, plugin: dict,
                              target_plugin: dict | None,
                              source_account: Account, target_account: Account,
                              target: dict[str, dict], source_config: dict,
                              issues: list[dict]) -> bool:
    name = entry.get("name")
    origin = _source_of(entry)
    if not isinstance(name, str) or origin is None:
        issues.append(_issue("codex_account_plugin_marketplace_invalid", marketplace=name or "?"))
        return False
    current = target.get(name)
    if _bundled(entry):
        if current is not None and _source_of(current) == origin and target_plugin is not None \
                and target_plugin.get("version") == plugin.get("version"):
            return True
        if await _materialize_bundled_marketplace(
                entry, plugin, source_account, target_account, issues) is None:
            return False
        if current is not None:
            if _source_of(current) != origin:
                issues.append(_issue("codex_account_plugin_origin_conflict", marketplace=name))
                return False
            return True
        try:
            refreshed = _marketplace_map(await _marketplaces(native))
        except (OSError, ValueError, RuntimeError, CodexNativoErro):
            issues.append(_issue("codex_account_plugin_marketplace_unavailable", marketplace=name))
            return False
        target.update(refreshed)
        current = target.get(name)
        if current is None or _source_of(current) != origin:
            issues.append(_issue("codex_account_plugin_marketplace_unavailable", marketplace=name))
            return False
        return True
    if current is not None:
        if _source_of(current) != origin:
            issues.append(_issue("codex_account_plugin_origin_conflict", marketplace=name))
            return False
        return True
    source = entry.get("marketplaceSource", {}).get("source")
    if not isinstance(source, str) or not source:
        issues.append(_issue("codex_account_plugin_marketplace_unavailable", marketplace=name))
        return False
    if origin[0] == "local" and not Path(source).is_dir():
        issues.append(_issue("codex_account_plugin_marketplace_unavailable", marketplace=name))
        return False
    args = ["plugin", "marketplace", "add", source]
    configured_markets = source_config.get("marketplaces", {}) if isinstance(source_config, dict) else {}
    configured = configured_markets.get(name, {}) if isinstance(configured_markets, dict) else {}
    sparse = configured.get("sparse_paths", []) if isinstance(configured, dict) else []
    if isinstance(sparse, list):
        for path in sparse:
            if isinstance(path, str) and path:
                args.extend(["--sparse", path])
    args.append("--json")
    try:
        await native.cli(args)
        current = _marketplace_map(await _marketplaces(native))
    except (OSError, ValueError, RuntimeError, CodexNativoErro):
        issues.append(_issue("codex_account_plugin_marketplace_add_failed", marketplace=name))
        return False
    if name not in current or _source_of(current[name]) != origin:
        issues.append(_issue("codex_account_plugin_marketplace_unavailable", marketplace=name))
        return False
    target.update(current)
    return True


async def _set_enabled(account: Account, choices: dict[str, bool], issues: list[dict]) -> None:
    if not choices:
        return
    path = account.home / "config.toml"
    private = _private_dir(account)
    backups = private / "backups"

    def preparar(current: dict):
        plugins = current.get("plugins", {})
        if not isinstance(plugins, dict):
            raise ValueError("plugins do config.toml são inválidos")
        edits = []
        for plugin_id, enabled in sorted(choices.items()):
            atual = plugins.get(plugin_id, {}).get("enabled") if isinstance(plugins.get(plugin_id), dict) else None
            if atual != enabled:
                key = ".".join(json.dumps(part, ensure_ascii=False)
                               for part in ("plugins", plugin_id, "enabled"))
                edits.append({"keyPath": key, "value": enabled, "mergeStrategy": "replace"})
        return edits, lambda: None

    try:
        await editar_config(path, backups, private, _NATIVO, preparar)
    except (OSError, ValueError, RuntimeError, CodexNativoErro) as exc:
        issues.append(_issue("codex_account_plugin_config_failed", error=type(exc).__name__))


def _entry(plugin: dict, source: Account, target: Account, target_item: dict | None,
           install: dict | None, origin: tuple[str, str]) -> dict:
    plugin_id = plugin["pluginId"]
    target_path = _cache_path(target, target_item or {}) if target_item else None
    if isinstance(install, dict) and isinstance(install.get("installedPath"), str):
        target_path = Path(install["installedPath"])
    source_path = _cache_path(source, plugin)
    source_info = plugin.get("marketplaceSource", {})
    if origin[0] == "bundled":
        source_info = {"sourceType": "bundled", "name": _marketplace_name(plugin_id, plugin)}
    return {
        "pluginId": plugin_id,
        "version": plugin.get("version"),
        "enabled": plugin["enabled"],
        "marketplace": _marketplace_name(plugin_id, plugin),
        "marketplaceSource": copy.deepcopy(source_info),
        "origem": {"type": origin[0], "source": origin[1]},
        "cache_fonte": str(source_path),
        "cache_destino": str(target_path) if target_path else "",
    }


async def _adapt_hooks(target: Account, item: dict, issues: list[dict]) -> bool:
    root = _cache_path(target, item)
    if not root.is_dir():
        return False
    changed = []
    try:
        adaptar_hooks_plugin(
            root,
            target.home,
            _private_dir(target) / "backups",
            normalizar=lambda data: normalizar_hooks(data, sys.executable,
                                                      _REPO / "scripts/codex-hook-allow.py",
                                                      windows=os.name == "nt"),
            ao_alterar=lambda: changed.append(True),
        )
    except (OSError, ValueError, RuntimeError) as exc:
        issues.append(_issue("codex_account_plugin_hooks_failed", error=type(exc).__name__))
    return bool(changed)


def _manifest_origin(item: dict | None) -> tuple[str, str] | None:
    if not isinstance(item, dict):
        return None
    origin = item.get("origem")
    if (isinstance(origin, dict) and origin.get("type") == "local"
            and _is_bundled_path(origin.get("source"))):
        name = _bundled_name(origin.get("source"), item)
        return ("bundled", name) if name else None
    if isinstance(origin, dict) and isinstance(origin.get("type"), str) and isinstance(origin.get("source"), str):
        return origin["type"], origin["source"]
    return _source_of(item)


async def _trust_state(native) -> bool | None:
    if not hasattr(native, "request") or not hasattr(native, "__aenter__"):
        return None
    try:
        async with native:
            result = await native.request("hooks/list", {"cwds": []})
    except (OSError, ValueError, RuntimeError, CodexNativoErro):
        return None
    entries = result.get("data") if isinstance(result, dict) else None
    if not isinstance(entries, list) or any(
        not isinstance(entry, dict) or not isinstance(entry.get("hooks", []), list)
        or any(not isinstance(hook, dict) for hook in entry.get("hooks", []))
        for entry in entries
    ):
        return None
    return any(
        hook.get("enabled") is True and hook.get("trustStatus") in {"untrusted", "modified"}
        for entry in entries for hook in entry.get("hooks", [])
    )


async def sync_plugins(source: Account, target: Account, previous: dict) -> dict:
    """Adota no destino apenas os plugins instalados na conta padrão."""
    manifest = _manifest(previous)
    issues: list[dict] = []
    trust_pending = False
    if source.home.resolve(strict=False) == target.home.resolve(strict=False):
        return {"manifest": manifest, "issues": [_issue("codex_account_source_destination_conflict")],
                "trust_pending": False}
    try:
        source_native = _NATIVO(Path.home(), source.home, account=source)
        target_native = _NATIVO(Path.home(), target.home, account=target)
        source_items = _plugins(await source_native.plugins_instalados())
        target_items = _plugins(await target_native.plugins_instalados())
        source_markets = _marketplace_map(await _marketplaces(source_native))
        target_markets = _marketplace_map(await _marketplaces(target_native))
        source_config = _config(source)
    except (OSError, ValueError, RuntimeError, CodexNativoErro) as exc:
        issues.append(_issue("codex_account_plugin_inventory_failed", error=type(exc).__name__))
        return {"manifest": manifest, "issues": issues, "trust_pending": None}

    desired: dict[str, bool] = {}
    managed: dict[str, dict] = {}
    installs: dict[str, dict] = {}
    previous_plugins = manifest["plugins"]

    def preserve(plugin_id: str) -> None:
        if plugin_id in previous_plugins:
            managed[plugin_id] = copy.deepcopy(previous_plugins[plugin_id])

    for plugin_id, plugin in sorted(source_items.items()):
        target_item = target_items.get(plugin_id)
        plugin_origin = _plugin_origin(plugin)
        if plugin_origin is not None and plugin_origin[0] == "remote" and target_item is not None:
            if _plugin_origin(target_item) != plugin_origin:
                issues.append(_issue("codex_account_plugin_origin_conflict", plugin=plugin_id))
                preserve(plugin_id)
                continue
            if target_item.get("version") != plugin.get("version"):
                issues.append(_issue("codex_account_plugin_version_conflict", plugin=plugin_id))
                preserve(plugin_id)
                continue
            desired[plugin_id] = plugin["enabled"]
            continue
        marketplace_name = _marketplace_name(plugin_id, plugin)
        marketplace = source_markets.get(marketplace_name)
        if marketplace is None and isinstance(plugin.get("marketplaceSource"), dict):
            marketplace = {"name": marketplace_name, "marketplaceSource": plugin["marketplaceSource"]}
        if marketplace is None:
            issues.append(_issue("codex_account_plugin_marketplace_missing", plugin=plugin_id))
            preserve(plugin_id)
            continue
        marketplace_origin = _source_of(marketplace)
        plugin_origin = _source_of(plugin)
        if marketplace_origin is None or (plugin_origin is not None and plugin_origin != marketplace_origin):
            issues.append(_issue("codex_account_plugin_origin_conflict", plugin=plugin_id))
            preserve(plugin_id)
            continue
        if not await _ensure_marketplace(target_native, marketplace, plugin, target_item,
                                         source, target,
                                         target_markets, source_config, issues):
            preserve(plugin_id)
            continue
        origin = marketplace_origin
        target_item = target_items.get(plugin_id)
        if target_item is not None:
            target_origin = _source_of(target_item)
            if target_origin is None:
                issues.append(_issue("codex_account_plugin_origin_unknown", plugin=plugin_id))
                preserve(plugin_id)
                continue
            if target_origin != origin:
                issues.append(_issue("codex_account_plugin_origin_conflict", plugin=plugin_id))
                preserve(plugin_id)
                continue
        install = None
        source_version = plugin.get("version")
        if target_item is None:
            try:
                install = await target_native.instalar_plugin(plugin_id)
            except (OSError, ValueError, RuntimeError, CodexNativoErro) as exc:
                issues.append(_issue("codex_account_plugin_install_failed", plugin=plugin_id,
                                     error=type(exc).__name__))
                preserve(plugin_id)
                continue
        elif target_item.get("version") != source_version:
            try:
                install = await target_native.instalar_plugin(plugin_id)
            except (OSError, ValueError, RuntimeError, CodexNativoErro) as exc:
                issues.append(_issue("codex_account_plugin_update_failed", plugin=plugin_id,
                                     error=type(exc).__name__))
                preserve(plugin_id)
                continue
        if install is not None:
            installs[plugin_id] = install
        desired[plugin_id] = plugin["enabled"]

    try:
        target_items = _plugins(await target_native.plugins_instalados())
    except (OSError, ValueError, RuntimeError, CodexNativoErro) as exc:
        issues.append(_issue("codex_account_plugin_inventory_failed", error=type(exc).__name__))
        target_items = {}
    for plugin_id in sorted(desired):
        plugin = source_items[plugin_id]
        item = target_items.get(plugin_id)
        if item is None:
            issues.append(_issue("codex_account_plugin_not_installed", plugin=plugin_id))
            continue
        if plugin.get("version") is not None and item.get("version") != plugin.get("version"):
            issues.append(_issue("codex_account_plugin_version_conflict", plugin=plugin_id))
            continue
        origin = _plugin_origin(plugin) or _source_of(
            source_markets.get(_marketplace_name(plugin_id, plugin)))
        if origin is None:
            issues.append(_issue("codex_account_plugin_origin_unknown", plugin=plugin_id))
            preserve(plugin_id)
            continue
        managed[plugin_id] = _entry(plugin, source, target, item, installs.get(plugin_id), origin)
        await _adapt_hooks(target, item, issues)

    removed = set(previous_plugins) - set(source_items)
    for plugin_id in sorted(removed):
        target_item = target_items.get(plugin_id)
        current_origin = _source_of(target_item)
        previous_origin = _manifest_origin(previous_plugins.get(plugin_id))
        if target_item is not None and current_origin is not None and previous_origin == current_origin:
            desired[plugin_id] = False
        elif target_item is not None:
            issues.append(_issue("codex_account_plugin_origin_conflict", plugin=plugin_id))
            preserve(plugin_id)
    await _set_enabled(target, desired, issues)
    try:
        target_items = _plugins(await target_native.plugins_instalados())
    except (OSError, ValueError, RuntimeError, CodexNativoErro):
        pass
    for plugin_id, entry in managed.items():
        if plugin_id in target_items and isinstance(target_items[plugin_id].get("enabled"), bool):
            entry["enabled"] = target_items[plugin_id]["enabled"]
    current_trust = await _trust_state(target_native)
    if current_trust is not None:
        trust_pending = current_trust
    elif hasattr(target_native, "request"):
        issues.append(_issue("codex_account_plugin_trust_unavailable"))
        trust_pending = None
    markets = {
        name: _marketplace_manifest(entry)
        for name, entry in source_markets.items()
        if any(_marketplace_name(plugin_id, plugin) == name for plugin_id, plugin in source_items.items())
    }
    return {"manifest": {"marketplaces": markets, "plugins": managed},
            "issues": issues, "trust_pending": trust_pending}
