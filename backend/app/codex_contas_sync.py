"""Herança seletiva do Codex padrão para contas adicionais."""

from __future__ import annotations

import copy
import hashlib
import json
import logging
import os
import re
import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path

from app import atomico
from app.codex_arquivos import (
    AlteradoExternamente,
    backup,
    editar_config,
    exclusivo,
    gravar,
    hash_bytes,
    json_bytes,
    ler,
)
from app.codex_contas import Account, default_home
from app.codex_importador import CodexNativo


_log = logging.getLogger("hangar.codex.contas_sync")

PREFERENCE_KEYS = frozenset({
    "model",
    "model_reasoning_effort",
    "model_reasoning_summary",
    "model_verbosity",
    "model_context_window",
    "model_auto_compact_token_limit",
    "model_catalog_json",
    "personality",
    "features",
    "tui",
    "desktop",
    "project_doc_fallback_filenames",
    "project_doc_max_bytes",
    "model_instructions_file",
    "agents",
    "mcp_servers",
    "shell_environment_policy",
    "model_providers",
    "approval_policy",
    "approvals_reviewer",
    "sandbox_mode",
    "sandbox_workspace_write",
    "permissions",
    "auto_review",
    "web_search",
    "notify",
    "otel",
    "analytics",
})

_IGNORED_KEYS = frozenset({
    "cli_auth_credentials_store",
    "projects",
    "notice",
    "history",
    "sqlite_home",
    "auth",
    "auth_file",
    "credentials",
})
_PROVIDER_KEYS = frozenset({"model_provider"})
_PROVIDER_ENV = frozenset({
    "OPENAI_BASE_URL", "OPENAI_API_BASE", "OPENAI_ORGANIZATION", "OPENAI_ORG_ID",
    "OPENAI_PROJECT", "OPENAI_PROJECT_ID", "OPENAI_ENDPOINT",
})
_RUNTIME_ENV = frozenset({
    "CODEX_HOME", "CODEX_CONFIG_HOME", "CODEX_SQLITE_HOME", "HOME", "USERPROFILE",
    "XDG_CONFIG_HOME", "XDG_DATA_HOME",
    "XDG_STATE_HOME", "XDG_CACHE_HOME",
})
_RESTRICTION_KEYS = ("forced_login_method", "forced_chatgpt_workspace_id")
_RESOURCE_DIRS = ("agents", "skills", "hooks", ".hangar-hooks")
_RESOURCE_FILES = ("AGENTS.md", "AGENTS.override.md", "hooks.json")
_PATH_KEYS = frozenset({
    "config_file", "model_instructions_file", "model_catalog_json", "cwd",
    "working_directory", "path", "file", "script", "program",
})
_SENSITIVE_RESOURCE_NAMES = frozenset({
    "auth.json", ".credentials.json", ".claude.json", "projects", "sessions",
    "archived_sessions", "agent.db", "models.db", "state.sqlite", "config.sqlite",
})
_AUTH_ENV = re.compile(
    r"^(?:OPENAI|CODEX|CHATGPT)_.+(?:(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)(?:_FILE)?|ACCOUNT_ID|ORG_ID|PROJECT_ID)$",
    re.IGNORECASE,
)
_INFORMATIONAL_ISSUES = frozenset({
    "codex_account_mcp_runtime_excluded",
    "codex_account_mcp_auth_excluded",
})
_NATIVO = CodexNativo
_UNMAPPED = object()


class _PreparationChanged(AlteradoExternamente):
    """A fonte ou o destino mudou antes da assinatura poder ser confirmada."""


def _issue(code: str, **params) -> dict:
    return {"code": code, "params": {k: str(v) for k, v in params.items()}}


def _cli_version() -> str:
    binary = shutil.which("codex")
    if not binary:
        return "indisponível"
    try:
        result = subprocess.run([binary, "--version"], capture_output=True, text=True,
                                timeout=2, check=False)
    except (OSError, subprocess.SubprocessError):
        return "indisponível"
    lines = (result.stdout or "").strip().splitlines()
    return lines[0][:80] if result.returncode == 0 and lines else "indisponível"


_ETAPAS = ("configuracoes", "recursos", "plugins")


def _status(status: str = "idle", *, issues: list[dict] | None = None,
            trust_pending: bool = False, etapa: str | None = None,
            herdado: dict | None = None) -> dict:
    return {"status": status, "trust_pending": bool(trust_pending),
            "issues": copy.deepcopy(issues or []), "etapa": etapa,
            "herdado": dict(herdado) if herdado else None}


def _contar_herdado(resources: dict, plugins: dict, config: dict) -> dict:
    """O que a conta recebeu, para a tela poder dizer em vez de só "concluído"."""
    def sob(prefixo: str) -> int:
        return sum(1 for caminho in resources if str(caminho).startswith(prefixo))

    valores = config.get("values") if isinstance(config.get("values"), dict) else {}
    mcps = valores.get("mcp_servers") if isinstance(valores.get("mcp_servers"), dict) else {}
    instalados = plugins.get("plugins") if isinstance(plugins.get("plugins"), dict) else {}
    hooks = sob("hooks/") + sob(".hangar-hooks/") + (1 if "hooks.json" in resources else 0)
    return {"skills": sob("skills/"), "hooks": hooks, "agents": sob("agents/"),
            "plugins": len(instalados), "mcps": len(mcps)}


def _has_blocking_issues(issues: list[dict]) -> bool:
    return any(
        not isinstance(issue, dict) or issue.get("code") not in _INFORMATIONAL_ISSUES
        for issue in issues
    )


def _canonical(path: Path) -> Path:
    return path.expanduser().resolve(strict=False)


def _state_dir(account: Account, *, create: bool = False) -> Path:
    return _private_dir(account.home, create=create)


def _state_path(account: Account) -> Path:
    return _state_dir(account) / "estado.json"


