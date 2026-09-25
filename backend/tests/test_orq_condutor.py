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
         "HOME": str(tmp_path), "TYPESAFE_API_KEY": "", "JEV_ENDPOINT": "", "JEV_MODEL": ""}
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
    h = _rodada_aprovada(e, r, g)
    out = run(e, "commit", "--task", "1", "--hash", h[:8]).stdout
    assert "ok" in out
    assert [m for m in sent(log) if m.startswith("arb ")] == [
        f"arb [decisao] Task 1 closed and checked: {h[:12]}, 1 file(s), tip = hash, "
        "matches the approved round. Release the next Task."]
    assert json.loads((d / "closed.jsonl").read_text())["task"] == 1
    assert run(e, "ball").stdout.strip() == ""


def test_task_reaberta_depois_de_fechada_volta_a_ter_a_vez(env, repo, tmp_path):
    d, _, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"),
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"),
        "--untouchable", "secret/*")
    h = _rodada_aprovada(e, r, g, extra=("secret/decisão.txt",))
    res = run(e, "commit", "--task", "1", "--hash", h, check=False)
    assert res.returncode == 1
    assert "untouchable in commit history: ['secret/decisão.txt']" in res.stdout
    assert not (d / "closed.jsonl").exists()


def test_commit_que_nao_e_a_ponta_e_recusado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
    h = _rodada_aprovada(e, r, g)
    (r / "a.txt").write_text("3\n")
    g("commit", "-qam", "outro")
    res = run(e, "commit", "--task", "1", "--hash", h, check=False)
    assert res.returncode == 1 and "is not the tip" in res.stdout


def test_commit_de_correcao_fecha_a_task_contando_desde_a_base_da_rodada(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
        "matches the approved round. Release the next Task."]


def test_intocavel_commitado_e_revertido_e_recusado(env, repo, tmp_path):
    d, log, e = env
    r, g = repo
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"),
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(tmp_path / "c.md"))
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
    assert jev_server["body"]["model"] == "jev-1.13.0"
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
QUESTIONS_SHA256 = "0c585afb5bf81ae72c4df6cb765bef86a82a1767989600a2e00a1f5e821dff6f"


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


def test_alarme_acorda_sem_consultar_o_jev(env, tmp_path, jev_server):
    d, log, e = env
    init(e, tmp_path)
    jev_server["resp"] = _answers(p=0.99, veto=0)
    run(_jev_env(e, jev_server, "on"), "notify", "--alarm", "[vigia] rev parado")
    assert sent(log) == ["--tmux arb [vigia] rev parado"]
    assert jev_server["body"] is None
    assert not (d / "jev-shadow.jsonl").exists()
