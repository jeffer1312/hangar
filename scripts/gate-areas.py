#!/usr/bin/env python3
"""Quais travas do portão de merge (gate.yml) um conjunto de arquivos mudados aciona.

Lê os caminhos mudados (um por linha, relativos à raiz) no stdin e escreve `server=`, `native=` e
`mobile=` (`true`/`false`) no formato do GITHUB_OUTPUT. `--all` liga as três, para quando não há
base com que comparar. As pastas do desktop saem da cadeia de dependências por caminho dos
Cargo.toml, não de uma lista: crate novo de que o desktop dependa entra sozinho.

Escreve também `rust_packages=` (os crates do workspace que mudaram e os que dependem deles, cujos
testes o PR roda) e `rust_server=` (se o hangar-server está entre eles). `--rust-packages` imprime
só essa lista, um por linha, para o scripts/verificar-local.
"""
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Mudou o próprio portão: tudo roda, senão um portão quebrado passaria sem ser exercitado.
GATE = ("scripts/gate-areas.py", "scripts/rule-tests.py", "scripts/rust-pr-tests.py", ".github/workflows/gate.yml")

# O que pode derrubar as travas do scripts/rule-tests.py; o resto é das suítes do push.
CRATES = ROOT / "crates"
# Fora do crates/ e lido pelo código ou pelos testes de Rust (include_str!, CARGO_MANIFEST_DIR/../..):
# mudança aqui vale para os crates cujo código cita o caminho, e para todos quando nenhum o cita.
RUST_READS = ("backend/tests/fixtures/", "resources/", "messages/", "packages/core/src/installCode.fixtures.json",
              "docs/atualizacoes/", "scripts/shell/", "scripts/omp/", "scripts/hangar-codex", "scripts/tests/",
              "examples/connectors/")
SERVER = ("crates/",) + RUST_READS
NATIVE = ("desktop-native/", "crates/Cargo.toml", "packages/core/src/contextRefs.fixtures.json",
          "packages/core/src/whatsappInvisible.fixtures.json")
MOBILE = ("mobile/", "packages/core/", "messages/", "project.inlang/", "scripts/i18n-compile.mjs",
          "scripts/verificar-app.mjs", ".node-version", "package.json", "package-lock.json")


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


# Dependência que o Cargo.toml não declara: os testes do hangar-server executam o binário do hangar-cano.
RUN_DEPENDENTS = {"hangar-cano": ("hangar-server",)}


def rust_members() -> dict[Path, str]:
    """Pasta de cada crate do workspace → nome do pacote."""
    members = tomllib.loads((CRATES / "Cargo.toml").read_text())["workspace"]["members"]
    found = {}
    for member in members:
        folder = (CRATES / member).resolve()
        found[folder] = tomllib.loads((folder / "Cargo.toml").read_text())["package"]["name"]
    return found


def readers(members: dict[Path, str], prefix: str) -> set[Path]:
    """Os crates cujo código Rust cita o caminho subindo da pasta do crate (`../../messages`)."""
    needle = "../" + prefix.rstrip("/")
    return {m for m in members if any(needle in f.read_text(errors="replace") for f in m.rglob("*.rs"))}


def rust_packages(files: list[str] | None) -> list[str]:
    """Os crates mudados e os que dependem deles, direta ou indiretamente (dependência de teste inclusa).
    Arquivo do crates/ fora de qualquer crate (Cargo.toml, Cargo.lock, toolchain) e `None` valem todos."""
    members = rust_members()
    if files is None:
        return sorted(members.values())
    changed: set[Path] = set()
    for prefix in (p for p in RUST_READS if touches(files, (p,))):
        found = readers(members, prefix)
        if not found:
            return sorted(members.values())
        changed |= found
    for name in (f for f in files if f.startswith("crates/")):
        path = (ROOT / name).resolve()
        owner = next((m for m in members if m == path.parent or m in path.parents), None)
        if owner is None:
            return sorted(members.values())
        changed.add(owner)
    dependents: dict[Path, set[Path]] = {m: set() for m in members}
    for member in members:
        for dependency in path_deps(member / "Cargo.toml"):
            if dependency in dependents:
                dependents[dependency].add(member)
    by_name = {name: folder for folder, name in members.items()}
    for name, users in RUN_DEPENDENTS.items():
        dependents[by_name[name]].update(by_name[user] for user in users)
    pending = list(changed)
    while pending:
        for user in dependents[pending.pop()]:
            if user not in changed:
                changed.add(user)
                pending.append(user)
    return sorted(members[m] for m in changed)


def touches(files: list[str], prefixes: tuple[str, ...]) -> bool:
    return any(f == p or (p.endswith("/") and f.startswith(p)) for f in files for p in prefixes)


def main() -> None:
    if "--rust-packages" in sys.argv[1:]:
        print("\n".join(rust_packages([line.strip() for line in sys.stdin if line.strip()])))
        return
    if "--all" in sys.argv[1:]:
        areas = dict(server=True, native=True, mobile=True)
        files = None
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
    packages = rust_packages(None if files is None or touches(files, GATE) else files) if areas["server"] else []
    print(f"rust_packages={' '.join(packages)}")
    print(f"rust_server={'true' if 'hangar-server' in packages else 'false'}")


if __name__ == "__main__":
    main()