def _read_state(account: Account) -> dict:
    path = _state_path(account)
    if path.is_symlink():
        raise ValueError("estado da conta Codex é um link")
    raw = ler(path)
    if raw is None:
        return {}
    data = json.loads(raw)
    if (not isinstance(data, dict) or not isinstance(data.get("public"), dict) or
            not isinstance(data["public"].get("status"), str)):
        raise ValueError("estado de conta Codex inválido")
    for key in ("config", "profiles", "resources", "source_snapshot", "destination_snapshot"):
        if key in data and not isinstance(data[key], dict):
            raise ValueError("estado de conta Codex inválido")
    return data


def _write_state(account: Account, state: dict) -> None:
    directory = _state_dir(account, create=True)
    if directory.is_symlink():
        raise ValueError("diretório privado da conta Codex é um link")
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(directory, 0o700)
    path = directory / "estado.json"
    if path.is_symlink():
        raise ValueError("estado da conta Codex é um link")
    gravar(path, json_bytes(state), ler(path))
    os.chmod(path, 0o600)


def _public_state(state: dict) -> dict:
    value = state.get("public") if isinstance(state.get("public"), dict) else state
    status = value.get("status", "idle")
    if status not in {"idle", "running", "ready", "partial", "error"}:
        status = "error"
    issues = value.get("issues", [])
    if not isinstance(issues, list):
        issues = [_issue("codex_account_state_invalid")]
    clean = []
    for issue in issues:
        if not isinstance(issue, dict) or not isinstance(issue.get("code"), str):
            clean.append(_issue("codex_account_state_invalid"))
            continue
        params = issue.get("params", {})
        params = ({key: str(value) for key, value in params.items() if isinstance(key, str)}
                  if isinstance(params, dict) else {})
        clean.append({"code": issue["code"], "params": params})
    etapa = value.get("etapa")
    herdado = value.get("herdado")
    herdado = ({chave: int(valor) for chave, valor in herdado.items()
                if isinstance(chave, str) and isinstance(valor, int)}
               if isinstance(herdado, dict) else None)
    return _status(status, issues=clean, trust_pending=value.get("trust_pending", False),
                   etapa=etapa if etapa in _ETAPAS else None, herdado=herdado)


def preparation_status(account: Account) -> dict:
    """Lê o estado persistido sem iniciar uma preparação."""
    if account.is_default:
        return _status("ready")
    if account.home.is_symlink() or not account.home.is_dir():
        return _status("error", issues=[_issue("codex_account_destination_invalid")])
    try:
        return _public_state(_read_state(account))
    except (OSError, ValueError, json.JSONDecodeError):
        return _status("error", issues=[_issue("codex_account_state_invalid")])


def _filter_environment(value):
    value = copy.deepcopy(value)
    if not isinstance(value, dict):
        return value
    variables = value.get("set")
    if isinstance(variables, dict):
        value["set"] = {k: v for k, v in variables.items()
                         if not (isinstance(k, str) and
                                 (_AUTH_ENV.fullmatch(k) or k in _PROVIDER_ENV or k in _RUNTIME_ENV))}
    return value


def _mcp_secret_name(name: object) -> bool:
    if not isinstance(name, str):
        return True
    upper = name.upper()
    return (
        _AUTH_ENV.fullmatch(upper) is not None
        or upper in _PROVIDER_ENV
        or upper in _RUNTIME_ENV
        or re.search(r"(?:API[_-]?KEY|ACCESS[_-]?TOKEN|REFRESH[_-]?TOKEN|AUTH(?:ORIZATION)?|SECRET|PASSWORD|PRIVATE[_-]?KEY)$", upper) is not None
    )


def _filter_mcp_servers(value, issues: list[dict]):
    if not isinstance(value, dict):
        return copy.deepcopy(value)
    result = copy.deepcopy(value)
    for server, config in result.items():
        if not isinstance(config, dict) or not isinstance(config.get("env"), dict):
            continue
        env = {}
        for name, item in config["env"].items():
            if _mcp_secret_name(name):
                code = "codex_account_mcp_runtime_excluded" if name in _RUNTIME_ENV else "codex_account_mcp_auth_excluded"
                issues.append(_issue(code, server=server, variable=name))
                continue
            env[name] = item
        config["env"] = env
    return result


def _filter_provider_definitions(value, issues: list[dict]):
    if not isinstance(value, dict):
        return copy.deepcopy(value)
    result = copy.deepcopy(value)
    for provider, config in result.items():
        if not isinstance(config, dict):
            continue
        for key in tuple(config):
            if key.lower() in {"api_key", "access_token", "refresh_token", "password", "secret"}:
                issues.append(_issue("codex_account_provider_auth_excluded", provider=provider, key=key))
                config.pop(key, None)
    return result


def _project_preferences(source: dict) -> tuple[dict, list[dict]]:
    if not isinstance(source, dict):
        raise ValueError("config.toml da conta padrão não é um objeto")
    projected = {}
    issues = []
    for key, value in source.items():
        if key in _IGNORED_KEYS or key in _RESTRICTION_KEYS:
            continue
        if key in {"marketplaces", "plugins"}:
            # A reconciliação nativa de plugins cuida dessas tabelas separadamente.
            continue
        if key in _PROVIDER_KEYS:
            issues.append(_issue("codex_account_provider_divergence", key=key))
            continue
        if key == "hooks":
            if not isinstance(value, dict):
                issues.append(_issue("codex_account_preference_invalid", key=key))
                continue
            definitions = {k: copy.deepcopy(v) for k, v in value.items() if k != "state"}
            if definitions:
                projected[key] = definitions
            continue
        if key not in PREFERENCE_KEYS:
            issues.append(_issue("codex_account_unknown_preference", key=key))
            continue
        if key == "shell_environment_policy":
            variables = value.get("set") if isinstance(value, dict) else None
            if isinstance(variables, dict):
                for variable in variables:
                    if variable in _PROVIDER_ENV:
                        issues.append(_issue("codex_account_provider_divergence", key=variable))
            projected[key] = _filter_environment(value)
        elif key == "mcp_servers":
            projected[key] = _filter_mcp_servers(value, issues)
        elif key == "model_providers":
            projected[key] = _filter_provider_definitions(value, issues)
        else:
            projected[key] = copy.deepcopy(value)
    projected["cli_auth_credentials_store"] = "file"
    return projected, issues


