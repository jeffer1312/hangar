"""Política CLAUDE.md opt-in, sem sobrescrever contexto personalizado."""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

import yaml

from app import peers
from app.omp_plugin_sync import InventoryError, _run, resolve_omp_directories

RULE_TEMPLATE = Path(__file__).resolve().parents[2] / "scripts/omp/carregar-claude-md-do-projeto.md"
DISABLED_CONTEXT_IDS = ("context-file:project:AGENTS.md", "context-file:user:AGENTS.md")


def _rule_info(content):
    normalized = re.sub(r"<!--[\s\S]*?-->", "", content.replace("\r\n", "\n").replace("\r", "\n"))
    match = re.match(r"\A---[ \t]*\n(.*?)\n---[ \t]*(?:\n|$)(.*)\Z", normalized, re.S)
    if not match:
        return " ".join(normalized.split()), False
    body = " ".join(match[2].split())
    try:
        metadata = yaml.compose(match[1], Loader=yaml.SafeLoader)
    except yaml.YAMLError:
        return body, False
    if not isinstance(metadata, yaml.MappingNode):
        return body, False
    values = {}
    for key, value in metadata.value:
        if not isinstance(key, yaml.ScalarNode) or key.tag != "tag:yaml.org,2002:str":
            return body, False
        name = re.sub(r"-([a-z])", lambda m: m[1].upper(), key.value)
        if name in values:
            return body, False
        values[name] = value
    always = values.get("alwaysApply")
    if not isinstance(always, yaml.ScalarNode) or always.tag != "tag:yaml.org,2002:bool" or always.value.lower() != "true":
        return body, False
    for name, value in values.items():
        if name in {"alwaysApply", "enabled"}:
            if not isinstance(value, yaml.ScalarNode) or value.tag != "tag:yaml.org,2002:bool" or value.value.lower() != "true":
                return body, False
        elif name in {"name", "description"}:
            if value.tag not in {"tag:yaml.org,2002:str", "tag:yaml.org,2002:null"}:
                return body, False
        elif value.tag != "tag:yaml.org,2002:null" and value.value not in ("", [], {}):
            # Escopo/condição não demonstrados não podem ser promovidos a política incondicional.
            return body, False
    return body, True


def _points_to(link, target):
    return link.is_symlink() and link.resolve() == target.resolve()


def _preflight(agent, global_file, body):
    append = agent / "APPEND_SYSTEM.md"
    rules = agent / "rules"
    canonical = rules / RULE_TEMPLATE.name
    if (append.exists() or append.is_symlink()) and not _points_to(append, global_file):
        raise InventoryError("APPEND_SYSTEM.md personalizado preservado; configuração não aplicada")
    if rules.is_symlink() or (rules.exists() and not rules.is_dir()):
        raise InventoryError("Diretório de regras externo ou inválido preservado")
    entries = {}
    eligible = []
    restricted = False
    if rules.is_dir():
        for candidate in sorted(rules.iterdir()):
            if candidate.suffix not in {".md", ".mdc"} or not candidate.is_file():
                continue
            candidate_body, active = _rule_info(candidate.read_text(encoding="utf-8"))
            entries.setdefault(candidate.stem, []).append((candidate, candidate_body, active))
            if candidate_body == body:
                if active:
                    eligible.append(candidate)
                else:
                    restricted = True
    if eligible:
        names = {candidate.stem for candidate in eligible}
        if len(names) != 1:
            raise InventoryError("Mais de uma identidade de regra equivalente; política não aplicada")
        selected = eligible[0]
        if any(text != body or not active for _, text, active in entries[selected.stem]):
            raise InventoryError("Identidade da regra disputada por outro arquivo; preservada")
        return selected
    if restricted:
        raise InventoryError("Regra equivalente desativada ou restrita; preferência preservada")
    if canonical.exists() or canonical.is_symlink() or canonical.stem in entries:
        raise InventoryError("Regra de contexto personalizada preservada; configuração não aplicada")
    return None


