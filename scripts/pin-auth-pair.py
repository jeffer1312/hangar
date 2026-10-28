#!/usr/bin/env python3
"""Publica a biblioteca imutável e o par conferido do servidor que acabou de ser produzido."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def publish(server, library):
    server, library = Path(server).resolve(strict=True), Path(library).resolve(strict=True)
    sha = digest(library)
    target = server.parent / "auth" / sha / library.name
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists():
        if digest(target) != sha:
            raise ValueError("auth_library_hash_mismatch")
    else:
        # Nunca substituir a DLL que um pai já fixou e pode manter carregada no Windows.
        with tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as stream:
            temp = Path(stream.name)
            stream.write(library.read_bytes())
        try:
            try:
                os.link(temp, target)
            except FileExistsError:
                if digest(target) != sha:
                    raise ValueError("auth_library_hash_mismatch")
        finally:
            temp.unlink(missing_ok=True)
    value = {"abi": 1, "store_version": 1, "server_sha256": digest(server),
             "library_sha256": sha, "library": target.relative_to(server.parent).as_posix()}
    with tempfile.NamedTemporaryFile(dir=server.parent, delete=False) as stream:
        temp = Path(stream.name)
        stream.write(json.dumps(value, sort_keys=True).encode("utf-8"))
    try:
        os.replace(temp, server.with_suffix(".auth.json"))
    finally:
        temp.unlink(missing_ok=True)


def pin_artifacts(records, source_target, target_dir, workspace):
    source_target, target_dir, workspace = (Path(path).resolve() for path in (source_target, target_dir, workspace))
    copies, servers, libraries = {}, [], []

    def source(path):
        path = Path(path).resolve(strict=True)
        if not path.is_relative_to(source_target):
            raise ValueError("cargo_artifact_outside_target")
        return path

    for record in records:
        if record.get("reason") != "compiler-artifact":
            continue
        target = record.get("target", {})
        if record.get("executable") is not None and not record["profile"]["test"]:
            executable = source(record["executable"])
            destination = target_dir / executable.relative_to(source_target)
            copies[destination] = executable
            if target.get("name") == "hangar-server":
                if Path(record["manifest_path"]).resolve() != workspace / "hangar-server/Cargo.toml":
                    raise ValueError("server_artifact_from_other_workspace")
                servers.append((record, executable, destination))
        if target.get("name") == "hangar_auth" and "cdylib" in target.get("crate_types", []):
            if Path(record["manifest_path"]).resolve() != workspace / "hangar-auth/Cargo.toml":
                raise ValueError("auth_artifact_from_other_workspace")
            for filename in record["filenames"]:
                if Path(filename).suffix in {".so", ".dylib", ".dll"}:
                    libraries.append((record, source(filename)))

    pairs = []
    for record, executable, destination in servers:
        candidates = [path for library_record, path in libraries
                      if path.parent in {executable.parent, executable.parent / "deps"}
                      and library_record["profile"] == record["profile"]]
        if not candidates:
            raise ValueError("auth_library_missing_from_build")
        if len({digest(path) for path in candidates}) != 1:
            raise ValueError("auth_library_ambiguous_from_build")
        library = sorted(candidates)[0]
        name = {".so": "libhangar_auth.so", ".dylib": "libhangar_auth.dylib", ".dll": "hangar_auth.dll"}[library.suffix]
        library_destination = destination.parent / name
        copies[library_destination] = library
        pairs.append((destination, library_destination))

    # O plano inteiro é validado antes da primeira troca no target desta worktree.
    for destination, artifact in copies.items():
        if destination == artifact:
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as stream:
            temporary = Path(stream.name)
        try:
            shutil.copyfile(artifact, temporary)
            shutil.copymode(artifact, temporary)
            os.replace(temporary, destination)
        finally:
            temporary.unlink(missing_ok=True)
    for server, library in pairs:
        publish(server, library)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--server", type=Path)
    parser.add_argument("--library", type=Path)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--source-target", type=Path)
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--workspace", type=Path)
    args = parser.parse_args()
    if args.artifacts:
        if not all((args.source_target, args.target_dir, args.workspace)):
            parser.error("a lista Cargo exige o target de origem, o de publicação e o workspace")
        records = [json.loads(line) for line in args.artifacts.read_text().splitlines()]
        pin_artifacts(records, args.source_target, args.target_dir, args.workspace)
    elif args.server and args.library:
        publish(args.server, args.library)
    else:
        parser.error("informe servidor e biblioteca ou a lista de artefatos")


if __name__ == "__main__":
    main()
