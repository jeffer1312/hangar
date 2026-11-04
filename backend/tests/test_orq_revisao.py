"""orq: revisão como subagente — `Revisão:` no plano, review-package e a trava do veredito."""
import json
import subprocess

import pytest

from test_orq_plano import PROJETO, congelar, escrever, iniciar, repo, run  # noqa: F401

pytestmark = pytest.mark.usefixtures("hangar_server_bin")


def test_plan_check_exige_revisao_valida(tmp_path):
    p = escrever(tmp_path, projeto=PROJETO.replace("Revisão: subagente\n", ""))
    r = run("plan-check", str(p), "--repo", str(tmp_path), check=False)
    assert "## Projeto: missing line 'Revisão:'" in r.stdout
    p = escrever(tmp_path, projeto=PROJETO.replace("Revisão: subagente", "Revisão: talvez"))
    r = run("plan-check", str(p), "--repo", str(tmp_path), check=False)
    assert "## Projeto: Revisão must be subagente | sessão" in r.stdout


def test_subagente_nao_e_sessao_para_a_vez_nem_para_o_vigia(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    assert run("ball", env=e).stdout.split() == ["ex"]
    assert "subagente" not in run("team", env=e).stdout.split()


def test_review_package_junta_tudo_e_diz_como_responder(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    c = json.loads((d / "orq.json").read_text())["contract"]
    open(c, "w").write("comum\n## Task 1\nsó da T1\n")
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    rel = tmp_path / "rel.md"; rel.write_text("fiz X\n")
    path = run("review-package", "--task", "1", "--rodada", "1", "--report", str(rel), env=e).stdout.strip()
    txt = open(path).read()
    for trecho in ("comum", "só da T1", "fiz X", f"Object: {h}", "+2", "event veredito --task 1 --rodada 1",
                   "checks/task1-"):
        assert trecho in txt, trecho


def test_veredito_de_subagente_recusado_se_a_arvore_mudou(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    (r / "a.txt").write_text("mexido durante a revisão\n")
    res = run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
              "--sessao", "revisor-orq", env=e, check=False)
    assert res.returncode == 2 and "worktree changed during the review" in res.stderr
    (r / "a.txt").write_text("2\n")
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
        "--sessao", "revisor-orq", env=e)


def _pacote(e, task, rodada):
    return open(run("review-package", "--task", str(task), "--rodada", str(rodada), env=e).stdout.strip()).read()


def test_review_package_leva_a_fase_da_rodada_ao_comando_do_veredito(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, "--fase", "codigo", env=e)
    txt = _pacote(e, 1, 1)
    assert "Phase: codigo" in txt
    cmd = txt.rstrip().splitlines()[-1]
    assert "event veredito --task 1 --rodada 1" in cmd and cmd.endswith("--fase codigo")


def test_review_package_lista_os_vereditos_anteriores(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    assert "## Earlier verdicts\nnone" in _pacote(e, 1, 1)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "reprova",
        "--sessao", "revisor-orq", "--motivo", "/x/parecer-r1.md", env=e)
    h2 = congelar(r, g, "3\n")
    run("check", "--task", "1", "--commit", h2, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "2", "--commit", h2, env=e)
    txt = _pacote(e, 1, 2)
    assert "Phase: none" in txt
    assert "- round 1: reprova, report /x/parecer-r1.md" in txt


def test_review_package_da_o_roteiro_como_caminho_absoluto(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "2", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "2", "--commit", h, env=e)
    run("event", "entrega", "--task", "2", "--rodada", "1", "--commit", h, env=e)
    assert f"## Roteiro\n{(tmp_path / 'roteiro-2.md').resolve()}\n" in _pacote(e, 2, 1)


def test_review_package_leva_a_linha_crua_do_plano_e_o_plano_do_usuario(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    txt = _pacote(e, 1, 1)
    # "What it is" (a) e "Where in their plan" (§1) chegam ao revisor, não só os campos do orq.
    assert ("## Plan row\nUser's plan: /x/plano.md\n| # | What it is | Where in their plan |" in txt)
    assert "| 1 | a | §1 | `a.txt` | `true` | 1 | — |" in txt
