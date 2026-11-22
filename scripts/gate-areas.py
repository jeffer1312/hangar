#!/usr/bin/env python3
"""Quais partes do portão de merge (gate.yml) um conjunto de arquivos mudados aciona.

Lê os caminhos mudados (um por linha, relativos à raiz) no stdin e escreve `server=`, `native=` e
`mobile=` (`true`/`false`) no formato do GITHUB_OUTPUT. `--all` liga as três, para quando não há
base com que comparar. As pastas do desktop saem da cadeia de dependências por caminho dos
Cargo.toml, não de uma lista: crate novo de que o desktop dependa entra sozinho.
"""
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Mudou o próprio portão: tudo roda, senão um portão quebrado passaria sem ser exercitado.
GATE = ("scripts/gate-areas.py", ".github/workflows/gate.yml")

# Além do código, o que os testes leem do resto do repositório (scripts, passos de atualização, fixtures do core,
# mensagens, novidades e manual).
SERVER = ("crates/", "backend/tests/fixtures/", "plugins/hangar/hooks/", "scripts/", "docs/atualizacoes/",
          "packages/core/src/", "messages/", ".github/workflows/server.yml", ".github/workflows/server-jobs.yml")
NATIVE = ("desktop-native/", "messages/", "VERSION", "crates/Cargo.toml", "packages/core/src/", "docs/manual/",
          "docs/novidades/", ".github/workflows/native.yml", ".github/workflows/native-check.yml")
MOBILE = ("mobile/", "packages/core/", "messages/", "project.inlang/", "scripts/i18n-compile.mjs",
          "package.json", "package-lock.json")


def workspace_root(manifest: Path) -> Path | None:
    for folder in manifest.parent.parents:
        candidate = folder / "Cargo.toml"
        if candidate.is_file() and "workspace" in tomllib.loads(candidate.read_text()):
            return candidate
        if folder == ROOT:
            return None
    return None


def path_deps(manifest: Path) -> list[Path]:
    data = tomllib.loads(manifest.read_text())
    tables = [data.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
    for target in data.get("target", {}).values():
        tables += [target.get(k, {}) for k in ("dependencies", "dev-dependencies", "build-dependencies")]
    workspace = None
    found = []
    for table in tables:
        for name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            if "path" in spec:
                found.append((manifest.parent / spec["path"]).resolve())
            elif spec.get("workspace"):
                workspace = workspace or workspace_root(manifest)
                if workspace is None:
                    raise SystemExit(f"{manifest}: {name} vem do workspace, mas não achei o workspace")
                shared = tomllib.loads(workspace.read_text())["workspace"].get("dependencies", {}).get(name)
                if isinstance(shared, dict) and "path" in shared:
                    found.append((workspace.parent / shared["path"]).resolve())
    return found


def native_crates() -> list[str]:
    """As pastas fora do desktop-native de que ele depende por caminho, direta ou indiretamente."""
    seen: set[Path] = set()
    pending = [ROOT / "desktop-native" / "Cargo.toml"]
    while pending:
        for folder in path_deps(pending.pop()):
            if folder in seen or ROOT not in folder.parents:
                continue
            seen.add(folder)
            pending.append(folder / "Cargo.toml")
    inside = ROOT / "desktop-native"
    return sorted(f"{p.relative_to(ROOT).as_posix()}/" for p in seen if inside not in p.parents and p != inside)


def touches(files: list[str], prefixes: tuple[str, ...]) -> bool:
    return any(f == p or (p.endswith("/") and f.startswith(p)) for f in files for p in prefixes)


def main() -> None:
    if "--all" in sys.argv[1:]:
        areas = dict(server=True, native=True, mobile=True)
    else:
        files = [line.strip() for line in sys.stdin if line.strip()]
        every = touches(files, GATE)
        areas = dict(
            server=every or touches(files, SERVER),
            native=every or touches(files, NATIVE + tuple(native_crates())),
            mobile=every or touches(files, MOBILE),
        )
    for name, on in areas.items():
        print(f"{name}={'true' if on else 'false'}")


if __name__ == "__main__":
    main()
