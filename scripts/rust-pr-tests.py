#!/usr/bin/env python3
"""Testes de Rust de um PR (gate.yml): os dos crates que mudaram e dos que dependem deles.

Uso: rust-pr-tests.py rest <pacote>...        os pacotes dados, menos o hangar-server
     rust-pr-tests.py server <fatia> <total>  uma fatia dos testes do hangar-server

Os testes de unidade e o executável `it` do hangar-server levam a maior parte da suíte. A fatia leva
os testes desses dois cujo nome cai nela pelo crc32, mais um em cada <total> dos outros executáveis
de tests/; a fatia 0 leva também binários, exemplos e doctests. Fatia sem teste nenhum é falha: um
filtro errado não pode passar verde.
"""
import subprocess
import sys
import tomllib
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"
SERVER = "hangar-server"


def cargo(*args: str) -> int:
    command = ["cargo", "test", "--locked", "--no-fail-fast", *args]
    print("+", " ".join(command[:12]) + (" …" if len(command) > 12 else ""), flush=True)
    return subprocess.run(command, cwd=CRATES).returncode


def rest(packages: list[str]) -> int:
    packages = [p for p in packages if p != SERVER]
    if not packages:
        print("nenhum crate mudado além do hangar-server")
        return 0
    return cargo(*(a for p in packages for a in ("-p", p)))


def listed(target: list[str]) -> list[str] | None:
    """Os testes de um executável do hangar-server, pelo próprio libtest; `None` quando não compila."""
    listing = subprocess.run(
        ["cargo", "test", "--locked", "-p", SERVER, *target, "--", "--list", "--format", "terse"],
        cwd=CRATES, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
    )
    if listing.returncode:
        print(listing.stdout)
        return None
    return [line[: -len(": test")] for line in listing.stdout.splitlines() if line.endswith(": test")]


def server(part: int, total: int) -> int:
    # Os testes procuram o hangar-cano e o hangar-fixture-mcp acima do próprio executável; só a suíte do workspace os compilava.
    for build in (
        ["cargo", "build", "--locked", "-p", "hangar-cano"],
        ["cargo", "build", "--locked", "-p", "hangar-integrations", "--bin", "hangar-fixture-mcp"],
    ):
        if subprocess.run(build, cwd=CRATES).returncode:
            return 1
    manifest = tomllib.loads((CRATES / SERVER / "Cargo.toml").read_text())
    targets = sorted(t["name"] for t in manifest.get("test", []))
    failed = 0
    ran = 0
    # Os dois executáveis grandes dividem os testes pelo nome; os outros, um em cada <total>.
    for target in (["--lib"], ["--test", "it"]):
        names = listed(target)
        if names is None:
            return 1
        if not names:
            print(f"::error::a lista dos testes de {' '.join(target)} do {SERVER} veio vazia")
            return 1
        mine = [n for n in names if zlib.crc32(n.encode()) % total == part]
        print(f"fatia {part}/{total}: {len(mine)} de {len(names)} testes de {' '.join(target)}")
        if mine:
            ran += len(mine)
            failed |= cargo("-p", SERVER, *target, "--", "--exact", *mine)
    small = [t for i, t in enumerate(t for t in targets if t != "it") if i % total == part]
    print(f"fatia {part}/{total}: executáveis {' '.join(small) or 'nenhum'}")
    extra = [a for t in small for a in ("--test", t)] + (["--bins", "--examples"] if part == 0 else [])
    if extra:
        ran += 1
        failed |= cargo("-p", SERVER, *extra)
    if part == 0:
        failed |= cargo("-p", SERVER, "--doc")
    if not ran:
        print(f"::error::a fatia {part}/{total} ficou sem teste")
        return 1
    return 1 if failed else 0


def main() -> int:
    args = sys.argv[1:]
    if args[:1] == ["rest"]:
        return rest(args[1:])
    if args[:1] == ["server"] and len(args) == 3 and args[1].isdigit() and args[2].isdigit() and int(args[1]) < int(args[2]):
        return server(int(args[1]), int(args[2]))
    print(__doc__.split("\n\n")[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
