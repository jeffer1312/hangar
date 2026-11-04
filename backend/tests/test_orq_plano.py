"""orq: o plano de orquestração (`## Projeto` + `## Tasks`), plan-check, init --plan, check e lote."""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.usefixtures("hangar_server_bin")

ORQ = Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq.py"

PROJETO = """## Projeto
Checagens: `true`
Integração: —
Prova: lote(2)
Paralelo: até 2
Revisão: subagente
Correção pelo revisor: até 20 linhas
"""

TASKS = """## Tasks
| # | What it is | Where in their plan | Files | Verification | Wave | Roteiro |
|---|---|---|---|---|---|---|
| 1 | a | §1 | `a.txt` | `true` | 1 | — |
| 2 | b | §2 | `b.txt` | `true` | 1 | `roteiro-2.md` |
"""


def run(*args, env=None, check=True):
    r = subprocess.run([sys.executable, str(ORQ), *args], capture_output=True, text=True,
                       env=env or {**os.environ, "ORQ_JEV": "off"})
    if check:
        assert r.returncode == 0, r.stdout + r.stderr
    return r


def escrever(tmp_path, projeto=PROJETO, tasks=TASKS):
    (tmp_path / "roteiro-2.md").write_text("abrir e conferir\n")
    p = tmp_path / "orq-plano.md"
    p.write_text(f"# Orchestration plan — x\nUser's plan: /x\n\n{projeto}\n{tasks}")
    return p


def test_plan_check_limpo_carimba_e_o_sha_bate(tmp_path):
    p = escrever(tmp_path)
    out = run("plan-check", str(p), "--repo", str(tmp_path), "--stamp").stdout
    assert "plan-check ok" in out
    txt = p.read_text()
    linha = next(l for l in txt.splitlines() if l.startswith("Preparado: "))
    assert "paralelo 2" in linha and "prova lote(2)" in linha and "plan-check limpo" in linha
    # Segundo carimbo troca a linha, nunca duplica.
    run("plan-check", str(p), "--repo", str(tmp_path), "--stamp")
    assert sum(1 for l in p.read_text().splitlines() if l.startswith("Preparado: ")) == 1


def test_plan_check_lista_cada_falta(tmp_path):
    tasks = TASKS.replace("| 2 | b | §2 | `b.txt` | `true` | 1 | `roteiro-2.md` |",
                          "| 2 | b | §2 | `sem/pasta/b.txt` |  | x | `nao-existe.md` |")
    p = escrever(tmp_path, projeto=PROJETO.replace("Paralelo: até 2\n", ""), tasks=tasks)
    r = run("plan-check", str(p), "--repo", str(tmp_path), check=False)
    assert r.returncode == 1
    out = r.stdout
    assert "## Projeto: missing line 'Paralelo:'" in out
    assert "T2: neither the file nor its directory exists: sem/pasta/b.txt" in out
    assert "T2: no Verification" in out
    assert "T2: Wave is not a number: x" in out
    assert "T2: roteiro not found: nao-existe.md" in out
    assert "Preparado:" not in p.read_text()


def test_plan_check_roda_as_checagens_e_conta_a_onda(tmp_path):
    p = escrever(tmp_path, projeto=PROJETO.replace("`true`", "`false`").replace("até 2", "sequencial"))
    r = run("plan-check", str(p), "--repo", str(tmp_path), check=False)
    assert "check failed: `false` (rc=1)" in r.stdout
    assert "wave 1 has 2 Tasks, Paralelo allows 1" in r.stdout


def test_plan_check_recusa_comando_sem_crase(tmp_path):
    p = escrever(tmp_path, projeto=PROJETO.replace("`true`", "npm test")
                 .replace("Integração: —", "Integração: make e2e"))
    r = run("plan-check", str(p), "--repo", str(tmp_path), check=False)
    assert r.returncode == 1
    assert "## Projeto: Checagens has no `command`" in r.stdout
    assert "## Projeto: Integração has no `command`" in r.stdout


