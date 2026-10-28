"""orq: correção pequena pelo revisor — veredito `corrige`, `apply-patch` e a rodada do patch."""
import json
import subprocess

import pytest

from test_orq_plano import PROJETO, congelar, iniciar, repo, run  # noqa: F401  (fixture repo)

pytestmark = pytest.mark.usefixtures("hangar_server_bin")


def patch_de(r, g, tmp_path, novo="3\n", arquivo="a.txt"):
    """Diff de `arquivo` para `novo`, sem mexer na árvore."""
    atual = (r / arquivo).read_text()
    (r / arquivo).write_text(novo)
    diff = subprocess.run(["git", "-C", str(r), "diff", "--", arquivo], capture_output=True, text=True).stdout
    (r / arquivo).write_text(atual)
    p = tmp_path / "t1-r1.patch"; p.write_text(diff)
    return str(p)


def entregar_r1(tmp_path, r, g, e):
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    return h


def test_corrige_manda_o_executor_aplicar_e_a_rodada_volta_ao_revisor(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r)
    entregar_r1(tmp_path, r, g, e)
    p = patch_de(r, g, tmp_path)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
        "--sessao", "rev", "--patch", p, env=e)
    assert log.read_text().splitlines()[-1].startswith("ex CORRIGE Task 1 round 1")
    run("apply-patch", "--task", "1", "--repo", str(r), env=e)
    assert (r / "a.txt").read_text() == "3\n"
    ent = [json.loads(l) for l in (d / "eventos.jsonl").read_text().splitlines()][-1]
    assert ent["tipo"] == "entrega" and ent["rodada"] == 2
    assert log.read_text().splitlines()[-1].startswith("rev Task 1 round 2 = round 1 + your patch")
    # Segundo corrige na mesma Task: recusado.
    res = run("event", "veredito", "--task", "1", "--rodada", "2", "--resultado", "corrige",
              "--sessao", "rev", "--patch", p, env=e, check=False)
    assert res.returncode == 2 and "one corrige per Task" in res.stderr
    # APROVA da rodada do patch libera o commit.
    run("event", "veredito", "--task", "1", "--rodada", "2", "--resultado", "aprova", "--sessao", "rev", env=e)
    g("commit", "-qm", "t1", "--", "a.txt")
    run("commit", "--task", "1", "--hash", g("rev-parse", "HEAD"), "--repo", str(r), env=e)


def test_corrige_recusa_patch_grande_ou_fora_da_rodada(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r, projeto=PROJETO.replace("até 20 linhas", "até 1 linhas"))
    entregar_r1(tmp_path, r, g, e)
    grande = patch_de(r, g, tmp_path, novo="3\n4\n5\n")
    res = run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
              "--sessao", "rev", "--patch", grande, env=e, check=False)
    assert res.returncode == 2 and "limit is 1" in res.stderr
    fora = patch_de(r, g, tmp_path, novo="9\n", arquivo="b.txt")
    res = run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
              "--sessao", "rev", "--patch", fora, env=e, check=False)
    assert res.returncode == 2 and "outside the round: ['b.txt']" in res.stderr


def test_corrige_desligado_quando_o_plano_diz_zero_ou_nao_tem_plano(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r, projeto=PROJETO.replace("até 20 linhas", "até 0 linhas"))
    entregar_r1(tmp_path, r, g, e)
    res = run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
              "--sessao", "rev", "--patch", patch_de(r, g, tmp_path), env=e, check=False)
    assert res.returncode == 2 and "off in the plan" in res.stderr


def test_patch_que_nao_aplica_ou_quebra_a_checagem_devolve_a_arvore_intacta(tmp_path, repo):
    r, g = repo
    # Passa com a.txt em 1 (carimbo) e em 2 (rodada); o patch põe 3 e ela falha.
    d, e, log = iniciar(tmp_path, r, projeto=PROJETO.replace("Checagens: `true`",
                                                             "Checagens: `! grep -q 3 a.txt`"))
    entregar_r1(tmp_path, r, g, e)
    p = patch_de(r, g, tmp_path)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
        "--sessao", "rev", "--patch", p, env=e)
    res = run("apply-patch", "--task", "1", "--repo", str(r), env=e, check=False)
    assert res.returncode == 1 and "the patch is your recipe now" in res.stdout
    assert (r / "a.txt").read_text() == "2\n"
    assert subprocess.run(["git", "-C", str(r), "diff", "--cached", "--name-only"],
                          capture_output=True, text=True).stdout.strip() == "a.txt"


