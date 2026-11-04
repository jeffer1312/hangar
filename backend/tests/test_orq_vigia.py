"""vigia.sh -e e a espera registrada (`orq event espera`), com curl, hangar-send, tmux e envio do orq
falsos: nada toca sessão ou backend reais."""
import importlib.util
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timedelta
from pathlib import Path

import pytest

pytestmark = pytest.mark.usefixtures("hangar_server_bin")

ROOT = Path(__file__).resolve().parents[2]
VIGIA = ROOT / "skills" / "orquestrar" / "scripts" / "vigia.sh"
ORQ = ROOT / "skills" / "orquestrar" / "scripts" / "orq.py"

CURL = r"""#!/bin/sh
for a; do url=$a; done
case "$url" in
  */api/sessions)
    if [ -f "$VT/on-sessions" ]; then sh "$VT/on-sessions"; rm -f "$VT/on-sessions"; fi
    cat "$VT/sessions.json" ;;
  *) exit 22 ;;
esac
"""
LOG_ARGS = '#!/bin/sh\nprintf "%s %s\\n" "$(basename "$0")" "$*" >> "$VT/sent.log"\n'

PLAN = """# Orchestration plan — t

## Projeto
Checagens: —
Integração: —
Prova: por-task
Paralelo: até 4
Revisão: sessão
Correção pelo revisor: até 0 linhas

## Tasks
| # | What it is | Where in their plan | Files | Verification | Wave | Roteiro |
|---|---|---|---|---|---|---|
| 1 | t | §1 | `README.md` | `true` | 1 | — |
| 2 | u | §2 | `README.md` | `true` | 1 | — |
"""


class Run:
    def __init__(self, tmp: Path):
        self.t = tmp
        self.d = tmp / "2026-09-30-g1"
        self.d.mkdir()
        bin_ = tmp / "bin"
        bin_.mkdir()
        for name, body in (("curl", CURL), ("hangar-send", LOG_ARGS), ("orq-send", LOG_ARGS),
                           ("tmux", "#!/bin/sh\nexit 1\n")):
            (bin_ / name).write_text(body)
            (bin_ / name).chmod(0o755)
        (tmp / "env").write_text("CP_AUTH_TOKEN=x\n")
        (tmp / "sent.log").write_text("")
        self.env = {**os.environ, "PATH": f"{bin_}{os.pathsep}{os.environ['PATH']}", "VT": str(tmp),
                    "HOME": str(tmp), "CLAUDE_CONFIG_DIR": str(tmp / "cfg"), "ORQ_SEND": str(bin_ / "orq-send"),
                    "ORQ_JEV": "off", "ORQ_WHOAMI": "false", "CP_ENV": str(tmp / "env"),
                    "CP_VIGIA_LOG": str(tmp / "err"), "CP_VIGIA_INTERVALO": "0"}
        plan = tmp / "plan.orq.md"
        plan.write_text(PLAN)
        self.orq("plan-check", str(plan), "--repo", str(ROOT), "--stamp")
        self.orq("init", "--arbiter", "arb", "--repo", str(ROOT), "--contract", str(tmp / "regras.md"),
                 "--plan", str(plan))

    def orq(self, *args, check=True):
        r = subprocess.run([sys.executable, str(ORQ), "--dir", str(self.d), *args], env=self.env,
                           capture_output=True, text=True)
        if check:
            assert r.returncode == 0, r.stdout + r.stderr
        return r

    def ball(self, *flags):
        return self.orq("ball", *flags).stdout.split()

    def wait(self, who, seconds=3600, *extra):
        ate = (datetime.now().astimezone() + timedelta(seconds=seconds)).isoformat(timespec="seconds")
        return self.orq("event", "espera", "--sessao", who, "--ate", ate, "--motivo", "external job", *extra)

    def start_task(self, n, executor, par, delivered=False):
        self.orq("event", "task_inicio", "--task", str(n), "--titulo", f"t{n}", "--executor", executor, "--par", par)
        if delivered:
            self.orq("event", "entrega", "--task", str(n), "--rodada", "1")

    def waiting(self):
        spec = importlib.util.spec_from_file_location("orq_vigia_state", ORQ)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod.state(self.d)["waiting"]

    def age_trail(self):
        old = time.time() - 2 * 3600
        for f in ("registro.md", "eventos.jsonl"):
            os.utime(self.d / f, (old, old))

    def sessions(self, **states):
        self.t.joinpath("sessions.json").write_text(
            "[" + ",".join(f'{{"name":"{n}","state":"{s}"}}' for n, s in states.items()) + "]")

    def vigia(self, cycles, minutes=1):
        r = subprocess.run(["bash", str(VIGIA), "arb", "-e", str(self.d), "-m", str(minutes), "--no-housekeeping"],
                           env={**self.env, "CP_VIGIA_CICLOS": str(cycles)},
                           capture_output=True, text=True, timeout=120)
        assert r.returncode == 0, r.stdout + r.stderr
        return r.stdout

    def sent(self):
        return self.t.joinpath("sent.log").read_text().splitlines()


