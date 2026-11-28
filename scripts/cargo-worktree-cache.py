#!/usr/bin/env python3
"""Confere se o Cargo pode compartilhar o cache com os membros isolados por checkout."""

import json
import os
from pathlib import Path
import sys
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


if __name__ == "__main__":
    try:
        with Path(sys.argv[1]).open(encoding="utf-8") as metadata_file:
            mode = cache_mode(json.load(metadata_file))
        if mode is None:
            sys.exit(1)
        print(mode)
    except (OSError, ValueError, KeyError, TypeError):
        sys.exit(1)
