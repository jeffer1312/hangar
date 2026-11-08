#!/usr/bin/env python3
"""Compara custos e uso em processos avulsos, com índices descartáveis.

Uso, da raiz: cd backend && uv run python ../scripts/comparar-custos.py
--self-test valida apenas o comparador e os argumentos, sem importar o backend.
Os escopos reais são lidos; as divergências nunca exibem seus valores.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import logging
import math
import os
import re
import signal
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
LOCAL = timezone(timedelta(hours=-3))
sys.dont_write_bytecode = True


class ComparisonError(Exception):
    pass


def compare(left: object, right: object, path: str, errors: list[str]) -> None:
    if type(left) is not type(right):
        errors.append(f"{path}: type_mismatch")
    elif isinstance(left, dict):
        if list(left) != list(right):
            errors.append(f"{path}: keys_or_order_mismatch")
        # O índice do campo localiza a diferença sem expor chaves derivadas de comandos.
        for index, key in enumerate(left):
            if key in right:
                compare(left[key], right[key], f"{path}.field[{index}]", errors)
    elif isinstance(left, list):
        if len(left) != len(right):
            errors.append(f"{path}: length_mismatch ({len(left)}, {len(right)})")
        for index, (a, b) in enumerate(zip(left, right)):
            compare(a, b, f"{path}[{index}]", errors)
    elif isinstance(left, float):
        if not math.isfinite(left) or not math.isfinite(right):
            errors.append(f"{path}: non_finite_number")
        elif abs(left - right) > 1e-9 * max(abs(left), abs(right), 1.0):
            errors.append(f"{path}: fraction_mismatch")
    elif left != right:
        errors.append(f"{path}: value_mismatch")


def parse_now(value: str) -> datetime:
    try:
        result = datetime.fromisoformat(value)
        return result.replace(tzinfo=LOCAL) if result.tzinfo is None else result.astimezone(LOCAL)
    except ValueError:
        raise argparse.ArgumentTypeError("now_invalid") from None


def positive_seconds(value: str) -> int:
    try:
        result = int(value)
        if result > 0:
            return result
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("timeout_invalid")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--self-test", action="store_true")
    result.add_argument("--self-test-snapshot", action="store_true", help="Confere o timeout do namespace com processos sintéticos.")
    result.add_argument("--rust-only", action="store_true", help="Mede somente o Rust por etapa, sem comparação Python.")
    result.add_argument("--profile", action="store_true", help="Mostra tempo e memória por etapa do Rust.")
    result.add_argument("--incremental", action="store_true", help="Mede uma segunda coleta no mesmo índice e confere os relatórios.")
    result.add_argument("--snapshot", action="store_true", help="Congela os bytes num namespace privado com os caminhos originais.")
    result.add_argument("--snapshot-worker", type=Path, help=argparse.SUPPRESS)
    result.add_argument("--binary", help="Usa o executável pré-compilado, sem rodar o Cargo.")
    result.add_argument("--binary-sha256", help="Confere o hash do executável pré-compilado antes de executá-lo.")
    result.add_argument("--now", type=parse_now)
    result.add_argument("--period", choices=("all", "1d", "7d", "30d", "90d"), default="all")
    result.add_argument("--timeout", type=positive_seconds, default=300)
    for name in ("conta", "projeto", "modelo", "plugin"):
        result.add_argument(f"--{name}", action="append", default=[])
    result.add_argument("--foco")
    return result


def protect_writes(root: Path) -> None:
    def check(path: object, dir_fd: int | None = None, *, follow_leaf: bool = True) -> None:
        if isinstance(path, int) or path is None:
            return
        target = Path(os.fsdecode(path))
        if dir_fd not in (None, -1) and not target.is_absolute():
            # O cleanup de TemporaryDirectory remove nomes relativos a um descritor aberto.
            base = Path(f"/proc/self/fd/{dir_fd}")
            if not base.exists():
                base = Path(f"/dev/fd/{dir_fd}")
            target = base.resolve() / target
        resolved = target.resolve() if follow_leaf else target.parent.resolve() / target.name
        if not resolved.is_relative_to(root):
            raise ComparisonError("write_outside_temporary_directory")

    def audit(event: str, args: tuple) -> None:
        if event == "open":
            path, mode, flags = args
            if (mode and any(c in mode for c in "wax+")) or flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC):
                check(path)
        elif event in ("os.remove", "os.rmdir"):
            check(args[0], args[1], follow_leaf=False)
        elif event in ("os.mkdir", "os.chmod"):
            check(args[0], args[2])
        elif event == "os.utime":
            check(args[0], args[3])
        elif event == "os.truncate":
            check(args[0])
        elif event in ("os.rename", "os.link"):
            check(args[0], args[2], follow_leaf=False); check(args[1], args[3], follow_leaf=False)
        elif event == "os.symlink":
            check(args[1], args[2], follow_leaf=False)
        elif event == "sqlite3.connect":
            check(args[0])

    sys.addaudithook(audit)


def copy_metadata(source: Path, target: Path) -> None:
    try:
        target.write_bytes(source.read_bytes())
    except FileNotFoundError:
        pass


def python_reports(args: argparse.Namespace, tmp: Path, now: datetime, *, collect: bool = True,
                   frozen: dict | None = None) -> tuple[dict, Path, Path, Path]:
    sys.path.insert(0, str(ROOT / "backend"))
    logging.disable(logging.CRITICAL)
    # Este import não abre o SQLite; a substituição precede os leitores e relatórios.
    from app import costs_cache
    costs_cache._CACHE_DIR = tmp / "py"
    from app import costs_sources, pricing, uso_areas, uso_report

    pricing_dir = tmp / "pricing"
    if frozen is None:
        pricing_dir.mkdir()
        for name in ("models.dev.json", "overrides.json"):
            copy_metadata(pricing._CACHE_DIR / name, pricing_dir / name)
    pricing._CACHE_DIR = pricing_dir
    area_file = tmp / "areas.json"
    if frozen is None:
        copy_metadata(uso_areas._arquivo(), area_file)
    uso_areas._arquivo = lambda: area_file
    uso_areas.recarregar()
    if frozen is None:
        accounts = costs_sources._contas_codex()
    else:
        accounts = [costs_sources.codex_contas.Account(a["id"], Path(a["home"]), a["is_default"])
                    for a in frozen["accounts"]]
    costs_sources.codex_contas.list_accounts = lambda: accounts
    costs_sources._contas_codex = lambda: accounts
    scope_file = tmp / "scopes.json"
    if frozen is None:
        scopes = costs_sources.scopes_for_rust()
        scope_file.write_text(json.dumps(scopes, ensure_ascii=False), encoding="utf-8")
    else:
        scopes = json.loads(scope_file.read_text(encoding="utf-8"))
        costs_sources._ROTULOS.clear()
        costs_sources._ROTULOS.update(frozen["labels"])
    # Congela as fontes e a ordem decididas pela referência antes das duas varreduras.
    costs_sources._config_dirs = lambda: [(str(Path(s["root"]).parent), s["account"]) for s in scopes["claude"]]
    costs_sources._contas_codex = lambda: accounts
    costs_sources._pi_roots = lambda: [(Path(s["root"]), s["source"]) for s in scopes["pi"]]
    if scopes["kimi"] is not None:
        costs_sources.raiz_kimi = lambda: Path(scopes["kimi"]["root"])
    else:
        costs_sources.raiz_kimi = lambda: tmp / "absent-kimi"
    inputs = {"accounts": [{"id": a.id, "home": str(a.home), "is_default": a.is_default} for a in accounts],
              "labels": dict(costs_sources._ROTULOS),
              "origins": frozen["origins"] if frozen else uso_report.origens_de_skill()}
    return inputs, scope_file, pricing_dir, area_file


def run_process(command: list[str], timeout: int, stage: str) -> bytes:
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          start_new_session=(os.name != "nt")) as process:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            # O lançador pode manter filhos com os pipes abertos, inclusive durante o build.
            if os.name == "nt":
                import psutil
                try:
                    for child in psutil.Process(process.pid).children(recursive=True):
                        try:
                            child.kill()
                        except psutil.NoSuchProcess:
                            pass
                except psutil.NoSuchProcess:
                    pass
                process.kill()
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            try:
                process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                raise ComparisonError(f"{stage}_drain_timeout") from None
            raise ComparisonError(f"{stage}_timeout") from None
        if process.returncode:
            if stage == "rust_process":
                for line in stderr.splitlines():
                    if re.fullmatch(rb"diagnostic: [a-z_]+(?: index=[0-9]+)?", line):
                        print(line.decode("ascii"))
                    elif line.startswith(b"phase: "):
                        try:
                            phase = json.loads(line[7:])
                        except ValueError:
                            continue
                        if (set(phase) == {"phase", "elapsed_s", "rss_mb", "peak_rss_mb", "rows", "usage_rows", "token_rows"}
                                and re.fullmatch(r"after_[a-z_]+", phase["phase"])
                                and all(type(value) in (int, float) for key, value in phase.items() if key != "phase")):
                            print(f"Etapa antes da falha {phase['phase']}: tempo={phase['elapsed_s']:.3f} s; "
                                  f"RSS={phase['rss_mb']} MiB; pico={phase['peak_rss_mb']} MiB; contagem={phase['rows']}.")
                for line in stderr.splitlines():
                    if line.startswith(b"erro: ") and re.fullmatch(rb"[a-z_]+", line[6:]):
                        raise ComparisonError(f"{stage}_failed:{line[6:].decode('ascii')} (exit={process.returncode})")
            raise ComparisonError(f"{stage}_failed (exit={process.returncode})")
        return stdout


def build_rust(args: argparse.Namespace) -> str:
    cargo = shutil_which("cargo")
    build = [cargo, "build", "-q", "--release", "--locked", "--manifest-path", str(ROOT / "crates/Cargo.toml"),
             "-p", "hangar-server", "--example", "custos", "--message-format=json"]
    artifacts = run_process(build, args.timeout, "rust_build")
    binary = None
    for line in artifacts.splitlines():
        try:
            artifact = json.loads(line)
        except ValueError:
            continue
        if (artifact.get("reason") == "compiler-artifact" and artifact.get("target", {}).get("name") == "custos"
                and artifact.get("executable")):
            binary = artifact["executable"]
    if binary is None:
        raise ComparisonError("rust_example_missing")
    return binary


def prebuilt_binary(args: argparse.Namespace) -> str:
    binary = Path(args.binary).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ComparisonError("prebuilt_binary_unavailable")
    if args.binary_sha256 is not None:
        if hashlib.sha256(binary.read_bytes()).hexdigest() != args.binary_sha256:
            raise ComparisonError("prebuilt_binary_hash_mismatch")
    return str(binary)


def rust_reports(args: argparse.Namespace, tmp: Path, now: datetime, scopes: Path, pricing: Path, areas: Path,
                 binary: str) -> dict:
    command = [str(binary), "--scopes", str(scopes),
               "--index", str(tmp / "rs"), "--now", now.isoformat(), "--period", args.period,
               "--home", str(Path.home()), "--pricing", str(pricing), "--areas", str(areas),
               "--timeout", str(args.timeout)]
    for name in ("conta", "projeto", "modelo", "plugin"):
        for value in getattr(args, name):
            command.extend((f"--{name}", value))
    if args.foco is not None:
        command.extend(("--foco", args.foco))
    if args.profile or args.rust_only:
        command.append("--profile")
    if args.incremental:
        command.append("--incremental")
    if args.snapshot_worker is not None:
        command.extend(("--origins", str(tmp / "origins.json")))
    output = run_process(command, args.timeout, "rust_process")
    try:
        return json.loads(output)
    except (ValueError, UnicodeError):
        raise ComparisonError("rust_output_invalid") from None


def shutil_which(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise ComparisonError("program_missing")
    return path


def clone_file(source: Path, target: Path) -> int:
    import fcntl
    with source.open("rb") as reader, target.open("xb") as writer:
        info = os.fstat(reader.fileno())
        try:
            # Reflink captura os bytes sem duplicar todo o histórico em disco.
            fcntl.ioctl(writer.fileno(), 0x40049409, reader.fileno())
        except OSError:
            remaining = info.st_size
            while remaining:
                data = reader.read(min(1024 * 1024, remaining))
                if not data:
                    raise ComparisonError("snapshot_source_shrank")
                writer.write(data)
                remaining -= len(data)
    os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns))
    return target.stat().st_size


def snapshot_overlays(scopes: dict, tmp: Path) -> tuple[list[tuple[Path, Path]], int, int]:
    requested = [Path(s["root"]) for s in scopes["claude"] + scopes["pi"]]
    for scope in scopes["codex"]:
        requested.extend((Path(scope["home"]) / "sessions", Path(scope["home"]) / "archived_sessions"))
    if scopes["kimi"] is not None:
        requested.extend((Path(scopes["kimi"]["root"]), Path(scopes["kimi"]["index"])))
    roots = sorted({p.resolve() for p in requested if p.exists()}, key=lambda p: (len(p.parts), str(p)))
    selected: list[Path] = []
    for path in roots:
        if not any(path == parent or (parent.is_dir() and path.is_relative_to(parent)) for parent in selected):
            selected.append(path)
    overlays: list[tuple[Path, Path]] = []
    queue = selected[:]
    seen: set[Path] = set()
    file_count = byte_count = 0
    snapshot = tmp / "snapshot"
    snapshot.mkdir()

    def covered(path: Path) -> bool:
        return any(path == parent or (parent.is_dir() and path.is_relative_to(parent)) for parent in selected)

    def copy_tree(source: Path, target: Path) -> None:
        nonlocal file_count, byte_count
        if source.is_dir():
            info = source.stat()
            target.mkdir()
            for child in sorted(source.iterdir()):
                destination = target / child.name
                if child.is_symlink():
                    destination.symlink_to(os.readlink(child), target_is_directory=child.is_dir())
                    canonical = child.resolve()
                    if child.exists() and (child.is_dir() or child.name.endswith(".jsonl")) and not covered(canonical):
                        selected.append(canonical)
                        queue.append(canonical)
                elif child.is_dir():
                    copy_tree(child, destination)
                elif child.is_file() and child.name.endswith(".jsonl"):
                    byte_count += clone_file(child, destination)
                    file_count += 1
            os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns))
        elif source.is_file():
            byte_count += clone_file(source, target)
            file_count += 1
        else:
            raise ComparisonError("snapshot_source_unavailable")

    while queue:
        source = queue.pop(0)
        if source in seen:
            continue
        seen.add(source)
        target = snapshot / str(len(overlays))
        copy_tree(source, target)
        overlays.append((source, target))
    return overlays, file_count, byte_count


def snapshot_command(tmp: Path, overlays: list[tuple[Path, Path]], command: list[str]) -> list[str]:
    # A morte do PID inicial encerra até netos que abriram outra sessão de processos.
    result = [shutil_which("bwrap"), "--die-with-parent", "--unshare-pid", "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev",
              "--bind", str(tmp), str(tmp), "--setenv", "TMPDIR", str(tmp), "--setenv", "SQLITE_TMPDIR", str(tmp)]
    # Pais precedem filhos para que uma sobreposição aninhada não desapareça.
    for source, target in sorted(overlays, key=lambda pair: (len(pair[0].parts), str(pair[0]))):
        result.extend(("--ro-bind", str(target), str(source)))
    result.extend(command)
    return result


def snapshot_reports(args: argparse.Namespace, tmp: Path, now: datetime, binary: str) -> int:
    run_process(snapshot_command(tmp, [], [sys.executable, "-c", "pass"]), 10, "snapshot_probe")
    frozen, scopes_path, _, _ = python_reports(args, tmp, now, collect=False)
    scopes = json.loads(scopes_path.read_text(encoding="utf-8"))
    frozen.update({"tmp": str(tmp), "now": now.isoformat()})
    (tmp / "origins.json").write_text(json.dumps(frozen["origins"], ensure_ascii=False), encoding="utf-8")
    descriptor = tmp / "snapshot-inputs.json"
    descriptor.write_text(json.dumps(frozen, ensure_ascii=False), encoding="utf-8")
    overlays, files, size = snapshot_overlays(scopes, tmp)
    command = [sys.executable, "-B", str(Path(__file__).resolve()), "--snapshot-worker", str(descriptor),
               "--binary", binary, "--period", args.period, "--timeout", str(args.timeout)]
    for name in ("conta", "projeto", "modelo", "plugin"):
        for value in getattr(args, name):
            command.extend((f"--{name}", value))
    for name in ("profile", "rust_only", "incremental"):
        if getattr(args, name):
            command.append(f"--{name.replace('_', '-')}")
    if args.foco is not None:
        command.extend(("--foco", args.foco))
    print(f"Snapshot privado: {files} arquivos; {size} bytes; {len(overlays)} sobreposições somente leitura.", flush=True)
    with subprocess.Popen(snapshot_command(tmp, overlays, command), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          start_new_session=True) as process:
        try:
            output, stderr = process.communicate(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)
            raise ComparisonError("snapshot_worker_timeout") from None
    # O worker só emite os diagnósticos anonimizados do comparador, nunca os relatórios.
    text = output.decode("utf-8", errors="strict")
    print(text, end="")
    if process.returncode not in (0, 1):
        # Só códigos emitidos por este script podem atravessar o stderr do namespace.
        for line in stderr.splitlines():
            if line.startswith(b"erro: "):
                code = line[6:]
                if re.fullmatch(rb"[a-z_]+(?::[a-z_]+)*(?: \(exit=-?[0-9]+\))?", code):
                    raise ComparisonError(f"snapshot_worker_failed:{code.decode('ascii')}")
        for marker, code in ((b"bwrap:", "snapshot_namespace_failed"),
                             (b"Traceback", "snapshot_python_failed"),
                             (b"Read-only file system", "snapshot_readonly_violation")):
            if marker in stderr:
                raise ComparisonError(code)
        raise ComparisonError("snapshot_worker_failed")
    return process.returncode


def index_snapshot(path: Path) -> tuple[dict, dict]:
    if not path.is_file():
        raise ComparisonError("temporary_index_missing")
    with sqlite3.connect(path) as connection:
        connection.execute("PRAGMA query_only=ON")
        files = {row[0]: row[1:] for row in connection.execute(
            "SELECT path, id, offset, size, mtime_ns, cauda FROM files ORDER BY path")}
        columns = {table: [row[1] for row in connection.execute(f"PRAGMA table_info({table})")][1:]
                   for table in ("custo", "uso")}
        records = {}
        for name, values in files.items():
            records[name] = {table: list(connection.execute(
                f"SELECT {','.join(columns[table])} FROM {table} WHERE file_id=? ORDER BY rowid", (values[0],)))
                             for table in columns}
    return files, records


def self_test() -> None:
    cases = [
        ({"a": [1, 0.5, None]}, {"a": [1, 0.5 + 1e-11, None]}, 0),
        ({"a": 1, "b": 2}, {"b": 2, "a": 1}, 1),
        ([2**60], [2**60 + 1], 1),
        ([1], [1.0], 1),
        ([1e-12], [2e-12], 0),
        ([0.0], [1e-15], 0),
        ([float("nan")], [float("nan")], 1),
        (["conteúdo privado"], ["outro conteúdo"], 1),
    ]
    for left, right, expected in cases:
        errors: list[str] = []
        compare(left, right, "report", errors)
        if len(errors) != expected or any("conteúdo" in error for error in errors):
            raise ComparisonError("comparator_self_test_failed")
    parsed = parser().parse_args(["--now", "2026-10-03T12:00:00", "--conta", "a", "--conta", "b"])
    if parsed.now.utcoffset() != timedelta(hours=-3) or parsed.conta != ["a", "b"]:
        raise ComparisonError("arguments_self_test_failed")
    args = parser().parse_args(["--snapshot-worker", "synthetic"])
    py = {"costs": {}, "uso": {}, "scan_s": 1.0}
    expected_origins = {"first": "plugin-a", "second": "plugin-b"}
    actual_origins = {"second": "plugin-b", "first": "plugin-a"}
    rs = {"costs": {}, "uso": {}, "scan_s": 1.0, "peak_rss_mb": 20,
          "origins_check": {"expected": len(expected_origins), "actual": len(actual_origins),
                            "missing": len(expected_origins.keys() - actual_origins.keys()),
                            "extra": len(actual_origins.keys() - expected_origins.keys()),
                            "different": sum(actual_origins[key] != value for key, value in expected_origins.items()),
                            "order_matches": list(expected_origins) == list(actual_origins)}}
    output = io.StringIO()
    with patch.dict(globals(), python_reports=lambda *a, **k: (py, None, None, None),
                    rust_reports=lambda *a, **k: rs), contextlib.redirect_stdout(output):
        status = compare_reports(args, Path("synthetic"), parse_now("2026-10-03T12:00:00"), "synthetic")
    if status != 1 or "origins_order_mismatch" not in output.getvalue():
        raise ComparisonError("origins_order_self_test_failed")
    rs["origins_check"]["order_matches"] = True
    with patch.dict(globals(), python_reports=lambda *a, **k: (py, None, None, None),
                    rust_reports=lambda *a, **k: rs), contextlib.redirect_stdout(io.StringIO()):
        if compare_reports(args, Path("synthetic"), parse_now("2026-10-03T12:00:00"), "synthetic") != 0:
            raise ComparisonError("origins_matching_self_test_failed")
    print("Autoverificação: 8 casos, argumentos e reprovação da ordem das origens aprovados; nenhum dado real lido.")


def self_test_snapshot() -> None:
    if sys.platform != "linux":
        raise ComparisonError("snapshot_probe_requires_linux")

    def host_processes(marker: bytes) -> dict[int, tuple[str, str, str]]:
        found = {}
        for command in Path("/proc").glob("[0-9]*/cmdline"):
            try:
                raw = command.read_bytes()
                if marker not in raw:
                    continue
                pid = int(command.parent.name)
                parts = (command.parent / "stat").read_text().rsplit(")", 1)[1].split()
                argv = raw.rstrip(b"\0").split(b"\0")
                role = argv[-1].decode("ascii") if argv[0] == os.fsencode(sys.executable) else ""
                found[pid] = (parts[19], parts[0], role)
            except (OSError, ValueError, IndexError):
                continue
        return found

    def alive(pid: int, started: str) -> bool:
        try:
            parts = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            return parts[19] == started and parts[0] != "Z"
        except (OSError, IndexError):
            return False

    with tempfile.TemporaryDirectory(prefix="hangar-snapshot-timeout-synthetic-") as directory:
        tmp = Path(directory).resolve()
        marker = str(tmp).encode()
        ready = tmp / "ready"
        # O neto entra em outra sessão, como o executável Rust do comparador.
        grandchild = f"from pathlib import Path; import time; Path({str(ready)!r}).touch(); time.sleep(60)"
        child = ("import subprocess,sys,time; subprocess.Popen([sys.executable,'-B','-c',"
                 f"{grandchild!r},'synthetic-grandchild'],start_new_session=True); time.sleep(60)")
        worker = ("import subprocess,sys,time; subprocess.Popen([sys.executable,'-B','-c',"
                  f"{child!r},'synthetic-child'],start_new_session=True); time.sleep(60)")
        originals: dict[int, tuple[str, str, str]] = {}
        stop = threading.Event()

        def observe() -> None:
            while not stop.wait(0.01):
                if ready.exists():
                    originals.update(host_processes(marker))
                    return

        def inputs(*args, **kwargs):
            scopes = tmp / "scopes.json"
            scopes.write_text("{}", encoding="utf-8")
            return {"origins": {}}, scopes, tmp / "pricing", tmp / "areas.json"

        original_command = snapshot_command

        def command(root, overlays, argv):
            if "--snapshot-worker" in argv:
                argv = [sys.executable, "-B", "-c", worker, "synthetic-worker"]
            return original_command(root, overlays, argv)

        thread = threading.Thread(target=observe)
        thread.start()
        outcome = None
        try:
            args = parser().parse_args(["--timeout", "1"])
            with patch.dict(globals(), python_reports=inputs, snapshot_overlays=lambda *a: ([], 0, 0),
                            snapshot_command=command), contextlib.redirect_stdout(io.StringIO()):
                try:
                    snapshot_reports(args, tmp, parse_now("2026-10-03T12:00:00"), "synthetic")
                except ComparisonError as error:
                    outcome = str(error)
                except subprocess.TimeoutExpired:
                    outcome = "snapshot_drain_timeout"
            stop.set()
            thread.join(timeout=2)
            roles = {info[2] for info in originals.values()}
            if thread.is_alive() or not {"synthetic-worker", "synthetic-child", "synthetic-grandchild"} <= roles:
                raise ComparisonError("snapshot_host_pid_capture_failed")
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline and any(alive(pid, info[0]) for pid, info in originals.items()):
                time.sleep(0.01)
            if any(alive(pid, info[0]) for pid, info in originals.items()):
                raise ComparisonError("snapshot_orphan_self_test_failed")
            if outcome != "snapshot_worker_timeout":
                raise ComparisonError("snapshot_timeout_code_self_test_failed")
            print(f"Timeout sintético: {len(originals)} PIDs originais do host encerrados ou zombies; código snapshot_worker_timeout.")
        finally:
            stop.set()
            thread.join(timeout=2)
            # A regressão vermelha também limpa somente processos deste marcador privado.
            current = host_processes(marker)
            for pid, info in current.items():
                if alive(pid, info[0]):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
    if tmp.exists():
        raise ComparisonError("snapshot_cleanup_self_test_failed")
    print("Cleanup sintético aprovado; nenhum processo ou dado real utilizado.")


def main() -> int:
    args = parser().parse_args()
    if args.self_test:
        self_test()
        return 0
    if args.self_test_snapshot:
        self_test_snapshot()
        return 0
    if args.snapshot_worker is not None:
        frozen = json.loads(args.snapshot_worker.read_text(encoding="utf-8"))
        tmp = Path(frozen["tmp"])
        protect_writes(tmp)
        return compare_reports(args, tmp, parse_now(frozen["now"]), args.binary, frozen)
    now = args.now or datetime.now(LOCAL).replace(microsecond=0)
    with tempfile.TemporaryDirectory(prefix="hangar-costs-comparison-") as directory:
        tmp = Path(directory).resolve()
        protect_writes(tmp)
        # Compilar antes reduz o intervalo entre as leituras dos arquivos que ainda crescem.
        binary = prebuilt_binary(args) if args.binary else build_rust(args)
        if args.snapshot:
            return snapshot_reports(args, tmp, now, binary)
        return compare_reports(args, tmp, now, binary)


def compare_reports(args: argparse.Namespace, tmp: Path, now: datetime, binary: str, frozen: dict | None = None) -> int:
    py, scopes, pricing, areas = python_reports(args, tmp, now, frozen=frozen)
    rs = rust_reports(args, tmp, now, scopes, pricing, areas, binary)
    if args.profile or args.rust_only:
        allowed_phases = {"after_prepare", "after_read_costs", "after_read_usage", "after_origins",
                          "after_build_costs", "after_build_usage", "after_serialize", "after_incremental",
                          "after_incremental_reports"}
        for phase in rs.get("phases", []):
            if phase.get("phase") not in allowed_phases:
                raise ComparisonError("rust_phase_invalid")
            print(f"Etapa {phase['phase']}: tempo acumulado={phase['elapsed_s']:.3f} s; "
                  f"RSS={phase['rss_mb']} MiB; pico={phase['peak_rss_mb']} MiB; contagem={phase['rows']}.")
            if phase["phase"] == "after_read_usage":
                print(f"Linhas separadas: uso={phase['usage_rows']}; tokens={phase['token_rows']}.")
    origin_errors = False
    if args.snapshot_worker is not None:
        check = rs.get("origins_check")
        if not isinstance(check, dict):
            raise ComparisonError("rust_origins_check_missing")
        print(f"Origens reais: Python={check['expected']}; Rust={check['actual']}; ausentes={check['missing']}; "
              f"extras={check['extra']}; valores diferentes={check['different']}; ordem igual={check['order_matches']}.")
        origin_errors = any(check[key] for key in ("missing", "extra", "different")) or check["order_matches"] is not True
        if check["order_matches"] is not True:
            print("  origins: origins_order_mismatch")
    incremental_errors = False
    if args.incremental:
        check = rs.get("incremental")
        if not isinstance(check, dict) or type(check.get("scan_s")) not in (int, float):
            raise ComparisonError("incremental_measurement_missing")
        print(f"Coleta incremental Rust: {check['scan_s']:.6f} s; arquivos={check['files']}; "
              f"pico completo={check['peak_rss_mb']} MiB; relatórios idênticos={check['reports_equal']}.")
        incremental_errors = check["reports_equal"] is not True
    if args.rust_only:
        print(f"Varredura Rust: {rs['scan_s']:.3f} s; pico completo: {rs['peak_rss_mb']} MiB; "
              "paridade Python não executada nesta rodada diagnóstica.")
        return int(origin_errors or incremental_errors)
    for name in ("costs", "uso"):
        if name not in rs:
            raise ComparisonError("rust_report_missing")
    scan_s, rss_mb = rs.get("scan_s"), rs.get("peak_rss_mb")
    if type(scan_s) not in (int, float) or not math.isfinite(scan_s) or scan_s < 0:
        raise ComparisonError("rust_scan_measurement_invalid")
    if type(rss_mb) is not int or rss_mb < 0:
        raise ComparisonError("rust_memory_measurement_invalid")
    print(f"Varredura Rust: {scan_s:.3f} s; pico Rust: {rss_mb} MiB")
    limits_ok = scan_s < 5 and 0 < rss_mb < 100
    if not limits_ok:
        print("Meta de desempenho não atingida ou memória indisponível (scan < 5 s; pico < 100 MiB).")
    return int(origin_errors or incremental_errors or not limits_ok)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        code = str(error) if isinstance(error, ComparisonError) else "comparison_failed"
        print(f"erro: {code}", file=sys.stderr)
        raise SystemExit(2) from None
