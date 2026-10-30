"""Captura escopos e efeitos Git reais sem ler configurações pessoais."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch

loader = Path(__file__).with_name("generate_instructions.py")
scope = {"__file__": str(loader)}
exec(compile(loader.read_text().split("cases=[]", 1)[0], str(loader), "exec"), scope)
original = scope["ns"]
cases = []

for name in ["scopes", "uppercase", "registered", "explicit-isolated", "project-only", "outside-git", "repeat-exclude", "empty-exclude", "unterminated-exclude", "existing-exclude", "registered-record", "remove-source", "disabled"]:
    with tempfile.TemporaryDirectory() as raw, patch.dict(os.environ, {"HOME": raw, "USERPROFILE": raw, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}):
        home = Path(raw)
        claude, codex, project = home / ".claude", home / ".codex", home / "project"
        for path in [claude, codex, project / "src", home / "other"]:
            path.mkdir(parents=True)
        (claude / "CLAUDE.md").write_text("GLOBAL\n", encoding="utf-8")
        (home / "CLAUDE.md").write_text("FORA\n", encoding="utf-8")
        (project / ("CLAUDE.MD" if name == "uppercase" else "CLAUDE.md")).write_text("RAIZ\n", encoding="utf-8")
        (project / "src/CLAUDE.md").write_text("LOCAL\n", encoding="utf-8")
        (home / "other/CLAUDE.md").write_text("ALHEIO\n", encoding="utf-8")
        (home / "other/AGENTS.override.md").write_text("PESSOAL\n", encoding="utf-8")
        if name == "project-only":
            (codex / "AGENTS.override.md").write_text("GLOBAL HERDADO\n", encoding="utf-8")
        has_git = name != "outside-git"
        initial_exclude = {"empty-exclude": "", "unterminated-exclude": "pessoal", "existing-exclude": "AGENTS.override.md\n"}.get(name, "# pessoal\n")
        if has_git:
            subprocess.run(["git", "init", "-q", str(project)], check=True, capture_output=True)
            (project / ".git/info/exclude").write_text(initial_exclude, encoding="utf-8")
        cwd = None if name == "registered" else project / "src"
        if name in ["registered", "explicit-isolated"]:
            registered = project / "src" if name == "registered" else home / "other"
            (codex / "config.toml").write_text(f"[projects.{json.dumps(str(registered))}]\ntrust_level = 'trusted'\n", encoding="utf-8")
        before = {str(path.relative_to(home)): path.read_bytes().hex() for path in home.rglob("*") if path.is_file() and ".git" not in path.parts}
        results = []
        rounds = 2 if name in ["repeat-exclude", "registered-record", "remove-source"] else 1
        for iteration in range(rounds):
            if iteration and name == "registered-record":
                cwd = None
                (project / "src/CLAUDE.md").write_text("ALTERADO\n", encoding="utf-8")
            if iteration and name == "remove-source":
                (project / "src/CLAUDE.md").unlink()
            saved = original["_sync_enabled"]
            if name == "disabled":
                original["_sync_enabled"] = lambda force: False
            try:
                if name == "project-only":
                    original["preparar_projeto"](codex, cwd, force=True)
                else:
                    original["preparar_instrucoes"](home, codex, cwd, force=True)
                results.append({"ok": True})
            except Exception as error:
                results.append({"error": type(error).__name__, "message": str(error)})
            finally:
                original["_sync_enabled"] = saved
        files, records, backups = {}, [], []
        for path in home.rglob("*"):
            if ".git" in path.parts or path.name == ".hangar-instrucoes.lock":
                continue
            if not path.is_file() and not path.is_symlink():
                continue
            relative = str(path.relative_to(home))
            if ".hangar-instrucoes/" in relative:
                (backups if "/backups/" in relative else records).append(json.loads(path.read_text()))
            else:
                files[relative] = {"link": os.readlink(path)} if path.is_symlink() else {"hex": path.read_bytes().hex()}
        records.sort(key=lambda item: item["alvo"])
        exclude = (project / ".git/info/exclude").read_text() if has_git else None
        status = subprocess.check_output(["git", "-C", str(project), "status", "--porcelain", "--untracked-files=all"], text=True) if has_git else None
        case = {"name": name, "before": before, "git": has_git, "initial_exclude": initial_exclude,
                "results": results, "files": files, "records": records, "backups": backups, "exclude": exclude, "git_status": status}
        # O config.toml contém o caminho da pasta temporária dentro dos bytes capturados.
        for mapping in [case["before"], case["files"]]:
            for key, value in list(mapping.items()):
                if key.endswith("config.toml"):
                    encoded = value if isinstance(value, str) else value["hex"]
                    replaced = bytes.fromhex(encoded).decode().replace(raw, "$ROOT").encode().hex()
                    if isinstance(value, str): mapping[key] = replaced
                    else: value["hex"] = replaced
        cases.append(json.loads(json.dumps(case).replace(raw, "$ROOT")))

Path(__file__).with_name("instruction-projects.json").write_text(json.dumps({
    "source_commit": scope["COMMIT"], "command": "cd backend && uv run python tests/fixtures/contract/codex-integration/generate_instruction_projects.py",
    "injections": ["Git isolado em HOME temporário; gate desativado somente no cenário disabled"], "cases": cases,
}, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
