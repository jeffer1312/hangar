"""Reconciliação conservadora entre plugins Claude e instalações nativas do OMP."""
from copy import copy
from dataclasses import dataclass
import sys
import asyncio
import logging
from math import isfinite
from contextlib import contextmanager, nullcontext
import configparser
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import threading
import time
from urllib.parse import urlsplit

from app import atomico, peers

_log = logging.getLogger("hangar.omp_plugin_sync")
_SHA = re.compile(r"[0-9a-f]{40}")
_NAME = re.compile(r"(?:@[a-z0-9._-]+/)?[a-z0-9][a-z0-9._-]*")
_FEATURE = re.compile(r"[A-Za-z0-9._-]+")


class InventoryError(ValueError):
    """Inventário sem prova suficiente para autorizar uma alteração."""


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise InventoryError("JSON contém chaves repetidas")
        value[key] = item
    return value


def _read(path: Path, default=None):
    if path.is_symlink():
        raise InventoryError("Registro simbólico recusado")
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
    except FileNotFoundError:
        if default is not None:
            return default
        raise InventoryError("Registro obrigatório ausente") from None
    except (ValueError, UnicodeError):
        raise InventoryError("Registro JSON inválido") from None
    if not isinstance(value, dict):
        raise InventoryError("Registro deve ser um objeto")
    return value


