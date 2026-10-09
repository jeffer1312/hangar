"""orq (skills/orquestrar/scripts/orq.py): o condutor da orquestração, rodado como CLI."""
import hashlib
import http.server
import importlib.util
import json
import os
import subprocess
import sys
import threading
import time
from datetime import datetime
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
         "HOME": str(tmp_path), "CLAUDE_CONFIG_DIR": str(tmp_path / ".claude"),
         "ORQ_WHOAMI": "false",   # nunca o hangar-send real: o remetente fica desconhecido
         "TYPESAFE_API_KEY": "", "JEV_ENDPOINT": "", "JEV_MODEL": ""}
    return d, log, e


def run(e, *args, check=True):
    r = subprocess.run([sys.executable, str(ORQ), *args], env=e, capture_output=True, text=True)
    if check:
        assert r.returncode == 0, r.stdout + r.stderr
    return r


def sent(log):
    return log.read_text().splitlines() if log.exists() else []


PLANO = ("# Orchestration plan — t\n\n## Projeto\nChecagens: —\n"
    "Integração: —\nProva: por-task\nParalelo: até 4\nRevisão: sessão\nCorreção pelo revisor: até 0 linhas\n\n"
    "## Tasks\n| # | What it is | Where in their plan | Files | Verification | Wave | Roteiro |\n"
    "|---|---|---|---|---|---|---|\n| 1 | t | §1 | `a.txt` | `true` | 1 | — |\n")


def plano(tmp_path) -> str:
    """Plano mínimo carimbado: o init novo recusa plano sem `Preparado:`."""
    p = tmp_path / "orq-plano.md"
    if not p.exists():
        p.write_text(PLANO)
        subprocess.run([sys.executable, str(ORQ), "plan-check", str(p), "--repo", str(tmp_path),
                        "--stamp"], check=True, capture_output=True)
    return str(p)


def init(e, tmp_path, contract="", untouchable=(), flags=()):
    c = tmp_path / "regras.md"
    c.write_text(contract)
    extra = [x for u in untouchable for x in ("--untouchable", u)]
    run(e, "init", "--arbiter", "arb", "--repo", str(tmp_path), "--contract", str(c), "--plan", plano(tmp_path),
        *extra, *flags)


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


def test_ball_retoma_depois_de_execucao_fim_sem_ressuscitar_task_velha(env, tmp_path):
    _, _, e = env
    init(e, tmp_path)
    ball = lambda: run(e, "ball").stdout.split()
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", "abc")
    run(e, "event", "execucao_fim", "--resultado", "ok")
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "u", "--executor", "ey", "--par", "rev2")
    assert ball() == ["ey"]
    run(e, "event", "execucao_fim", "--resultado", "ok")
    assert ball() == []


def test_ball_com_arbitro_termina_no_arbitro_da_vez(env, tmp_path):
    _, _, e = env
    init(e, tmp_path)
    assert run(e, "ball", "--with-arbiter").stdout.split() == ["arb"]
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    run(e, "event", "sessao_trocada", "--de", "arb", "--para", "arb2")
    assert run(e, "ball", "--with-arbiter").stdout.split() == ["ex", "arb2"]
    assert run(e, "ball").stdout.split() == ["ex"]


def test_read_contract_da_parte_comum_e_so_a_task_pedida(env, tmp_path):
    _, _, e = env
    init(e, tmp_path, "> cabecalho\n## Quem é quem\n| a |\n## Task 1\nsó da um\n## Task 2\nsó da dois\n### detalhe\nainda dois\n")
    out = run(e, "read", "contract", "--task", "2").stdout
    assert "cabecalho" in out and "Quem é quem" in out
    assert "só da dois" in out and "ainda dois" in out
    assert "só da um" not in out