def project_preferences(source: dict) -> dict:
    """Seleciona preferências públicas, removendo identidade e estado privado."""
    return _project_preferences(source)[0]


def _safe_relative(path: str) -> bool:
    candidate = Path(path)
    return not candidate.is_absolute() and ".." not in candidate.parts and path not in ("", ".")


def _forbidden_resource(relative: Path) -> bool:
    for part in relative.parts:
        if part in _SENSITIVE_RESOURCE_NAMES or part.startswith(".hangar-"):
            if part == ".hangar-hooks":
                continue
            return True
        if part.endswith((".sqlite", ".sqlite3", ".db")):
            return True
    return False


def _is_account_root(path: Path) -> bool:
    home = Path.home().resolve()
    return any(parent.name.startswith(".codex-") and parent.parent == home
               for parent in (path, *path.parents))


def _forbidden_source_target(path: Path, source: Path) -> bool:
    try:
        relative = path.relative_to(source.resolve())
    except ValueError:
        return _is_account_root(path)
    return relative == Path("config.toml") or _forbidden_resource(relative)


def _external_agents_supported(path: Path) -> bool:
    try:
        entries = list(path.iterdir())
    except OSError:
        return False
    return bool(entries) and all(
        entry.is_file() and entry.suffix.lower() == ".md" and entry.name.lower() != "readme.md"
        for entry in entries
    )


def _walk_source(path: Path, relative: Path, files: dict[str, bytes], issues: list[dict],
                 active: set[Path], source: Path) -> None:
    if ".hangar-uploads" in relative.parts:
        return
    if _forbidden_resource(relative):
        issues.append(_issue("codex_account_source_forbidden", path=relative.as_posix()))
        return
    try:
        real = path.resolve(strict=True)
        if real.is_dir():
            # A resource root link (``agents -> ..`` or ``agents -> HOME``) is too broad.
            # Links below a permitted root may point to one external skill/agent directory.
            if path.is_symlink() and len(relative.parts) == 1:
                issues.append(_issue("codex_account_source_root_link", path=relative.as_posix()))
                return
            if path.is_symlink():
                root_name = relative.parts[0]
                source_real = source.resolve()
                home_real = Path.home().resolve()
                account_root = _is_account_root(real)
                if real in {source_real, home_real} or real in source_real.parents or account_root:
                    issues.append(_issue("codex_account_source_broad_link", path=relative.as_posix()))
                    return
                if root_name == "skills" and not (real / "SKILL.md").is_file():
                    issues.append(_issue("codex_account_source_broad_link", path=relative.as_posix()))
                    return
                if root_name == "agents" and not _external_agents_supported(real):
                    issues.append(_issue("codex_account_source_broad_link", path=relative.as_posix()))
                    return
                if root_name not in {"skills", "agents"} and real.name != relative.name:
                    issues.append(_issue("codex_account_source_broad_link", path=relative.as_posix()))
                    return
            if real in active:
                issues.append(_issue("codex_account_source_cycle", path=relative.as_posix()))
                return
            active.add(real)
            try:
                for child in sorted(path.iterdir(), key=lambda item: item.name):
                    _walk_source(child, relative / child.name, files, issues, active, source)
            finally:
                active.remove(real)
            return
        if real.is_file():
            if path.is_symlink() and (_forbidden_source_target(real, source) or
                                      _forbidden_resource(Path(real.name))):
                issues.append(_issue("codex_account_source_forbidden", path=relative.as_posix()))
                return
            files[relative.as_posix()] = path.read_bytes()
    except (OSError, RuntimeError, ValueError) as exc:
        issues.append(_issue("codex_account_source_unreadable", path=relative.as_posix(), error=type(exc).__name__))


def _source_resources(source: Path) -> tuple[dict[str, bytes], list[dict]]:
    files: dict[str, bytes] = {}
    issues: list[dict] = []
    active: set[Path] = set()
    for name in _RESOURCE_FILES:
        path = source / name
        if path.exists() or path.is_symlink():
            _walk_source(path, Path(name), files, issues, active, source)
    for name in _RESOURCE_DIRS:
        path = source / name
        if path.exists() or path.is_symlink():
            _walk_source(path, Path(name), files, issues, active, source)
    hooks = files.get("hooks.json")
    if hooks is not None:
        try:
            data = json.loads(hooks)
            if not isinstance(data, dict):
                raise ValueError("objeto esperado")
        except (UnicodeError, ValueError, json.JSONDecodeError):
            files.pop("hooks.json", None)
            issues.append(_issue("codex_account_source_invalid", path="hooks.json"))
    return files, issues


def _source_relative(value: str, source: Path) -> str | None:
    candidate = Path(value).expanduser()
    try:
        if candidate.is_absolute():
            try:
                return candidate.absolute().relative_to(source.absolute()).as_posix()
            except ValueError:
                return candidate.resolve(strict=False).relative_to(source.resolve()).as_posix()
        if (source / candidate).exists() or (source / candidate).is_symlink():
            return candidate.as_posix()
    except (OSError, RuntimeError, ValueError):
        return None
    return None


def _map_resource_path(value: str, source: Path, destination: Path,
                       resource_paths: set[str], issues: list[dict], key: str):
    relative = _source_relative(value, source)
    if relative is None:
        return value
    if relative in resource_paths:
        return str(destination / relative)
    issues.append(_issue("codex_account_unmapped_reference", key=key, path=relative))
    return _UNMAPPED