def _write(path: Path, data: dict):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink():
        raise InventoryError("Registro simbólico recusado")
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(data, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        atomico.substituir(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def _identity(url: str) -> tuple:
    if not isinstance(url, str) or any(c.isspace() for c in url):
        raise InventoryError("Origem Git inválida")
    url = url.removeprefix("git+")
    if re.fullmatch(r"[^/@:]+@[^/:]+:.+", url):
        user_host, remote_path = url.split(":", 1)
        url = "ssh://" + user_host + "/" + remote_path
    parsed = urlsplit(url)
    if parsed.scheme not in {"https", "http", "ssh", "git"} or not parsed.hostname:
        raise InventoryError("Origem Git não verificável")
    if parsed.password or (parsed.username and parsed.scheme != "ssh") or parsed.query or parsed.fragment:
        raise InventoryError("Origem com credenciais ou parâmetros recusada")
    try:
        port = parsed.port or {"https": 443, "http": 80, "ssh": 22, "git": 9418}[parsed.scheme]
    except ValueError:
        raise InventoryError("Porta da origem inválida") from None
    if not parsed.path or any(p in {".", ".."} for p in parsed.path.split("/")):
        raise InventoryError("Caminho da origem inválido")
    return parsed.scheme, parsed.hostname.lower(), port, parsed.username or "", parsed.path.removesuffix(".git")


def _spec(value: str):
    if not isinstance(value, str) or "#" not in value:
        raise InventoryError("Instalação sem pin Git completo")
    origin, revision = value.rsplit("#", 1)
    if not _SHA.fullmatch(revision):
        raise InventoryError("Revisão Git não verificável")
    return _identity(origin), revision


def _inside(path: Path, root: Path) -> Path:
    resolved = path.resolve(strict=True)
    if not resolved.is_relative_to(root.resolve(strict=True)):
        raise InventoryError("Caminho escapa da raiz do pacote")
    return resolved


def _manifest(root: Path) -> dict:
    if root.is_symlink():
        raise InventoryError("Raiz simbólica não pode ser gerenciada")
    path = root / "package.json"
    _inside(path, root)
    package = _read(path)
    if not isinstance(package.get("name"), str) or not _NAME.fullmatch(package["name"]):
        raise InventoryError("Nome de pacote inválido")
    manifest = package.get("omp", package.get("pi"))
    if not isinstance(manifest, dict):
        raise InventoryError("Manifesto OMP/Pi ausente")
    groups = [manifest]
    features = manifest.get("features", {})
    if not isinstance(features, dict) or any(not isinstance(v, dict) for v in features.values()):
        raise InventoryError("Features inválidas")
    groups.extend(features.values())
    for group in groups:
        for resource in ("extensions", "skills", "prompts", "commands", "agents", "tools"):
            paths = group.get(resource, [])
            if not isinstance(paths, list) or any(not isinstance(p, str) for p in paths):
                raise InventoryError("Caminhos de recursos inválidos")
            for entry in paths:
                if Path(entry).is_absolute() or ".." in Path(entry).parts:
                    raise InventoryError("Recurso fora da raiz instalável")
                _inside(root / entry, root)
    return package


def _git_identity(root: Path):
    """Lê somente metadados; não dispara hooks, migrações ou processos no dry-run."""
    git_dir = root / ".git"
    if not git_dir.is_dir() or git_dir.is_symlink():
        return None
    parser = configparser.RawConfigParser(strict=True)
    parser.read_string((git_dir / "config").read_text(encoding="utf-8"))
    origin = parser.get('remote "origin"', "url", fallback="")
    head = (git_dir / "HEAD").read_text(encoding="ascii").strip()
    if head.startswith("ref: "):
        reference = head[5:]
        if not reference.startswith("refs/") or ".." in Path(reference).parts:
            raise InventoryError("Referência Git inválida")
        ref_path = git_dir / reference
        if ref_path.is_file():
            head = _inside(ref_path, git_dir).read_text(encoding="ascii").strip()
        else:
            packed = git_dir / "packed-refs"
            head = next((line.split(" ", 1)[0] for line in packed.read_text(encoding="ascii").splitlines()
                         if line.endswith(" " + reference)), "") if packed.is_file() else ""
    if not _SHA.fullmatch(head):
        raise InventoryError("HEAD Git inválido")
    return _identity(origin), head


def _bun_revision(root: Path, name: str):
    path = root / "bun.lock"
    if not path.is_file() or path.is_symlink():
        return None
    # O lock do Bun é JSONC: preservar strings ao remover comentários/vírgulas finais.
    text = path.read_text(encoding="utf-8")
    quoted = r'("(?:[^"\\]|\\.)*")'
    text = re.sub(quoted + r'|//[^\n]*|/\*[\s\S]*?\*/', lambda m: m.group(1) or "", text)
    text = re.sub(quoted + r'|,(?=\s*[}\]])', lambda m: m.group(1) or "", text)
    value = json.loads(text, object_pairs_hook=_unique_object)
    entry = value.get("packages", {}).get(name)
    if not isinstance(entry, list) or not entry or not isinstance(entry[0], str):
        return None
    resolved = entry[0]
    if not resolved.startswith(name + "@"):
        return None
    return _spec(resolved[len(name) + 1:])


# Assinatura (caminho, mtime, tamanho) por arquivo; o hash só é refeito quando ela muda.
# Sem isso cada passagem do laço relia todo byte de todo plugin, mesmo sem nada ter mudado.
_DIGEST_CACHE: dict[str, tuple[tuple, str]] = {}


def _digest(root: Path) -> str:
    entries: list[tuple[str, int, int]] = []
    for directory, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d != ".git")
        for name in dirs + sorted(files):
            path = Path(directory) / name
            if name == ".git":
                continue
            if path.is_symlink():
                raise InventoryError("Conteúdo simbólico não pode ser gerenciado")
            info = path.lstat()
            entries.append((str(path.relative_to(root)), info.st_mtime_ns, info.st_size if path.is_file() else -1))
    signature = tuple(entries)
    cached = _DIGEST_CACHE.get(str(root))
    if cached and cached[0] == signature:
        return cached[1]
    digest = hashlib.sha256()
    for relative, _, size in entries:
        digest.update(relative.encode())
        digest.update(b"\0")
        if size >= 0:
            digest.update((root / relative).read_bytes())
    result = digest.hexdigest()
    _DIGEST_CACHE[str(root)] = (signature, result)
    return result


def _run(args, *, cwd, env, timeout):
    process = subprocess.Popen(args, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               encoding="utf-8", errors="replace", start_new_session=os.name != "nt")
    try:
        out, err = process.communicate(timeout=timeout)
    except BaseException:
        if os.name != "nt":
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        else:
            process.kill()
        process.communicate()
        raise
    return subprocess.CompletedProcess(args, process.returncode, out, err)


@dataclass(frozen=True)
class OmpDirectories:
    config_root: Path
    agent_dir: Path
    data_root: Path


def resolve_omp_directories(home: Path, env: dict[str, str], cwd: Path) -> OmpDirectories:
    """Espelha a resolução lexical nativa, sem criar diretórios nem seguir symlinks."""
    def lexical(value):
        value = os.path.normpath(value)
        if "\0" in value:
            raise InventoryError("Diretório OMP inválido")
        if os.name != "nt" and value.startswith("//"):
            value = "/" + value.lstrip("/")
        return Path(value)

    def join(*parts):
        return lexical(os.sep.join(str(part) for part in parts))

    def absolute(value):
        if os.name == "nt" and os.path.splitdrive(value)[0] and not os.path.isabs(value):
            raise InventoryError("Diretório relativo a outra unidade não pode ser resolvido")
        return lexical(os.path.join(str(cwd), value))

    def normalize_profile(value):
        profile = (value or "").strip()
        if not profile or profile == "default":
            return None
        if (profile in {".", ".."} or profile.endswith(".")
                or not re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,63}", profile)
                or re.match(r"^(CON|PRN|AUX|NUL|COM[0-9]|LPT[0-9])(?:\.|$)", profile, re.I)):
            raise InventoryError("Nome de perfil OMP inválido")
        return profile

    directory_name = env.get("PI_CONFIG_DIR") or ".omp"
    if os.name == "nt" and os.path.splitdrive(directory_name)[0]:
        raise InventoryError("Nome do diretório de configuração OMP inválido")
    base = join(home, directory_name)
    profile = normalize_profile(env.get("OMP_PROFILE") if "OMP_PROFILE" in env else env.get("PI_PROFILE"))
    config_root = join(base, "profiles", profile) if profile else base
    default_agent = join(config_root, "agent")
    override = env.get("PI_CODING_AGENT_DIR")
    if profile:
        override = None
    else:
        try:
            legacy = normalize_profile(env.get("PI_PROFILE"))
        except InventoryError:
            legacy = None
        if legacy and override == str(join(base, "profiles", legacy, "agent")):
            override = None
    agent = absolute(override) if override else default_agent
    data_root = config_root
    if sys.platform in {"linux", "darwin"} and agent == default_agent and env.get("XDG_DATA_HOME"):
        candidate = join(env["XDG_DATA_HOME"], "omp")
        if profile:
            candidate = join(candidate, "profiles", profile)
        effective = absolute(str(candidate))
        if effective.exists():
            data_root = effective
    return OmpDirectories(config_root, agent, data_root)