def test_contrato_acima_do_teto_entrega_a_task_com_aviso_e_codigo_3(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    c = json.loads((d / "orq.json").read_text())["contract"]
    Path(c).write_text("x" * 9000 + "\n## Task 2\nsó da Task 2\n")
    r = run(e, "read", "contract", "--task", "2", check=False)
    assert r.returncode == 3
    assert r.stdout.splitlines()[0].startswith("WARNING: the contract's common part has 9001 characters")
    assert "só da Task 2" in r.stdout


def test_lock_generico_e_screen_como_apelido(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    assert run(e, "lock", "take", "porta-8080", "--owner", "a", "--wait-min", "0").stdout.strip() == "taken"
    assert (d / "lock-porta-8080.lock").read_text() == "a"
    r = run(e, "lock", "take", "porta-8080", "--owner", "b", "--wait-min", "0", check=False)
    assert r.returncode == 1 and "held by a" in r.stdout
    run(e, "screen", "take", "--owner", "c", "--wait-min", "0")
    assert (d / "screen.lock").read_text() == "c"
    assert "porta-8080 taken by a" in (d / "registro.md").read_text()


def test_init_recusa_contrato_acima_do_teto_e_a_task_longa_nao_conta(env, tmp_path):
    d, _, e = env
    c = tmp_path / "regras.md"
    c.write_text("x" * 8100 + "\n## Task 1\nt\n")
    r = run(e, "init", "--arbiter", "arb", "--repo", str(tmp_path), "--contract", str(c), "--plan", plano(tmp_path), check=False)
    assert r.returncode == 2 and "8000" in r.stderr
    assert not (d / "orq.json").exists()
    c.write_text("x" * 100 + "\n## Task 1\n" + "t" * 9000 + "\n")
    run(e, "init", "--arbiter", "arb", "--repo", str(tmp_path), "--contract", str(c), "--plan", plano(tmp_path))
    assert (d / "orq.json").exists()


@pytest.mark.parametrize("kind", ["dir", "binary"])
def test_init_contract_unreadable_exits_2_without_traceback(env, tmp_path, kind):
    d, _, e = env
    c = tmp_path / "regras.md"
    if kind == "dir":
        c.mkdir()
    else:
        c.write_bytes(b"\xff\xfe\x80 not utf-8")
    r = run(e, "init", "--arbiter", "arb", "--repo", str(tmp_path), "--contract", str(c), "--plan", plano(tmp_path), check=False)
    assert r.returncode == 2 and "orq: contract unreadable:" in r.stderr
    assert "Traceback" not in r.stderr and not (d / "orq.json").exists()


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
    # cada troca grava a linha do evento e a da identidade indisponível: o teto gira mais de uma vez
    ultimo = max(d.glob("registro-arquivo-*.md"), key=lambda f: int(f.stem.rsplit("-", 1)[1]))
    assert ultimo.name in (d / "registro.md").read_text().splitlines()[0]
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


@pytest.fixture
def repo(tmp_path):
    r = tmp_path / "repo"
    r.mkdir()

    def g(*a):
        return subprocess.run(["git", "-C", str(r), *a], check=True, capture_output=True,
                              text=True).stdout.strip()
    g("init", "-q")
    g("config", "user.email", "t@t")
    g("config", "user.name", "t")
    (r / "a.txt").write_text("1\n")
    (r / "plano.md").write_text("p\n")
    g("add", "a.txt", "plano.md")
    g("commit", "-qm", "base")
    return r, g


def _aprova(e, g):
    """O stage atual vira a rodada 1 da Task 1, entregue e aprovada."""
    h = g("stash", "create")
    g("stash", "store", "-m", "task-1 round 1", h)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", h)
    run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova", "--sessao", "rev")


def _rodada_aprovada(e, r, g, extra=()):
    """Task 1 com a.txt na rodada; o plano sujo do árbitro fica fora do stage."""
    (r / "a.txt").write_text("2\n")
    (r / "plano.md").write_text("p2\n")
    g("add", "a.txt")
    _aprova(e, g)
    for f in extra:
        (r / f).parent.mkdir(parents=True, exist_ok=True)
        (r / f).write_text("x\n")
        g("add", f)
    g("commit", "-qm", "t1")
    return g("rev-parse", "HEAD")


def test_aprova_avisa_o_executor_e_nao_o_arbitro(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    _rodada_aprovada(e, r, g)
    msgs = sent(log)
    assert any(m.startswith("ex APROVA Task 1 round 1") for m in msgs)
    assert not any(m.startswith("arb ") for m in msgs)


def test_reprova_nao_acorda_ninguem_e_devolvido_acorda_o_arbitro(env, tmp_path):
    d, log, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova", "--sessao", "rev")
    assert sent(log) == []
    run(e, "event", "veredito", "--task", "1", "--rodada", "2", "--resultado", "reprova",
        "--sessao", "rev", "--reincide", "--motivo", "/x/parecer.md")
    assert sent(log)[-1].startswith("arb [decisao] Task 1 round 2: reprova (reincide)")
    run(e, "event", "sessao_trocada", "--de", "arb", "--para", "arb2")
    run(e, "event", "veredito", "--task", "1", "--rodada", "3", "--resultado", "devolvido", "--sessao", "rev")
    assert sent(log)[-1].startswith("arb2 [decisao] Task 1 round 3: devolvido")


def test_commit_conferido_fecha_a_task_e_acorda_o_arbitro_uma_vez(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    h = _rodada_aprovada(e, r, g)
    out = run(e, "commit", "--task", "1", "--hash", h[:8]).stdout
    assert "ok" in out
    assert [m for m in sent(log) if m.startswith("arb ")] == [
        f"arb [decisao] Task 1 closed and checked: {h[:12]}, 1 file(s), tip = hash, "
        "matches the approved round. Release the next ready Task(s)."]
    assert json.loads((d / "closed.jsonl").read_text())["task"] == 1
    assert run(e, "ball").stdout.strip() == ""


def test_task_reaberta_depois_de_fechada_volta_a_ter_a_vez(env, repo, tmp_path):
    d, _, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    run(e, "commit", "--task", "1", "--hash", _rodada_aprovada(e, r, g))
    assert run(e, "ball").stdout.split() == []
    time.sleep(1.1)  # ts has second precision; a close in the same second still wins
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex2", "--par", "rev2")
    assert run(e, "ball").stdout.split() == ["ex2"]
    # An old closed line without ts closes everything before it.
    with (d / "closed.jsonl").open("a") as f:
        f.write(json.dumps({"task": 1}) + "\n")
    assert run(e, "ball").stdout.split() == []


def test_commit_de_rodada_que_nao_e_stash_e_recusado_limpo(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    (r / "a.txt").write_text("2\n")
    g("commit", "-qam", "t1")
    tip = g("rev-parse", "HEAD")  # a normal commit: one parent
    for rnd, obj in ((1, "diff-task-1-rodada1.txt sha256 2bcdbe6f"), (2, tip)):
        run(e, "event", "entrega", "--task", "1", "--rodada", str(rnd), "--commit", obj)
        run(e, "event", "veredito", "--task", "1", "--rodada", str(rnd), "--resultado", "aprova",
            "--sessao", "rev")
        res = run(e, "commit", "--task", "1", "--hash", tip, check=False)
        assert res.returncode == 1, res.stdout + res.stderr
        assert res.stdout.startswith("REFUSED:")
        assert (f"round object {obj} is not a stash commit; freeze rounds with git stash create + "
                "git stash store (executor.md step 5)") in res.stdout
    assert not (d / "closed.jsonl").exists()
    assert not any(m.startswith("arb ") for m in sent(log))


def test_commit_sem_a_edicao_nao_staged_que_o_revisor_viu_e_recusado(env, repo, tmp_path):
    d, _, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    (r / "a.txt").write_text("2\n")
    g("add", "a.txt")
    (r / "a.txt").write_text("3\n")  # in the round's tree, not in its index
    _aprova(e, g)
    g("commit", "-qm", "t1")
    res = run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), check=False)
    assert res.returncode == 1
    assert "content differs from the approved round in: ['a.txt']" in res.stdout
    assert not (d / "closed.jsonl").exists()


def test_commit_com_arquivo_fora_da_rodada_ou_intocavel_e_recusado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path),
        "--untouchable", "secret/*")
    h = _rodada_aprovada(e, r, g, extra=("secret/k.txt",))
    res = run(e, "commit", "--task", "1", "--hash", h, check=False)
    assert res.returncode == 1
    assert "only in commit ['secret/k.txt']" in res.stdout
    assert "untouchable in commit history: ['secret/k.txt']" in res.stdout
    assert not any(m.startswith("arb ") for m in sent(log))
    assert not (d / "closed.jsonl").exists()


def test_commit_com_intocavel_acentuado_e_recusado_com_o_nome_cru(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path),
        "--untouchable", "secret/*")
    h = _rodada_aprovada(e, r, g, extra=("secret/decisão.txt",))
    res = run(e, "commit", "--task", "1", "--hash", h, check=False)
    assert res.returncode == 1
    assert "untouchable in commit history: ['secret/decisão.txt']" in res.stdout
    assert not (d / "closed.jsonl").exists()


def test_commit_que_nao_e_a_ponta_e_recusado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    h = _rodada_aprovada(e, r, g)
    (r / "a.txt").write_text("3\n")
    g("commit", "-qam", "outro")
    res = run(e, "commit", "--task", "1", "--hash", h, check=False)
    assert res.returncode == 1 and "is not the tip" in res.stdout


def test_commit_de_correcao_fecha_a_task_contando_desde_a_base_da_rodada(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    (r / "a.txt").write_text("2\n")
    (r / "b.txt").write_text("b\n")
    g("add", "a.txt", "b.txt")
    _aprova(e, g)
    g("commit", "-qm", "t1", "a.txt")
    res = run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), check=False)
    assert res.returncode == 1 and "only in round ['b.txt']" in res.stdout
    g("commit", "-qm", "t1 fix", "b.txt")
    tip = g("rev-parse", "HEAD")
    run(e, "commit", "--task", "1", "--hash", tip)
    assert [m for m in sent(log) if m.startswith("arb ")] == [
        f"arb [decisao] Task 1 closed and checked: {tip[:12]}, 2 file(s), tip = hash, "
        "matches the approved round. Release the next ready Task(s)."]