def _map_command(value: str, source: Path, destination: Path,
                 resource_paths: set[str], issues: list[dict]):
    spans = []
    index = 0
    while index < len(value):
        while index < len(value) and value[index].isspace():
            index += 1
        if index >= len(value):
            break
        start = index
        quote = value[index] if value[index] in "\"'" else ""
        if quote:
            index += 1
            body_start = index
            while index < len(value):
                if value[index] == quote:
                    break
                if quote == '"' and value[index] == "\\" and index + 1 < len(value):
                    index += 2
                else:
                    index += 1
            if index >= len(value):
                if str(source) in value:
                    issues.append(_issue("codex_account_unmapped_reference", key="command"))
                    return _UNMAPPED
                return value
            body_end = index
            index += 1
        else:
            body_start = index
            while index < len(value) and not value[index].isspace():
                index += 1
            body_end = index
        spans.append((start, index, quote, body_start, body_end))
    replacements = []
    for start, end, quote, body_start, body_end in spans:
        token = value[body_start:body_end]
        mapped = _map_resource_path(token, source, destination, resource_paths, issues, "command")
        if mapped is _UNMAPPED:
            return _UNMAPPED
        if mapped != token:
            replacements.append((body_start, body_end, mapped))
    for body_start, body_end, mapped in reversed(replacements):
        value = value[:body_start] + mapped + value[body_end:]
    return value


def _map_resource_tree(value, source: Path, destination: Path,
                       resource_paths: set[str], issues: list[dict], key: str = ""):
    if isinstance(value, dict):
        result = {}
        for name, item in value.items():
            mapped = _map_resource_tree(item, source, destination, resource_paths, issues, name)
            if mapped is _UNMAPPED:
                return _UNMAPPED
            result[name] = mapped
        return result
    if isinstance(value, list):
        result = []
        for item in value:
            mapped = _map_resource_tree(item, source, destination, resource_paths, issues, key)
            if mapped is _UNMAPPED:
                return _UNMAPPED
            result.append(mapped)
        return result
    if not isinstance(value, str):
        return copy.deepcopy(value)
    if key == "command":
        return _map_command(value, source, destination, resource_paths, issues)
    if key in _PATH_KEYS:
        return _map_resource_path(value, source, destination, resource_paths, issues, key)
    return value


def _transform_resources(source_files: dict[str, bytes], source: Path, destination: Path,
                         resource_paths: set[str], issues: list[dict]) -> tuple[dict[str, bytes], dict[str, dict]]:
    result = {}
    toml_overrides = {}
    for relative, raw in source_files.items():
        if relative.startswith("agents/") and relative.endswith(".toml"):
            try:
                data = tomllib.loads(raw.decode("utf-8"))
                mapped = _map_resource_tree(data, source, destination, resource_paths, issues)
                if mapped is _UNMAPPED or mapped != data:
                    if mapped is _UNMAPPED:
                        issues.append(_issue("codex_account_unmapped_reference", path=relative))
                        continue
                    toml_overrides[relative] = mapped
            except (UnicodeError, tomllib.TOMLDecodeError):
                issues.append(_issue("codex_account_source_invalid", path=relative))
                continue
        if not relative.endswith(".json"):
            result[relative] = raw
            continue
        try:
            data = json.loads(raw)
            mapped = _map_resource_tree(data, source, destination, resource_paths, issues)
            if mapped is _UNMAPPED:
                continue
            result[relative] = (json.dumps(mapped, ensure_ascii=False, indent=2) + "\n").encode()
        except (UnicodeError, ValueError, json.JSONDecodeError):
            # hooks.json was checked above; other JSON resources stay untouched if they are data.
            result[relative] = raw
    return result, toml_overrides


def _snapshot(root: Path, relative_paths: set[str], previous: dict | None = None,
              *, safe_destination: bool = False) -> tuple[str, dict]:
    previous_files = previous.get("files", {}) if isinstance(previous, dict) else {}
    files = {}
    for relative in sorted(relative_paths):
        path = root / relative
        if safe_destination:
            ancestors = (path.parent, *path.parent.parents)
            if root.is_symlink() or any(parent.is_symlink() for parent in ancestors if parent != root):
                files[relative] = {"kind": "path-conflict", "hash": ""}
                continue
        try:
            stat = path.lstat()
        except FileNotFoundError:
            files[relative] = {"kind": "missing", "hash": ""}
            continue
        if path.is_symlink():
            target = os.readlink(path)
            digest = hash_bytes(target.encode())
            if not safe_destination:
                try:
                    digest = hash_bytes(path.read_bytes())
                except OSError:
                    pass
            entry = {"kind": "symlink", "target": target, "hash": digest}
            if not safe_destination and path.is_file():
                entry["resolved"] = str(path.resolve())
                entry["executable"] = bool(path.stat().st_mode & 0o100)
            files[relative] = entry
            continue
        if not path.is_file():
            files[relative] = {"kind": "other", "hash": ""}
            continue
        meta = {"size": stat.st_size, "mtime_ns": stat.st_mtime_ns,
                "ctime_ns": getattr(stat, "st_ctime_ns", 0), "ino": getattr(stat, "st_ino", 0)}
        old = previous_files.get(relative, {}) if isinstance(previous_files, dict) else {}
        if old.get("meta") == meta and isinstance(old.get("hash"), str):
            digest = old["hash"]
        else:
            digest = hash_bytes(path.read_bytes())
        files[relative] = {"kind": "file", "meta": meta, "hash": digest,
                           "executable": bool(stat.st_mode & 0o100)}
    digest = hashlib.sha256()
    for relative, data in files.items():
        digest.update(relative.encode())
        digest.update(data["kind"].encode())
        digest.update(data["hash"].encode())
        digest.update(json_bytes({key: data[key] for key in ("target", "resolved", "executable")
                                  if key in data}))
    return digest.hexdigest(), {"files": files}