class PluginSynchronizer:
    def __init__(self, *, home: Path, claude_dir: Path, runner=None):
        self.home = Path(home).resolve()
        self.claude_dir = Path(claude_dir).resolve()
        self.runner = runner or _run
        self.env = dict(os.environ)
        for key in tuple(self.env):
            if key.startswith("GIT_"):
                del self.env[key]
        self.env.update(HOME=str(self.home), USERPROFILE=str(self.home),
                        CLAUDE_CONFIG_DIR=str(self.claude_dir), GIT_CONFIG_NOSYSTEM="1",
                        GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0", GIT_OPTIONAL_LOCKS="0")
        self.directories = None
        self.agent_dir = None
        self.native_root = None
        self._directory_error = None
        self._refresh_directories()
        self.ledger_path = self.home / ".hangar/omp-plugin-sync.json"
        self.lock_path = self.ledger_path.with_suffix(".lock")

    def _refresh_directories(self):
        try:
            self.directories = resolve_omp_directories(self.home, self.env, self.home)
        except (InventoryError, OSError, ValueError) as error:
            self._directory_error = str(error) if isinstance(error, InventoryError) else type(error).__name__
            self.directories = self.agent_dir = self.native_root = None
            return False
        self.agent_dir = self.directories.agent_dir
        self.native_root = self.directories.data_root / "plugins"
        self._directory_error = None
        return True

    @contextmanager
    def _locked(self):
        if peers.fcntl is None and peers.msvcrt is None:
            raise InventoryError("Trava de processo indisponível")
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        if self.lock_path.is_symlink() or self.ledger_path.is_symlink():
            raise InventoryError("Estado gerenciado simbólico recusado")
        with self.lock_path.open("a+", encoding="utf-8") as lock:
            peers._travar(lock)
            try:
                if not self._refresh_directories():
                    raise InventoryError(self._directory_error)
                yield
            finally:
                peers._destravar(lock)

    def _cli(self, action, argument=None):
        args = ["omp", "plugin", action]
        if argument is not None:
            args.append(argument)
        args.append("--json")
        result = self.runner(args, cwd=self.home, env=self.env, timeout=120 if action == "install" else 30)
        if result.returncode:
            raise InventoryError(f"CLI OMP falhou em {action} (código {result.returncode})")
        try:
            data = json.loads(result.stdout, object_pairs_hook=_unique_object)
        except (ValueError, TypeError):
            raise InventoryError("Resposta JSON do CLI inválida") from None
        if not isinstance(data, dict):
            raise InventoryError("Resposta do CLI deve ser um objeto")
        return data

    def _change_pin(self, name, expected, replacement):
        path = self.native_root / "package.json"
        package = _read(path)
        dependencies = package.get("dependencies", {})
        if not isinstance(dependencies, dict) or dependencies.get(name) != expected:
            raise InventoryError("Pin nativo foi alterado por outro escritor")
        dependencies[name] = replacement
        _write(path, package)

    def _restore_pin(self, name, prepared, previous):
        observed, _ = self._native(dry_run=True)
        current = observed.get(name)
        if not current or current["spec"] != prepared:
            return
        unchanged = all(current[k] == previous[k] for k in previous if k not in {"spec", "proof"})
        proof = _git_identity(Path(current["path"])) or _bun_revision(self.native_root, name)
        original = previous["proof"]
        if unchanged and proof == (tuple(original["origin"]), original["revision"]):
            # Reverter só nossa chave, mesclando o registro atual; nunca uma cópia global antiga.
            self._change_pin(name, prepared, previous["spec"])

    @staticmethod
    def _catalog_identity(kind, uri):
        if not isinstance(uri, str) or not uri:
            raise InventoryError("Origem do catálogo ausente")
        if kind == "local":
            return ("local", str(Path(uri).expanduser().resolve(strict=True)))
        if kind == "github":
            uri = "https://github.com/" + uri.removesuffix(".git") + ".git"
        if kind not in {"git", "github", "url"}:
            raise InventoryError("Tipo de catálogo não suportado")
        return _identity(uri)

    def _marketplaces(self):
        registry = _read(self.native_root.parent / "marketplaces.json", {"version": 1, "marketplaces": []})
        entries = registry.get("marketplaces")
        if registry.get("version") != 1 or not isinstance(entries, list):
            raise InventoryError("Registro de marketplaces inválido")
        result = {}
        for entry in entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("name"), str):
                raise InventoryError("Entrada de marketplace inválida")
            if entry["name"] in result:
                raise InventoryError("Marketplace duplicado no registro nativo")
            self._catalog_identity(entry.get("sourceType"), entry.get("sourceUri"))
            result[entry["name"]] = entry
        return result

    def import_marketplaces(self, *, dry_run=False, stop_requested=None):
        """Importa catálogos pelo CLI nativo; não instala, migra ou atualiza seus plugins."""
        report = {"complete_inventory": False, "items": [], "errors": [],
                  "mode": "read_only_local" if dry_run else "native"}
        stop = stop_requested or (lambda: False)
        try:
            # Diretórios pertencem à passagem; uma inspeção não altera a visão de outro worker.
            context = copy(self)
            if not context._refresh_directories():
                raise InventoryError(context._directory_error)
            self = context
            with nullcontext() if dry_run else self._locked():
                known = _read(self.claude_dir / "plugins/known_marketplaces.json", {})
                native = self._marketplaces()
                report["complete_inventory"] = True
                for alias, entry in known.items():
                    if stop():
                        break
                    item = {"identity": alias, "action": "diagnostic"}
                    report["items"].append(item)
                    try:
                        if not isinstance(entry, dict) or not isinstance(entry.get("source"), dict):
                            raise InventoryError("Fonte do marketplace Claude inválida")
                        source = entry["source"]
                        kind = source.get("source")
                        if source.get("ref") or source.get("branch"):
                            raise InventoryError("Referência de catálogo não representável pelo importador nativo")
                        if kind == "directory":
                            native_kind, uri = "local", source.get("path")
                        elif kind == "github":
                            native_kind, uri = "github", source.get("repo")
                        elif kind in {"git", "url"}:
                            native_kind, uri = kind, source.get("url")
                        else:
                            raise InventoryError("Fonte de catálogo não suportada")
                        identity = self._catalog_identity(native_kind, uri)
                        location = entry.get("installLocation")
                        if not isinstance(location, str):
                            raise InventoryError("Catálogo local não encontrado")
                        root = Path(location)
                        catalog_path = root / ".claude-plugin/marketplace.json"
                        if not catalog_path.exists():
                            catalog_path = root / "marketplace.json"
                        catalog = _read(catalog_path)
                        name = catalog.get("name")
                        if (not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name)
                                or not isinstance(catalog.get("plugins"), list)):
                            raise InventoryError("Catálogo local inválido")
                        item["name"] = name
                        existing = native.get(name)
                        if existing is not None:
                            same = self._catalog_identity(existing["sourceType"], existing["sourceUri"]) == identity
                            item["action"] = "unchanged" if same else "conflict"
                            item["reason"] = "Catálogo existente preservado" if same else "Nome existente com outra origem; preservado"
                            continue
                        item["action"] = "import"
                        if dry_run:
                            item["planned"] = True
                            continue
                        if stop():
                            item["action"] = "interrupted"
                            item["reason"] = "Parada solicitada antes da importação"
                            break
                        result = self.runner(["omp", "plugin", "marketplace", "add", uri],
                                             cwd=self.home, env=self.env, timeout=120)
                        if result.returncode:
                            raise InventoryError(f"Importação nativa falhou (código {result.returncode})")
                        native = self._marketplaces()
                        imported = native.get(name)
                        if imported is None or self._catalog_identity(imported["sourceType"], imported["sourceUri"]) != identity:
                            raise InventoryError("CLI não confirmou o catálogo e sua origem")
                        item["reason"] = "Catálogo registrado pelo mecanismo nativo"
                    except (InventoryError, OSError, ValueError, subprocess.SubprocessError) as error:
                        item["action"] = "error"
                        item["reason"] = str(error) if isinstance(error, InventoryError) else type(error).__name__
                        report["errors"].append({"identity": alias, "reason": item["reason"]})
        except (InventoryError, OSError, ValueError) as error:
            report["errors"].append(str(error) if isinstance(error, InventoryError) else type(error).__name__)
        return report

    def _native(self, *, dry_run, stop=None):
        package = _read(self.native_root / "package.json", {"dependencies": {}})
        lock = _read(self.native_root / "omp-plugins.lock.json", {"plugins": {}, "settings": {}})
        marketplace = _read(self.native_root / "installed_plugins.json", {"version": 2, "plugins": {}})
        dependencies, preferences = package.get("dependencies", {}), lock.get("plugins", {})
        settings = lock.get("settings", {})
        if any(not isinstance(value, dict) for value in (dependencies, preferences, settings, marketplace.get("plugins"))):
            raise InventoryError("Inventário nativo inválido")
        states = {}
        for name, spec in dependencies.items():
            if not isinstance(name, str) or not _NAME.fullmatch(name) or not isinstance(spec, str):
                raise InventoryError("Dependência nativa inválida")
            root = self.native_root / "node_modules" / name
            preference = preferences.get(name)
            if not isinstance(preference, dict) or type(preference.get("enabled")) is not bool:
                raise InventoryError("Preferências nativas ausentes ou inválidas")
            features = preference.get("enabledFeatures")
            if features is not None and (not isinstance(features, list) or any(not isinstance(f, str) or not _FEATURE.fullmatch(f) for f in features)):
                raise InventoryError("Seleção de features inválida")
            installed = _read(root / "package.json")
            if installed.get("name") != name:
                raise InventoryError("Nome instalado diverge do registro")
            proof = None
            try:
                declared = _spec(spec)
                resolved = _git_identity(root) or _bun_revision(self.native_root, name)
                if resolved == declared:
                    _manifest(root)
                    proof = {"origin": list(declared[0]), "revision": declared[1]}
            except (ValueError, OSError, configparser.Error):
                pass
            states[name] = {"spec": spec, "enabled": preference["enabled"], "features": features,
                            "settings": settings.get(name, {}), "version": installed.get("version"),
                            "path": str(root), "proof": proof, "digest": _digest(root)}
        if not dry_run:
            if stop is not None and stop():
                raise InventoryError("Passagem interrompida antes de consultar o CLI")
            listing = self._cli("list")
            if not isinstance(listing.get("npm"), list) or not isinstance(listing.get("marketplace"), list):
                raise InventoryError("Formato do inventário CLI inválido")
            seen = set()
            for item in listing["npm"]:
                if not isinstance(item, dict) or item.get("name") not in states or item["name"] in seen:
                    raise InventoryError("CLI e registros nativos divergem")
                seen.add(item["name"])
                state = states[item["name"]]
                listed_path = item.get("path")
                if not isinstance(listed_path, str):
                    raise InventoryError("Caminho nativo ausente no inventário CLI")
                listed_path = os.path.normpath(os.path.join(str(self.home), listed_path))
                if (item.get("enabled") != state["enabled"] or item.get("enabledFeatures") != state["features"]
                        or item.get("version") != state["version"] or listed_path != state["path"]
                        or not isinstance(item.get("manifest"), dict)):
                    raise InventoryError("CLI e registros nativos divergem")
            if seen != set(states):
                raise InventoryError("CLI omitiu uma instalação nativa")
            ids = [entry.get("id") for entry in listing["marketplace"] if isinstance(entry, dict)]
            if len(ids) != len(listing["marketplace"]) or set(ids) != set(marketplace["plugins"]):
                raise InventoryError("Inventário marketplace divergente")
        return states, marketplace["plugins"]

    def _sources(self, *, dry_run, report, stop=None):
        registry = _read(self.claude_dir / "plugins/installed_plugins.json")
        if registry.get("version") != 2 or not isinstance(registry.get("plugins"), dict):
            raise InventoryError("Inventário Claude inválido")
        known = _read(self.claude_dir / "plugins/known_marketplaces.json", {})
        enabled = _read(self.claude_dir / "settings.json", {}).get("enabledPlugins", {})
        if not isinstance(enabled, dict):
            raise InventoryError("Configuração de habilitação inválida")
        sources = {}
        ambiguous = set()
        for plugin_id, entries in registry["plugins"].items():
            if not isinstance(plugin_id, str) or "@" not in plugin_id or not isinstance(entries, list) or not entries:
                raise InventoryError("Entrada Claude inválida")
            for entry in entries:
                if (not isinstance(entry, dict) or not isinstance(entry.get("installPath"), str)
                        or not isinstance(entry.get("scope"), str)):
                    raise InventoryError("Entrada Claude incompleta")
            try:
                user_entries = [e for e in entries if e["scope"] == "user"]
                if len(user_entries) != 1:
                    raise InventoryError("Escopo não pode ser preservado como instalação global")
                entry = user_entries[0]
                revision = entry.get("gitCommitSha")
                if not isinstance(revision, str) or not _SHA.fullmatch(revision):
                    raise InventoryError("Revisão Claude sem SHA completo")
                active = enabled.get(plugin_id, False)
                if type(active) is not bool:
                    raise InventoryError("Habilitação Claude inválida")
                name, catalog_name = plugin_id.rsplit("@", 1)
                catalog = known.get(catalog_name)
                if not isinstance(catalog, dict) or not isinstance(catalog.get("installLocation"), str):
                    raise InventoryError("Origem do catálogo ausente")
                catalog_root = Path(catalog["installLocation"])
                catalog_data = _read(catalog_root / ".claude-plugin/marketplace.json")
                candidates = catalog_data.get("plugins")
                if not isinstance(candidates, list):
                    raise InventoryError("Catálogo inválido")
                matches = [item for item in candidates if isinstance(item, dict) and item.get("name") == name]
                if len(matches) != 1:
                    raise InventoryError("Origem do plugin ambígua")
                source = matches[0].get("source")
                if isinstance(source, str):
                    if source not in {".", "./"}:
                        raise InventoryError("Plugin em subpasta não pode ser instalado pela raiz")
                    source = catalog.get("source", {})
                if not isinstance(source, dict) or source.get("source") not in {"url", "git", "github"}:
                    raise InventoryError("Origem sem raiz instalável verificável")
                origin = ("https://github.com/" + source.get("repo", "") + ".git"
                          if source.get("source") == "github" else source.get("url", ""))
                identity = _identity(origin)
                if source.get("sha") not in (None, revision):
                    raise InventoryError("SHA do catálogo diverge da instalação Claude")
                root = Path(entry["installPath"])
                package = _manifest(root)
                if _git_identity(root) != (identity, revision):
                    raise InventoryError("Checkout Claude não comprova origem e revisão")
                if stop is not None and stop():
                    break
                if not dry_run:
                    result = _run(["git", "-c", "core.hooksPath=" + os.devnull, "-c", "core.fsmonitor=false",
                                   "show", revision + ":package.json"], cwd=root, env=self.env, timeout=15)
                    if result.returncode or json.loads(result.stdout) != package:
                        raise InventoryError("Manifesto local diverge da revisão Git")
                package_name = package["name"]
                if package_name in sources or package_name in ambiguous:
                    previous = sources.pop(package_name, None)
                    ambiguous.add(package_name)
                    if previous is not None:
                        report["items"].append({"identity": previous["id"], "name": package_name,
                                                "action": "diagnostic", "reason": "Mais de uma origem para o pacote nativo"})
                    raise InventoryError("Mais de um plugin Claude usa o mesmo pacote nativo")
                sources[package_name] = {"id": plugin_id, "name": package_name, "origin": list(identity),
                                         "url": origin, "revision": revision, "enabled": active}
            except (InventoryError, OSError, ValueError, configparser.Error) as error:
                reason = str(error) if isinstance(error, InventoryError) else type(error).__name__
                report["items"].append({"identity": plugin_id, "action": "diagnostic", "reason": reason})
        return sources, set(registry["plugins"]), ambiguous

    def _validate_ledger(self, ledger):
        if ledger.get("version") != 1 or not isinstance(ledger.get("items"), dict):
            raise InventoryError("Registro de gestão inválido")
        for name, record in ledger["items"].items():
            if (not isinstance(name, str) or not _NAME.fullmatch(name)
                    or not isinstance(record, dict) or not isinstance(record.get("status"), str)
                    or record["status"] not in {"managed", "pending", "suspended"}):
                raise InventoryError("Registro de propriedade inválido")
            source = record.get("source")
            if (not isinstance(source, dict) or source.get("name") != name
                    or not isinstance(source.get("id"), str) or "@" not in source["id"]
                    or not all(source["id"].rsplit("@", 1))
                    or not isinstance(source.get("revision"), str) or not _SHA.fullmatch(source["revision"])
                    or type(source.get("enabled")) is not bool or not isinstance(source.get("url"), str)):
                raise InventoryError("Vínculo de origem inválido no registro de gestão")
            origin = source.get("origin")
            if (not isinstance(origin, list) or len(origin) != 5 or type(origin[2]) is not int
                    or origin != list(_identity(source["url"]))):
                raise InventoryError("Origem inconsistente no registro de gestão")
            if "native" not in record:
                raise InventoryError("Estado nativo ausente no registro de gestão")
            native = record["native"]
            if native is None:
                if record["status"] == "managed":
                    raise InventoryError("Gestão confirmada exige prova nativa")
                continue
            if (not isinstance(native, dict) or type(native.get("enabled")) is not bool
                    or not isinstance(native.get("settings"), dict)
                    or not isinstance(native.get("digest"), str)
                    or not re.fullmatch(r"[0-9a-f]{64}", native["digest"])
                    or native.get("path") != str(self.native_root / "node_modules" / name)
                    or not isinstance(native.get("spec"), str)
                    or "version" not in native
                    or (native.get("version") is not None and not isinstance(native["version"], str))):
                raise InventoryError("Estado nativo inválido no registro de gestão")
            if "features" not in native:
                raise InventoryError("Seleção nativa ausente no registro de gestão")
            features = native["features"]
            if features is not None and (not isinstance(features, list)
                                        or any(not isinstance(f, str) or not _FEATURE.fullmatch(f) for f in features)):
                raise InventoryError("Seleção nativa inválida no registro de gestão")
            proof = native.get("proof")
            if (not isinstance(proof, dict) or proof.get("origin") != origin
                    or not isinstance(proof.get("origin"), list) or len(proof["origin"]) != 5
                    or type(proof["origin"][2]) is not int
                    or not isinstance(proof.get("revision"), str) or not _SHA.fullmatch(proof["revision"])
                    or _spec(native["spec"]) != (tuple(origin), proof["revision"])):
                raise InventoryError("Prova nativa inconsistente no registro de gestão")
            try:
                json.dumps(native["settings"], allow_nan=False)
            except ValueError:
                raise InventoryError("Preferências nativas não contêm JSON finito") from None

    def _save_ledger(self, ledger):
        self._validate_ledger(ledger)
        _write(self.ledger_path, ledger)

    def reconcile(self, *, dry_run: bool = False, stop_requested=None) -> dict:
        report = {"complete_inventory": False, "items": [], "errors": [],
                  "mode": "read_only_local" if dry_run else "native"}
        stop = stop_requested or (lambda: False)
        try:
            context = copy(self)
            if not context._refresh_directories():
                raise InventoryError(context._directory_error)
            self = context
            if dry_run:
                self._reconcile(report, dry_run=True, stop=stop)
            elif not stop():
                with self._locked():
                    if not stop():
                        self._reconcile(report, dry_run=False, stop=stop)
        except (InventoryError, OSError, ValueError, configparser.Error, subprocess.SubprocessError) as error:
            report["errors"].append(str(error) if isinstance(error, InventoryError) else type(error).__name__)
        return report

    def _reconcile(self, report, *, dry_run, stop):
        ledger = _read(self.ledger_path, {"version": 1, "items": {}})
        self._validate_ledger(ledger)
        sources, source_ids, ambiguous = self._sources(dry_run=dry_run, report=report, stop=stop)
        if stop():
            return
        native, marketplaces = self._native(dry_run=dry_run, stop=stop)
        report["complete_inventory"] = True
        records = ledger["items"]
        for name in sorted(set(sources) | set(records) | ambiguous):
            if stop():
                break
            if name in ambiguous:
                report["items"].append({"identity": name, "name": name, "action": "diagnostic",
                                        "reason": "Origem ambígua; instalação e propriedade preservadas"})
                continue
            source, state, record = sources.get(name), native.get(name), records.get(name)
            if record is not None and not isinstance(record, dict):
                raise InventoryError("Registro de propriedade inválido")
            desired_enabled = source["enabled"] if source else None
            if source and state and record and source["enabled"] == record["source"]["enabled"]:
                desired_enabled = state["enabled"]
            action, reason = "unchanged", "Sem alteração"
            if record and record.get("status") != "managed":
                action, reason = "suspended", "Gestão suspensa; intervenção manual preservada"
            elif record and record.get("native") != state:
                action, reason = "suspended", "Instalação OMP foi alterada fora da sincronização"
            elif source is None:
                if record and record.get("source", {}).get("id") not in source_ids:
                    action = "uninstall"
                else:
                    action, reason = "diagnostic", "Origem atual não pode ser confirmada"
            elif source["id"] in marketplaces:
                action, reason = "diagnostic", "Plugin de marketplace preservado; atualizações pertencem ao mecanismo nativo"
            elif state and (not state["proof"] or state["proof"]["origin"] != source["origin"]):
                action, reason = "diagnostic", "Instalação existente não comprova a mesma origem"
            elif not record and state and not source["enabled"]:
                action, reason = "diagnostic", "Plugin Claude inativo não autoriza adotar instalação manual"
            elif not record and state:
                if state["proof"]["revision"] == source["revision"]:
                    action = "adopt"
                else:
                    action, reason = "diagnostic", "Revisões diferentes não autorizam adoção"
            elif not source["enabled"]:
                action = "disable" if state and state["enabled"] and not desired_enabled else "pending" if state else "unchanged"
            elif state is None or state["proof"]["revision"] != source["revision"]:
                action = "install"
            elif not state["enabled"] and desired_enabled:
                action = "enable"
            item = {"identity": source["id"] if source else record.get("source", {}).get("id", name),
                    "name": name, "action": action, "reason": reason,
                    "revision": source["revision"] if source else None}
            report["items"].append(item)
            if dry_run:
                item["planned"] = True
                continue
            if action == "suspended":
                if record:
                    record["status"] = "suspended"
                    self._save_ledger(ledger)
                continue
            if action in {"diagnostic", "pending", "unchanged"}:
                if record and source and action != "diagnostic" and record["source"] != source:
                    record["source"] = source
                    self._save_ledger(ledger)
                continue
            if action == "adopt":
                records[name] = {"status": "managed", "source": source, "native": state}
                self._save_ledger(ledger)
                continue
            # Revalidar antes de criar uma operação pendente; parar não perde a propriedade anterior.
            before, _ = self._native(dry_run=False, stop=stop)
            if before.get(name) != state:
                raise InventoryError("Instalação mudou antes da ação")
            if stop():
                item["action"] = "interrupted"
                item["reason"] = "Parada solicitada antes da operação"
                break
            records[name] = {"status": "pending", "source": source or record["source"], "native": state}
            self._save_ledger(ledger)
            prepared_pin = None
            try:
                argument = name
                if action == "install":
                    features = state["features"] if state else None
                    suffix = "" if features is None else "[" + ",".join(features) + "]"
                    pin = "git+" + source["url"].removeprefix("git+") + "#" + source["revision"]
                    argument = pin + suffix
                    if state and state["spec"] != pin:
                        # Declarar o destino antes do instalador evita duas arestas Git para o mesmo pacote.
                        self._change_pin(name, state["spec"], pin)
                        prepared_pin = pin
                self._cli(action, argument)
                if action == "install" and not desired_enabled:
                    self._cli("disable", name)
                if stop():
                    records[name]["status"] = "suspended"
                    self._save_ledger(ledger)
                    break
                after, _ = self._native(dry_run=False, stop=stop)
                confirmed = after.get(name)
                if action == "uninstall":
                    if confirmed is not None:
                        raise InventoryError("CLI não confirmou a remoção")
                    del records[name]
                else:
                    if (not confirmed or not confirmed["proof"] or confirmed["proof"]["origin"] != source["origin"]
                            or confirmed["enabled"] != desired_enabled
                            or (action == "install" and confirmed["proof"]["revision"] != source["revision"])
                            or (state and (confirmed["features"] != state["features"] or confirmed["settings"] != state["settings"]))):
                        raise InventoryError("Efeito nativo não corresponde à ação solicitada")
                    records[name] = {"status": "managed", "source": source, "native": confirmed}
                self._save_ledger(ledger)
                native = after
            except (InventoryError, OSError, ValueError, subprocess.SubprocessError) as failure:
                # Quem falhou foi a ação; o desfazer que falhar também não pode tomar o lugar dela no relatório.
                try:
                    if prepared_pin is not None:
                        self._restore_pin(name, prepared_pin, state)
                    observed, _ = self._native(dry_run=True)
                    if observed.get(name) == state:
                        if record is None:
                            records.pop(name, None)
                        else:
                            records[name] = record
                    else:
                        records[name]["status"] = "suspended"
                    self._save_ledger(ledger)
                except (InventoryError, OSError, ValueError, subprocess.SubprocessError) as cleanup:
                    _log.warning("omp plugin %s: desfazer falhou (%s) após %s", name, cleanup, failure)
                raise failure


