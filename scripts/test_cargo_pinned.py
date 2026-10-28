"""Wrapper contra Cargo externo de ensaio: nenhum compilador Rust é iniciado."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
pytestmark = pytest.mark.skipif(not sys.platform.startswith("linux"), reason="wrapper usa flock/ionice no Linux")


def _module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


FAKE_CARGO = r'''
import json, os, pathlib, sys
root = pathlib.Path(os.environ["FAKE_REPO"])
initial = pathlib.Path(os.environ["FAKE_TARGET"])
if sys.argv[1] == "metadata":
    print(json.dumps({"target_directory": str(initial)}))
    sys.exit(0)
target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", initial))
for index, arg in enumerate(sys.argv):
    if arg == "--target-dir":
        target = pathlib.Path(sys.argv[index + 1])
    elif arg.startswith("--target-dir="):
        target = pathlib.Path(arg.split("=", 1)[1])
mode = os.environ.get("FAKE_WRONG", "")
bucket = target / "debug"
bucket.mkdir(parents=True, exist_ok=True)
server = bucket / "hangar-server"
server.write_bytes(b"new-server-from-cargo")
server.chmod(0o755)
library = (target / "different-target" / "debug" if mode == "target" else bucket) / "deps/libhangar_auth.so"
library.parent.mkdir(parents=True, exist_ok=True)
library.write_bytes(b"new-library-from-cargo")
profile = {"opt_level": "0", "debuginfo": 2, "debug_assertions": True, "overflow_checks": True, "test": False}
manifest = root / "crates/hangar-auth/Cargo.toml"
if mode == "workspace":
    manifest = root.parent / "other-worktree/crates/hangar-auth/Cargo.toml"
auth_profile = {**profile, "opt_level": "3"} if mode == "profile" else profile
if mode != "missing":
    print(json.dumps({"reason": "compiler-artifact", "manifest_path": str(manifest),
        "package_id": "path+file://" + str(manifest.parent) + "#hangar-auth@0.1.0",
        "target": {"name": "hangar_auth", "crate_types": ["rlib", "cdylib"]},
        "profile": auth_profile, "filenames": [str(library)], "executable": None}), flush=True)
print(json.dumps({"reason": "compiler-artifact", "manifest_path": str(root / "crates/hangar-server/Cargo.toml"),
    "package_id": "path+file://" + str(root / "crates/hangar-server") + "#hangar-server@0.1.0",
    "target": {"name": "hangar-server", "crate_types": ["bin"]}, "profile": profile,
    "filenames": [str(server)], "executable": str(server)}), flush=True)
print("saída do teste de ensaio", flush=True)
sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
'''


@pytest.fixture
def checkout(tmp_path):
    root = tmp_path / "checkout"
    scripts = root / "scripts"
    scripts.mkdir(parents=True)
    for name in ("cargo-pinned", "pin-auth-pair.py"):
        shutil.copyfile(ROOT / "scripts" / name, scripts / name)
    for name in ("hangar-auth", "hangar-server"):
        manifest = root / "crates" / name / "Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
    tools = tmp_path / "tools"
    tools.mkdir()
    cargo = tools / "cargo"
    cargo.write_text(f"#!{sys.executable}\n" + FAKE_CARGO)
    cargo.chmod(0o755)
    git = tools / "git"
    git.write_text('#!/bin/sh\n[ "$1" = rev-parse ] && [ "$2" = --show-toplevel ] || exit 1\nprintf "%s\\n" "$FAKE_REPO"\n')
    git.chmod(0o755)
    pinned = root / "crates/target/debug"
    pinned.mkdir(parents=True)
    server, library = pinned / "hangar-server", pinned / "libhangar_auth.so"
    server.write_bytes(b"old-server")
    server.chmod(0o755)
    library.write_bytes(b"old-library")
    publisher = _module("pin_auth_for_wrapper", ROOT / "scripts/pin-auth-pair.py")
    publisher.publish(server, library)
    resolver = _module("rust_bins_for_wrapper", ROOT / "backend/app/rust_bins.py")
    environment = {"PATH": str(tools) + os.pathsep + os.defpath, "HOME": str(tmp_path),
        "FAKE_REPO": str(root), "FAKE_TARGET": str(tmp_path / "shared-target"),
        "HANGAR_BUILD_LOCK": str(tmp_path / "build.lock"), "CARGO_BUILD_JOBS": "3"}
    return root, server, library, resolver, environment


def _published(server):
    return {path.relative_to(server.parent): path.read_bytes() for path in server.parent.rglob("*") if path.is_file()}


@pytest.mark.parametrize("target", ["shared", "local", "explicit-local"])
def test_failed_cargo_after_artifact_output_preserves_the_whole_pair(checkout, target):
    root, server, library, resolver, environment = checkout
    before = _published(server)
    old_library, old_hash = resolver.auth_pair(server)
    args = ["build", "-p", "hangar-server"]
    if target == "local":
        environment["FAKE_TARGET"] = str(root / "crates/target")
    elif target == "explicit-local":
        args.extend(["--target-dir", str(root / "crates/target")])
    result = subprocess.run(["bash", str(root / "scripts/cargo-pinned"), *args], cwd=root,
                            env={**environment, "FAKE_EXIT": "37"}, capture_output=True, text=True)
    assert result.returncode == 37, result.stdout + result.stderr
    assert "saída do teste de ensaio" in result.stdout
    assert _published(server) == before
    assert server.read_bytes() == b"old-server" and library.read_bytes() == b"old-library"
    assert resolver.auth_pair(server) == (old_library, old_hash)


def test_successful_cargo_pairs_the_attested_cdylib_from_deps(checkout):
    root, server, _, resolver, environment = checkout
    result = subprocess.run(["bash", str(root / "scripts/cargo-pinned"), "test", "-p", "hangar-server"],
                            cwd=root, env=environment, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    library, digest = resolver.auth_pair(server)
    assert server.read_bytes() == b"new-server-from-cargo"
    assert library.read_bytes() == b"new-library-from-cargo"
    assert digest == hashlib.sha256(b"new-library-from-cargo").hexdigest()
    assert library == server.parent / "auth" / digest / "libhangar_auth.so"
    assert "saída do teste de ensaio" in result.stdout
    metadata = json.loads(server.with_suffix(".auth.json").read_bytes())
    assert metadata["server_sha256"] == hashlib.sha256(server.read_bytes()).hexdigest()


@pytest.mark.parametrize("wrong", ["profile", "target", "workspace", "missing"])
def test_unattested_library_never_reuses_another_pair_or_filesystem_candidate(checkout, wrong):
    root, server, _, resolver, environment = checkout
    before = _published(server)
    old_pair = resolver.auth_pair(server)
    result = subprocess.run(["bash", str(root / "scripts/cargo-pinned"), "test", "-p", "hangar-server"],
                            cwd=root, env={**environment, "FAKE_WRONG": wrong}, capture_output=True, text=True)
    assert result.returncode != 0
    assert _published(server) == before and resolver.auth_pair(server) == old_pair
