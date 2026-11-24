#!/usr/bin/env python3
"""Testes de Rust de um PR (gate.yml): os dos crates que mudaram e dos que dependem deles.

Uso: rust-pr-tests.py rest <pacote>...        os pacotes dados, menos o hangar-server
     rust-pr-tests.py server <fatia> <total>  uma fatia dos testes do hangar-server

Os testes de unidade do hangar-server ficam num executável só e levam a maior parte da suíte. A fatia
leva os testes dele cujo nome cai nela pelo crc32, mais um em cada <total> dos executáveis de
tests/; a fatia 0 leva também binários, exemplos e doctests. Fatia sem teste nenhum é falha: um
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


def server(part: int, total: int) -> int:
    listing = subprocess.run(
        ["cargo", "test", "--locked", "-p", SERVER, "--lib", "--", "--list", "--format", "terse"],
        cwd=CRATES, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace",
    )
    if listing.returncode:
        print(listing.stdout)
        return listing.returncode
    names = [line[: -len(": test")] for line in listing.stdout.splitlines() if line.endswith(": test")]
    if not names:
        print(f"::error::a lista dos testes de unidade do {SERVER} veio vazia")
        return 1
    mine = [n for n in names if zlib.crc32(n.encode()) % total == part]
    manifest = tomllib.loads((CRATES / SERVER / "Cargo.toml").read_text())
    targets = sorted(t["name"] for t in manifest.get("test", []))
    targets = [t for i, t in enumerate(targets) if i % total == part]
    print(f"fatia {part}/{total}: {len(mine)} de {len(names)} testes de unidade, executáveis {' '.join(targets) or 'nenhum'}")
    failed = 0
    if mine:
        failed |= cargo("-p", SERVER, "--lib", "--", "--exact", *mine)
    extra = [a for t in targets for a in ("--test", t)] + (["--bins", "--examples"] if part == 0 else [])
    if extra:
        failed |= cargo("-p", SERVER, *extra)
    if part == 0:
        failed |= cargo("-p", SERVER, "--doc")
    if not mine and not extra:
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