def _config_source(source: Path, relative: str) -> tuple[dict, bytes]:
    path = source / relative
    # Codex nunca rodado: sem config.toml a conta padrao e vazia, nao invalida.
    if relative == "config.toml" and not path.exists() and not path.is_symlink():
        return {}, b""
    raw = path.read_bytes()
    data = tomllib.loads(raw.decode("utf-8"))
    if not isinstance(data, dict):
        raise ValueError(f"{relative} não é um objeto TOML")
    return data, raw


def _map_known_paths(value, source: Path, destination: Path, resource_paths: set[str],
    issues: list[dict], key: str = ""):
    if isinstance(value, dict):
        result = {}
        for name, item in value.items():
            mapped = _map_known_paths(item, source, destination, resource_paths, issues, name)
            if mapped is not _UNMAPPED:
                result[name] = mapped
        return result
    if isinstance(value, list):
        return [mapped for item in value
                if (mapped := _map_known_paths(item, source, destination, resource_paths, issues, key)) is not _UNMAPPED]
    if key not in _PATH_KEYS or not isinstance(value, str):
        return copy.deepcopy(value)
    relative = _source_relative(value, source)
    if relative is None:
        return value
    if relative in resource_paths:
        return str(destination / relative)
    issues.append(_issue("codex_account_unmapped_reference", key=key, path=relative))
    return _UNMAPPED


def _merge_value(current, desired, previous):
    if not isinstance(desired, dict):
        return copy.deepcopy(desired)
    result = copy.deepcopy(current) if isinstance(current, dict) else {}
    old = previous if isinstance(previous, dict) else {}
    for key, old_value in old.items():
        if key not in desired and key in result and result[key] == old_value:
            result.pop(key, None)
    for key, value in desired.items():
        result[key] = _merge_value(result.get(key), value, old.get(key))
    return result


def _edits(current: dict, target: dict, managed: set[str]) -> list[dict]:
    edits = []
    for key in sorted(managed):
        if key in target and current.get(key) != target[key]:
            value = target[key]
        elif key not in target and key in current:
            value = None
        else:
            continue
        edits.append({"keyPath": json.dumps(key), "value": value, "mergeStrategy": "replace"})
    return edits


def _safe_destination(root: Path, relative: str, *, allow_leaf_link: bool = False) -> Path:
    if not _safe_relative(relative):
        raise ValueError("caminho relativo inválido")
    path = root / relative
    if root.is_symlink():
        raise ValueError("destino é um link")
    for parent in (path.parent, *path.parent.parents):
        if parent == root:
            break
        if parent.is_symlink():
            raise ValueError(f"ancestral é um link: {parent}")
    if path.is_symlink() and not allow_leaf_link:
        raise ValueError(f"destino é um link: {path}")
    return path


def _resource_link(source: Path, relative: str, data: bytes, entry: dict) -> str | None:
    # Copiar um hook externo rompe auxiliares localizados a partir do arquivo real.
    target = entry.get("resolved")
    if (Path(relative).parts[0] in {"hooks", ".hangar-hooks"} and
            entry.get("kind") == "symlink" and isinstance(target, str) and
            not Path(target).is_relative_to(source.resolve()) and
            entry.get("hash") == hash_bytes(data)):
        return target
    return None


def _write_hook_resource(path: Path, target: str | None, data: bytes,
                         current: bytes | None, backups: Path) -> None:
    old_target = os.readlink(path) if path.is_symlink() else None
    if target is not None and old_target == target:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hook-", dir=path.parent) as folder:
        temporary = Path(folder) / path.name
        if target is None:
            gravar(temporary, data, None)
        else:
            temporary.symlink_to(target)
        if current is not None or old_target is not None:
            backup(path, current or b"", backups)
        if ((old_target is None and ler(path) != current) or
                (os.readlink(path) if path.is_symlink() else None) != old_target):
            raise AlteradoExternamente(f"hook alterado durante a preparação: {path.name}")
        atomico.substituir(temporary, path)


def _sync_resources(source_files: dict[str, bytes], destination: Path, previous: dict,
                    backups: Path, issues: list[dict], *, source: Path, source_snapshot: dict,
                    allow_removals: bool = True) -> dict:
    old = previous if isinstance(previous, dict) else {}
    result = {}
    for relative, data in sorted(source_files.items()):
        try:
            path = _safe_destination(destination, relative, allow_leaf_link=True)
            anterior = old.get(relative, {}) if isinstance(old.get(relative), dict) else {}
            if path.is_symlink() and os.readlink(path) != anterior.get("target"):
                raise ValueError("link pessoal no destino")
            atual = None if path.is_symlink() else ler(path)
            esperado = anterior.get("hash")
            if atual is not None and esperado is None and atual != data:
                issues.append(_issue("codex_account_resource_conflict", path=relative))
                continue
            entry = source_snapshot["files"][relative]
            target = _resource_link(source, relative, data, entry)
            if target is not None:
                if (atual is not None and not path.is_symlink() and
                        (esperado is None or hash_bytes(atual) != esperado)):
                    raise ValueError("cópia do hook alterada localmente")
                _write_hook_resource(path, target, data, atual, backups)
                result[relative] = {"hash": hash_bytes(data), "target": target}
                continue
            if path.is_symlink():
                _write_hook_resource(path, None, data, atual, backups)
            elif atual != data:
                gravar(path, data, atual, backups)
            executable = entry.get("executable", False)
            if os.name != "nt":
                mode = path.stat().st_mode & 0o777
                wanted = (mode & ~0o100) | (0o100 if executable else 0)
                if wanted != mode:
                    os.chmod(path, wanted)
            result[relative] = {"hash": hash_bytes(data), "executable": executable}
        except (OSError, ValueError, AlteradoExternamente) as exc:
            issues.append(_issue("codex_account_path_conflict", path=relative, error=type(exc).__name__))
            if relative in old:
                result[relative] = copy.deepcopy(old[relative])
    if not allow_removals:
        for relative, anterior in old.items():
            if relative not in result:
                result[relative] = copy.deepcopy(anterior)
        return result
    for relative, anterior in old.items():
        if relative in source_files or not isinstance(anterior, dict):
            continue
        try:
            path = _safe_destination(destination, relative, allow_leaf_link=True)
            if path.is_symlink():
                if os.readlink(path) != anterior.get("target"):
                    raise ValueError("link pessoal no destino")
                backup(path, b"", backups)
                if os.readlink(path) != anterior.get("target"):
                    raise AlteradoExternamente("link alterado durante a retirada")
                path.unlink()
                continue
            atual = ler(path)
            if atual is None:
                continue
            if hash_bytes(atual) != anterior.get("hash"):
                issues.append(_issue("codex_account_resource_local_change", path=relative))
                result[relative] = copy.deepcopy(anterior)
                continue
            backup(path, atual, backups)
            if ler(path) != atual:
                raise AlteradoExternamente(f"arquivo alterado durante a retirada: {relative}")
            path.unlink()
        except (OSError, ValueError, AlteradoExternamente) as exc:
            issues.append(_issue("codex_account_path_conflict", path=relative, error=type(exc).__name__))
            result[relative] = copy.deepcopy(anterior)
    return result


