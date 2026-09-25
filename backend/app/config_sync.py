"""Configuração compartilhada entre máquinas Hangar.

Exporta a configuração de Claude Code e Codex desta máquina num pacote canônico (caminho vira
marcador) e aplica aqui o pacote de outra máquina, com backup. Quem envia vence; o que depende
desta máquina (hooks e skills do Hangar, MCP `hangar`, caminhos e programas) continua daqui.
"""
import asyncio
import errno
import hashlib
import inspect
import io
import json
import os
import re
import shutil
import tarfile
import time
import tomllib
import uuid
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath, PureWindowsPath

from app import atomico, codex_contas_sync, hook_installer, runtime_config
from app.config_sync_paths import (HEAVY_DIRS, PROGRAMS, Roots, canonicalize, local_path,
                                   map_strings, mark, marked_paths, resolve)

VERSION = 1
MAX_BUNDLE = 90 * 1024 * 1024
MAX_UNPACKED = 300 * 1024 * 1024
MAX_REF_DIR = 20 * 1024 * 1024
ITEMS = ("claude_instructions", "claude_skills", "claude_agents", "claude_hooks",
         "claude_plugins", "claude_mcp", "claude_env", "claude_settings", "codex", "engines",
         "hangar_prefs")
_DIRS = {"claude_instructions": ("rules",), "claude_skills": ("skills",),
         "claude_agents": ("agents", "commands", "output-styles"), "claude_hooks": ("hooks",)}
# Chaves do settings.json com item próprio; o resto vai em claude_settings.
_OWNED_SETTINGS = frozenset({"hooks", "statusLine", "enabledPlugins", "extraKnownMarketplaces",
                             "env"})
# Preferências que descrevem ESTA máquina: cofre da lista de servidores, origens do terminal e
# pastas do seletor.
_MACHINE_PREFS = frozenset({"sync", "term_origins", "scan_roots"})
_HANGAR_MCP = "hangar"


@dataclass
class FileBlob:
    data: bytes
    mode: int


@dataclass
class Bundle:
    items: dict[str, dict] = field(default_factory=dict)
    files: dict[str, FileBlob] = field(default_factory=dict)
    warnings: dict[str, list[dict]] = field(default_factory=dict)


class BundleError(Exception):
    def __init__(self, code: str, msg: str, **params):
        super().__init__(msg)
        self.code = code
        self.params = {k: str(v) for k, v in params.items()}


class BundleTooBig(Exception):
    def __init__(self, largest: list[tuple[str, int]]):
        super().__init__("pacote maior que o teto: "
                         + ", ".join(f"{item} ({size} bytes)" for item, size in largest))
        self.largest = largest


def _warn(code: str, **params) -> dict:
    return {"code": code, "params": {k: str(v) for k, v in params.items()}}


