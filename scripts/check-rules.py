#!/usr/bin/env python3
"""Regras do CLAUDE.md que se conferem pelo diff e pelos Cargo.toml, sem compilar.

Uso: check-rules.py <base> <head>      (compara head com o ancestral comum)
Saída 0 = ok; 1 = regra descumprida; 2 = intervalo ilegível.
"""
import re
import subprocess
import sys
import tomllib
from pathlib import Path

SERVER = "hangar-server"
PYTHON_MARK = re.compile(r"(^|[^A-Za-z_])HANGAR_PYTHON_AUTORIZADO:[ \t]+\S", re.M)


def git(*args: str) -> str:
    done = subprocess.run(["git", *args], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if done.returncode:
        print(f"git {' '.join(args)}: {done.stderr.strip()}", file=sys.stderr)
        raise SystemExit(2)
    return done.stdout


def new_python(base: str, head: str) -> list[str]:
    """Linhas que o backend ganhou: nada nasce no Python sem autorização do dono."""
    if PYTHON_MARK.search(git("log", "--no-merges", "--format=%B", f"{base}..{head}")):
        return []
    found, name = [], ""
    # `--no-renames`: arquivo movido para dentro conta inteiro.
    for line in git("diff", "--no-renames", "--no-ext-diff", "--no-color", "-U0", f"{base}...{head}", "--", "backend/app").splitlines():
        if line.startswith("+++ "):
            name = line[6:]
        elif line.startswith("+") and line[1:].strip() and not line[1:].lstrip().startswith("#"):
            found.append(f"{name}: {line[1:].strip()[:120]}")
    return found


def server_dependents() -> list[str]:
    """Nenhum crate depende do hangar-server: o que ele precisa desce para o hangar-core."""
    root = Path(git("rev-parse", "--show-toplevel").strip())
    found = []
    for manifest in [*sorted(root.glob("crates/*/Cargo.toml")), root / "desktop-native" / "Cargo.toml"]:
        if not manifest.is_file() or manifest.parent.name == SERVER:
            continue
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for scope in [data, *data.get("target", {}).values()]:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for name, spec in scope.get(kind, {}).items():
                    if name == SERVER or (isinstance(spec, dict) and spec.get("package") == SERVER):
                        found.append(f"{manifest.relative_to(root).as_posix()} ({kind})")
    return found


def main() -> int:
    if len(sys.argv) != 3:
        print("uso: check-rules.py <base> <head>", file=sys.stderr)
        return 2
    base, head = sys.argv[1:]
    broken = [
        ("Linha nova no backend Python",
         "Nada nasce no Python: o lugar é o crates/. Com autorização do dono, ponha"
         " 'HANGAR_PYTHON_AUTORIZADO: <motivo>' na mensagem de um commit.", new_python(base, head)),
        ("Crate dependendo do hangar-server",
         "O que o módulo precisa desce para o hangar-core, ou vira trait que o servidor implementa.", server_dependents()),
    ]
    failed = False
    for title, help_text, found in broken:
        if not found:
            print(f"ok: {title}: nenhuma")
            continue
        failed = True
        print(f"::error title={title}::{help_text}")
        for item in found[:20]:
            print(f"  {item}")
        if len(found) > 20:
            print(f"  … e mais {len(found) - 20}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