def test_plan_check_um_log_por_checagem_e_o_caminho_na_falta(tmp_path):
    p = escrever(tmp_path, projeto=PROJETO.replace(
        "`true`", "`echo primeira; exit 3` `echo segunda; exit 4`"))
    r = run("plan-check", str(p), "--repo", str(tmp_path), check=False)
    log1, log2 = tmp_path / "plan-check-1.log", tmp_path / "plan-check-2.log"
    assert f"(rc=3), log {log1}" in r.stdout and f"(rc=4), log {log2}" in r.stdout
    assert "primeira" in log1.read_text() and "segunda" in log2.read_text()


def test_init_exige_plano_carimbado_e_sem_mudanca(tmp_path):
    d = tmp_path / "orq"; d.mkdir()
    p = escrever(tmp_path)
    c = tmp_path / "regras.md"; c.write_text("")
    base = ["--dir", str(d), "init", "--arbiter", "arb", "--repo", str(tmp_path),
            "--contract", str(c), "--plan", str(p)]
    r = run(*base, check=False)
    assert r.returncode == 2 and "plan not prepared" in r.stderr
    run("plan-check", str(p), "--repo", str(tmp_path), "--stamp")
    p.write_text(p.read_text() + "\nmudou depois\n")
    r = run(*base, check=False)
    assert r.returncode == 2 and "plan changed after preparation" in r.stderr
    run("plan-check", str(p), "--repo", str(tmp_path), "--stamp")
    run(*base)
    assert json.loads((d / "orq.json").read_text())["plan"] == str(p.resolve())


def _init_sem_plano(tmp_path, d):
    c = tmp_path / "regras.md"; c.write_text("")
    return run("--dir", str(d), "init", "--arbiter", "arb", "--repo", str(tmp_path),
               "--contract", str(c), "--untouchable", "x/*", check=False)


def test_init_sem_plano_so_reinicia_execucao_que_nunca_teve_plano(tmp_path):
    d = tmp_path / "orq"; d.mkdir()
    r = _init_sem_plano(tmp_path, d)
    assert r.returncode == 2 and "plan required" in r.stderr
    # Execução antiga (orq.json sem `plan`): a exceção de intocável reinicia e segue sem plano.
    (d / "orq.json").write_text(json.dumps({"arbiter": "arb", "repo": str(tmp_path), "contract": "c",
                                            "untouchables": []}))
    assert _init_sem_plano(tmp_path, d).returncode == 0
    cfg = json.loads((d / "orq.json").read_text())
    assert "plan" not in cfg and cfg["untouchables"] == ["x/*"]
    cfg["plan"] = "/p.md"; (d / "orq.json").write_text(json.dumps(cfg))
    r = _init_sem_plano(tmp_path, d)
    assert r.returncode == 2 and "plan required" in r.stderr


@pytest.fixture
def repo(tmp_path):
    r = tmp_path / "repo"; r.mkdir()
    g = lambda *a: subprocess.run(["git", "-C", str(r), *a], check=True, capture_output=True,
                                  text=True).stdout.strip()
    g("init", "-q"); g("config", "user.email", "t@t"); g("config", "user.name", "t")
    (r / "a.txt").write_text("1\n"); (r / "b.txt").write_text("1\n")
    g("add", "a.txt", "b.txt"); g("commit", "-qm", "base")
    return r, g


def iniciar(tmp_path, r, projeto=PROJETO, tasks=TASKS):
    d = tmp_path / "orq"; d.mkdir()
    (tmp_path / "roteiro-1.md").write_text("x\n")
    (tmp_path / "roteiro-2.md").write_text("x\n")
    p = tmp_path / "orq-plano.md"
    p.write_text(f"# Orchestration plan — x\nUser's plan: /x/plano.md\n\n{projeto}\n{tasks}")
    run("plan-check", str(p), "--repo", str(r), "--stamp")
    c = tmp_path / "regras.md"; c.write_text("")
    log = tmp_path / "sent.log"
    fake = tmp_path / "fake-send"
    fake.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{log}"\n'); fake.chmod(0o755)
    e = {**os.environ, "ORQ_DIR": str(d), "ORQ_SEND": str(fake), "ORQ_JEV": "off"}
    run("init", "--arbiter", "arb", "--repo", str(r), "--contract", str(c), "--plan", str(p), env=e)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex", "--par", "rev", env=e)
    return d, e, log