def test_intocavel_commitado_e_revertido_e_recusado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path),
        "--untouchable", "secret/*")
    _rodada_aprovada(e, r, g, extra=("secret/k.txt",))
    g("rm", "-q", "secret/k.txt")
    g("commit", "-qm", "revert")
    res = run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), check=False)
    assert res.returncode == 1
    assert "untouchable in commit history: ['secret/k.txt']" in res.stdout
    assert not any(m.startswith("arb ") for m in sent(log))
    assert not (d / "closed.jsonl").exists()


def test_conteudo_diferente_da_rodada_e_recusado_e_a_correcao_fecha(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    (r / "a.txt").write_text("2\n")
    g("add", "a.txt")
    _aprova(e, g)
    (r / "a.txt").write_text("3\n")
    g("commit", "-qam", "t1")
    res = run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), check=False)
    assert res.returncode == 1
    assert "content differs from the approved round in: ['a.txt']" in res.stdout
    (r / "a.txt").write_text("2\n")
    g("commit", "-qam", "t1 fix")
    run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"))
    assert json.loads((d / "closed.jsonl").read_text())["task"] == 1


def test_commit_com_repo_confere_na_worktree_do_lote(env, repo, tmp_path):
    d, _, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    wt = tmp_path / "wt"
    g("worktree", "add", "-q", "-b", "t1", str(wt))

    def gw(*a):
        return subprocess.run(["git", "-C", str(wt), *a], check=True, capture_output=True,
                              text=True).stdout.strip()
    h = _rodada_aprovada(e, wt, gw)
    res = run(e, "commit", "--task", "1", "--hash", h, check=False)
    assert res.returncode == 1 and "is not the tip" in res.stdout
    run(e, "commit", "--task", "1", "--hash", h, "--repo", str(wt))
    assert json.loads((d / "closed.jsonl").read_text())["hash"] == h


def test_notify_aviso_vai_pro_registro_decisao_e_sem_marca_acordam(env, tmp_path):
    d, log, e = env
    init(e, tmp_path)
    run(e, "notify", "[aviso] binário congelado abc")
    assert sent(log) == []
    assert "binário congelado" in (d / "registro.md").read_text()
    run(e, "notify", "[decisão] preciso sair da receita: motivo")
    run(e, "notify", "texto sem marca")
    assert sent(log) == ["arb [decisão] preciso sair da receita: motivo", "arb texto sem marca"]
    run(e, "notify", "--alarm", "[vigia] x parado")
    assert sent(log)[-1] == "--tmux arb [vigia] x parado"


def test_notify_registra_antes_de_enviar_e_o_envio_falho_deixa_rastro(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "notify", "[decisao] T1: preciso da tela")
    run(e, "notify", "sem marca")
    falho = tmp_path / "send-falho"
    falho.write_text("#!/bin/sh\nexit 3\n")
    falho.chmod(0o755)
    r = run({**e, "ORQ_SEND": str(falho)}, "notify", "[decisao] T1: rodada 2 congelada", check=False)
    assert r.returncode == 2
    j = (d / "registro.md").read_text()
    for t in ("[decisao] T1: preciso da tela", "sem marca", "[decisao] T1: rodada 2 congelada"):
        assert f"notify → arbiter: {t}" in j


def test_log_anexa_decisao_com_a_task(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "log", "--task", "3", "decidi X")
    assert "T3 decidi X" in (d / "registro.md").read_text()