def _contains_managed(current, desired) -> bool:
    if isinstance(desired, dict):
        return isinstance(current, dict) and all(
            key in current and _contains_managed(current[key], value)
            for key, value in desired.items()
        )
    return current == desired


def _verify_config(path: Path, manifest: dict) -> None:
    raw = ler(path)
    if raw is None:
        raise _PreparationChanged(f"configuração ausente: {path.name}")
    try:
        current = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise _PreparationChanged(f"configuração inválida: {path.name}") from exc
    values = manifest.get("values", {}) if isinstance(manifest, dict) else {}
    restrictions = manifest.get("restrictions", {}) if isinstance(manifest, dict) else {}
    if not _contains_managed(current, values) or not _contains_managed(current, restrictions):
        raise _PreparationChanged(f"configuração não materializada: {path.name}")


def _verify_resources(destination: Path, resources: dict, snapshot: dict) -> None:
    files = snapshot.get("files", {}) if isinstance(snapshot, dict) else {}
    for relative, manifest in resources.items():
        entry = files.get(relative, {})
        if isinstance(manifest, dict) and manifest.get("target"):
            if entry.get("kind") != "symlink" or entry.get("target") != manifest["target"]:
                raise _PreparationChanged(f"link do hook não materializado: {relative}")
            continue
        if (not isinstance(manifest, dict) or entry.get("kind") != "file" or
                entry.get("hash") != manifest.get("hash")):
            raise _PreparationChanged(f"recurso não materializado: {relative}")
        if (os.name != "nt" and "executable" in manifest and
                entry.get("executable") != manifest["executable"]):
            raise _PreparationChanged(f"permissão de execução não preservada: {relative}")


def _restriction_target(current: dict, source: dict, previous: dict, issues: list[dict]) -> dict:
    target = {}
    for key in _RESTRICTION_KEYS:
        if key not in source:
            continue
        wanted = source[key]
        actual = current.get(key)
        old = previous.get(key) if isinstance(previous, dict) else None
        if actual is None or actual == wanted or (old is not None and actual == old):
            target[key] = copy.deepcopy(wanted)
        else:
            issues.append(_issue("codex_account_restriction_conflict", key=key))
    return target


async def _edit_config(path: Path, source: dict, source_root: Path, destination: Path,
                       previous: dict, resource_paths: set[str], backups: Path,
                       issues: list[dict]) -> dict:
    projected, projection_issues = _project_preferences(source)
    issues.extend(projection_issues)
    before_mapping = len(issues)
    projected = _map_known_paths(projected, source_root, destination, resource_paths, issues)
    blocked_projection = any(issue.get("code") == "codex_account_unmapped_reference"
                             for issue in issues[before_mapping:])
    old_values = previous.get("values", {}) if isinstance(previous, dict) else {}
    old_values = old_values if isinstance(old_values, dict) else {}
    managed = set(projected)
    restrictions = previous.get("restrictions", {}) if isinstance(previous, dict) else {}
    restrictions = restrictions if isinstance(restrictions, dict) else {}
    result = {"values": copy.deepcopy(projected), "restrictions": copy.deepcopy(restrictions)}

    def preparar(current):
        if not isinstance(current, dict):
            raise ValueError("config.toml não é um objeto")
        target = {}
        for key, value in projected.items():
            target[key] = _merge_value(current.get(key), value, old_values.get(key))
        for key, old in old_values.items():
            if blocked_projection and key not in projected:
                result["values"][key] = copy.deepcopy(old)
                continue
            atual = current.get(key)
            comparavel = atual
            if key == "hooks" and isinstance(atual, dict):
                comparavel = {k: v for k, v in atual.items() if k != "state"}
            if key not in projected and key in current and comparavel != old:
                issues.append(_issue("codex_account_local_change", key=key))
            elif key not in projected and key in current:
                managed.add(key)
                if key == "hooks" and isinstance(current[key], dict):
                    target[key] = ({"state": copy.deepcopy(current[key]["state"])}
                                    if "state" in current[key] else {})
        for key in _RESTRICTION_KEYS:
            if key in source:
                continue
            # A remoção na fonte nunca remove uma restrição local.
            if key in current:
                result["restrictions"][key] = copy.deepcopy(current[key])
            else:
                result["restrictions"].pop(key, None)
        restriction_target = _restriction_target(current, source, restrictions, issues)
        for key in _RESTRICTION_KEYS:
            if key in source and key not in restriction_target:
                result["restrictions"].pop(key, None)
        for key, value in restriction_target.items():
            target[key] = value
            result["restrictions"][key] = copy.deepcopy(value)
        return _edits(current, target, managed | set(restriction_target)), lambda: None

    await editar_config(path, backups, _private_dir(destination, create=True), _NATIVO, preparar)
    return result


