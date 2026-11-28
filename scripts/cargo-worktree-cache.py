#!/usr/bin/env python3
"""Confere se o Cargo pode compartilhar o cache com os membros isolados por checkout."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import tomllib


def cache_mode(metadata):
    workspace = Path(metadata["workspace_root"])
    members = set(metadata["workspace_members"])
    packages = [package for package in metadata["packages"] if package["id"] in members]
    member_dirs = {Path(package["manifest_path"]).resolve().parent for package in packages}
    path_dependencies = {Path(dep["path"]).resolve() for package in packages
                         for dep in package["dependencies"] if dep.get("path")}
    if not path_dependencies <= member_dirs:
        return None

    # Substituições de fonte podem trazer dependências locais que o metadata --no-deps não resolve.
    manifest = tomllib.loads((workspace / "Cargo.toml").read_text(encoding="utf-8"))
    if {"patch", "replace"} & manifest.keys():
        return None

    cargo_home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    config_dirs = [cargo_home, *(directory / ".cargo" for directory in (workspace, *workspace.parents))]
    mode = "default"
    for directory in config_dirs:
        for filename in ("config", "config.toml"):
            try:
                with (directory / filename).open("rb") as stream:
                    config = tomllib.load(stream)
            except FileNotFoundError:
                continue
            if {"include", "paths", "patch", "replace", "source"} & config.keys():
                return None
            build_options = config.get("build", {})
            if {"rustc-wrapper", "rustc-workspace-wrapper"} & build_options.keys():
                return None
            if {"target-dir", "build-dir"} & build_options.keys():
                mode = "configured"
    return mode


def prune_removed_worktrees(cache):
    if cache.is_symlink() or not cache.is_dir():
        return
    stamp = cache / ".hangar-pruned"
    if stamp.is_symlink():
        return
    if stamp.is_file() and 0 <= time.time() - stamp.stat().st_mtime < 86400:
        return

    listing = subprocess.check_output(["git", "worktree", "list", "--porcelain", "-z"])
    active = {Path(os.fsdecode(item.removeprefix(b"worktree "))).resolve()
              for item in listing.split(b"\0") if item.startswith(b"worktree ")}
    current = Path(os.environ["HANGAR_CARGO_CACHE_WORKTREE"]).resolve()
    if current not in active:
        raise ValueError("lista de worktrees não contém o checkout atual")

    for profile in ("*", "*/*"):
        for owner in cache.glob(f"{profile}/build/*/*/.hangar-worktree"):
            unit = owner.parent
            if not re.fullmatch(r"[0-9a-f]{16}", unit.name):
                continue
            if any(part.is_symlink() for part in (owner, *unit.parents, unit) if part.is_relative_to(cache)):
                continue
            checkout = Path(os.fsdecode(owner.read_bytes()))
            # Outro clone pode dividir o target configurado: só sai a unidade cuja worktree sumiu do disco.
            if checkout.is_absolute() and checkout.resolve() not in active and not checkout.exists():
                shutil.rmtree(unit)
    stamp.touch()


if __name__ == "__main__":
    try:
        if sys.argv[1] == "--prune":
            prune_removed_worktrees(Path(sys.argv[2]))
            sys.exit(0)
        with Path(sys.argv[1]).open(encoding="utf-8") as metadata_file:
            mode = cache_mode(json.load(metadata_file))
        if mode is None:
            sys.exit(1)
        print(mode)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError):
        sys.exit(1)