class PluginSyncLoop:
    """Uma passagem por vez, fora do event loop, com parada cooperativa aguardada."""

    def __init__(self, synchronizer, *, enabled=False, interval=300, permitted=lambda: True):
        if isinstance(interval, bool) or not isinstance(interval, (int, float)) or not isfinite(interval) or interval <= 0:
            raise ValueError("O intervalo deve ser positivo e finito")
        self.synchronizer = synchronizer
        self.enabled = enabled
        self.interval = interval
        self.permitted = permitted
        self._stop = threading.Event()
        self._wake = asyncio.Event()
        self._task = None
        self._interrupted = False
        self._state = "idle" if enabled else "disabled"
        self._report = None
        self._started_at = None
        self._completed_at = None

    def status(self):
        return {"enabled": self.enabled, "state": self._state, "interval": self.interval,
                "started_at": self._started_at, "completed_at": self._completed_at,
                "last_report": self._report}

    async def start(self):
        if self.enabled and self._task is None and not self._stop.is_set():
            self._task = asyncio.create_task(self._loop(), name="omp-plugin-sync")

    async def close(self):
        self._stop.set()
        self._wake.set()
        if self._task is not None:
            # Cancelar to_thread não interrompe o processo externo; aguardar a passagem é obrigatório.
            await asyncio.shield(self._task)
            self._state = "stopped"

    def _should_stop(self):
        self._interrupted = self._interrupted or self._stop.is_set() or not self.permitted()
        return self._interrupted

    def _cycle(self):
        reports = {}
        for name, operation in (("marketplaces", self.synchronizer.import_marketplaces),
                                ("plugins", self.synchronizer.reconcile)):
            if self._should_stop():
                break
            try:
                reports[name] = operation(stop_requested=self._should_stop)
            except Exception as error:
                reports[name] = {"complete_inventory": False, "items": [], "errors": [type(error).__name__]}
        return reports

    async def _wait(self):
        try:
            await asyncio.wait_for(self._wake.wait(), timeout=self.interval)
        except TimeoutError:
            pass
        self._wake.clear()

    async def _loop(self):
        while not self._stop.is_set():
            try:
                if not self.permitted():
                    self._state = "paused"
                else:
                    self._state = "running"
                    self._started_at = time.time()
                    self._interrupted = False
                    reports = await asyncio.to_thread(self._cycle)
                    self._report = reports
                    self._completed_at = time.time()
                    if self._stop.is_set():
                        self._state = "stopped"
                    elif self._interrupted or not self.permitted():
                        self._state = "paused"
                    elif any(r.get("errors") or r.get("complete_inventory") is not True for r in reports.values()):
                        self._state = "error"
                    else:
                        actions = {item.get("action") for r in reports.values() for item in r.get("items", [])}
                        if actions & {"suspended", "conflict"}:
                            self._state = "suspended"
                        elif actions & {"install", "uninstall", "enable", "disable", "adopt", "import"}:
                            self._state = "updated"
                        else:
                            self._state = "unchanged"
            except Exception as error:
                self._state = "error"
                self._report = {"errors": [type(error).__name__]}
                logging.getLogger("hangar").warning("Sincronização OMP interrompida: %s", type(error).__name__)
            if not self._stop.is_set():
                await self._wait()
