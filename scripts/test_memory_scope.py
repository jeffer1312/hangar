import os
import shutil
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent / "memory-scope"
# Sem /proc (macOS) não há cgroup a comparar: a saída fixa só prova que o comando rodou.
CGROUP = "cat /proc/self/cgroup 2>/dev/null || echo sem-proc; exit 7"
OWN = Path("/proc/self/cgroup").read_text() if Path("/proc/self/cgroup").exists() else "sem-proc\n"


def _run(env=None, command=CGROUP):
    return subprocess.run(["bash", str(SCRIPT), "bash", "-c", command], env={**os.environ, **(env or {})},
                          capture_output=True, text=True)


def _systemd_user():
    return shutil.which("systemd-run") and subprocess.run(
        ["systemd-run", "--user", "--scope", "--quiet", "--collect", "true"], capture_output=True).returncode == 0


@pytest.mark.skipif(not _systemd_user(), reason="sem o systemd do usuário")
def test_command_runs_in_its_own_capped_scope():
    result = _run({"HANGAR_BUILD_SWAP_MAX": "1G"},
                  'g=$(cut -d: -f3 /proc/self/cgroup); echo "$g"; cat "/sys/fs/cgroup$g/memory.swap.max" '
                  '"/sys/fs/cgroup$g/memory.high"; exit 7')
    assert result.returncode == 7, result.stderr
    cgroup, swap_max, high = result.stdout.split()
    # O Node tira o heap do menor teto do grupo: um MemoryHigh o deixa curto para o svelte-check.
    assert high == "max"
    assert cgroup != Path("/proc/self/cgroup").read_text().strip().rsplit(":", 1)[-1]
    assert cgroup.endswith(".scope") and "/run-" in cgroup
    assert swap_max == str(1 << 30)


def test_without_systemd_runs_in_place(tmp_path):
    (tmp_path / "bash").symlink_to(shutil.which("bash"))
    (tmp_path / "cat").symlink_to(shutil.which("cat"))
    result = _run({"PATH": str(tmp_path)})
    assert result.returncode == 7, result.stderr
    assert result.stdout == OWN


def test_opt_out_runs_in_place():
    result = _run({"HANGAR_NO_MEMORY_SCOPE": "1"})
    assert result.returncode == 7, result.stderr
    assert result.stdout == OWN