async def _apply_toml_overrides(overrides: dict[str, dict], destination: Path,
                                backups: Path) -> None:
    for relative, desired in overrides.items():
        path = _safe_destination(destination, relative)

        def preparar(current):
            if not isinstance(current, dict):
                raise ValueError(f"TOML de agente inválido: {relative}")
            edits = [{"keyPath": json.dumps(key), "value": value, "mergeStrategy": "replace"}
                     for key, value in desired.items() if current.get(key) != value]
            return edits, lambda: None

        await editar_config(path, backups, _private_dir(destination, create=True), _NATIVO, preparar)


async def _remove_config(path: Path, destination: Path, previous: dict,
                         backups: Path, issues: list[dict]) -> dict:
    old_values = previous.get("values", {}) if isinstance(previous, dict) else {}
    old_values = old_values if isinstance(old_values, dict) else {}
    remaining = {}

    def preparar(current):
        if not isinstance(current, dict):
            raise ValueError("configuração de perfil não é um objeto")
        edits = []
        for key, old in old_values.items():
            if key not in current:
                continue
            if current[key] == old:
                edits.append({"keyPath": json.dumps(key), "value": None, "mergeStrategy": "replace"})
            else:
                issues.append(_issue("codex_account_local_change", key=key))
                remaining[key] = copy.deepcopy(old)
        return edits, lambda: None

    await editar_config(path, backups, _private_dir(destination, create=True), _NATIVO, preparar)
    raw = ler(path)
    if raw is not None:
        try:
            if not tomllib.loads(raw.decode("utf-8")):
                backup(path, raw, backups)
                path.unlink()
        except (UnicodeError, tomllib.TOMLDecodeError):
            pass
    return {"values": remaining}


def _private_dir(destination: Path, *, create: bool = False) -> Path:
    # The writer's temporary files belong to the private account state, never CODEX_HOME.
    key = hashlib.sha256(str(_canonical(destination)).encode()).hexdigest()
    root = Path.home() / ".hangar"
    if root.is_symlink():
        raise ValueError("cofre privado do Hangar é um link")
    if create:
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(root, 0o700)
    elif not root.exists():
        return root / "codex-contas" / key
    contas = root / "codex-contas"
    if contas.is_symlink():
        raise ValueError("diretório privado de contas Codex é um link")
    if create:
        contas.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(contas, 0o700)
    elif not contas.exists():
        return contas / key
    directory = contas / key
    if directory.is_symlink():
        raise ValueError("estado privado da conta Codex é um link")
    if create:
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(directory, 0o700)
    return directory


def _destination_paths(state: dict) -> set[str]:
    paths = {"config.toml"}
    for relative in (state.get("resources", {}) or {}):
        if isinstance(relative, str):
            paths.add(relative)
    for relative in (state.get("profiles", {}) or {}):
        if isinstance(relative, str):
            paths.add(relative)
    return paths