def test_corrige_numa_rodada_de_codigo_mantem_a_fase_de_codigo(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--fase", "codigo", "--commit", h, env=e)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige", "--fase", "codigo",
        "--sessao", "rev", "--patch", patch_de(r, g, tmp_path), env=e)
    run("apply-patch", "--task", "1", "--repo", str(r), env=e)
    ent = [json.loads(l) for l in (d / "eventos.jsonl").read_text().splitlines()][-1]
    assert ent["rodada"] == 2 and ent["fase"] == "codigo"
    # Sem a fase, o APROVA da rodada do patch mandaria direto ao commit, sem prova.
    res = run("event", "veredito", "--task", "1", "--rodada", "2", "--resultado", "aprova",
              "--sessao", "rev", env=e, check=False)
    assert res.returncode == 2 and "verdict phase must match" in res.stderr


def test_checagem_que_suja_a_arvore_nao_entra_na_rodada_e_o_desfazer_falho_e_dito(tmp_path, repo):
    r, g = repo
    # Passa sem 3 em a.txt; com o patch, escreve em a.txt e falha.
    d, e, _ = iniciar(tmp_path, r, projeto=PROJETO.replace(
        "Checagens: `true`", "Checagens: `! grep -q 3 a.txt || { echo x >> a.txt; false; }`"))
    entregar_r1(tmp_path, r, g, e)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
        "--sessao", "rev", "--patch", patch_de(r, g, tmp_path), env=e)
    res = run("apply-patch", "--task", "1", "--repo", str(r), env=e, check=False)
    assert res.returncode == 1 and "Worktree NOT restored to round 1" in res.stdout
    congelado = g("stash", "list").splitlines()[0]
    assert "round 2 (reviewer patch)" in congelado
    assert g("show", "stash@{0}:a.txt") == "3"


def test_apply_patch_no_modo_subagente_manda_o_executor_abrir_um_revisor_novo(tmp_path, repo):
    r, g = repo
    d, e, log = iniciar(tmp_path, r)
    run("event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex",
        "--par", "subagente", env=e)
    h = congelar(r, g)
    run("check", "--task", "1", "--commit", h, env=e)
    run("event", "entrega", "--task", "1", "--rodada", "1", "--commit", h, env=e)
    p = patch_de(r, g, tmp_path)
    run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
        "--sessao", "revisor-orq", "--patch", p, env=e)
    out = run("apply-patch", "--task", "1", "--repo", str(r), env=e).stdout
    assert "orq review-package --task 1 --rodada 2" in out and "NEW revisor-orq" in out
    assert not any(l.startswith("subagente ") for l in log.read_text().splitlines())


def test_corrige_recusa_patch_binario_que_o_limite_nao_mede(tmp_path, repo):
    r, g = repo
    d, e, _ = iniciar(tmp_path, r)
    entregar_r1(tmp_path, r, g, e)
    atual = (r / "a.txt").read_bytes()
    (r / "a.txt").write_bytes(b"\x00bin\n")
    diff = subprocess.run(["git", "-C", str(r), "diff", "--binary", "--", "a.txt"],
                          capture_output=True).stdout
    (r / "a.txt").write_bytes(atual)
    p = tmp_path / "t1-r1.patch"; p.write_bytes(diff)
    assert subprocess.run(["git", "-C", str(r), "apply", "--numstat", str(p)],
                          capture_output=True, text=True).stdout.startswith("-\t-\ta.txt")
    res = run("event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "corrige",
              "--sessao", "rev", "--patch", str(p), env=e, check=False)
    assert res.returncode == 2 and "corrige does not take binary changes" in res.stderr