def congelar(r, g, content="2\n"):
    (r / "a.txt").write_text(content); g("add", "a.txt")
    h = g("stash", "create"); g("stash", "store", "-m", "round", h)
    return h


def test_entrega_sem_check_e_recusada_e_com_check_passa(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    h = congelar(r, g)
    res = run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e, check=False)
    assert res.returncode == 2 and f"run `orq check --task 1 --commit {h}` first" in res.stderr
    out = run("check", "--task", "1", "--commit", h, env=e).stdout
    assert out.startswith("check T1 ok 1/1")
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)


def test_entrega_sem_commit_nao_herda_check_de_outra_rodada(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    res = run("event", "entrega", "--task", "1", "--rodada", "2", env=e, check=False)
    assert res.returncode == 2 and "--commit <stash>" in res.stderr


def test_check_recusa_arvore_diferente_do_objeto(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    h = congelar(r, g)
    (r / "a.txt").write_text("mexido depois\n")
    res = run("check", "--task", "1", "--commit", h, env=e, check=False)
    assert res.returncode == 2 and "worktree differs from" in res.stderr


def test_checagem_que_falha_nao_libera_entrega(tmp_path, repo):
    r, g = repo
    # A checagem passa no carimbo (a marca existe) e falha depois (a marca some).
    (r / "marca").write_text("x")
    d, e, _ = iniciar(tmp_path, r, projeto=PROJETO.replace("Checagens: `true`", "Checagens: `test -f marca`"))
    (r / "marca").unlink()
    h = congelar(r, g)
    res = run("check", "--task", "1", "--commit", h, env=e, check=False)
    assert res.returncode == 1 and "check failed" in res.stdout
    res = run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e, check=False)
    assert res.returncode == 2 and "none passed on this object" in res.stderr


def test_execucao_sem_plano_entrega_sem_check(tmp_path, repo):
    """orq.json de antes dos planos carimbados: nenhuma trava nova."""
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    cfg = json.loads((d / "orq.json").read_text()); cfg.pop("plan")
    (d / "orq.json").write_text(json.dumps(cfg))
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", congelar(r, g), env=e)


def fechar(tmp_path, r, g, e, task, arquivo):
    run("event", "task_inicio", "--task", str(task), "--titulo", "t", "--executor", f"ex{task}",
        "--par", f"rev{task}", env=e)
    (r / arquivo).write_text(f"{task}0\n"); g("add", arquivo)
    h = g("stash", "create"); g("stash", "store", "-m", "round", h)
    run("check", "--task", str(task), "--commit", h, env=e)
    run("event", "entrega", "--task", str(task), "--rodada", "1", "--commit", h, env=e)
    run("event", "veredito", "--task", str(task), "--rodada", "1", "--resultado", "aprova",
        "--sessao", f"rev{task}", env=e)
    g("commit", "-qm", f"t{task}", "--", arquivo)
    run("commit", "--task", str(task), "--hash", g("rev-parse", "HEAD"), env=e)


def _tasks(onda1, onda2, rot1="`roteiro-1.md`"):
    return TASKS.replace("| `true` | 1 | — |", f"| `true` | {onda1} | {rot1} |").replace(
        "| `true` | 1 | `roteiro-2.md` |", f"| `true` | {onda2} | `roteiro-2.md` |")


def test_lote_espera_a_onda_mesmo_sem_task_aberta(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r, tasks=_tasks(1, 1))   # lote(2), T1 e T2 com roteiro, onda 1
    fechar(tmp_path, r, g, e, 1, "a.txt")
    # Nada aberto, mas T2 da mesma onda ainda não fechou: espera.
    assert log.read_text().splitlines()[-1].endswith("Proof queued (1/2).")
    fechar(tmp_path, r, g, e, 2, "b.txt")
    assert "Proof batch ready: T1, T2." in log.read_text().splitlines()[-1]


def test_lote_sai_quando_a_onda_da_task_acaba(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r, tasks=_tasks(1, 2))   # lote(2), T1 onda 1, T2 onda 2
    # T2 aberta: só o fim da onda 1 explica o aviso de T1.
    run("event", "task_inicio", "--task", "2", "--titulo", "t", "--executor", "ex2", "--par", "rev2", env=e)
    fechar(tmp_path, r, g, e, 1, "a.txt")
    assert "Proof batch ready: T1." in log.read_text().splitlines()[-1]
    out = run("batch", "take", env=e).stdout
    # Absolute: the proof session runs outside the plan's directory.
    assert out.startswith(f"lote 1: T1 {(tmp_path / 'roteiro-1.md').resolve()} ")
    assert run("batch", "take", env=e).stdout.strip() == "no pending proof"
    # Commit repetido depois do take: fila vazia, nenhum lote vazio anunciado.
    run("commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), env=e)
    assert log.read_text().splitlines()[-1].endswith("Release the next ready Task(s).")
    fechar(tmp_path, r, g, e, 2, "b.txt")
    assert "Proof batch ready: T2." in log.read_text().splitlines()[-1]
    assert run("batch", "take", env=e).stdout.startswith("lote 2: T2 ")


def test_task_sem_roteiro_que_fecha_a_onda_anuncia_o_lote(tmp_path, repo):
    r, g = repo
    tasks = _tasks(1, 2) + "| 3 | c | §3 | `c.txt` | `true` | 1 | — |\n"
    d, e, log = iniciar(tmp_path, r, projeto=PROJETO.replace("lote(2)", "lote(8)"), tasks=tasks)
    fechar(tmp_path, r, g, e, 1, "a.txt")
    assert log.read_text().splitlines()[-1].endswith("Proof queued (1/8).")
    fechar(tmp_path, r, g, e, 3, "c.txt")
    assert "Proof batch ready: T1." in log.read_text().splitlines()[-1]
    fila = [json.loads(l) for l in (d / "prova-fila.jsonl").read_text().splitlines()]
    assert [f["task"] for f in fila] == [1]


def test_lote_so_enfileira_com_roteiro_e_commit_repetido_nao_duplica(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r)          # TASKS: T1 sem roteiro, T2 com roteiro; lote(2)
    fechar(tmp_path, r, g, e, 1, "a.txt")
    assert not (d / "prova-fila.jsonl").exists()
    fechar(tmp_path, r, g, e, 2, "b.txt")
    run("commit", "--task", "2", "--hash", g("rev-parse", "HEAD"), env=e)
    fila = [json.loads(l) for l in (d / "prova-fila.jsonl").read_text().splitlines()]
    assert [f["task"] for f in fila] == [2]
    assert "Proof batch ready: T2." in log.read_text().splitlines()[-1]


def test_por_task_nao_enfileira(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r, projeto=PROJETO.replace("lote(2)", "por-task"))
    fechar(tmp_path, r, g, e, 2, "b.txt")
    assert not (d / "prova-fila.jsonl").exists()


def test_prova_manual_guarda_roteiro_sem_anunciar_e_entrega_no_fim(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r, projeto=PROJETO.replace("lote(2)", "manual"))
    fechar(tmp_path, r, g, e, 2, "b.txt")
    fila = [json.loads(l) for l in (d / "prova-fila.jsonl").read_text().splitlines()]
    assert [f["task"] for f in fila] == [2]
    assert "Proof" not in log.read_text().splitlines()[-1]
    out = run("batch", "take", env=e).stdout
    assert out.startswith("lote 1: T2 ")


def test_plan_check_aceita_manual_com_roteiro(tmp_path):
    p = escrever(tmp_path, projeto=PROJETO.replace("lote(2)", "manual"))
    assert "plan-check ok" in run("plan-check", str(p), "--repo", str(tmp_path)).stdout


def test_commit_avisa_o_fechamento_mesmo_com_o_plano_ilegivel(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
        "--sessao", "rev", env=e)
    g("commit", "-qm", "t1", "--", "a.txt")
    (tmp_path / "orq-plano.md").unlink()
    run("commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), env=e)
    last = log.read_text().splitlines()[-1]
    assert "Task 1 closed and checked" in last and "(proof queue skipped: plan not found:" in last