@pytest.fixture
def run(tmp_path):
    return Run(tmp_path)


def test_watchdog_keeps_watching_after_twenty_nobody_alarms(run):
    run.sessions(arb="idle")
    out = run.vigia(cycles=22)
    assert len([m for m in run.sent() if "Nobody has had the ball" in m]) == 22
    assert out.rstrip().endswith("1440min over; last state: arb=idle")


def test_new_arbiter_gets_its_own_channel_proof(run):
    run.sessions(rev1="working", arb="idle", arb2="idle")
    run.orq("event", "task_inicio", "--task", "1", "--titulo", "x", "--executor", "exec1", "--par", "rev1")
    run.orq("event", "entrega", "--task", "1", "--rodada", "1")
    run.t.joinpath("on-sessions").write_text(
        f"{sys.executable} {ORQ} --dir {run.d} event sessao_trocada --de arb --para arb2 >/dev/null\n")
    run.vigia(cycles=3, minutes=99)
    armed = [m for m in run.sent() if "[vigia] ARMED" in m]
    assert [m.split()[2] for m in armed] == ["arb", "arb2"], armed
    assert "ARMED over: rev1 arb2" in armed[1]
    assert run.d.joinpath(".vigia-armado").read_text() == "arb2"


def test_wait_takes_only_its_owner_off_the_ball_until_the_deadline(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.start_task(2, "exec2", "rev2")
    assert run.ball() == ["rev1", "exec2"]
    run.wait("rev1")
    assert run.ball() == ["exec2"] and run.ball("--coverage") == ["none"]
    run.wait("exec2")
    assert run.ball("--with-arbiter") == ["arb"] and run.ball("--coverage") == ["owners"]
    run.wait("arb")
    assert run.ball("--coverage") == ["all"]
    assert "espera sessao=rev1 ate=" in run.d.joinpath("registro.md").read_text()


def test_renewed_wait_replaces_the_old_one_and_expires_at_its_deadline(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.wait("rev1")
    run.wait("rev1", 2)
    assert run.ball() == []
    time.sleep(3)
    assert run.ball() == ["rev1"] and run.ball("--coverage") == ["none"]


def test_later_event_of_the_task_ends_the_wait(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.wait("rev1", 3600, "--task", "1")
    run.orq("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova", "--sessao", "rev1")
    run.orq("event", "entrega", "--task", "1", "--rodada", "2")
    assert run.ball() == ["rev1"]


def test_wait_without_task_ends_on_an_event_of_a_task_its_session_holds(run):
    run.start_task(1, "exec1", "rev1")
    run.wait("exec1")
    assert run.ball() == []
    run.orq("event", "entrega", "--task", "1", "--rodada", "1")
    run.orq("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova", "--sessao", "rev1")
    assert run.ball() == ["exec1"]


@pytest.mark.parametrize("args,error", [
    (("--sessao", "rev1", "--ate", "2099-01-01T10:00:00+00:00", "--task", "9"), "Task 9 has no task_inicio"),
    (("--sessao", "stranger", "--ate", "2099-01-01T10:00:00+00:00"), "neither the arbiter nor a session"),
    (("--sessao", "rev1", "--ate", "2020-01-01T10:00:00+00:00"), "future ISO-8601"),
    (("--sessao", "rev1", "--ate", "tomorrow"), "future ISO-8601"),
    (("--sessao", "rev1", "--ate", "2099-01-01T10:00:00"), "future ISO-8601"),
])
def test_invalid_wait_is_refused(run, args, error):
    run.start_task(1, "exec1", "rev1", delivered=True)
    r = run.orq("event", "espera", *args, check=False)
    assert r.returncode != 0 and error in r.stdout + r.stderr
    assert '"espera"' not in run.d.joinpath("eventos.jsonl").read_text()


def test_wait_after_the_run_ended_is_refused_and_keeps_it_ended(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.orq("event", "execucao_fim", "--resultado", "done")
    r = run.orq("event", "espera", "--sessao", "arb", "--ate", "2099-01-01T10:00:00+00:00", "--task", "1", check=False)
    assert r.returncode != 0 and "the run has ended" in r.stdout + r.stderr
    assert '"espera"' not in run.d.joinpath("eventos.jsonl").read_text()


def test_covered_owner_raises_neither_its_own_nor_the_collective_alarm(run):
    run.sessions(rev1="idle", arb="idle")
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.wait("rev1")
    run.vigia(cycles=3)
    assert not [m for m in run.sent() if "is stopped" in m or "Nobody has had the ball" in m], run.sent()


def test_trail_alarm_waits_only_while_every_owner_and_the_arbiter_are_covered(run):
    run.sessions(rev1="idle", exec2="idle", arb="idle")
    run.d.joinpath(".vigia-armado").write_text("arb")
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.start_task(2, "exec2", "rev2")
    run.wait("rev1")
    run.wait("arb")
    run.age_trail()
    run.vigia(cycles=1, minutes=99)
    assert [m for m in run.sent() if "The trail (" in m], "exec2 is not covered: the trail must alarm"
    run.t.joinpath("sent.log").write_text("")
    run.wait("exec2")
    run.age_trail()
    run.vigia(cycles=1, minutes=99)
    assert not [m for m in run.sent() if "The trail (" in m], run.sent()


def test_wait_on_a_task_belongs_to_its_owners_or_the_arbiter(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.start_task(2, "exec2", "rev2")
    r = run.orq("event", "espera", "--sessao", "exec2", "--ate", "2099-01-01T10:00:00+00:00", "--task", "1", check=False)
    assert r.returncode != 0 and "executor or reviewer of Task 1" in r.stdout + r.stderr
    assert '"espera"' not in run.d.joinpath("eventos.jsonl").read_text()
    run.wait("exec2", 3600, "--task", "2")
    run.wait("arb", 3600, "--task", "1")
    run.orq("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova", "--sessao", "rev1")
    assert run.waiting() == {"exec2"}
    assert run.ball() == ["exec1"]


def test_recorded_event_of_the_waiting_session_ends_only_its_wait(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    run.start_task(2, "exec2", "rev2")
    run.wait("arb")
    run.wait("exec2")
    run.orq("log", "back from the user, nothing recorded yet")
    assert run.waiting() == {"arb", "exec2"}
    run.orq("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova", "--sessao", "arb")
    assert run.waiting() == {"exec2"}
    assert run.ball() == ["exec1"]


def test_wait_on_a_closed_task_is_refused_until_it_reopens(run):
    run.start_task(1, "exec1", "rev1", delivered=True)
    ts = datetime.now().astimezone().isoformat(timespec="seconds")
    run.d.joinpath("closed.jsonl").write_text(json.dumps({"ts": ts, "task": 1, "hash": "abc"}) + "\n")
    time.sleep(1.1)   # encerramentos e eventos têm precisão de segundos
    run.start_task(2, "exec1", "rev2")
    r = run.orq("event", "espera", "--sessao", "exec1", "--ate", "2099-01-01T10:00:00+00:00", "--task", "1", check=False)
    assert r.returncode != 0 and "Task 1 is not open" in r.stdout + r.stderr
    assert '"espera"' not in run.d.joinpath("eventos.jsonl").read_text()
    assert run.ball() == ["exec1"] and run.ball("--coverage") == ["none"]
    run.start_task(1, "exec1", "rev1")
    run.wait("exec1", 3600, "--task", "1")
    assert run.waiting() == {"exec1"}


@pytest.mark.parametrize("on_task", [True, False])
def test_wait_from_a_closed_task_does_not_cover_the_next_task_of_the_same_executor(run, on_task):
    run.start_task(1, "exec1", "rev1")
    run.wait("exec1", 3600, *(("--task", "1") if on_task else ()))
    assert run.waiting() == {"exec1"}
    ts = datetime.now().astimezone().isoformat(timespec="seconds")
    run.d.joinpath("closed.jsonl").write_text(json.dumps({"ts": ts, "task": 1, "hash": "abc"}) + "\n")
    time.sleep(1.1)   # closes and events have second precision
    run.start_task(2, "exec1", "rev2")
    assert run.waiting() == set()
    assert run.ball() == ["exec1"] and run.ball("--coverage") == ["none"]


def test_wait_on_a_task_delivered_without_task_inicio_is_refused(run):
    run.orq("event", "entrega", "--task", "9", "--rodada", "1")
    r = run.orq("event", "espera", "--sessao", "arb", "--ate", "2099-01-01T10:00:00+00:00", "--task", "9", check=False)
    assert r.returncode != 0 and "Task 9 has no task_inicio" in r.stdout + r.stderr
    assert '"espera"' not in run.d.joinpath("eventos.jsonl").read_text()