def _hash(value) -> str:
    raw = value if isinstance(value, bytes) else json.dumps(
        value, sort_keys=True, ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(raw).hexdigest()


def _mode(path: Path) -> int:
    return 0o755 if path.stat().st_mode & 0o111 else 0o644


def _read_json(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}
    if not isinstance(data, dict):
        raise BundleError("config_sync_invalid_json", f"{path.name} não é um objeto JSON",
                          file=path.name)
    return data


def _settings(roots: Roots) -> dict:
    return _read_json(Path(roots.claude) / "settings.json")


def _canon(value, roots: Roots):
    return map_strings(value, lambda s: canonicalize(s, roots))


def _uncanon(value, roots: Roots):
    return map_strings(value, lambda s: resolve(s, roots))


def _read_blob(path: Path, roots: Roots) -> tuple[FileBlob, bool]:
    """Conteúdo do arquivo; texto ganha marcador no lugar dos caminhos desta máquina. Devolve se
    trocou algo: o destino só resolve marcador no arquivo que a origem marcou."""
    data = path.read_bytes()
    if b"\0" not in data:
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            text = None
        if text is not None and (canon := canonicalize(text, roots)) != text:
            return FileBlob(canon.encode("utf-8"), _mode(path)), True
    return FileBlob(data, _mode(path)), False


def _hangar_skill_names(roots: Roots) -> set[str]:
    folder = Path(roots.hangar) / "skills"
    return {p.name for p in folder.iterdir()} if folder.is_dir() else set()


def _hangar_hook_names(roots: Roots) -> set[str]:
    return {p.name for p in (Path(roots.hangar) / "backend" / "hooks").glob("*.py")}


def _is_hangar_hook(command, own: set[str]) -> bool:
    return any(hook_installer._refers_to(command, name, por_nome=True) for name in own)


def _split_hooks(groups, own: set[str], *, hangar: bool) -> list:
    """Grupos de hooks só com os do Hangar (`hangar=True`) ou só com os outros."""
    kept = []
    for group in groups if isinstance(groups, list) else []:
        if not isinstance(group, dict):
            continue
        inner = [h for h in group.get("hooks") or []
                 if isinstance(h, dict) and _is_hangar_hook(h.get("command"), own) == hangar]
        if inner:
            kept.append({**group, "hooks": inner})
    return kept


def _hook_commands(hooks: dict) -> list[str]:
    return [h["command"] for groups in hooks.values() for g in groups
            for h in g.get("hooks") or [] if isinstance(h.get("command"), str)]


def _points_into(path: Path, root: str) -> bool:
    return path.is_symlink() and Path(os.path.realpath(path)).is_relative_to(Path(root).resolve())


def _walk(root: Path, roots: Roots, entry: str,
          warnings: list[dict]) -> tuple[dict[str, FileBlob], list[str]]:
    """Arquivos de `root`, seguindo links e sem as pastas pesadas. Chave = caminho relativo com
    `/`; arquivo solto tem chave vazia. Devolve também os relativos que ganharam marcador."""
    if root.is_file():
        blob, marked = _read_blob(root, roots)
        return {"": blob}, [""] if marked else []
    files: dict[str, FileBlob] = {}
    texts: list[str] = []
    seen: set[str] = set()
    skipped: set[str] = set()
    for current, dirs, names in os.walk(root, followlinks=True):
        real = os.path.realpath(current)
        if real in seen:   # link em ciclo
            dirs[:] = []
            continue
        seen.add(real)
        skipped |= {d for d in dirs if d in HEAVY_DIRS and d != "__pycache__"}
        dirs[:] = sorted(d for d in dirs if d not in HEAVY_DIRS)
        for name in sorted(names):
            if name.startswith(".hangar"):
                continue
            path = Path(current) / name
            rel = path.relative_to(root).as_posix()
            try:
                blob, marked = _read_blob(path, roots)
            except OSError as exc:
                warnings.append(_warn("config_sync_unreadable", entry=f"{entry}/{rel}",
                                      error=type(exc).__name__))
                continue
            files[rel] = blob
            if marked:
                texts.append(rel)
    for folder in sorted(skipped):
        warnings.append(_warn("config_sync_heavy_dir_skipped", entry=entry, dir=folder))
    return files, texts


def _files_hash(files: dict[str, FileBlob]) -> str:
    return _hash([[rel, hashlib.sha256(b.data).hexdigest(), bool(b.mode & 0o111)]
                  for rel, b in sorted(files.items())])


def _member(item: str, entry: str, rel: str) -> str:
    return f"files/{item}/{entry}" + (f"/{rel}" if rel else "")


def _entries(roots: Roots, item: str, warnings: list[dict]) -> dict[str, Path]:
    claude = Path(roots.claude)
    found: dict[str, Path] = {}
    if item == "claude_instructions":
        for path in sorted(claude.glob("*.md")):
            if path.name != "CLAUDE.local.md":
                found[path.name] = path
    own_skills, own_hooks = _hangar_skill_names(roots), _hangar_hook_names(roots)
    for folder in _DIRS.get(item, ()):
        base = claude / folder
        if not base.is_dir():
            continue
        for path in sorted(base.iterdir()):
            if path.name.startswith("."):
                continue
            if folder == "skills" and path.name in own_skills:
                continue
            if folder == "hooks" and (path.name in own_hooks or _points_into(path, roots.hangar)):
                continue
            if not path.exists():
                warnings.append(_warn("config_sync_broken_link", entry=f"{folder}/{path.name}"))
                continue
            found[f"{folder}/{path.name}"] = path
    return found


def _export_dir_item(roots: Roots, item: str, bundle: Bundle) -> dict:
    warnings = bundle.warnings.setdefault(item, [])
    entries: dict[str, dict] = {}
    for name, path in _entries(roots, item, warnings).items():
        files, texts = _walk(path, roots, name, warnings)
        for rel, blob in files.items():
            bundle.files[_member(item, name, rel)] = blob
        entries[name] = {"kind": "file" if "" in files else "dir", "files": sorted(files),
                         "text": texts, "hash": _files_hash(files)}
    return {"entries": entries, "hashes": {n: e["hash"] for n, e in entries.items()}}


def _ref_files(marked: str, local: Path, roots: Roots) -> dict[str, Path]:
    """O arquivo do comando e, se a pasta dele estiver a dois níveis ou mais do marcador e for
    pequena, a pasta inteira: script costuma chamar vizinho (`. ./lib.sh`)."""
    rest = marked.split("⟧", 1)[1].replace("\\", "/").strip("/")
    if len(PurePosixPath(rest).parts) < 3:
        return {marked: local}
    found: dict[str, Path] = {}
    total = 0
    for current, dirs, names in os.walk(local.parent):
        dirs[:] = sorted(d for d in dirs if d not in HEAVY_DIRS)
        for name in sorted(names):
            path = Path(current) / name
            if not path.is_file():
                continue
            total += path.stat().st_size
            if total > MAX_REF_DIR:
                return {marked: local}
            found[canonicalize(str(path), roots).replace("\\", "/")] = path
    return found or {marked: local}


def _export_refs(roots: Roots, commands: list[str], bundle: Bundle) -> dict[str, dict]:
    refs: dict[str, dict] = {}
    for command in commands:
        for marked in marked_paths(command):
            if marked.startswith(mark("HANGAR")):
                refs.setdefault(marked, {"member": ""})   # código do Hangar: o destino só confere
                continue
            name = PurePosixPath(marked.replace("\\", "/")).name.removesuffix(".exe").lower()
            local = local_path(marked, roots)
            if name in PROGRAMS or not local.is_file():
                continue   # programa, pasta ou caminho inexistente: nada para levar
            for ref, path in _ref_files(marked, local, roots).items():
                if ref in refs:
                    continue
                blob, text = _read_blob(path, roots)
                member = "refs/" + hashlib.sha1(ref.encode("utf-8")).hexdigest()
                bundle.files[member] = blob
                refs[ref] = {"member": member, "text": text}
    return refs


def _ref_hashes(refs: dict[str, dict], bundle: Bundle) -> dict[str, str]:
    """Sem isto, duas máquinas que só diferem no script de um hook dariam o mesmo manifesto. Ref do
    Hangar fica fora: o conteúdo dele é o código do Hangar do destino."""
    return {f"ref:{ref}": _hash(bundle.files[r["member"]].data)
            for ref, r in refs.items() if r["member"]}


def _export_hooks(roots: Roots, bundle: Bundle) -> dict:
    data = _export_dir_item(roots, "claude_hooks", bundle)
    settings = _settings(roots)
    own = _hangar_hook_names(roots)
    hooks = {}
    for event, groups in (settings.get("hooks") or {}).items():
        mine = _split_hooks(groups, own, hangar=False)
        if mine:
            hooks[event] = _canon(mine, roots)
    data["hooks"] = hooks
    commands = _hook_commands(hooks)
    if isinstance(settings.get("statusLine"), dict):
        data["statusLine"] = _canon(settings["statusLine"], roots)
        commands.append(str(data["statusLine"].get("command") or ""))
        data["hashes"]["statusLine"] = _hash(data["statusLine"])
    data["hashes"].update({f"hooks:{event}": _hash(g) for event, g in hooks.items()})
    data["refs"] = _export_refs(roots, commands, bundle)
    data["hashes"].update(_ref_hashes(data["refs"], bundle))
    return data


def _export_plugins(roots: Roots, bundle: Bundle) -> dict:
    warnings = bundle.warnings.setdefault("claude_plugins", [])
    settings = _settings(roots)
    enabled = settings.get("enabledPlugins")
    enabled = enabled if isinstance(enabled, dict) else {}
    extra = settings.get("extraKnownMarketplaces")
    extra = extra if isinstance(extra, dict) else {}
    known = _read_json(Path(roots.claude) / "plugins" / "known_marketplaces.json")
    sources = {}
    for name in sorted({p.split("@", 1)[1] for p in enabled if "@" in p} | set(extra)):
        source = known[name].get("source") if isinstance(known.get(name), dict) else None
        if not isinstance(source, dict) and isinstance(extra.get(name), dict):
            source = extra[name].get("source")
        if isinstance(source, dict):
            sources[name] = _canon(source, roots)
        else:
            warnings.append(_warn("config_sync_marketplace_unknown", marketplace=name))
    hashes = {f"plugin:{p}": _hash(v) for p, v in enabled.items()}
    hashes.update({f"marketplace:{n}": _hash(s) for n, s in sources.items()})
    return {"enabledPlugins": enabled, "extraKnownMarketplaces": _canon(extra, roots),
            "sources": sources, "hashes": hashes}


def _export_mcp(roots: Roots, bundle: Bundle) -> dict:
    data = _read_json(Path(roots.home) / ".claude.json")
    servers = data.get("mcpServers") if isinstance(data.get("mcpServers"), dict) else {}
    servers = _canon({k: v for k, v in servers.items() if k != _HANGAR_MCP}, roots)
    commands = [" ".join([str(s.get("command") or "")]
                         + [a for a in s.get("args") or [] if isinstance(a, str)])
                for s in servers.values() if isinstance(s, dict)]
    refs = _export_refs(roots, commands, bundle)
    return {"servers": servers, "refs": refs,
            "hashes": {k: _hash(v) for k, v in servers.items()} | _ref_hashes(refs, bundle)}


def _export_env(roots: Roots, bundle: Bundle) -> dict:
    env = _settings(roots).get("env")
    env = _canon(env if isinstance(env, dict) else {}, roots)
    return {"env": env, "hashes": {k: _hash(v) for k, v in env.items()}}


def _export_settings(roots: Roots, bundle: Bundle) -> dict:
    rest = _canon({k: v for k, v in _settings(roots).items() if k not in _OWNED_SETTINGS}, roots)
    return {"settings": rest, "hashes": {k: _hash(v) for k, v in rest.items()}}


def _engines_path(roots: Roots) -> Path:
    return Path(os.environ.get("CP_ENGINES_FILE") or Path(roots.claude) / "engines.json")


def _export_engines(roots: Roots, bundle: Bundle) -> dict:
    engines = _canon(_read_json(_engines_path(roots)), roots)
    return {"engines": engines, "hashes": {k: _hash(v) for k, v in engines.items()}}


def _export_prefs(roots: Roots, bundle: Bundle) -> dict:
    prefs = _canon({k: v for k, v in runtime_config._carregar().items()
                    if k in runtime_config.EDITAVEIS and k not in _MACHINE_PREFS}, roots)
    return {"prefs": prefs, "hashes": {k: _hash(v) for k, v in prefs.items()}}


def _jsonable(value) -> bool:
    try:
        json.dumps(value)
    except (TypeError, ValueError):
        return False
    return True


def _export_codex(roots: Roots, bundle: Bundle) -> dict:
    warnings = bundle.warnings.setdefault("codex", [])
    codex = Path(roots.codex)
    data: dict = {"hashes": {}}
    agents = codex / "AGENTS.md"
    if agents.is_file():
        blob, text = _read_blob(agents, roots)
        bundle.files["files/codex/AGENTS.md"] = blob
        data["agents_md"] = {"member": "files/codex/AGENTS.md", "text": text}
        data["hashes"]["AGENTS.md"] = _hash(blob.data)
    raw = codex / "config.toml"
    config = tomllib.loads(raw.read_text(encoding="utf-8")) if raw.is_file() else {}
    prefs = {}
    for key, value in config.items():
        if key not in codex_contas_sync.PREFERENCE_KEYS:
            continue
        if key == "mcp_servers" and isinstance(value, dict):
            value = {k: v for k, v in value.items() if k != _HANGAR_MCP}
        if not _jsonable(value):   # data do TOML não tem forma em JSON
            warnings.append(_warn("config_sync_unsupported_value", key=key))
            continue
        prefs[key] = value
    data["config"] = _canon(prefs, roots)
    data["hashes"].update({f"config:{k}": _hash(v) for k, v in data["config"].items()})
    return data


_EXPORTERS = {
    "claude_instructions": lambda r, b: _export_dir_item(r, "claude_instructions", b),
    "claude_skills": lambda r, b: _export_dir_item(r, "claude_skills", b),
    "claude_agents": lambda r, b: _export_dir_item(r, "claude_agents", b),
    "claude_hooks": _export_hooks,
    "claude_plugins": _export_plugins,
    "claude_mcp": _export_mcp,
    "claude_env": _export_env,
    "claude_settings": _export_settings,
    "codex": _export_codex,
    "engines": _export_engines,
    "hangar_prefs": _export_prefs,
}


def _item_bytes(bundle: Bundle, item: str) -> int:
    data = bundle.items.get(item) or {}
    members = {m for m in bundle.files if m.startswith(f"files/{item}/")}
    members |= {r["member"] for r in (data.get("refs") or {}).values() if r.get("member")}
    return sum(len(bundle.files[m].data) for m in members if m in bundle.files)


def export_bundle(roots: Roots, items, *, limit: bool = True) -> Bundle:
    bundle = Bundle()
    for item in items:
        try:
            bundle.items[item] = _EXPORTERS[item](roots, bundle)
        except (OSError, ValueError, BundleError) as exc:
            for member in [m for m in bundle.files if m.startswith(f"files/{item}/")]:
                del bundle.files[member]
            bundle.warnings.setdefault(item, []).append(
                _warn("config_sync_item_failed", item=item, error=str(exc)[:300]))
    if limit and sum(len(b.data) for b in bundle.files.values()) > MAX_BUNDLE:
        sizes = sorted(((i, _item_bytes(bundle, i)) for i in bundle.items),
                       key=lambda p: p[1], reverse=True)
        raise BundleTooBig(sizes[:3])
    return bundle


def manifest(roots: Roots) -> dict:
    """Só impressões digitais, na forma canônica: duas máquinas com a mesma configuração dão os
    mesmos hashes mesmo com casas diferentes. Nada de conteúdo, nem de segredo."""
    bundle = export_bundle(roots, ITEMS, limit=False)
    return {"version": VERSION, "items": {
        item: {"ok": item in bundle.items,
               "hashes": (bundle.items.get(item) or {}).get("hashes", {}),
               "bytes": _item_bytes(bundle, item),
               "warnings": bundle.warnings.get(item, [])}
        for item in ITEMS}}


def _add(tar: tarfile.TarFile, name: str, data: bytes, mode: int) -> None:
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = mode & 0o777
    tar.addfile(info, io.BytesIO(data))


def pack(bundle: Bundle) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        meta = {"version": VERSION, "items": bundle.items, "warnings": bundle.warnings}
        _add(tar, "manifest.json", json.dumps(meta, ensure_ascii=False).encode("utf-8"), 0o600)
        for name, blob in sorted(bundle.files.items()):
            _add(tar, name, blob.data, blob.mode)
    return buf.getvalue()


_MEMBER = re.compile(r"^(?:manifest\.json|files/[a-z_]+/.+|refs/[0-9a-f]{40})$")


def unpack(raw: bytes) -> Bundle:
    """Lê o pacote só na memória: nada é extraído para o disco pelo tarfile."""
    try:
        tar = tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz")
        members = tar.getmembers()
    except (tarfile.TarError, OSError, EOFError) as exc:
        raise BundleError("config_sync_invalid_bundle", "pacote ilegível") from exc
    bundle, meta, total = Bundle(), None, 0
    with tar:
        for member in members:
            name = member.name
            if (not member.isfile() or not _MEMBER.match(name)
                    or ".." in PurePosixPath(name).parts):
                raise BundleError("config_sync_invalid_bundle",
                                  f"entrada inválida no pacote: {name}", entry=name)
            total += member.size
            if total > MAX_UNPACKED:
                raise BundleError("config_sync_invalid_bundle", "pacote grande demais aberto")
            data = tar.extractfile(member).read()
            if name == "manifest.json":
                try:
                    meta = json.loads(data.decode("utf-8"))
                except ValueError as exc:
                    raise BundleError("config_sync_invalid_bundle", "manifesto ilegível") from exc
            else:
                bundle.files[name] = FileBlob(data, member.mode or 0o644)
    if not isinstance(meta, dict):
        raise BundleError("config_sync_invalid_bundle", "pacote sem manifesto")
    if meta.get("version") != VERSION:
        raise BundleError("config_sync_version", "o pacote veio de um Hangar com outro formato",
                          version=meta.get("version"))
    bundle.items = meta.get("items") if isinstance(meta.get("items"), dict) else {}
    bundle.warnings = meta.get("warnings") if isinstance(meta.get("warnings"), dict) else {}
    return bundle


@dataclass
class _Apply:
    roots: Roots
    backups: Path
    report: dict[str, dict]
    bundle: Bundle
    runner: object = None


def _result(ctx: _Apply, item: str) -> dict:
    if item not in ctx.report:
        ctx.report[item] = {"status": "same", "changed": [],
                            "warnings": list(ctx.bundle.warnings.get(item) or [])}
    return ctx.report[item]


def _backup_target(path: Path, ctx: _Apply) -> Path:
    """Lugar no backup desta rodada, espelhando o caminho a partir da casa."""
    try:
        rel = path.absolute().relative_to(Path(ctx.roots.home))
    except ValueError:
        rel = Path(*path.absolute().parts[1:])
    target = ctx.backups / "files" / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    os.chmod(ctx.backups, 0o700)   # pode ter segredo
    return target


def _move(src: Path, dst: Path) -> None:
    """Rename puro; só copia entre discos. Com arquivo aberto no Windows o rename falha, e o
    `shutil.move` cairia em copiar e apagar a origem, que para no meio e deixa a pasta pela
    metade."""
    try:
        os.rename(src, dst)
    except OSError as exc:
        if exc.errno != errno.EXDEV:
            raise
        shutil.move(str(src), str(dst))


def _current(path: Path) -> dict[str, tuple[bytes, bool]] | None:
    """O que está hoje no destino, seguindo links e sem as pastas pesadas; None se não existe."""
    if not path.exists():
        return None
    if path.is_file():
        return {"": (path.read_bytes(), bool(path.stat().st_mode & 0o111))}
    found: dict[str, tuple[bytes, bool]] = {}
    seen: set[str] = set()
    for current, dirs, names in os.walk(path, followlinks=True):
        real = os.path.realpath(current)
        if real in seen:
            dirs[:] = []
            continue
        seen.add(real)
        dirs[:] = [d for d in dirs if d not in HEAVY_DIRS]
        for name in names:
            p = Path(current) / name
            if p.is_file() and not name.startswith(".hangar"):
                found[p.relative_to(path).as_posix()] = (p.read_bytes(),
                                                        bool(p.stat().st_mode & 0o111))
    return found


def _same(current, files: dict[str, FileBlob]) -> bool:
    if current is None or set(current) != set(files):
        return False
    return all(current[rel][0] == b.data
               and (os.name == "nt" or current[rel][1] == bool(b.mode & 0o111))
               for rel, b in files.items())


def _write_file(path: Path, blob: FileBlob) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.hangar-novo-{uuid.uuid4().hex[:8]}")
    tmp.write_bytes(blob.data)
    os.chmod(tmp, blob.mode & 0o777)
    atomico.substituir(tmp, path)


def _write_entry(dest: Path, kind: str, files: dict[str, FileBlob], ctx: _Apply, item: str,
                 name: str) -> bool:
    """Deixa `dest` igual ao que veio e devolve se mudou algo. O que sai vai para o backup; as
    pastas pesadas que já existiam aqui (venv, node_modules, .git) continuam."""
    if _same(_current(dest), files):
        return False
    staging = None
    if kind != "file":
        staging = dest.with_name(f".{dest.name}.hangar-novo-{uuid.uuid4().hex[:8]}")
        try:
            staging.mkdir(parents=True)
            for rel, blob in files.items():
                _write_file(staging / rel, blob)
        except BaseException:
            shutil.rmtree(staging, ignore_errors=True)
            raise
    link = os.readlink(dest) if dest.is_symlink() else None
    done: list[tuple[Path, Path]] = []   # (onde ficou, de onde saiu), para desfazer

    def move(src: Path, dst: Path) -> None:
        _move(src, dst)
        done.append((dst, src))

    # Falha no meio desfaz na ordem inversa: sem isso a entrada some do destino e o .git/.venv
    # fica preso na pasta escondida.
    try:
        if kind == "file":
            if link is not None or dest.is_dir():
                move(dest, _backup_target(dest, ctx))
            elif dest.exists():
                shutil.copy2(dest, _backup_target(dest, ctx))
            _write_file(dest, files[""])
        else:
            if link is None and dest.is_dir():
                for heavy in sorted(HEAVY_DIRS):
                    old = dest / heavy
                    if old.is_dir() and not old.is_symlink():
                        move(old, staging / heavy)
            if link is not None or dest.exists():
                move(dest, _backup_target(dest, ctx))
            _move(staging, dest)
    except BaseException:
        for now, back in reversed(done):
            _move(now, back)
        if staging is not None:
            shutil.rmtree(staging, ignore_errors=True)
        raise
    if link is not None:
        _result(ctx, item)["warnings"].append(
            _warn("config_sync_link_replaced", entry=name, target=link))
    return True


def _blob(ctx: _Apply, member: str, text: bool) -> FileBlob:
    blob = ctx.bundle.files[member]
    if not text:
        return blob
    return FileBlob(resolve(blob.data.decode("utf-8"), ctx.roots).encode("utf-8"), blob.mode)


_ENTRY = re.compile(r"^(?:[^/\\]+|(?:rules|skills|agents|commands|output-styles|hooks)/[^/\\]+)$")


def _escapes(rel: str) -> bool:
    """Caminho dentro da entrada que sairia da pasta. Contrabarra e letra de drive só escapam no
    destino Windows, mas são recusadas em qualquer um; `:` solto continua valendo no Linux."""
    return ("\\" in rel or rel.startswith("/") or ".." in PurePosixPath(rel).parts
            or bool(PureWindowsPath(rel).drive))


def _apply_dir_item(ctx: _Apply, item: str) -> None:
    res = _result(ctx, item)
    own_skills, own_hooks = _hangar_skill_names(ctx.roots), _hangar_hook_names(ctx.roots)
    allowed = _DIRS[item] + (("",) if item == "claude_instructions" else ())
    for name, entry in sorted((ctx.bundle.items[item].get("entries") or {}).items()):
        folder, _, base = name.rpartition("/")
        rels = entry.get("files") or []
        # Nome com contrabarra já não passa no _ENTRY.
        if (not _ENTRY.match(name) or folder not in allowed or base in ("", ".", "..")
                or (folder == "" and not base.endswith(".md"))
                or PureWindowsPath(name).drive or any(_escapes(r) for r in rels)):
            res["warnings"].append(_warn("config_sync_invalid_entry", entry=name))
            continue
        if folder == "skills" and base in own_skills:
            res["warnings"].append(_warn("config_sync_hangar_skill_kept", entry=name))
            continue
        if folder == "hooks" and base in own_hooks:
            continue
        texts = set(entry.get("text") or [])
        files = {rel: _blob(ctx, _member(item, name, rel), rel in texts) for rel in rels}
        if _write_entry(Path(ctx.roots.claude) / name, entry.get("kind", "dir"), files, ctx,
                        item, name):
            res["changed"].append(name)


_REF_PREFIXES = tuple(mark(m) for m in ("CLAUDE", "CODEX", "HOME"))


def _apply_refs(ctx: _Apply, item: str) -> None:
    """Arquivos que os comandos usam. Os do Hangar só são conferidos: é código do Hangar daqui."""
    res = _result(ctx, item)
    for marked, ref in sorted((ctx.bundle.items[item].get("refs") or {}).items()):
        local = local_path(marked, ctx.roots)
        if marked.startswith(mark("HANGAR")):
            if not local.exists():
                res["warnings"].append(_warn("config_sync_hangar_outdated", file=str(local)))
            continue
        rest = marked.split("⟧", 1)[-1].replace("\\", "/")
        if not marked.startswith(_REF_PREFIXES) or ".." in PurePosixPath(rest).parts:
            res["warnings"].append(_warn("config_sync_invalid_entry", entry=marked))
            continue
        blob = _blob(ctx, ref["member"], bool(ref.get("text")))
        if _write_entry(local, "file", {"": blob}, ctx, item, marked):
            res["changed"].append(str(local))


def _apply_hooks(ctx: _Apply) -> None:
    _apply_dir_item(ctx, "claude_hooks")
    _apply_refs(ctx, "claude_hooks")


_APPLIERS = {
    "claude_instructions": lambda ctx: _apply_dir_item(ctx, "claude_instructions"),
    "claude_skills": lambda ctx: _apply_dir_item(ctx, "claude_skills"),
    "claude_agents": lambda ctx: _apply_dir_item(ctx, "claude_agents"),
    "claude_hooks": _apply_hooks,
}


def _new_backups(roots: Roots) -> Path:
    stamp = f"{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
    return Path(roots.home) / ".hangar" / "config-sync" / "backups" / stamp


async def apply_bundle(bundle: Bundle, items: list[str], roots: Roots, *, runner=None,
                       after=None) -> dict:
    """Aplica os itens escolhidos, na ordem de ITEMS. Item que quebra vira `failed` no relatório
    e os outros seguem: parar no meio deixaria a máquina pela metade sem dizer o quê."""
    ctx = _Apply(roots=roots, backups=_new_backups(roots), report={}, bundle=bundle,
                 runner=runner)
    chosen = [i for i in ITEMS if i in items]
    for item in chosen:
        res = _result(ctx, item)
        applier = _APPLIERS.get(item)
        if item not in bundle.items or applier is None:
            res["status"] = "failed"
            res["warnings"].append(_warn("config_sync_item_missing", item=item))
            continue
        try:
            if inspect.iscoroutinefunction(applier):
                await applier(ctx)
            else:
                await asyncio.to_thread(applier, ctx)
        except Exception as exc:  # noqa: BLE001 — um item quebrado não para os outros
            res["status"] = "failed"
            res["warnings"].append(_warn("config_sync_item_failed", item=item,
                                         error=str(exc)[:300]))
            continue
        res["status"] = "applied" if res["changed"] else "same"
    if after is not None:
        await after(ctx, chosen)
    return {"items": ctx.report, "backup": str(ctx.backups) if ctx.backups.exists() else ""}
