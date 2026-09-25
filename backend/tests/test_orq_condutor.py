"""orq (skills/orquestrar/scripts/orq.py): o condutor da orquestração, rodado como CLI."""
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

ORQ = Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq.py"


@pytest.fixture
def env(tmp_path):
    d = tmp_path / "orq"
    d.mkdir()
    log = tmp_path / "sent.log"
    fake = tmp_path / "fake-send"
    fake.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{log}"\n')
    fake.chmod(0o755)
    e = {**os.environ, "ORQ_DIR": str(d), "ORQ_SEND": str(fake), "ORQ_JEV": "off",
         "HOME": str(tmp_path), "TYPESAFE_API_KEY": ""}
    return d, log, e


def run(e, *args, check=True):
    r = subprocess.run([sys.executable, str(ORQ), *args], env=e, capture_output=True, text=True)
    if check:
        assert r.returncode == 0, r.stdout + r.stderr
    return r


def sent(log):
    return log.read_text().splitlines() if log.exists() else []


def init(e, tmp_path, contract="", untouchable=()):
    c = tmp_path / "regras.md"
    c.write_text(contract)
    extra = [x for u in untouchable for x in ("--untouchable", u)]
    run(e, "init", "--arbiter", "arb", "--repo", str(tmp_path), "--contract", str(c), *extra)


def test_event_valida_anexa_e_escreve_no_registro(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    ev = json.loads((d / "eventos.jsonl").read_text().splitlines()[-1])
    assert ev["tipo"] == "task_inicio" and ev["task"] == 1 and "ts" in ev
    assert "task_inicio T1" in (d / "registro.md").read_text()


def test_event_invalido_recusa_e_nao_anexa(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    r = run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "talvez",
            "--sessao", "rev", check=False)
    assert r.returncode == 2 and "resultado" in r.stderr
    assert not (d / "eventos.jsonl").exists()


def test_ball_segue_a_rodada_a_troca_e_o_lote(env, tmp_path):
    _, _, e = env
    init(e, tmp_path)
    ball = lambda: run(e, "ball").stdout.split()
    assert ball() == []
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    assert ball() == ["ex"]
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", "abc")
    assert ball() == ["rev"]
    run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova", "--sessao", "rev")
    assert ball() == ["ex"]
    run(e, "event", "sessao_trocada", "--de", "ex", "--para", "ex2")
    assert ball() == ["ex2"]
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "u", "--executor", "ey", "--par", "rev2")
    assert sorted(ball()) == ["ex2", "ey"]
    run(e, "event", "veredito", "--task", "2", "--rodada", "1", "--resultado", "devolvido", "--sessao", "rev2")
    assert ball() == ["ex2"]
    run(e, "event", "execucao_fim", "--resultado", "ok")
    assert ball() == []


def test_read_contract_da_parte_comum_e_so_a_task_pedida(env, tmp_path):
    _, _, e = env
    init(e, tmp_path, "> cabecalho\n## Quem é quem\n| a |\n## Task 1\nsó da um\n## Task 2\nsó da dois\n### detalhe\nainda dois\n")
    out = run(e, "read", "contract", "--task", "2").stdout
    assert "cabecalho" in out and "Quem é quem" in out
    assert "só da dois" in out and "ainda dois" in out
    assert "só da um" not in out


def test_read_contract_avisa_parte_comum_acima_do_teto(env, tmp_path):
    _, _, e = env
    init(e, tmp_path, "x" * 8100 + "\n## Task 1\nt\n")
    r = run(e, "read", "contract", "--task", "1")
    assert "8000" in r.stderr


def test_read_contract_ausente_sai_com_erro_do_orq(env, tmp_path):
    _, _, e = env
    init(e, tmp_path)
    (tmp_path / "regras.md").unlink()
    r = run(e, "read", "contract", "--task", "1", check=False)
    assert r.returncode == 2 and "orq: contract not found:" in r.stderr
    assert "Traceback" not in r.stderr


def test_registro_gira_no_teto_de_caracteres(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    for i in range(45):
        run(e, "event", "sessao_trocada", "--de", f"s{i}", "--para", "y" * 900)
    assert (d / "registro-arquivo-1.md").exists()
    assert "registro-arquivo-1.md" in (d / "registro.md").read_text().splitlines()[0]
    assert (d / "registro.md").stat().st_size < 40_000


def test_read_journal_da_as_ultimas_e_as_da_task(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "7", "--titulo", "t", "--executor", "ex", "--par", "rev")
    for i in range(20):
        run(e, "event", "sessao_trocada", "--de", f"a{i}", "--para", f"b{i}")
    out = run(e, "read", "journal", "--task", "7", "--last", "3").stdout.splitlines()
    assert len(out) == 4 and "T7" in out[0] and "b19" in out[-1]


def test_screen_trava_por_dono_e_expira(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "screen", "take", "--owner", "a")
    r = run(e, "screen", "take", "--owner", "b", "--wait-min", "0", check=False)
    assert r.returncode == 1 and "held by a" in r.stdout
    assert run(e, "screen", "release", "--owner", "b", check=False).returncode == 1
    run(e, "screen", "release", "--owner", "a")
    run(e, "screen", "take", "--owner", "b")
    old = time.time() - 61 * 60
    os.utime(d / "screen.lock", (old, old))
    run(e, "screen", "take", "--owner", "c", "--wait-min", "0")
    assert (d / "screen.lock").read_text() == "c"
    assert "stale" in (d / "registro.md").read_text()