def configure_claude_context(*, home=None, claude_dir=None, enabled=False, runner=None):
    result = {"enabled": enabled, "changed": False, "items": [], "errors": []}
    if not enabled:
        return result
    home = Path(home or Path.home()).resolve()
    claude_dir = Path(claude_dir or os.environ.get("CLAUDE_CONFIG_DIR") or home / ".claude").resolve()
    environment = dict(os.environ, HOME=str(home), USERPROFILE=str(home), CLAUDE_CONFIG_DIR=str(claude_dir))
    execute = runner or _run
    binary = "omp" if runner else (shutil.which("omp") or str(home / ".local/bin/omp"))
    try:
        agent = resolve_omp_directories(home, environment, home).agent_dir
        global_file = claude_dir / "CLAUDE.md"
        append = agent / "APPEND_SYSTEM.md"
        rules = agent / "rules"
        canonical = rules / RULE_TEMPLATE.name
        body, valid = _rule_info(RULE_TEMPLATE.read_text(encoding="utf-8"))
        if not valid:
            raise InventoryError("Template de contexto inválido")
        _preflight(agent, global_file, body)
        if peers.fcntl is None and peers.msvcrt is None:
            raise InventoryError("Trava de configuração indisponível")

        def command(action, key, value=None):
            args = [binary, "config", action, key]
            if value is not None:
                args.append(json.dumps(value, ensure_ascii=False))
            args.append("--json")
            completed = execute(args, cwd=home, env=environment, timeout=30)
            if completed.returncode:
                raise InventoryError(f"Configuração nativa OMP falhou (código {completed.returncode})")
            response = json.loads(completed.stdout)
            values = response.get("value") if isinstance(response, dict) else None
            if not isinstance(values, list) or any(not isinstance(item, str) for item in values):
                raise InventoryError("Controle nativo de regras inválido")
            return values

        def observe_controls(previous=None):
            # Cada `omp config get` é um processo (~0,75 s); as três chaves não dependem uma da outra.
            # Na releitura só `disabledExtensions` pode ter mudado — é a única que o `set` toca.
            keys = ("disabledExtensions",) if previous else ("disabledExtensions", "ttsr.disabledRules", "disabledProviders")
            with ThreadPoolExecutor(max_workers=len(keys)) as pool:
                fetched = dict(zip(keys, pool.map(lambda key: command("get", key), keys)))
            controls = {**(previous or {}), **fetched}
            selected = _preflight(agent, global_file, body)
            name = (selected or canonical).stem
            if ("native" in controls["disabledProviders"]
                    or "rule:" + name in controls["disabledExtensions"]
                    or name in {entry.strip() for entry in controls["ttsr.disabledRules"]}):
                raise InventoryError("Política bloqueada pela configuração pessoal; bloqueio preservado")
            return controls, selected

        agent.mkdir(parents=True, exist_ok=True)
        lock_path = agent / ".hangar-claude-context.lock"
        if lock_path.is_symlink():
            raise InventoryError("Trava simbólica de configuração recusada")
        with lock_path.open("a+", encoding="utf-8") as lock:
            peers._travar(lock)
            try:
                _preflight(agent, global_file, body)
                controls, selected = observe_controls()
                previous = controls["disabledExtensions"]
                merged = previous + [item for item in DISABLED_CONTEXT_IDS if item not in previous]
                if merged != previous:
                    if command("set", "disabledExtensions", merged) != merged:
                        raise InventoryError("CLI não confirmou a configuração solicitada")
                    result["changed"] = True
                controls, selected = observe_controls(controls)
                if not set(DISABLED_CONTEXT_IDS).issubset(controls["disabledExtensions"]):
                    raise InventoryError("Configuração de contexto mudou durante a operação")
                result["global_context"] = "available" if global_file.is_file() else "missing"
                if global_file.is_file() and not append.is_symlink():
                    _preflight(agent, global_file, body)
                    append.symlink_to(global_file)
                    result["changed"] = True
                if selected is None:
                    _preflight(agent, global_file, body)
                    rules.mkdir(parents=True, exist_ok=True)
                    if _preflight(agent, global_file, body) is not None:
                        raise InventoryError("Regra mudou durante a configuração")
                    canonical.symlink_to(RULE_TEMPLATE)
                    selected = canonical
                    result["changed"] = True
                effective = _preflight(agent, global_file, body)
                if effective is None or effective.stem != selected.stem:
                    raise InventoryError("Regra efetiva mudou durante a configuração")
                result["items"] = [{"item": "context-files", "state": "configured"},
                                   {"item": "rule", "state": "configured" if selected == canonical else "reused", "name": selected.stem}]
            finally:
                peers._destravar(lock)
    except (InventoryError, OSError, ValueError, subprocess.SubprocessError) as error:
        result["errors"].append(str(error) if isinstance(error, InventoryError) else type(error).__name__)
    return result