@pytest.fixture
def jev_server():
    """Jev falso: devolve o `answers` que o teste pôs em `resp`, ou um status de erro."""
    ctl = {"status": 200, "resp": {}, "body": None}

    class H(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            ctl["body"] = json.loads(self.rfile.read(int(self.headers["content-length"])))
            ctl["auth"] = self.headers.get("authorization")
            out = json.dumps({"answers": ctl["resp"]}).encode()
            self.send_response(ctl["status"])
            self.send_header("content-type", "application/json")
            if ctl.get("cut"):
                # Promises more than it sends: the connection drops mid-answer.
                self.send_header("content-length", str(len(out) + 100))
            self.end_headers()
            self.wfile.write(out)

        def log_message(self, *a):
            pass

    srv = http.server.HTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    ctl["url"] = f"http://127.0.0.1:{srv.server_port}/v1/systemone"
    yield ctl
    srv.shutdown()


def _jev_env(e, ctl, mode):
    return {**e, "ORQ_JEV": mode, "ORQ_JEV_URL": ctl["url"], "TYPESAFE_API_KEY": "k"}


VETOS = ("context", "user", "problem", "deviation")


def _answers(choice="nothing", p=0.9, veto=0.1, **over):
    """Resposta do Jev: a escolha `kind` e os 4 vetos `noul`; `over` troca um veto."""
    return {"kind": {"choice": choice, "probabilities": {choice: p}},
            **{k: {"noul": veto} for k in VETOS}, **{k: {"noul": v} for k, v in over.items()}}


def test_sombra_consulta_registra_veto_e_acorda_igual(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers(p=0.97, user=0.2)
    run(_jev_env(e, jev_server, "shadow"), "notify", "tela fechada, 41 de 60 ações")
    assert sent(log) == ["arb tela fechada, 41 de 60 ações"]
    linha = json.loads((d / "jev-shadow.jsonl").read_text())
    assert linha["would_drop"] is True and linha["mode"] == "shadow"
    assert linha["choice"] == "nothing" and linha["p"] == 0.97
    assert linha["veto"] == {"context": 0.1, "user": 0.2, "problem": 0.1, "deviation": 0.1}
    assert jev_server["body"]["model"] == "jev-latest"
    assert jev_server["auth"] == "Bearer k"


def test_jev_usa_endpoint_e_modelo_do_ambiente(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers("act", 0.9)
    amb = {**e, "ORQ_JEV": "shadow", "TYPESAFE_API_KEY": "sk-or-x",
           "JEV_ENDPOINT": jev_server["url"], "JEV_MODEL": "typesafe/jev-1.13-20260917"}
    run(amb, "notify", "x")
    assert jev_server["body"]["model"] == "typesafe/jev-1.13-20260917"
    assert jev_server["auth"] == "Bearer sk-or-x"


def test_pedido_leva_as_5_perguntas_calibradas(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers()
    run(_jev_env(e, jev_server, "shadow"), "notify", "ok")
    q = jev_server["body"]["questions"]
    assert set(q) == {"kind", *VETOS}
    assert q["kind"]["type"] == "choice" and all(q[k]["type"] == "noul" for k in VETOS)
    assert q["kind"]["instructions"] == (
        "A session of a software team sent this message to the team's coordinator. "
        "Routine bookkeeping is automatic; the coordinator is needed only to decide "
        "or act. What does the coordinator have to do with this message?")
    assert jev_server["body"]["state"] == "ok"


def test_ligado_descarta_informe_certo_sem_veto(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers()
    run(_jev_env(e, jev_server, "on"), "notify", "ok, recebido")
    assert sent(log) == []
    assert "(jev: no action) ok, recebido" in (d / "registro.md").read_text()


def test_um_veto_acima_do_limite_acorda(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers(problem=0.5)
    run(_jev_env(e, jev_server, "on"), "notify", "deu 401 no hangar-send")
    assert sent(log) == ["arb deu 401 no hangar-send"]


def test_limites_de_descarte_e_de_veto(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers(p=0.84)
    run(_jev_env(e, jev_server, "on"), "notify", "quase")
    assert sent(log) == ["arb quase"]
    jev_server["resp"] = _answers(p=0.85, veto=0.40)
    run(_jev_env(e, jev_server, "on"), "notify", "no limite")
    assert sent(log) == ["arb quase"]
    assert "(jev: no action) no limite" in (d / "registro.md").read_text()


def test_veto_nan_fora_da_primeira_posicao_acorda(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers(deviation=float("nan"))
    run(_jev_env(e, jev_server, "on"), "notify", "veto nan")
    assert sent(log) == ["arb veto nan"]


# The calibrated question set and thresholds: a changed word or number means measuring again.
QUESTIONS_SHA256 = "ca05f591a55f640dd759536f4f33976a01a375f632c68ac345f39c62d1d4c22f"


def test_perguntas_e_limites_calibrados_nao_mudam():
    spec = importlib.util.spec_from_file_location("orq_pin", ORQ)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    h = hashlib.sha256(json.dumps(m.JEV_QUESTIONS, sort_keys=True).encode()).hexdigest()
    assert (h, m.DISCARD_P, m.VETO_P, m.JEV_VETOES) == (
        QUESTIONS_SHA256, 0.85, 0.40, ("context", "user", "problem", "deviation"))


def test_escolha_agir_acorda_mesmo_com_certeza(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers("act", 0.99, veto=0)
    run(_jev_env(e, jev_server, "on"), "notify", "posso usar a tela?")
    assert sent(log) == ["arb posso usar a tela?"]
    assert json.loads((d / "jev-shadow.jsonl").read_text())["p"] == 0.0


def test_resposta_sem_um_veto_acorda_o_arbitro(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    resp = _answers()
    del resp["deviation"]
    jev_server["resp"] = resp
    run(_jev_env(e, jev_server, "on"), "notify", "sem veto")
    assert sent(log) == ["arb sem veto"]
    assert json.loads((d / "jev-shadow.jsonl").read_text())["error"].startswith("KeyError")


def test_jev_com_erro_json_torto_ou_sem_chave_acorda_o_arbitro(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["status"] = 529
    run(_jev_env(e, jev_server, "on"), "notify", "um")
    jev_server["status"] = 200
    jev_server["resp"] = "torto"
    run(_jev_env(e, jev_server, "on"), "notify", "dois")
    run({**_jev_env(e, jev_server, "on"), "TYPESAFE_API_KEY": ""}, "notify", "três")
    assert sent(log) == ["arb um", "arb dois", "arb três"]
    erros = [json.loads(l).get("error") for l in (d / "jev-shadow.jsonl").read_text().splitlines()]
    assert erros[0].startswith("HTTPError") and erros[2] == "no key" and erros[1]


def test_jev_resposta_cortada_acorda_o_arbitro(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["cut"] = True
    run(_jev_env(e, jev_server, "on"), "notify", "quatro")
    assert sent(log) == ["arb quatro"]
    assert json.loads((d / "jev-shadow.jsonl").read_text())["error"].startswith("IncompleteRead")


def test_chave_do_settings_json_vale_sem_variavel(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    (tmp_path / ".claude").mkdir()
    (tmp_path / ".claude" / "settings.json").write_text(json.dumps({"env": {"TYPESAFE_API_KEY": "s"}}))
    jev_server["resp"] = _answers("act", 0.9)
    run({**_jev_env(e, jev_server, "shadow"), "TYPESAFE_API_KEY": ""}, "notify", "x")
    assert jev_server["auth"] == "Bearer s"


def test_settings_json_que_nao_e_objeto_acorda_o_arbitro(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    (tmp_path / ".claude").mkdir()
    (tmp_path / ".claude" / "settings.json").write_text("[]")
    run({**_jev_env(e, jev_server, "shadow"), "TYPESAFE_API_KEY": ""}, "notify", "y")
    assert sent(log) == ["arb y"]
    assert json.loads((d / "jev-shadow.jsonl").read_text())["error"] == "no key"


@pytest.fixture
def jev_cfg(tmp_path, monkeypatch):
    """jev_config() com o ambiente limpo e o CLAUDE_CONFIG_DIR num tmp; devolve (módulo, pasta)."""
    for k in ("TYPESAFE_API_KEY", "ORQ_JEV_URL", "JEV_ENDPOINT", "JEV_MODEL"):
        monkeypatch.delenv(k, raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))
    conta = tmp_path / "conta"
    conta.mkdir()
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(conta))
    spec = importlib.util.spec_from_file_location("orq_cfg", ORQ)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m, conta


def _runtime(conta, **campos):
    (conta / "runtime-config.json").write_text(json.dumps(campos))


def test_jev_config_sem_nada_e_a_typesafe_sem_chave(jev_cfg):
    m, _ = jev_cfg
    assert m.jev_config(auto=True) == {"key": "", "url": m.JEV_URL, "model": m.JEV_MODEL}


def test_execucao_comum_ignora_o_runtime_config(jev_cfg):
    m, conta = jev_cfg
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint="https://rc/v1", jev_model="rc-model")
    assert m.jev_config() == {"key": "", "url": m.JEV_URL, "model": m.JEV_MODEL}


def test_jev_config_le_o_runtime_config_da_conta(jev_cfg):
    m, conta = jev_cfg
    _runtime(conta, jev_api_key="tk", jev_endpoint="https://jev.local/v1", jev_model="m1")
    assert m.jev_config(auto=True) == {"key": "tk", "url": "https://jev.local/v1", "model": "m1"}


def test_jev_config_sem_config_dir_le_o_da_home(jev_cfg, tmp_path, monkeypatch):
    m, _ = jev_cfg
    monkeypatch.delenv("CLAUDE_CONFIG_DIR")
    (tmp_path / ".claude").mkdir()
    _runtime(tmp_path / ".claude", jev_api_key="da-home")
    assert m.jev_config(auto=True)["key"] == "da-home"


def test_chave_openrouter_sem_endereco_usa_o_da_openrouter(jev_cfg):
    m, conta = jev_cfg
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint="", jev_model="")
    assert m.jev_config(auto=True) == {"key": "sk-or-v1-x", "url": "https://openrouter.ai/api/alpha/decisions",
                              "model": "~typesafe/jev-latest"}


def test_endereco_do_openrouter_nunca_leva_o_nome_da_typesafe(jev_cfg):
    m, conta = jev_cfg
    url = "https://openrouter.ai/api/alpha/decisions"
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint=url, jev_model="")
    assert m.jev_config(auto=True)["model"] == "~typesafe/jev-latest"
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint=url, jev_model="typesafe/jev-latest")
    assert m.jev_config(auto=True)["model"] == "~typesafe/jev-latest", "sem o til o OpenRouter recusa"


def test_endereco_do_openrouter_nunca_leva_o_nome_da_typesafe(jev_cfg):
    m, conta = jev_cfg
    url = "https://openrouter.ai/api/alpha/decisions"
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint=url, jev_model="")
    assert m.jev_config(auto=True)["model"] == "~typesafe/jev-latest"
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint=url, jev_model="typesafe/jev-latest")
    assert m.jev_config(auto=True)["model"] == "~typesafe/jev-latest", "sem o til o OpenRouter recusa"


def test_ambiente_vence_o_runtime_config(jev_cfg, monkeypatch):
    m, conta = jev_cfg
    _runtime(conta, jev_api_key="sk-or-v1-x", jev_endpoint="https://rc/v1", jev_model="rc-model")
    monkeypatch.setenv("TYPESAFE_API_KEY", "env-key")
    monkeypatch.setenv("JEV_ENDPOINT", "https://env/v1")
    monkeypatch.setenv("JEV_MODEL", "env-model")
    assert m.jev_config(auto=True) == {"key": "env-key", "url": "https://env/v1", "model": "env-model"}
    monkeypatch.setenv("ORQ_JEV_URL", "http://127.0.0.1:1/t")
    assert m.jev_config(auto=True)["url"] == "http://127.0.0.1:1/t"


def test_runtime_config_torto_vale_como_ausente(jev_cfg):
    m, conta = jev_cfg
    (conta / "runtime-config.json").write_text("[]")
    assert m.jev_config(auto=True)["key"] == ""
    (conta / "runtime-config.json").write_text("{torto")
    assert m.jev_config(auto=True)["key"] == ""


def test_notify_comum_nao_le_a_chave_do_runtime_config(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    (tmp_path / ".claude").mkdir()
    _runtime(tmp_path / ".claude", jev_api_key="rc-key")
    jev_server["resp"] = _answers("act", 0.9)
    run({**_jev_env(e, jev_server, "shadow"), "TYPESAFE_API_KEY": ""}, "notify", "x")
    assert jev_server["body"] is None
    assert sent(log) == ["arb x"]
    assert json.loads((d / "jev-shadow.jsonl").read_text())["error"] == "no key"


def test_alarme_acorda_sem_consultar_o_jev(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers(p=0.99, veto=0)
    run(_jev_env(e, jev_server, "on"), "notify", "--alarm", "[vigia] rev parado")
    assert sent(log) == ["--tmux arb [vigia] rev parado"]
    assert jev_server["body"] is None
    assert not (d / "jev-shadow.jsonl").exists()


def _corpo_do_registro(d):
    """Cada linha do registro sem o `- <ts> · ` da frente."""
    return [l.split(" · ", 1)[1] for l in (d / "registro.md").read_text().splitlines()]


def test_registro_leva_prefixo_estavel_por_tipo(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    run(e, "notify", "[aviso] binário congelado")
    run(e, "notify", "--alarm", "[vigia] rev parado")
    run(e, "notify", "[decisao] preciso da tela")
    jev_server["resp"] = _answers()
    run(_jev_env(e, jev_server, "on"), "notify", "ok, recebido")
    corpo = _corpo_do_registro(d)
    assert "aviso: [aviso] binário congelado" in corpo
    assert "alarm: [vigia] rev parado" in corpo
    assert "notify → arbiter: [decisao] preciso da tela" in corpo
    assert "(jev: no action) ok, recebido" in corpo


def test_envio_falho_deixa_notify_failed_no_registro(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    falho = tmp_path / "send-falho"
    falho.write_text("#!/bin/sh\necho sem rota >&2\nexit 3\n")
    falho.chmod(0o755)
    r = run({**e, "ORQ_SEND": str(falho)}, "notify", "--alarm", "[vigia] x parado", check=False)
    assert r.returncode == 2
    corpo = _corpo_do_registro(d)
    assert corpo[-2] == "alarm: [vigia] x parado"
    assert corpo[-1] == "notify FAILED: hangar-send arb failed (rc=3): sem rota"


def test_envio_pendurado_vira_erro_no_prazo(tmp_path, monkeypatch):
    spec = importlib.util.spec_from_file_location("orq_send", ORQ)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    lento = tmp_path / "send-lento"
    lento.write_text("#!/bin/sh\nexec sleep 5\n")
    lento.chmod(0o755)
    monkeypatch.setenv("ORQ_SEND", str(lento))
    monkeypatch.setattr(m, "SEND_TIMEOUT_S", 0.2)
    t0 = time.monotonic()
    with pytest.raises(m.OrqError, match="did not answer"):
        m.send("arb", "x")
    assert time.monotonic() - t0 < 3


def test_sombra_que_nao_grava_ainda_acorda_o_arbitro(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    (d / "jev-shadow.jsonl").mkdir()          # open("a") num diretório: OSError
    jev_server["resp"] = _answers()
    r = run(_jev_env(e, jev_server, "shadow"), "notify", "ok")
    assert sent(log) == ["arb ok"]
    assert "jev-shadow.jsonl not written" in r.stderr


def test_evento_gravado_com_envio_falho_diz_como_reenviar(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    falho = tmp_path / "send-falho"
    falho.write_text("#!/bin/sh\nexit 3\n")
    falho.chmod(0o755)
    e2 = {**e, "ORQ_SEND": str(falho)}
    r = run(e2, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
            "--sessao", "rev", check=False)
    assert r.returncode == 2
    assert "The event IS recorded: do not run `orq event` again" in r.stderr
    assert "Resend with: hangar-send ex 'APROVA Task 1 round 1:" in r.stderr
    assert json.loads((d / "eventos.jsonl").read_text().splitlines()[-1])["resultado"] == "aprova"
    r = run(e2, "event", "veredito", "--task", "1", "--rodada", "2", "--resultado", "devolvido",
            "--sessao", "rev", "--motivo", "m", check=False)
    assert "Resend with: orq notify '[decisao] Task 1 round 2: devolvido. Report: m'" in r.stderr


def _fecha(d, task):
    """A linha que `orq commit` grava ao fechar a Task, sem passar pelo git."""
    ts = datetime.now().astimezone().isoformat(timespec="seconds")
    with (d / "closed.jsonl").open("a") as f:
        f.write(json.dumps({"ts": ts, "task": task, "hash": "abc"}) + "\n")


def _done(e):
    return [tuple(l.split(" ", 1)) for l in run(e, "done").stdout.splitlines()]


def test_done_lista_executor_de_task_fechada_e_quem_foi_trocado(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev")
    _fecha(d, 1)
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "u", "--executor", "ex2", "--par", "rev")
    run(e, "event", "sessao_trocada", "--de", "ex2", "--para", "ex3")
    assert _done(e) == [("ex1", "Task 1 closed"), ("ex2", "replaced by ex3")]


def test_done_nunca_lista_dono_de_task_aberta_nem_o_arbitro_atual(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev")
    _fecha(d, 1)
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "u", "--executor", "rev", "--par", "ex1")
    run(e, "event", "sessao_trocada", "--de", "arb", "--para", "arb2")
    run(e, "event", "sessao_trocada", "--de", "arb2", "--para", "arb")
    assert _done(e) == []


def test_done_troca_so_lista_quem_foi_executor_ou_revisor(env, tmp_path):
    # An arbiter swapped out may be the user's own coordinator session.
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "sessao_trocada", "--de", "coord", "--para", "arb9")
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev")
    run(e, "event", "sessao_trocada", "--de", "ex1", "--para", "ex2")
    run(e, "event", "sessao_trocada", "--de", "ex2", "--para", "ex3")
    assert _done(e) == [("ex1", "replaced by ex2"), ("ex2", "replaced by ex3")]


def test_done_nao_lista_arbitro_trocado_mesmo_sendo_revisor(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "arb")
    run(e, "event", "sessao_trocada", "--de", "arb", "--para", "arb2")
    assert _done(e) == []


def test_done_depois_do_fim_lista_o_time_e_trabalho_retomado_desfaz(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev")
    run(e, "event", "execucao_fim", "--resultado", "concluida")
    assert sorted(_done(e)) == [("ex1", "execution ended"), ("rev", "execution ended")]
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "u", "--executor", "ex2", "--par", "rev")
    assert _done(e) == []


def test_done_ignora_task_fechada_que_nao_esta_nos_eventos(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    _fecha(d, 9)
    assert _done(e) == []


def test_team_e_os_donos_de_task_aberta_e_o_arbitro_e_nao_cruza_com_done(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev1")
    _fecha(d, 1)
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "u", "--executor", "ex2", "--par", "rev2")
    run(e, "event", "entrega", "--task", "2", "--rodada", "1", "--commit", "abc")
    time_ = run(e, "team").stdout.split()
    assert time_ == ["ex2", "rev2", "arb"]
    assert _done(e) == [("ex1", "Task 1 closed"), ("rev1", "Task 1 closed")]


def _stash(g, r, content="2\n"):
    """a.txt com `content` no stage, congelado como stash guardado; devolve o hash."""
    (r / "a.txt").write_text(content)
    g("add", "a.txt")
    h = g("stash", "create")
    g("stash", "store", "-m", "round", h)
    return h


def _code_ok(e, h, rnd=1):
    run(e, "event", "entrega", "--task", "1", "--rodada", str(rnd), "--fase", "codigo", "--commit", h)
    run(e, "event", "veredito", "--task", "1", "--rodada", str(rnd), "--fase", "codigo",
        "--resultado", "aprova", "--sessao", "rev")


def _fases_init(e, r, tmp_path):
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"), "--plan", plano(tmp_path))
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")


def test_codigo_aprovado_manda_provar_e_nao_commitar(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    _fases_init(e, r, tmp_path)
    h = _stash(g, r)
    _code_ok(e, h)
    msg = sent(log)[-1]
    assert msg.startswith("ex CODE OK Task 1 round 1")
    assert f"--fase prova --commit {h}" in msg
    assert "Do not commit yet" in msg
    assert not any(m.startswith("arb ") for m in sent(log))
    assert run(e, "ball").stdout.split() == ["ex"]


def test_prova_so_entra_com_o_stash_do_codigo_aprovado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    _fases_init(e, r, tmp_path)
    h1 = _stash(g, r, "2\n")
    antes = (d / "eventos.jsonl").read_text()
    bad = run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--fase", "prova",
              "--commit", h1, check=False)
    assert bad.returncode != 0
    assert "the proof must run on the approved code" in bad.stderr
    assert (d / "eventos.jsonl").read_text() == antes
    _code_ok(e, h1)
    outro = _stash(g, r, "3\n")
    bad = run(e, "event", "entrega", "--task", "1", "--rodada", "2", "--fase", "prova",
              "--commit", outro, check=False)
    assert bad.returncode != 0
    assert "the proof must run on the approved code" in bad.stderr
    run(e, "event", "entrega", "--task", "1", "--rodada", "2", "--fase", "prova", "--commit", h1[:8])
    assert run(e, "ball").stdout.split() == ["rev"]


def test_prova_depois_de_correcao_usa_o_codigo_aprovado_mais_novo(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    _fases_init(e, r, tmp_path)
    h1 = _stash(g, r, "2\n")
    _code_ok(e, h1, 1)
    run(e, "event", "entrega", "--task", "1", "--rodada", "2", "--fase", "prova", "--commit", h1)
    n = len(sent(log))
    run(e, "event", "veredito", "--task", "1", "--rodada", "2", "--fase", "prova",
        "--resultado", "reprova", "--sessao", "rev")
    assert len(sent(log)) == n
    assert run(e, "ball").stdout.split() == ["ex"]
    h3 = _stash(g, r, "3\n")
    _code_ok(e, h3, 3)
    bad = run(e, "event", "entrega", "--task", "1", "--rodada", "4", "--fase", "prova",
              "--commit", h1, check=False)
    assert bad.returncode != 0
    run(e, "event", "entrega", "--task", "1", "--rodada", "4", "--fase", "prova", "--commit", h3)


def test_commit_so_depois_da_prova_aprovada(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    _fases_init(e, r, tmp_path)
    h = _stash(g, r)
    _code_ok(e, h)
    g("commit", "-qm", "t1")
    head = g("rev-parse", "HEAD")
    out = run(e, "commit", "--task", "1", "--hash", head, check=False)
    assert out.returncode == 1
    assert "no APROVA for Task 1" in out.stdout
    run(e, "event", "entrega", "--task", "1", "--rodada", "2", "--fase", "prova", "--commit", h)
    run(e, "event", "veredito", "--task", "1", "--rodada", "2", "--fase", "prova",
        "--resultado", "aprova", "--sessao", "rev")
    assert sent(log)[-1].startswith("ex APROVA Task 1 round 2")
    assert "ok" in run(e, "commit", "--task", "1", "--hash", head).stdout


def test_fase_fora_do_vocabulario_e_recusada(env, tmp_path):
    d, log, e = env
    init(e, tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev")
    bad = run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--fase", "tela",
              "--commit", "abc", check=False)
    assert bad.returncode != 0
    assert "fase" in bad.stdout + bad.stderr
    assert '"tela"' not in (d / "eventos.jsonl").read_text()


def test_veredito_sem_a_fase_da_entrega_e_recusado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    _fases_init(e, r, tmp_path)
    h = _stash(g, r)
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--fase", "codigo", "--commit", h)
    antes = (d / "eventos.jsonl").read_text()
    bad = run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
              "--sessao", "rev", check=False)
    assert bad.returncode != 0
    assert ("verdict phase must match the delivered round: round 1 was delivered with --fase codigo"
            in bad.stderr)
    assert (d / "eventos.jsonl").read_text() == antes
    assert sent(log) == []
    run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--fase", "codigo",
        "--resultado", "aprova", "--sessao", "rev")
    assert sent(log)[-1].startswith("ex CODE OK Task 1 round 1")


def test_prova_grava_o_stash_inteiro_mesmo_com_prefixo(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    _fases_init(e, r, tmp_path)
    h = _stash(g, r)
    _code_ok(e, h)
    run(e, "event", "entrega", "--task", "1", "--rodada", "2", "--fase", "prova", "--commit", h[:6])
    ev = json.loads((d / "eventos.jsonl").read_text().splitlines()[-1])
    assert ev["fase"] == "prova" and ev["commit"] == h


# ── orquestrar-auto: init --auto, linha do tempo e triagem ──────────────────

def _orq_mod():
    spec = importlib.util.spec_from_file_location("orq_auto", ORQ)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def linha_do_tempo(d):
    p = d / f"timeline-{d.name}.jsonl"
    return [json.loads(l) for l in p.read_text().splitlines()] if p.exists() else []


def test_init_auto_grava_a_marca_e_os_modos(env, tmp_path):
    d, _, e = env
    init(e, tmp_path, flags=("--auto",))
    cfg = json.loads((d / "orq.json").read_text())
    assert (cfg["auto"], cfg["jev"], cfg["regex"]) == (True, _orq_mod().AUTO_JEV_DEFAULT, "shadow")
    init(e, tmp_path, flags=("--auto", "--jev", "on", "--regex", "on"))
    cfg = json.loads((d / "orq.json").read_text())
    assert (cfg["jev"], cfg["regex"]) == ("on", "on")
    assert "auto jev=on regex=on" in (d / "registro.md").read_text()


def test_modo_sem_auto_e_recusado_e_init_comum_nao_ganha_marca(env, tmp_path):
    d, _, e = env
    init(e, tmp_path)
    c = tmp_path / "regras.md"
    r = run(e, "init", "--arbiter", "arb", "--repo", str(tmp_path), "--contract", str(c),
            "--plan", plano(tmp_path), "--regex", "on", check=False)
    assert r.returncode == 2 and "only apply with --auto" in r.stderr
    assert set(json.loads((d / "orq.json").read_text())) == {"arbiter", "repo", "contract", "untouchables", "plan"}


def test_execucao_comum_nao_usa_regex_nem_linha_do_tempo(env, tmp_path):
    d, log, e = env
    init(e, tmp_path)
    run(e, "notify", "janela de prova fechada")
    assert sent(log) == ["arb janela de prova fechada"]
    assert not list(d.glob("timeline-*"))


def test_auto_sem_chave_regex_so_anotando_acorda_e_marca_teria_descartado(env, tmp_path):
    d, log, e = env
    init(e, tmp_path, flags=("--auto",))
    run(e, "notify", "janela de prova fechada")
    assert sent(log) == ["arb janela de prova fechada"]
    [t] = linha_do_tempo(d)
    assert t["kind"] == "would_drop" and t["task"] is None
    assert t["text"] == "teria descartado (regex: janela); acordou o árbitro: janela de prova fechada"


def _quem(tmp_path, nome, aviso=False):
    """--whoami falso: imprime `nome`; com `aviso`, também o aviso do fallback do me()."""
    f = tmp_path / f"whoami-{nome}"
    extra = 'echo "aviso: sessão sem CP_SESSION_NAME válido" >&2\n' if aviso else ""
    f.write_text(f'#!/bin/sh\n[ "$1" = "--whoami" ] || exit 9\n{extra}echo "{nome}"\n')
    f.chmod(0o755)
    return str(f)


def test_auto_grava_o_remetente_do_recado(env, tmp_path):
    d, log, e = env
    init(e, tmp_path, flags=("--auto",))
    run({**e, "ORQ_WHOAMI": _quem(tmp_path, "w-t4")}, "notify", "[decisao] T4: pode?")
    run({**e, "ORQ_WHOAMI": _quem(tmp_path, "w-t4")}, "notify", "--alarm", "[vigia] x parado")
    run({**e, "ORQ_WHOAMI": _quem(tmp_path, "cli")}, "notify", "[decisao] T4: e agora?")
    run({**e, "ORQ_WHOAMI": _quem(tmp_path, "outra", aviso=True)}, "notify", "[decisao] T4: e isto?")
    a, b, c, x = linha_do_tempo(d)
    assert (a["from"], b["from"], c["from"], x["from"]) == ("w-t4", "vigia", None, None)
    assert sent(log)[0] == "arb [decisao] T4: pode?"


def test_linha_do_orquestrador_nao_leva_from(env, tmp_path):
    d, _, e = env
    init(e, tmp_path, flags=("--auto",))
    _orq_mod().timeline(d, "advance", "T1 fechada → integração verde", 1)
    assert "from" not in linha_do_tempo(d)[0]


def test_entrega_em_execucao_auto_vira_linha_do_tempo(env, tmp_path):
    d, _, e = env
    init(e, tmp_path, flags=("--auto",))
    (d / "advance.log").mkdir()   # sem passada do advance em segundo plano (molde de test_orq_advance.py)
    run(e, "event", "task_inicio", "--task", "4", "--titulo", "t", "--executor", "ex", "--par", "rev")
    run(e, "event", "entrega", "--task", "4", "--rodada", "2", "--commit", "3b799e6665ba59f7801a11e5836d9f5f62772c46")
    # O spawn_advance que falha grava outra linha depois: procurar pela frase, nunca por [-1].
    assert {"kind": "advance", "task": 4, "text": "T4 entregou a rodada 2 · 3b799e6"}.items() <= next(
        l for l in linha_do_tempo(d) if l["text"].startswith("T4 entregou")).items()


def test_jev_grava_as_probabilidades_da_escolha(env, tmp_path, jev_server):
    d, _, e = env
    init(e, tmp_path)
    jev_server["resp"] = {**_answers("act", 0.94), "kind": {"choice": "act",
                          "probabilities": {"act": 0.94, "nothing": 0.06}}}
    run(_jev_env(e, jev_server, "shadow"), "notify", "preciso da tela, posso usar?")
    r = json.loads((d / "jev-shadow.jsonl").read_text())
    assert r["probs"] == {"act": 0.94, "nothing": 0.06} and r["p"] == 0.0


def test_ids_da_sessao_pelo_sidecar(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    fake = tmp_path / "bin"   # o tmux do fallback nunca enxerga as sessões reais da máquina
    fake.mkdir()
    (fake / "tmux").write_text("#!/bin/sh\nexit 1\n")
    (fake / "tmux").chmod(0o755)
    monkeypatch.setenv("PATH", f"{fake}{os.pathsep}{os.environ['PATH']}")
    side = tmp_path / ".hangar" / "claude-headless"
    side.mkdir(parents=True)
    (side / "w-t4.json").write_text(json.dumps({"name": "w-t4", "provider": "claude",
        "session_id": "de3d45d5-a9c1-436b-b324-607f23b74058", "config_dir": "/home/x/.claude-200-01"}))
    cx = tmp_path / ".hangar" / "codex-sessions"
    cx.mkdir(parents=True)
    (cx / "w-arbiter.json").write_text(json.dumps({"name": "w-arbiter", "provider": "codex",
        "thread_id": "01a0efc7-25ef-7ee0-b078-1d31dc2c7c7d", "codex_home": "/home/x/.codex-j", "rollout_path": ""}))
    d = tmp_path / "run"
    d.mkdir()
    m = _orq_mod()
    m._record_session(d, "w-t4", "executor", 4)
    m._record_session(d, "w-arbiter", "arbitro", None)
    m._record_session(d, "sumiu", "revisor", 4)   # sem sidecar nem pane: grava o nome, ids nulos
    a, b, c = [json.loads(l) for l in (d / "sessions.jsonl").read_text().splitlines()]
    assert (a["provider"], a["session_id"], a["config_dir"], a["role"], a["task"]) == (
        "claude", "de3d45d5-a9c1-436b-b324-607f23b74058", "/home/x/.claude-200-01", "executor", 4)
    assert (b["provider"], b["thread_id"], b["codex_home"]) == ("codex", "01a0efc7-25ef-7ee0-b078-1d31dc2c7c7d", "/home/x/.codex-j")
    assert c["name"] == "sumiu" and c["session_id"] is None


def test_auto_regex_ligada_descarta_com_texto_inteiro(env, tmp_path):
    d, log, e = env
    init(e, tmp_path, flags=("--auto", "--regex", "on"))
    run(e, "notify", "T4 rodada 2 entregue ao revisor")
    run(e, "notify", "Peço a tela")
    assert sent(log) == ["arb Peço a tela"]
    assert "(regex: no action) T4 rodada 2 entregue ao revisor" in _corpo_do_registro(d)
    assert [(t["kind"], t["text"]) for t in linha_do_tempo(d)] == [
        ("dropped", "recado registrado sem acordar o árbitro (regex: entrega): T4 rodada 2 entregue ao revisor"),
        ("woke", "acordou o árbitro: Peço a tela"),
    ]


def test_auto_marcado_e_alarme_acordam_sem_triagem(env, tmp_path):
    d, log, e = env
    init(e, tmp_path, flags=("--auto", "--regex", "on"))
    run(e, "notify", "[decisao] janela de prova fechada")
    run(e, "notify", "--alarm", "[vigia] janela de prova fechada")
    assert sent(log) == ["arb [decisao] janela de prova fechada", "--tmux arb [vigia] janela de prova fechada"]
    assert [t["kind"] for t in linha_do_tempo(d)] == ["woke", "woke"]


def test_auto_com_chave_o_jev_decide_no_modo_do_orq_json(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path, flags=("--auto", "--jev", "on"))
    jev_server["resp"] = _answers()
    # ORQ_JEV do ambiente não manda no modo da execução auto: só "off" desliga o Jev.
    run(_jev_env(e, jev_server, "shadow"), "notify", "Peço a tela")
    assert sent(log) == []
    assert "(jev: no action) Peço a tela" in _corpo_do_registro(d)
    assert linha_do_tempo(d)[-1]["text"] == "recado registrado sem acordar o árbitro (jev): Peço a tela"
    init(e, tmp_path, flags=("--auto", "--jev", "shadow"))
    run(_jev_env(e, jev_server, "on"), "notify", "ok")
    assert sent(log) == ["arb ok"]
    assert linha_do_tempo(d)[-1]["text"] == "teria descartado (jev); acordou o árbitro: ok"


def test_auto_usa_a_chave_do_runtime_config(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path, flags=("--auto", "--jev", "shadow"))
    (tmp_path / ".claude").mkdir()
    _runtime(tmp_path / ".claude", jev_api_key="rc-key")
    jev_server["resp"] = _answers("act", 0.9)
    run({**_jev_env(e, jev_server, "shadow"), "TYPESAFE_API_KEY": ""}, "notify", "x")
    assert jev_server["auth"] == "Bearer rc-key"
    assert linha_do_tempo(d)[-1]["kind"] == "woke"


def test_auto_jev_com_erro_cai_na_regex(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path, flags=("--auto", "--jev", "on", "--regex", "on"))
    jev_server["status"] = 529
    run(_jev_env(e, jev_server, "on"), "notify", "janela de prova fechada")
    assert sent(log) == []
    assert linha_do_tempo(d)[-1]["text"].startswith("recado registrado sem acordar o árbitro (regex: janela)")
    assert json.loads((d / "jev-shadow.jsonl").read_text())["error"].startswith("HTTPError")


def test_auto_envio_falho_vira_falha_na_linha_do_tempo(env, tmp_path):
    d, _, e = env
    init(e, tmp_path, flags=("--auto",))
    falho = tmp_path / "send-falho"
    falho.write_text("#!/bin/sh\nexit 3\n")
    falho.chmod(0o755)
    r = run({**e, "ORQ_SEND": str(falho)}, "notify", "Peço a tela", check=False)
    assert r.returncode == 2
    [t] = linha_do_tempo(d)
    assert t["kind"] == "failed" and t["text"].endswith(": Peço a tela")


def test_timeline_recusa_tipo_desconhecido_e_so_escreve_em_auto(tmp_path):
    m = _orq_mod()
    with pytest.raises(ValueError):
        m.timeline(tmp_path, "sei-la", "x")
    m.timeline(tmp_path, "notice", "sem orq.json")
    (tmp_path / "orq.json").write_text(json.dumps({"auto": True}))
    m.timeline(tmp_path, "advance", "T1 integrada", task=1)
    [t] = [json.loads(l) for l in (tmp_path / f"timeline-{tmp_path.name}.jsonl").read_text().splitlines()]
    assert (t["kind"], t["text"], t["task"]) == ("advance", "T1 integrada", 1) and "ts" in t