async def _prepare_locked(account: Account, force: bool, state: dict) -> dict:
    source = default_home()
    destination = account.home
    cli_version = _cli_version()
    if destination.is_symlink() or not destination.is_dir():
        return _status("error", issues=[_issue("codex_account_destination_invalid")])
    if _canonical(source) == _canonical(destination):
        return _status("error", issues=[_issue("codex_account_source_destination_conflict")])
    try:
        source_config, _ = _config_source(source, "config.toml")
        source_profiles = {}
        for path in sorted(source.glob("*.config.toml")):
            if path.is_file() or path.is_symlink():
                source_profiles[path.name], _ = _config_source(source, path.name)
        source_files, source_issues = _source_resources(source)
        source_files, toml_overrides = _transform_resources(
            source_files, source, destination, set(source_files), source_issues)
    except (OSError, UnicodeError, tomllib.TOMLDecodeError, ValueError) as exc:
        # A tela só vê o tipo; qual arquivo (config.toml, perfil, TOML de agent) fica aqui.
        _log.warning("conta Codex: origem %s inválida", source, exc_info=True)
        result = _status("error", issues=[_issue("codex_account_source_invalid", path=type(exc).__name__)])
        try:
            _write_state(account, {**state, "public": result})
        except (OSError, ValueError):
            pass
        return result
    relative_paths = {"config.toml", *source_files, *source_profiles}
    old_paths = _destination_paths(state)
    source_digest, source_snapshot = _snapshot(source, relative_paths,
                                               state.get("source_snapshot"))
    destination_digest, destination_snapshot = _snapshot(
        destination, old_paths, state.get("destination_snapshot"), safe_destination=True)
    previous_plugins = state.get("plugins", {}) if isinstance(state.get("plugins"), dict) else {}
    plugins_configured = bool(previous_plugins) or any(
        isinstance(source_config.get(key), dict) for key in ("marketplaces", "plugins")
    )
    if (not force and state.get("source_digest") == source_digest and
            state.get("destination_digest") == destination_digest and
            state.get("cli_version") == cli_version and
            _public_state(state).get("status") == "ready" and
            not _has_blocking_issues(source_issues) and
            not plugins_configured):
        return _public_state(state)

    state_dir = _private_dir(destination, create=True)
    backups = state_dir / "backups"
    issues = list(source_issues)
    herdado = _public_state(state).get("trust_pending", False)
    status_running = {**state, "public": _status("running", issues=issues, trust_pending=herdado),
                      "source_digest": source_digest, "destination_digest": destination_digest,
                      "cli_version": cli_version}

    def etapa(nome: str) -> None:
        """A preparação leva minutos (só os plugins, 65-103s medidos): sem dizer onde está,
        a tela fica num "aguarde" que não distingue trabalho de travamento."""
        status_running["public"] = _status("running", issues=issues, trust_pending=herdado, etapa=nome)
        _write_state(account, status_running)

    etapa("configuracoes")
    old_config = state.get("config", {}) if isinstance(state.get("config"), dict) else {}
    old_profiles = state.get("profiles", {}) if isinstance(state.get("profiles"), dict) else {}
    resource_paths = set(source_files) | set(source_profiles)
    try:
        config_path = _safe_destination(destination, "config.toml")
    except (OSError, ValueError, AlteradoExternamente) as exc:
        issues.append(_issue("codex_account_path_conflict", path="config.toml", error=type(exc).__name__))
        config_result = copy.deepcopy(old_config)
    else:
        config_result = await _edit_config(config_path, source_config, source, destination,
                                           old_config, resource_paths, backups, issues)
    profile_results = {}
    for relative, profile in source_profiles.items():
        old_profile = old_profiles.get(relative, {}) if isinstance(old_profiles.get(relative), dict) else {}
        try:
            profile_path = _safe_destination(destination, relative)
        except (OSError, ValueError, AlteradoExternamente) as exc:
            issues.append(_issue("codex_account_path_conflict", path=relative, error=type(exc).__name__))
            profile_results[relative] = copy.deepcopy(old_profile)
        else:
            profile_results[relative] = await _edit_config(
                profile_path, profile, source, destination, old_profile, resource_paths, backups, issues)
    for relative, old_profile in old_profiles.items():
        if relative in source_profiles or not isinstance(old_profile, dict):
            continue
        try:
            path = _safe_destination(destination, relative)
            if ler(path) is None:
                continue
            result = await _remove_config(path, destination, old_profile, backups, issues)
            if result.get("values"):
                profile_results[relative] = result
        except (OSError, ValueError, AlteradoExternamente) as exc:
            issues.append(_issue("codex_account_path_conflict", path=relative, error=type(exc).__name__))
            profile_results[relative] = copy.deepcopy(old_profile)
    etapa("recursos")
    resources = _sync_resources(source_files, destination, state.get("resources", {}), backups, issues,
                                 source=source, source_snapshot=source_snapshot,
                                 allow_removals=not source_issues)
    await _apply_toml_overrides(
        {relative: desired for relative, desired in toml_overrides.items() if relative in resources},
        destination, backups)
    for relative in toml_overrides:
        raw = ler(destination / relative)
        if raw is not None and relative in resources:
            resources[relative]["hash"] = hash_bytes(raw)
    plugin_manifest = copy.deepcopy(previous_plugins)
    plugin_trust_pending = (
        _public_state(state).get("trust_pending", False)
    )
    if plugins_configured:
        etapa("plugins")
        from app.codex_contas_plugins import sync_plugins
        plugin_result = await sync_plugins(
            Account("default", source, True), account, {"manifest": previous_plugins})
        plugin_manifest = plugin_result.get("manifest", plugin_manifest)
        issues.extend(plugin_result.get("issues", []))
        trust_result = plugin_result.get("trust_pending")
        if isinstance(trust_result, bool):
            plugin_trust_pending = trust_result
    final_source_files, final_source_issues = _source_resources(source)
    final_source_files, _ = _transform_resources(
        final_source_files, source, destination, set(final_source_files), final_source_issues)
    final_profile_paths = {path.name for path in source.glob("*.config.toml")
                           if path.is_file() or path.is_symlink()}
    final_source_paths = {"config.toml", *final_source_files, *final_profile_paths}
    final_source_digest, final_source_snapshot = _snapshot(source, final_source_paths, None)
    if final_source_issues != source_issues or final_source_digest != source_digest:
        raise _PreparationChanged("fonte mudou durante a preparação")
    final_paths = {"config.toml", *resources, *profile_results}
    final_destination_digest, final_destination_snapshot = _snapshot(
        destination, final_paths, None, safe_destination=True)
    check_destination_digest, _ = _snapshot(destination, final_paths, None, safe_destination=True)
    if check_destination_digest != final_destination_digest:
        raise _PreparationChanged("destino mudou durante a preparação")
    _verify_config(destination / "config.toml", config_result)
    for relative, manifest in profile_results.items():
        if manifest.get("values") or manifest.get("restrictions"):
            _verify_config(destination / relative, manifest)
    _verify_resources(destination, resources, final_destination_snapshot)
    final_status = "partial" if _has_blocking_issues(issues) else "ready"
    result = {
        "public": _status(final_status, issues=issues, trust_pending=plugin_trust_pending,
                          herdado=_contar_herdado(resources, plugin_manifest, config_result)),
        "source_digest": final_source_digest,
        "destination_digest": final_destination_digest,
        "cli_version": cli_version,
        "source_snapshot": final_source_snapshot,
        "destination_snapshot": final_destination_snapshot,
        "config": config_result,
        "profiles": profile_results,
        "resources": resources,
        "plugins": plugin_manifest,
    }
    _write_state(account, result)
    return _public_state(result)


async def prepare_account(account: Account, force: bool = False) -> dict:
    """Prepara uma conta adicional; a conta padrão é deliberadamente no-op."""
    if account.is_default:
        return _status("ready")
    if account.home.is_symlink() or not account.home.is_dir():
        return _status("error", issues=[_issue("codex_account_destination_invalid")])
    try:
        state = _read_state(account)
    except (OSError, ValueError, json.JSONDecodeError):
        return _status("error", issues=[_issue("codex_account_state_invalid")])
    try:
        async with exclusivo(account.home / ".hangar-integracao.lock"):
            state = _read_state(account)
            return await _prepare_locked(account, force, state)
    except _PreparationChanged:
        result = _status("error", issues=[_issue("codex_account_changed_during_prepare")])
        try:
            _write_state(account, {**state, "public": result})
        except (OSError, ValueError):
            pass
        return result
    except (OSError, UnicodeError, tomllib.TOMLDecodeError, ValueError, RuntimeError) as exc:
        result = _status("error", issues=[_issue("codex_account_prepare_failed", error=type(exc).__name__)])
        try:
            _write_state(account, {**state, "public": result})
        except (OSError, ValueError):
            pass
        return result
