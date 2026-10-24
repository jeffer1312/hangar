"""Motor da atualização: pré-voo, resgate, ordem das etapas, estado.

Todo teste roda contra um repo git DE VERDADE criado em `tmp_path` — o módulo conversa com o git
por subprocess, e um mock de `subprocess.run` testaria o mock, não o comportamento que importa
(saber distinguir arquivo rastreado de solto, saber que uma branch de resgate ficou mesmo criada).
"""
import json
import subprocess
import sys
from datetime import datetime, timedelta

import pytest

from app import atualizar


def _git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True,
                          encoding="utf-8", errors="replace",
                          env={"HOME": str(repo), "GIT_CONFIG_GLOBAL": "/dev/null",
                               "GIT_CONFIG_SYSTEM": "/dev/null", "PATH": "/usr/bin:/bin",
                               "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                               "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"})


def _pid_morto() -> int:
    """Pid de um processo que já terminou. Número fixo não serve: com pid_max alto, 999999 pode ser
    um processo vivo da máquina (já foi uma thread do kded6)."""
    # No Windows o pid volta a ser usado logo: confere que ainda está livre antes de devolver.
    for _ in range(20):
        p = subprocess.Popen([sys.executable, "-c", "pass"])
        p.wait()
        if not atualizar._vivo(p.pid):
            return p.pid
    pytest.fail("nenhum pid livre depois de 20 processos")


@pytest.fixture
def repo(tmp_path, monkeypatch):
    """Repo com um commit na main, apontado como REPO do módulo."""
    d = tmp_path / "repo"
    d.mkdir()
    _git(d, "init", "-b", "main")
    (d / "a.txt").write_text("um\n", encoding="utf-8")
    _git(d, "add", "a.txt")
    _git(d, "commit", "-m", "primeiro")
    monkeypatch.setattr(atualizar, "REPO", d)
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "cfg"))
    monkeypatch.setattr(atualizar.log_paths, "base", lambda: tmp_path / "logs")
    # Sem escopo systemd nos testes: `_scope_prefix` sonda o systemd com um `subprocess.run` de
    # verdade, e quem troca o `Popen` por um dublê acaba interceptando a sonda em vez do
    # lançamento. O que o prefixo faz está coberto por `test_lancamento_usa_escopo_systemd`.
    monkeypatch.setattr(atualizar.tmux, "_scope_prefix", lambda: [])
    # Nos testes o processo FAZ o papel do motor. Em produção esta chave só liga dentro de
    # `executar()`, pra o backend (que roda os mesmos `_git` no endpoint) não escrever no log e
    # sobrescrever o que o motor gravou.
    monkeypatch.setattr(atualizar, "_SOU_O_MOTOR", True)
    # A prova por pid lê o systemd DESTA máquina (o backend real está no ar durante a suíte) e
    # veria "mesmo pid" em todo teste de fluxo. Quem testa a prova liga ela de volta.
    monkeypatch.setattr(atualizar, "_pid_do_servidor", lambda topologia, porta: None)
    # Sem Rust o checkout vai ao topo sem ler release nenhuma; quem testa a escolha liga de volta.
    monkeypatch.setattr(atualizar.config.settings, "rust_server", False)
    # A pasta pessoal é da máquina real: a marca do app nativo instalado faria o fluxo baixar o
    # manifesto, e a volta mexeria nos binários de ~/.hangar/bin.
    monkeypatch.setattr(atualizar.Path, "home", lambda: tmp_path / "home")
    return d


# ─── Estado ────────────────────────────────────────────────────────────────────────────────────

@pytest.mark.parametrize("conteudo", ["null", "[1, 2]", '"texto"', "{{quebrado"])
def test_estado_exige_dict(repo, tmp_path, conteudo):
    """JSON válido do tipo errado não levanta ValueError — e o `.get()` de quem lê morreria.

    Mesmo furo que já derrubou a resolução de estado de TODAS as sessões no `statusline.read`.
    """
    alvo = atualizar._caminho_estado()
    alvo.parent.mkdir(parents=True, exist_ok=True)
    alvo.write_text(conteudo, encoding="utf-8")
    assert atualizar.estado() == {}


def test_escrever_mescla_e_nao_deixa_tmp(repo):
    atualizar._escrever(fase="rodando", passo=1)
    atualizar._escrever(passo=2)
    e = atualizar.estado()
    assert e["fase"] == "rodando" and e["passo"] == 2 and e["ts"]
    assert not list(atualizar._base().glob("*.tmp"))


# ─── Pré-voo ───────────────────────────────────────────────────────────────────────────────────

def test_checar_repo_limpo(repo):
    p = atualizar.checar()
    assert p["branch"] == "main"
    assert p["sujo"] == 0 and p["ahead"] == 0 and p["behind"] == 0
    assert p["divergiu"] is False


def test_arquivo_solto_nao_conta_como_sujo(repo):
    """Um `.bak` largado na pasta não atrapalha fast-forward nenhum — não pode bloquear o botão."""
    (repo / "solto.bak").write_text("x", encoding="utf-8")
    assert atualizar.checar()["sujo"] == 0


def test_arquivo_rastreado_modificado_conta(repo):
    (repo / "a.txt").write_text("mudou\n", encoding="utf-8")
    assert atualizar.checar()["sujo"] == 1


def test_branch_diferente_aparece(repo):
    _git(repo, "checkout", "-b", "experimento")
    assert atualizar.checar()["branch"] == "experimento"


def test_dependencia_faltando_recusa(repo, monkeypatch):
    """Dependência ausente recusa ANTES de começar, nomeando o que falta (install.sh:90)."""
    monkeypatch.setattr(atualizar.shutil, "which", lambda p: None if p == "uv" else "/usr/bin/" + p)
    p = atualizar.checar()
    assert p["pode"] is False and "uv" in p["faltando"]


# ─── Resgate ───────────────────────────────────────────────────────────────────────────────────

def test_repo_limpo_na_main_nao_cria_resgate(repo):
    """Criar branch de resgate a cada pull de repo limpo só encheria o repo de refs mortas."""
    assert atualizar.resguardar(atualizar.checar()) is None


def test_resgate_guarda_mudanca_e_a_ref_existe(repo):
    (repo / "a.txt").write_text("trabalho de alguem\n", encoding="utf-8")
    nome = atualizar.resguardar(atualizar.checar())
    assert nome and nome.startswith("resgate/")
    assert _git(repo, "rev-parse", "--verify", f"refs/heads/{nome}").returncode == 0
    assert "stash@{0}" in _git(repo, "stash", "list").stdout
    # O stash tirou a mudança da árvore -> o fast-forward que vem depois não encontra obstáculo.
    assert atualizar.checar()["sujo"] == 0


def test_resgate_guarda_commit_local(repo):
    (repo / "b.txt").write_text("dois\n", encoding="utf-8")
    _git(repo, "add", "b.txt")
    _git(repo, "commit", "-m", "trabalho local")
    alvo = _git(repo, "rev-parse", "HEAD").stdout.strip()
    pre = atualizar.checar()
    pre["ahead"] = 1                      # sem upstream no repo de teste; o efeito é o mesmo
    nome = atualizar.resguardar(pre)
    assert _git(repo, "rev-parse", f"refs/heads/{nome}").stdout.strip() == alvo


def test_resgate_que_falha_para_tudo(repo, monkeypatch):
    """Sem prova de que há para onde voltar, nada destrutivo pode acontecer depois."""
    def _falha(*args, **kw):
        class P:
            returncode = 1
            stdout = ""
            stderr = "erro de mentira"
        return P()
    monkeypatch.setattr(atualizar, "_git", _falha)
    with pytest.raises(atualizar.FalhaDeResgate):
        atualizar.resguardar({"sujo": 1, "ahead": 0, "branch": "main"})


# ─── Ordem de executar() ───────────────────────────────────────────────────────────────────────

def test_falha_no_meio_nao_reinicia(repo, monkeypatch):
    """O requisito duro: falhar antes do restart deixa a máquina na versão anterior, inteira."""
    chamou = []
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: chamou.append("puxar"))
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: chamou.append("passos"))
    def _instalar_quebra(topologia, **kw):
        chamou.append("instalar")
        raise RuntimeError("build quebrou")
    monkeypatch.setattr(atualizar, "_preparar", _instalar_quebra)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: chamou.append("reiniciar"))

    final = atualizar.executar()
    assert "reiniciar" not in chamou
    assert final["ok"] is False and "build quebrou" in final["erro"]


def test_erro_inesperado_vira_estado_e_nao_traceback(repo, monkeypatch):
    """Num processo destacado, exceção que escapa deixa o estado preso em "rodando" pra sempre.

    A tela gira a barra e ninguém descobre que falhou. Foi o que aconteceu com `PassoFalhou`, que
    não estava na lista de tipos capturados.
    """
    class ErroEstranho(Exception):
        pass

    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    def _explode():
        raise ErroEstranho("passo com defeito")
    monkeypatch.setattr(atualizar, "_aplicar_passos", _explode)

    final = atualizar.executar()
    assert final["fase"] == "pronto"
    assert final["ok"] is False and "passo com defeito" in final["erro"]


def test_dependencia_faltando_nao_toca_no_repo(repo, monkeypatch):
    monkeypatch.setattr(atualizar.shutil, "which", lambda p: None if p == "npm" else "/usr/bin/" + p)
    tocou = []
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: tocou.append("puxar"))
    final = atualizar.executar()
    assert tocou == []
    assert final["ok"] is False and "npm" in final["erro"]


def test_branch_de_trabalho_recusa_sem_tocar_no_repo(repo, monkeypatch):
    """Medido em 25/08/2026: com o checkout numa branch de trabalho, o `reset --hard origin/main`
    levou A BRANCH junto — ela passou a apontar pra um commit da main e o disco ficou misturado.

    Recusa antes de qualquer coisa, inclusive antes do resguardo: aqui nada aconteceu ainda, e a
    saída é uma frase — não uma branch de resgate pra alguém desfazer depois.
    """
    _git(repo, "checkout", "-b", "mobile-expo")
    tocou = []
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: tocou.append("puxar"))
    monkeypatch.setattr(atualizar, "resguardar", lambda pre: tocou.append("resguardar"))

    final = atualizar.executar()
    assert tocou == []
    assert final["ok"] is False and "mobile-expo" in final["erro"]
    assert _git(repo, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip() == "mobile-expo"


def test_main_e_master_seguem_atualizando(repo):
    """A recusa é sobre branch de TRABALHO. Quem clona com `master` não pode ficar sem atualizar."""
    assert atualizar.checar()["branch_de_trabalho"] is False
    _git(repo, "checkout", "-b", "master")
    assert atualizar.checar()["branch_de_trabalho"] is False


def test_backend_que_nao_sobe_volta_pro_commit_anterior(repo, monkeypatch):
    antes = _git(repo, "rev-parse", "HEAD").stdout.strip()

    def _puxar_avanca(pre):
        (repo / "c.txt").write_text("versao nova\n", encoding="utf-8")
        _git(repo, "add", "c.txt")
        _git(repo, "commit", "-m", "versao nova")

    monkeypatch.setattr(atualizar, "_puxar", _puxar_avanca)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: False)

    final = atualizar.executar()
    assert _git(repo, "rev-parse", "HEAD").stdout.strip() == antes
    assert final["ok"] is False and "nao respondeu" in final["erro"]


@pytest.mark.parametrize("arquivo, esperado", [
    ("shell/main.cjs", True),
    ("shell/preview_ctl.test.cjs", False),
    ("backend/x.py", False),
])
def test_shell_mudou_so_quando_o_pull_toca_a_janela(repo, monkeypatch, arquivo, esperado):
    monkeypatch.setattr(atualizar, "_electron_instalado", lambda: True)

    def _puxar_avanca(pre):
        alvo = repo / arquivo
        alvo.parent.mkdir(parents=True, exist_ok=True)
        alvo.write_text("novo\n", encoding="utf-8")
        _git(repo, "add", arquivo)
        _git(repo, "commit", "-m", "versao nova")

    monkeypatch.setattr(atualizar, "_puxar", _puxar_avanca)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    final = atualizar.executar()
    assert final["ok"] is True
    assert final["shell_mudou"] is esperado


def test_shell_mudou_nao_avisa_sem_electron_instalado(repo, monkeypatch):
    def _puxar_avanca(pre):
        (repo / "shell").mkdir(exist_ok=True)
        (repo / "shell" / "main.cjs").write_text("novo\n", encoding="utf-8")
        _git(repo, "add", "shell/main.cjs")
        _git(repo, "commit", "-m", "shell novo")

    monkeypatch.setattr(atualizar, "_puxar", _puxar_avanca)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    final = atualizar.executar()
    assert final["ok"] is True
    assert final["shell_mudou"] is False


def test_mesmo_pid_na_porta_depois_do_restart_vai_pro_rollback(repo, monkeypatch):
    """HTTP < 500 o processo velho também responde. Se o pid não trocou, não houve restart —
    foi assim que o `-Update` do Windows dizia ok com a instância anterior no ar."""
    for nome in ("_puxar", "_preparar", "_reiniciar"):
        monkeypatch.setattr(atualizar, nome, lambda *a, **k: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    monkeypatch.setattr(atualizar, "_pid_do_servidor", lambda topologia, porta: 4242)
    voltou = []
    monkeypatch.setattr(atualizar, "_voltar",
                        lambda c, m, t, p, **k: (voltou.append(m), {"ok": False})[1])
    atualizar.executar()
    assert voltou and "mesmo de antes" in voltou[0] and "4242" in voltou[0]


def test_pid_diferente_depois_do_restart_e_pronto(repo, monkeypatch):
    for nome in ("_puxar", "_preparar", "_reiniciar"):
        monkeypatch.setattr(atualizar, nome, lambda *a, **k: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    pids = iter([100, 200])
    monkeypatch.setattr(atualizar, "_pid_do_servidor", lambda topologia, porta: next(pids))
    assert atualizar.executar()["ok"] is True


def test_pronto_marca_ok(repo, monkeypatch):
    for nome in ("_puxar", "_aplicar_passos", "_preparar", "_reiniciar"):
        monkeypatch.setattr(atualizar, nome,
                            (lambda *a, **k: None) if nome != "_aplicar_passos" else (lambda: None))
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    final = atualizar.executar()
    assert final["ok"] is True and final["fase"] == "pronto"


# ─── Lançamento ────────────────────────────────────────────────────────────────────────────────

def test_nao_inicia_duas_vezes(repo):
    """Quem manda é o LOCK, não o estado: só ele fecha a janela entre checar e escrever."""
    import os as _os
    trava = atualizar._base() / "rodando.lock"
    trava.parent.mkdir(parents=True, exist_ok=True)
    trava.write_text(str(_os.getpid()), encoding="utf-8")   # dono vivo
    assert atualizar.iniciar()["erro"] == "ja_rodando"


def test_pid_morto_nao_bloqueia(repo, monkeypatch):
    """Máquina que desligou no meio de uma atualização não pode ficar travada para sempre."""
    trava = atualizar._base() / "rodando.lock"
    trava.parent.mkdir(parents=True, exist_ok=True)
    trava.write_text(str(_pid_morto()), encoding="utf-8")
    lancou = []
    class P:
        pid = 4242
    monkeypatch.setattr(atualizar.subprocess, "Popen", lambda *a, **k: (lancou.append(a), P())[1])
    assert atualizar.iniciar()["ok"] is True
    assert lancou


def test_estado_do_lancamento_e_json_valido(repo, monkeypatch):
    class P:
        pid = 4242
    monkeypatch.setattr(atualizar.subprocess, "Popen", lambda *a, **k: P())
    atualizar.iniciar()
    bruto = json.loads(atualizar._caminho_estado().read_text(encoding="utf-8"))
    assert bruto["fase"] == "rodando" and bruto["texto"] == "Começando"
    # O pid do filho NÃO é gravado aqui: quem grava pid no estado é o próprio motor, e o do filho
    # (que diz se a atualização morreu) mora no lock. Escrever depois do `Popen` era uma corrida.
    assert (atualizar._base() / "rodando.lock").read_text(encoding="utf-8").strip() == "4242"


# ─── Sessões vivas e o sistema ─────────────────────────────────────────────────────────────────

def test_falhou_mede_se_o_servidor_esta_no_ar(repo, monkeypatch):
    """Supor "no ar" é mentira no Windows: lá o installer já derrubou o backend antes de falhar.

    A tela dizia "o que estava no ar continua no ar" com a máquina sem servidor nenhum.
    """
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: False)
    assert atualizar._falhou("deu ruim")["no_ar"] is False
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    assert atualizar._falhou("deu ruim")["no_ar"] is True


def test_aviso_do_instalador_chega_no_estado(repo, monkeypatch):
    """O instalador pode terminar BEM e ter deixado algo pra trás (a janela nativa é o caso).

    Sem isto a tela dizia "Atualizado" e pronto, e o que ficou quebrado era uma linha perdida no
    meio do log — que ainda por cima pode rolar pra fora do teto de 400 linhas.
    """
    class P:
        returncode = 0
        stdout = ("instalando\n"
                  "##HANGAR-AVISO## a janela nativa (Electron) ficou com dependencias desatualizadas\n"
                  "pronto\n")
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda *a, **kw: P())
    atualizar._reaplicar("systemd")
    assert atualizar.estado()["avisos"] == [
        "a janela nativa (Electron) ficou com dependencias desatualizadas"]


def _preparo_gravado(repo, monkeypatch, *, lock_igual: bool, node_modules: bool,
                     topologia: str = "systemd", fetch=lambda **kw: []):
    chamadas = []
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: (chamadas.append(args), P())[1])
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda **k: None)
    monkeypatch.setattr(atualizar.rust_release, "fetch", fetch)
    monkeypatch.setattr(atualizar, "_renovar_chromium", lambda: None)
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    (repo / "backend").mkdir(exist_ok=True)
    (repo / "package-lock.json").write_text("{}", encoding="utf-8")
    if node_modules:
        (repo / "node_modules").mkdir()
    marca = atualizar._base() / "package-lock.sha"
    marca.parent.mkdir(parents=True, exist_ok=True)
    marca.write_text(atualizar._hash_arquivo(repo / "package-lock.json") if lock_igual else "outro",
                     encoding="utf-8")
    atualizar._preparar(topologia)
    return [" ".join(c) for c in chamadas]


def test_renovar_chromium_onde_o_script_ja_rodou_mesmo_com_download_falho(tmp_path, monkeypatch):
    chamadas = []
    class P:
        returncode = 1
        stdout = ""
        stderr = "sem rede"
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: (chamadas.append(args), P())[1])
    monkeypatch.setattr(atualizar.sys, "platform", "linux")
    monkeypatch.setattr(atualizar.Path, "home", lambda: tmp_path)
    atualizar._renovar_chromium()
    assert chamadas == []
    # Download que falhou sem shell anterior: a pasta não nasceu, mas a marca sim, e a próxima atualização tenta de novo.
    (tmp_path / ".hangar" / "native").mkdir(parents=True)
    (tmp_path / ".hangar" / "native" / "chromium-ok").write_text("sistema: /usr/bin/google-chrome (baixar falhou: x)\n")
    atualizar._renovar_chromium()
    assert len(chamadas) == 1 and chamadas[0][0].endswith("install-chromium.sh")


@pytest.mark.parametrize("texto,idade_h,roda", [
    ("baixado: /x/chrome-headless-shell (1.2.3.4)\n", 2, False),
    ("baixado: /x/chrome-headless-shell (1.2.3.4)\n", 25, True),
    ("download falhou: sem rede\n", 2, True),
])
def test_renovar_chromium_no_maximo_uma_vez_por_dia(tmp_path, monkeypatch, texto, idade_h, roda):
    chamadas = []
    class P:
        returncode = 0
        stdout = stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: (chamadas.append(args), P())[1])
    monkeypatch.setattr(atualizar.sys, "platform", "linux")
    monkeypatch.setattr(atualizar.Path, "home", lambda: tmp_path)
    marca = tmp_path / ".hangar" / "native" / "chromium-ok"
    marca.parent.mkdir(parents=True)
    marca.write_text(texto)
    agora = marca.stat().st_mtime + idade_h * 3600
    atualizar._renovar_chromium(agora=agora)
    assert (len(chamadas) == 1) is roda


def test_preparar_sincroniza_o_uv_e_nao_chama_o_instalador(repo, monkeypatch):
    """O caminho do botão: dist + `uv sync`. Nada de `install.sh --update`."""
    cmds = _preparo_gravado(repo, monkeypatch, lock_igual=True, node_modules=True)
    assert any(c.endswith("uv sync") for c in cmds)
    assert not any("install." in c for c in cmds)
    assert not any("npm ci" in c for c in cmds)


def test_preparar_roda_npm_ci_so_quando_o_lock_mudou(repo, monkeypatch):
    cmds = _preparo_gravado(repo, monkeypatch, lock_igual=False, node_modules=True)
    assert any("npm ci" in c for c in cmds)
    assert not any("Stop-HangarFrontend" in c for c in cmds)
    # E grava o hash: a próxima não repete.
    assert (atualizar._base() / "package-lock.sha").read_text() == \
        atualizar._hash_arquivo(repo / "package-lock.json")


def test_prepare_stops_windows_front_before_npm_ci(repo, monkeypatch):
    """No Windows o `vite preview` da tarefa segura o binário nativo do `node_modules`, e o
    `npm ci` morre em EPERM deixando a pasta pela metade. O front cai ANTES."""
    cmds = _preparo_gravado(repo, monkeypatch, lock_igual=False, node_modules=True,
                            topologia="windows")
    parar = next(i for i, c in enumerate(cmds) if "Stop-HangarFrontend" in c)
    assert parar < next(i for i, c in enumerate(cmds) if "npm ci" in c)


def test_preparar_sem_marca_assume_o_node_modules_do_instalador(repo, monkeypatch):
    """Primeira atualização depois desta mudança: sem marca gravada, o `node_modules` é o do
    instalador — `npm ci` só por falta de registro custou 5 min na VM. A marca nasce do disco."""
    chamadas = []
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: (chamadas.append(args), P())[1])
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda **k: None)
    monkeypatch.setattr(atualizar.rust_release, "fetch", lambda **kw: [])
    monkeypatch.setattr(atualizar, "_renovar_chromium", lambda: None)
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    (repo / "backend").mkdir(exist_ok=True)
    (repo / "package-lock.json").write_text("{}", encoding="utf-8")
    (repo / "node_modules").mkdir()
    atualizar._preparar("systemd")
    assert not any("npm ci" in " ".join(c) for c in chamadas)
    assert (atualizar._base() / "package-lock.sha").read_text() == \
        atualizar._hash_arquivo(repo / "package-lock.json")


def test_preparar_sem_node_modules_nao_instala_front(repo, monkeypatch):
    """Máquina que serve o dist do CI sem nunca ter rodado `npm ci` não ganha um `node_modules`
    de 400 MB só porque o lock mudou."""
    cmds = _preparo_gravado(repo, monkeypatch, lock_igual=False, node_modules=False)
    assert not any("npm ci" in c for c in cmds)


def test_preparar_baixa_os_binarios_e_a_falha_vira_aviso(repo, monkeypatch):
    """Sem os binários o Python atende sozinho: o download que falha avisa e não para o `uv sync`."""
    cmds = _preparo_gravado(repo, monkeypatch, lock_igual=True, node_modules=True,
                            fetch=lambda **kw: ["binários Rust não baixados: rede fora"])
    assert any(c.endswith("uv sync") for c in cmds)
    assert "binários Rust não baixados: rede fora" in atualizar.estado()["avisos"]


def test_preparar_sem_build_para_a_maquina_nao_avisa(repo, monkeypatch):
    _preparo_gravado(repo, monkeypatch, lock_igual=True, node_modules=True, fetch=lambda **kw: None)
    assert not atualizar.estado().get("avisos")


def test_volta_para_a_versao_anterior_nao_troca_binario(repo, monkeypatch):
    """O `_voltar` roda `_preparar(dist=False)`: a release é a mais nova, não a do commit de antes."""
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: P())
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    monkeypatch.setattr(atualizar.rust_release, "fetch",
                        lambda: pytest.fail("a volta não baixa binário"))
    (repo / "backend").mkdir(exist_ok=True)
    atualizar._preparar("systemd", dist=False)


def test_falha_do_instalador_mostra_o_motivo_marcado_nao_a_cauda(repo, monkeypatch):
    """As 12 últimas linhas do `install.ps1` são o portão do fim — a lista de pendências sem o
    porquê. O motivo é impresso no passo que falhou, com a marca; é ele que vai pra tela."""
    class P:
        returncode = 1
        stdout = ("7/8 Subir junto com o Windows\n"
                  "  --  tarefas agendadas: A tarefa hangar-backend nao manteve a protecao\n"
                  "##HANGAR-FALHA## tarefas agendadas: A tarefa hangar-backend nao manteve a protecao\n"
                  "##HANGAR-FALHA## backend no ar: nenhuma tarefa chegou a ser reiniciada\n"
                  + "\n".join(f"linha {i}" for i in range(20)) + "\n"
                  "NAO terminou: tarefas agendadas, backend no ar\n")
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda *a, **kw: P())
    with pytest.raises(RuntimeError) as e:
        atualizar._reaplicar("windows")
    msg = str(e.value)
    assert "nao manteve a protecao" in msg and "nenhuma tarefa chegou" in msg
    assert "linha 19" not in msg          # não é a cauda


def test_falha_do_instalador_sem_marca_cai_na_cauda(repo, monkeypatch):
    class P:
        returncode = 1
        stdout = "x\n" * 30 + "ultima linha\n"
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda *a, **kw: P())
    with pytest.raises(RuntimeError, match="ultima linha"):
        atualizar._reaplicar("systemd")


def test_sem_marca_nao_inventa_aviso(repo, monkeypatch):
    class P:
        returncode = 0
        stdout = "tudo certo por aqui\n"
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda *a, **kw: P())
    atualizar._reaplicar("systemd")
    assert not atualizar.estado().get("avisos")


def test_windows_reinicia_as_tarefas_pelo_helper_sem_instalador(repo, monkeypatch):
    """O restart do Windows é o `Restart-HangarTasks` do `windows-tasks.ps1` (o mesmo da vigia),
    não o `install.ps1 -Update` inteiro. E não pede reinício manual."""
    chamadas = []
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: (chamadas.append(args), P())[1])
    (repo / "backend").mkdir(exist_ok=True)
    (repo / "backend" / ".env").write_text("CP_FRONT_PORT=5173\n", encoding="utf-8")
    atualizar._reiniciar("windows", 8765)
    assert not atualizar.estado().get("reiniciar_manual")
    cmd = " ".join(chamadas[-1])
    assert "windows-tasks.ps1" in cmd and "Restart-HangarTasks" in cmd and "8765 5173" in cmd
    assert "install.ps1" not in cmd


def test_windows_restart_que_falha_levanta(repo, monkeypatch):
    class P:
        returncode = 1
        stdout = "Porta 8765 ocupada por outro processo"
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda *a, **kw: P())
    with pytest.raises(RuntimeError, match="ocupada"):
        atualizar._reiniciar("windows", 8765)


def test_instalacao_na_mao_tambem_pede_reinicio(repo):
    atualizar._reiniciar("manual")
    assert atualizar.estado()["reiniciar_manual"] is True


def test_sem_restart_nao_cobra_prova_de_vida(repo, monkeypatch):
    """O backend velho continua respondendo: um `_subiu` verde ali não provaria nada."""
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: atualizar._escrever(reiniciar_manual=True))
    def _nunca(*a, **kw):
        raise AssertionError("_subiu nao devia ser chamado sem restart")
    monkeypatch.setattr(atualizar, "_subiu", _nunca)
    final = atualizar.executar()
    assert final["ok"] is True and final["reiniciar_manual"] is True


def test_dono_morto_vira_falha_na_tela(repo):
    """Estado congelado em "rodando" com o processo morto só saía editando o JSON na mão.

    Acontece de verdade: no Windows, o `install.ps1 -Update` chegou a matar o processo da própria
    atualização (ele casa o filtro de "instância anterior"), deixando a tela presa pra sempre.
    """
    atualizar._escrever(fase="rodando", passo=4, total=5, pid=_pid_morto())
    d = atualizar.estado_para_tela()
    assert d["fase"] == "pronto" and d["ok"] is False
    assert "interrompida" in d["erro"]
    # Converteu no ARQUIVO: a conclusão não pode depender de quem perguntou.
    assert atualizar.estado()["fase"] == "pronto"


def test_atualizacao_recem_lancada_nao_e_declarada_morta(repo):
    """`iniciar()` grava "rodando" SEM pid — quem o grava é o motor, segundos depois.

    Sem a guarda, o polling da tela (2s) matava toda atualização no berço: `_vivo(None)` é False.
    """
    atualizar._escrever(fase="rodando", passo=0, total=5, texto="Começando")   # sem pid, agora
    assert atualizar.estado_para_tela()["fase"] == "rodando"


def test_sem_pid_e_antigo_ainda_e_orfao(repo):
    """Passada a janela de nascimento, "rodando" sem pid é abandono — não pode prender a tela."""
    velho = (datetime.now().astimezone() - timedelta(seconds=600)).isoformat(timespec="seconds")
    atualizar._escrever(fase="rodando", passo=0, total=5)
    atualizar._escrever(ts=velho)          # o `_escrever` carimba `ts`; sobrescreve depois
    caminho = atualizar._caminho_estado()
    d = json.loads(caminho.read_text(encoding="utf-8"))
    d["ts"] = velho
    caminho.write_text(json.dumps(d, ensure_ascii=False), encoding="utf-8")
    assert atualizar.estado_para_tela()["fase"] == "pronto"


def test_dono_vivo_nao_e_declarado_morto(repo):
    import os as _os
    atualizar._escrever(fase="rodando", passo=2, total=5, pid=_os.getpid())
    assert atualizar.estado_para_tela()["fase"] == "rodando"


def test_falha_ao_gravar_estado_nao_derruba_quem_chamou(repo, monkeypatch):
    """No Windows o `atomico.substituir` levanta WinError 5 com o arquivo em uso.

    Isso subiu sem tratamento até o `GET /api/atualizacao` e derrubou o request inteiro — a tela
    perdia a resposta por causa de uma linha de log.
    """
    def _nega(*a, **kw):
        raise PermissionError("[WinError 5] Acesso negado")
    monkeypatch.setattr(atualizar.atomico, "substituir", _nega)
    atualizar._escrever(fase="rodando", passo=1)     # não levanta
    assert not list(atualizar._base().glob("*.tmp")), "deixou tmp orfao"


def test_backend_nao_escreve_no_log_do_motor(repo, monkeypatch):
    """O `GET /api/atualizacao` roda `git` a cada 2s e mescla-e-grava o MESMO estado.

    Sem esta trava, o backend sobrescrevia o log que o processo da atualização tinha acabado de
    escrever — e, como `_escrever` mescla o dict inteiro, a disputa podia levar junto `fase` e
    `passo`. Visto no Windows: a caixa mostrava só os comandos do endpoint e parecia travada.
    """
    atualizar._escrever(fase="rodando", log=["$ do motor"])
    monkeypatch.setattr(atualizar, "_SOU_O_MOTOR", False)    # este processo é o BACKEND
    atualizar._rodar(["sh", "-c", "echo do backend"])
    assert atualizar.estado()["log"] == ["$ do motor"]


def test_log_guarda_o_comando_e_a_saida(repo):
    """É o que a tela mostra na caixinha — sem isso a etapa longa parece travada."""
    atualizar._rodar(["sh", "-c", "echo primeira; echo segunda"])
    log = atualizar.estado()["log"]
    texto = "\n".join(log)
    assert "$ sh -c echo primeira; echo segunda" in texto
    assert "primeira" in texto and "segunda" in texto


def test_comando_nao_abre_janela_no_windows(repo, monkeypatch):
    """No Windows, cada comando de console abria um terminal preto na frente de quem usa.

    Visto em 25/08/2026, no meio de uma atualização: subiu uma janela escrita "npm ci" por cima do
    app. No Linux o defeito não existe (não há console a criar), então só a flag é verificável aqui.
    """
    visto = {}
    class P:
        stdout = iter([])
        returncode = 0
        def wait(self, timeout=None):
            return 0
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda *a, **kw: (visto.update(kw), P())[1])
    monkeypatch.setattr(atualizar, "_SEM_JANELA_WINDOWS", 0x08000000)
    atualizar._rodar(["true"])
    assert visto.get("creationflags") == 0x08000000


def test_timeout_vale_mesmo_com_o_processo_calado(repo):
    """Iterando em `proc.stdout`, o relógio só era olhado quando chegava linha.

    Um processo que trava CALADO (npm num prompt, rede pendurada) nunca devolvia o controle e o
    prazo virava letra morta — medido: `sleep 5` com teto de 1s voltava normal, em 5s.
    """
    import time as _t
    inicio = _t.monotonic()
    with pytest.raises(subprocess.TimeoutExpired):
        atualizar._rodar(["sh", "-c", "sleep 30"], timeout=2.0)
    assert _t.monotonic() - inicio < 10, "nao respeitou o prazo"


def test_log_registra_codigo_de_saida_ruim(repo):
    atualizar._rodar(["sh", "-c", "exit 7"])
    assert "[saiu com 7]" in "\n".join(atualizar.estado()["log"])


def test_log_do_comando_perde_a_cor_de_terminal(repo):
    atualizar._rodar(["sh", "-c", "printf '\\033[31mvermelho\\033[0m\\n'"])
    texto = "\n".join(atualizar.estado()["log"])
    assert "vermelho" in texto and "\x1b" not in texto


def test_log_tem_teto(repo):
    """Sem teto, um comando falante incharia o estado que a tela lê a cada 2s."""
    atualizar._rodar(["sh", "-c", f"seq 1 {atualizar._TETO_LOG * 2}"])
    linhas = atualizar.estado()["log"]
    assert len(linhas) <= atualizar._TETO_LOG
    # Corta o começo, não o fim: o que interessa a quem está olhando é a última linha.
    assert str(atualizar._TETO_LOG * 2) in "\n".join(linhas)


def test_saida_do_comando_volta_inteira_mesmo_com_log_cortado(repo):
    """Quem CHAMA precisa do texto completo (o `_cauda` monta a mensagem de erro a partir dele)."""
    p = atualizar._rodar(["sh", "-c", "echo alfa; echo beta 1>&2"])
    assert "alfa" in p.stdout and "beta" in p.stdout      # stderr é fundido, e em ordem


def test_erro_nao_leva_cor_de_terminal_pra_tela(repo):
    """O `install.sh` colore a saída, e ela vai inteira pra tela — sem limpar, vira lixo visível."""
    class P:
        returncode = 1
        stdout = ""
        stderr = "\x1b[1m1/8 Dependências\x1b[0m\n  \x1b[31mX\x1b[0m   tmux faltando"

    saida = atualizar._cauda(P())
    assert "\x1b" not in saida
    assert "tmux faltando" in saida and "1/8 Dependências" in saida


def test_reset_do_rollback_que_falha_nao_mente(repo, monkeypatch):
    """Seguir com o reset falhado reinstalaria em cima do código quebrado, dizendo que voltou."""
    real = atualizar._git

    def _git_falso(*args, **kw):
        if args[:2] == ("reset", "--hard"):
            class P:
                returncode = 1
                stdout = ""
                stderr = "fatal: disco cheio"
            return P()
        return real(*args, **kw)

    monkeypatch.setattr(atualizar, "_git", _git_falso)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: pytest.fail("nao pode reinstalar"))
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: pytest.fail("nao pode reiniciar"))

    final = atualizar._voltar("abc123", "o servidor caiu", "systemd", 1)
    assert final["ok"] is False and final["voltou"] is False
    assert final["no_ar"] is False and "nao consegui voltar" in final["erro"]


def test_restart_que_falha_vai_pro_rollback(repo, monkeypatch):
    """`systemctl restart` já derrubou o processo antigo: isto NÃO é "está tudo como estava"."""
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    def _quebra(t, *a, **k):
        raise RuntimeError("systemctl nao subiu")
    monkeypatch.setattr(atualizar, "_reiniciar", _quebra)
    voltou = []
    monkeypatch.setattr(atualizar, "_voltar",
                        lambda c, m, t, p, **k: (voltou.append(m), {"ok": False})[1])
    atualizar.executar()
    assert voltou and "nao reiniciou" in voltou[0]


def test_arquivo_solto_que_colide_e_guardado_antes_do_reset(repo, monkeypatch):
    """Não-rastreado que colide com o commit novo era sobrescrito calado pelo `reset --hard`.

    `resguardar` não pega este caso (só olha rastreado), então o stash tem que acontecer no
    fallback do próprio `_puxar` — senão "automático nunca é irreversível" deixa de valer pro
    arquivo que ninguém versionou.
    """
    guardou = []
    real = atualizar._git

    def _espiao(*args, **kw):
        if args[:2] == ("stash", "push"):
            guardou.append(args)
        if args[0] in ("fetch", "merge", "reset"):
            class P:
                returncode = 0 if args[0] in ("fetch", "reset") else 1
                stdout = ""
                stderr = "untracked working tree files would be overwritten"
            return P()
        return real(*args, **kw)

    monkeypatch.setattr(atualizar, "_git", _espiao)
    atualizar._puxar({"sujo": 0})
    assert guardou, "nao guardou os nao-rastreados antes do reset"
    assert "--include-untracked" in guardou[0]


def test_stash_que_falha_impede_o_reset(repo, monkeypatch):
    """Stash falhado com o reset acontecendo mesmo assim reproduz o defeito que ele veio corrigir."""
    resetou = []
    real = atualizar._git

    def _espiao(*args, **kw):
        if args[0] in ("fetch", "merge", "stash", "reset"):
            class P:
                returncode = 0 if args[0] == "fetch" else 1
                stdout = ""
                stderr = "index.lock existe"
            if args[0] == "reset":
                resetou.append(args)
            return P()
        return real(*args, **kw)

    monkeypatch.setattr(atualizar, "_git", _espiao)
    with pytest.raises(RuntimeError, match="guardar o que estava no disco"):
        atualizar._puxar({"sujo": 0})
    assert not resetou, "resetou mesmo sem ter conseguido guardar nada"


def test_passo_malformado_vai_pro_estado_e_nao_so_pro_log(repo, monkeypatch):
    """O log do processo destacado vai pro /dev/null; sem isto o passo some sem ninguém ver."""
    from app import atualizacoes
    monkeypatch.setattr(atualizacoes, "invalidos", lambda: ["2026-01-01-torto.md"])
    monkeypatch.setattr(atualizacoes, "aplicar_pendentes", lambda: [])
    atualizar._aplicar_passos()
    assert atualizar.estado()["passos_invalidos"] == ["2026-01-01-torto.md"]


def test_duas_chamadas_so_uma_lanca(repo, monkeypatch):
    """Check-then-act deixava dois processos rodarem `git reset` no mesmo repo ao mesmo tempo."""
    import os as _os
    lancados = []
    class P:
        pid = _os.getpid()      # dono VIVO: é o que o lock precisa achar na segunda chamada
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda *a, **k: (lancados.append(1), P())[1])
    assert atualizar.iniciar()["ok"] is True
    assert atualizar.iniciar().get("erro") == "ja_rodando"
    assert len(lancados) == 1


def test_troca_de_dono_do_lock_nunca_deixa_o_arquivo_vazio(repo, monkeypatch):
    """`write_text` trunca antes de escrever: quem lesse nessa janela veria "" e tomaria a vez."""
    import os as _os
    class P:
        pid = _os.getpid()
    monkeypatch.setattr(atualizar.subprocess, "Popen", lambda *a, **k: P())
    atualizar.iniciar()
    trava = atualizar._base() / "rodando.lock"
    assert trava.read_text(encoding="utf-8").strip() == str(_os.getpid())
    assert not list(trava.parent.glob("*.tmp")), "sobrou tmp da troca de dono"


def test_volta_devolve_o_binario_anterior_antes_de_reiniciar(repo, monkeypatch):
    """Código antigo com o binário novo é contrato diferente: o Python atenderia sozinho."""
    ordem = []

    def _preparar_quebra(t, *a, **k):
        ordem.append("preparar")
        raise RuntimeError("uv sync falhou")

    monkeypatch.setattr(atualizar, "_preparar", _preparar_quebra)
    monkeypatch.setattr(atualizar.rust_release, "restore_previous", lambda: ordem.append("binario") or [])
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: ordem.append("reiniciar"))
    atualizar._voltar(atualizar._git("rev-parse", "HEAD").stdout.strip(), "o servidor caiu", "systemd", 1)
    # O resto da volta pode falhar; o binário já voltou antes dele.
    assert ordem == ["binario", "preparar"]


def test_volta_que_nao_devolve_o_binario_avisa(repo, monkeypatch):
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar.rust_release, "restore_previous", lambda: ["hangar-server: preso"])
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    final = atualizar._voltar(atualizar._git("rev-parse", "HEAD").stdout.strip(), "o servidor caiu", "systemd", 1)
    assert "hangar-server: preso" in final["avisos"]


def test_atualizacao_que_deu_certo_descarta_a_copia_do_binario(repo, monkeypatch):
    descartes = []
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: descartes.append("preparar"))
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    monkeypatch.setattr(atualizar, "_instalar_nativo", lambda destino, porta: None)
    monkeypatch.setattr(atualizar.rust_release, "discard_previous", lambda: descartes.append("descarta") or [])
    assert atualizar.executar()["ok"] is True
    # Antes de instalar, uma cópia de outra rodada nunca é a que a volta devolve.
    assert descartes == ["descarta", "preparar", "descarta"]


def test_copia_velha_que_nao_sai_para_antes_de_mexer_em_algo(repo, monkeypatch):
    antes = atualizar._git("rev-parse", "HEAD").stdout.strip()
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: pytest.fail("nao pode puxar"))
    monkeypatch.setattr(atualizar.rust_release, "discard_previous",
                        lambda: ["hangar-server: a cópia anterior do binário não saiu"])
    final = atualizar.executar()
    assert final["ok"] is False and "não saiu" in final["erro"]
    assert atualizar._git("rev-parse", "HEAD").stdout.strip() == antes


def test_rollback_que_nao_sobe_nao_diz_que_esta_no_ar(repo, monkeypatch):
    """O restart que motivou o rollback já matou o processo antigo; se o do rollback também falha,
    não há nada rodando — dizer "no ar" manda a pessoa embora de um servidor morto."""
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    def _quebra(t, *a, **k):
        raise RuntimeError("nao subiu nem na volta")
    monkeypatch.setattr(atualizar, "_reiniciar", _quebra)
    final = atualizar._voltar(atualizar._git("rev-parse", "HEAD").stdout.strip(),
                              "o servidor caiu", "systemd", 1)
    assert final["ok"] is False and final["no_ar"] is False


def test_lock_de_processo_morto_nao_trava_a_maquina(repo):
    trava = atualizar._base() / "rodando.lock"
    trava.parent.mkdir(parents=True, exist_ok=True)
    trava.write_text(str(_pid_morto()), encoding="utf-8")
    assert atualizar._tomar_a_vez() is True


def test_a_vez_e_solta_mesmo_quando_falha(repo, monkeypatch):
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: (_ for _ in ()).throw(RuntimeError("x")))
    atualizar._tomar_a_vez()
    atualizar.executar()
    assert not (atualizar._base() / "rodando.lock").exists()


def test_lancamento_usa_escopo_systemd(repo, monkeypatch):
    """`setsid` não tira do CGROUP, e o restart mata o cgroup inteiro — junto com quem o ordenou.

    Sem o escopo transiente, o `systemctl restart` disparado pela própria atualização a mata antes
    de ela escrever o desfecho: o estado fica preso em "rodando" e a barra gira para sempre. Mesmo
    motivo, e mesma solução, do `tmux._scope_prefix` (que existe porque o restart matava o servidor
    tmux e todas as sessões).
    """
    args_vistos = []
    class P:
        pid = 1
    monkeypatch.setattr(atualizar.tmux, "_scope_prefix",
                        lambda: ["systemd-run", "--user", "--scope", "-q", "--"])
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda a, **kw: (args_vistos.extend(a), P())[1])
    atualizar.iniciar()
    assert args_vistos[:3] == ["systemd-run", "--user", "--scope"]
    assert "app.atualizar" in args_vistos


def test_reiniciar_agora_lanca_destacado_e_no_escopo(repo, monkeypatch):
    """Mesmo cuidado do lançamento da atualização: o `systemctl restart` mata o cgroup do backend,
    então quem dá o comando tem de estar FORA dele — senão morre no meio do próprio comando."""
    args_vistos: list[str] = []
    capturado: dict = {}
    class P:
        pid = 3
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(atualizar.tmux, "_scope_prefix",
                        lambda: ["systemd-run", "--user", "--scope", "-q", "--"])
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda a, **kw: (args_vistos.extend(a), capturado.update(kw), P())[2])
    r = atualizar.reiniciar_agora()
    assert r["ok"] is True and r["pid"] == 3
    assert capturado["env"][atualizar._RESTART_REQUEST_ENV] == r["request_id"]
    assert args_vistos[:3] == ["systemd-run", "--user", "--scope"]
    assert args_vistos[-2:] == ["--reiniciar", "8765"]
    assert capturado.get("start_new_session") is True


def test_reinicio_que_morre_antes_do_motor_publica_a_falha(repo, tmp_path, monkeypatch):
    from app import log_paths

    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(log_paths, "base", lambda: tmp_path / "logs")
    atualizar._escrever(fase="pronto", ok=True, pid=1, texto="Atualizado")
    real_popen = subprocess.Popen
    children = []

    def early_failure(args, **kwargs):
        child = real_popen([sys.executable, "-c", "raise RuntimeError('restart launch failed')"],
                           **{**kwargs, "cwd": str(tmp_path)})
        children.append(child)
        return child

    monkeypatch.setattr(atualizar.subprocess, "Popen", early_failure)
    try:
        requested = atualizar.reiniciar_agora()
        children[0].wait(timeout=10)
        final = atualizar.estado_para_tela()

        assert final["pid"] == requested["pid"]
        assert final["fase"] == "pronto" and final["ok"] is False
        assert final["erro"]
        assert "restart launch failed" in (tmp_path / "logs" / "privado" / "atualizacao-launch.log").read_text()
    finally:
        for child in children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=10)


def test_reinicio_correlaciona_pedido_quando_pid_do_motor_difere_do_lancador(repo, monkeypatch):
    launch = {}
    class Launcher:
        pid = 43

    def popen(args, **kwargs):
        launch.update(kwargs)
        return Launcher()

    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(atualizar.subprocess, "Popen", popen)
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda topo, porta: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)

    requested = atualizar.reiniciar_agora()
    assert requested["request_id"]
    for key, value in launch["env"].items():
        monkeypatch.setenv(key, value)
    atualizar.executar_reinicio()

    final = atualizar.estado_para_tela()
    assert final["request_id"] == requested["request_id"]
    assert final["pid"] != requested["pid"]
    assert final["ok"] is True


def test_reinicio_avulso_mostra_progresso_e_termina_pronto(repo, monkeypatch):
    """O botão "Reiniciar" dizia só "Reiniciando…": o reinício avulso passa a gravar as mesmas
    etapas que a atualização (barra, texto, log) e fecha em `pronto`."""
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    vistos = []
    monkeypatch.setattr(atualizar, "_reiniciar",
                        lambda topo, *a, **k: vistos.append(atualizar.estado()["etapa"]))
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    atualizar.executar_reinicio()
    final = atualizar.estado()
    assert vistos == ["reiniciar"] and final["fase"] == "pronto" and final["ok"] is True
    assert final["total"] == len(atualizar.ETAPAS_REINICIO)
    assert not (atualizar._base() / "rodando.lock").exists()     # soltou a vez


def test_reinicio_avulso_sem_servidor_de_volta_nao_diz_ok(repo, monkeypatch):
    """A tela recarrega ao ver `ok`; sem a prova de vida recarregava antes do servidor novo subir."""
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(atualizar, "_reiniciar", lambda topo, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: False)
    atualizar.executar_reinicio()
    final = atualizar.estado()
    assert final["ok"] is False and "nao respondeu" in final["erro"]


def test_reinicio_avulso_com_o_mesmo_processo_nao_diz_ok(repo, monkeypatch):
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(atualizar, "_reiniciar", lambda topo, *a: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    monkeypatch.setattr(atualizar, "_pid_do_servidor", lambda topo, porta: 43)

    atualizar.executar_reinicio()

    final = atualizar.estado_para_tela()
    assert final["ok"] is False
    assert "mesmo" in final["reinicio_erro"]


def test_reiniciar_agora_recusa_com_atualizacao_rodando(repo, monkeypatch):
    """Os dois escrevem no mesmo estado.json: um reinício por cima de uma atualização em curso
    sobrescrevia o pid dela e, morrendo, soltava a trava dela."""
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    assert atualizar._tomar_a_vez()
    try:
        assert atualizar.reiniciar_agora() == {"ok": False, "erro": "ja_rodando"}
    finally:
        atualizar._soltar_a_vez()


def test_reinicio_que_falha_deixa_rastro_no_estado(repo, monkeypatch):
    """O processo do reinício é destacado e tem o stderr no /dev/null: exceção aqui não vai pra log
    nenhum. Sem gravar no estado, um `systemctl` que falha some por completo e a tela fica esperando
    um servidor que nunca cai."""
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: None)
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(atualizar, "_reiniciar",
                        lambda topo, porta: (_ for _ in ()).throw(RuntimeError("systemctl explodiu")))
    atualizar.executar_reinicio()
    assert "systemctl explodiu" in atualizar.estado()["reinicio_erro"]


def _dist_do_ci(repo, monkeypatch, sha: str, index: bool = True):
    """Dublê do release `dist-latest`: um `.sha` e um tar.gz com (ou sem) index.html."""
    import io
    import tarfile
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        if index:
            dado = b"<html>novo</html>"
            info = tarfile.TarInfo("index.html")
            info.size = len(dado)
            tar.addfile(info, io.BytesIO(dado))
    corpo = {"frontend-dist.sha": sha.encode(), "frontend-dist.tar.gz": buf.getvalue()}

    class _Resp(io.BytesIO):
        def __enter__(self): return self
        def __exit__(self, *a): pass

    import urllib.request
    monkeypatch.setattr(urllib.request, "urlopen",
                        lambda url, timeout=0: _Resp(corpo[url.rsplit("/", 1)[1]]))


def test_reinicio_troca_o_dist_pelo_do_ci(repo, monkeypatch):
    """Reiniciar com o disco à frente do processo subia o backend novo servindo a tela velha:
    repo == processo, a tela dizia "em dia", e o navegador seguia no bundle de antes."""
    (repo / "frontend" / "dist").mkdir(parents=True)
    (repo / "frontend" / "dist" / "index.html").write_text("velho", encoding="utf-8")
    _git(repo, "add", "-A"); _git(repo, "commit", "-qm", "dist")
    head = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"],
                          capture_output=True, text=True).stdout.strip()
    _dist_do_ci(repo, monkeypatch, head)
    assert atualizar._atualizar_dist() is None
    assert (repo / "frontend" / "dist" / "index.html").read_text() == "<html>novo</html>"
    assert not list((repo / "frontend").glob(".dist-baixado.*"))


def test_reinicio_com_edicao_local_da_tela_compila_em_vez_de_baixar(repo, monkeypatch):
    """Árvore suja em frontend/packages é quem desenvolve: o dist do CI apagaria o código dela da
    tela, e recusar deixava a máquina de desenvolvimento sem jeito de atualizar a tela pelo app."""
    (repo / "packages" / "core").mkdir(parents=True)
    (repo / "packages" / "core" / "x.ts").write_text("edit", encoding="utf-8")
    chamadas = []
    monkeypatch.setattr(atualizar.shutil, "which", lambda n: "/usr/bin/npm" if n == "npm" else None)
    rodar_real = atualizar._rodar   # `_git` também passa por `_rodar`: só o npm é dublado
    def _build(args, cwd=None, timeout=0, log=None):
        if args[0] != "/usr/bin/npm":
            return rodar_real(args, cwd=cwd, timeout=timeout, log=log)
        chamadas.append(args)
        (repo / "frontend" / "dist").mkdir(parents=True, exist_ok=True)
        (repo / "frontend" / "dist" / "index.html").write_text("local", encoding="utf-8")
        return subprocess.CompletedProcess(args, 0, "", "")
    monkeypatch.setattr(atualizar, "_rodar", _build)
    import urllib.request
    monkeypatch.setattr(urllib.request, "urlopen",
                        lambda *a, **k: (_ for _ in ()).throw(AssertionError("não devia baixar do CI")))
    assert atualizar._atualizar_dist() is None
    assert chamadas == [["/usr/bin/npm", "--prefix", "frontend", "run", "build"]]


def test_reinicio_com_edicao_local_e_build_quebrado_avisa(repo, monkeypatch):
    (repo / "frontend").mkdir(exist_ok=True)
    (repo / "frontend" / "x.ts").write_text("edit", encoding="utf-8")
    monkeypatch.setattr(atualizar.shutil, "which", lambda n: "/usr/bin/npm")
    rodar_real = atualizar._rodar
    monkeypatch.setattr(atualizar, "_rodar",
                        lambda args, cwd=None, timeout=0, log=None: subprocess.CompletedProcess(args, 1, "", "erro X")
                        if args[0] == "/usr/bin/npm" else rodar_real(args, cwd=cwd, timeout=timeout, log=log))
    aviso = atualizar._atualizar_dist()
    assert aviso and "build local" in aviso and "erro X" in aviso


def test_reiniciar_agora_limpa_a_falha_anterior(repo, monkeypatch):
    """Falha de ontem não pode ficar na tela depois de um reinício novo."""
    class P:
        pid = 5
    atualizar._escrever(reinicio_erro="RuntimeError: da vez passada")
    monkeypatch.setattr(atualizar, "_topologia", lambda: "systemd")
    monkeypatch.setattr(atualizar.tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(atualizar.subprocess, "Popen", lambda a, **kw: P())
    atualizar.reiniciar_agora()
    assert atualizar.estado()["reinicio_erro"] is None


def test_reiniciar_agora_recusa_instalacao_manual(repo, monkeypatch):
    """Instalação na mão: não há serviço. Recusa nomeando a topologia — e NÃO lança processo
    nenhum, que é o que um `kill` inventado aqui faria."""
    monkeypatch.setattr(atualizar, "_topologia", lambda: "manual")
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda *a, **kw: (_ for _ in ()).throw(AssertionError("nao devia lancar")))
    assert atualizar.reiniciar_agora() == {"ok": False, "erro": "topologia", "topologia": "manual"}


def test_reiniciar_agora_no_windows_lanca_sem_janela(repo, monkeypatch):
    """Windows tem tarefa agendada, e o `_reiniciar` já sabe reiniciá-la: o botão não pode recusar.
    O motor nasce com os mesmos flags do Atualizar — sem eles um console abre na frente da pessoa."""
    capturado: dict = {}
    class P:
        pid = 3
    monkeypatch.setattr(atualizar, "_topologia", lambda: "windows")
    monkeypatch.setattr(atualizar, "_E_WINDOWS", True)
    monkeypatch.setattr(atualizar.tmux, "_scope_prefix", lambda: [])
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda a, **kw: (capturado.update(kw), P())[1])
    requested = atualizar.reiniciar_agora()
    assert requested["ok"] is True and requested["pid"] == 3
    assert capturado["env"][atualizar._RESTART_REQUEST_ENV] == requested["request_id"]
    assert capturado.get("creationflags") == atualizar._FLAGS_MOTOR_WINDOWS
    assert "start_new_session" not in capturado


def test_lancamento_escolhe_o_modo_do_sistema(repo, monkeypatch):
    """POSIX sai do grupo de processos com `setsid`; Windows usa DETACHED_PROCESS."""
    capturado: dict = {}
    class P:
        # Pid que NAO existe: o lock fica com o pid do filho, e o segundo `iniciar()` deste teste só
        # passa se o dono anterior estiver morto. `1` (init) parecia servir e nao serve — ele e vivo
        # e de outro dono, e `pid_vivo` responde a verdade sobre ele desde 26/08/2026.
        pid = 2 ** 22
    monkeypatch.setattr(atualizar.subprocess, "Popen",
                        lambda *a, **kw: (capturado.update(kw), P())[1])

    # A constante do módulo, NÃO `os.name`: patchar `os.name` leva o `pathlib` junto e o próximo
    # `Path(...) / "x"` estoura com "cannot instantiate 'WindowsPath' on your system".
    monkeypatch.setattr(atualizar, "_E_WINDOWS", False)
    atualizar.iniciar()
    assert capturado.get("start_new_session") is True
    assert "creationflags" not in capturado

    capturado.clear()
    atualizar._escrever(fase="pronto", pid=0)
    monkeypatch.setattr(atualizar, "_E_WINDOWS", True)
    atualizar.iniciar()
    # Valor exato: o DETACHED_PROCESS (0x8) aqui é o que fazia o trampolim do venv abrir uma
    # janela visível na cara de quem apertou Atualizar.
    assert capturado.get("creationflags") == 0x00000200 | 0x08000000
    assert not capturado.get("creationflags") & 0x00000008
    assert "start_new_session" not in capturado


# ─── Branch do Atualizar (CP_UPDATE_BRANCH) ────────────────────────────────────────────────────

def _commit(d, nome, texto):
    (d / nome).write_text(texto, encoding="utf-8")
    _git(d, "add", nome)
    _git(d, "commit", "-qm", nome)


def _rev(d, ref):
    return _git(d, "rev-parse", ref).stdout.strip()


def _branch(d):
    return _git(d, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()


def _branches(d):
    return _git(d, "branch", "--format=%(refname:short)").stdout.split()


@pytest.fixture
def canal(repo, tmp_path, monkeypatch):
    """`repo` com origin de verdade: a main e a branch `teste`, que só existe lá, configurada."""
    origem = tmp_path / "origem.git"
    _git(tmp_path, "clone", "-q", "--bare", str(repo), str(origem))
    _git(repo, "remote", "add", "origin", str(origem))
    _git(repo, "fetch", "-q", "origin")
    _git(repo, "branch", "-q", "--set-upstream-to", "origin/main")
    obra = tmp_path / "obra"
    _git(tmp_path, "clone", "-q", str(origem), str(obra))
    _git(obra, "checkout", "-q", "-b", "teste")
    _commit(obra, "t.txt", "teste\n")
    _git(obra, "push", "-q", "origin", "teste")
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    monkeypatch.setattr(atualizar.config.settings, "update_branch", "teste")
    return obra


def test_branch_configurada_alinha_o_checkout_com_ela(repo, canal):
    final = atualizar.executar()
    assert final["ok"] is True
    assert _branch(repo) == "teste" and _rev(repo, "HEAD") == _rev(canal, "HEAD")
    assert (atualizar._base() / "branch").read_text(encoding="utf-8") == "teste"


def test_branch_que_o_origin_nao_tem_falha_sem_tocar_em_nada(repo, canal, monkeypatch):
    monkeypatch.setattr(atualizar.config.settings, "update_branch", "nao-existe")
    (repo / "a.txt").write_text("trabalho\n", encoding="utf-8")   # com o resgate, viraria stash
    antes = _rev(repo, "HEAD")
    final = atualizar.executar()
    assert final["ok"] is False
    assert "nao existe no origin" in final["erro"] and "nada foi alterado" in final["erro"]
    assert _branch(repo) == "main" and _rev(repo, "HEAD") == antes
    assert (repo / "a.txt").read_text(encoding="utf-8") == "trabalho\n"
    assert _git(repo, "stash", "list").stdout == ""
    assert not [b for b in _branches(repo) if b.startswith("resgate/")]


def test_na_branch_configurada_atualiza_sem_recusa(repo, canal):
    _git(repo, "checkout", "-q", "--track", "origin/teste")
    _commit(canal, "u.txt", "nova\n")
    _git(canal, "push", "-q", "origin", "teste")
    assert atualizar.checar()["branch_de_trabalho"] is False
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == _rev(canal, "HEAD")
    assert not [b for b in _branches(repo) if b.startswith("resgate/")]   # limpo: nada a resgatar


def test_outra_branch_de_trabalho_segue_recusada(repo, canal, monkeypatch):
    _git(repo, "checkout", "-q", "-b", "outra")
    monkeypatch.setattr(atualizar, "resguardar", lambda pre: pytest.fail("nao pode resguardar"))
    assert atualizar.checar()["branch_de_trabalho"] is True
    final = atualizar.executar()
    assert final["ok"] is False and "'outra'" in final["erro"] and "origin/teste" in final["erro"]
    assert _branch(repo) == "outra"


def test_campo_esvaziado_volta_pra_main_com_resgate(repo, canal, monkeypatch):
    assert atualizar.executar()["ok"] is True          # a própria atualização pôs o checkout na teste
    _commit(repo, "local.txt", "commit local\n")
    local = _rev(repo, "HEAD")
    (repo / "t.txt").write_text("editado\n", encoding="utf-8")
    monkeypatch.setattr(atualizar.config.settings, "update_branch", "")
    pre = atualizar.checar()
    assert pre["alvo"] == "main" and pre["branch_de_trabalho"] is False
    final = atualizar.executar()
    assert final["ok"] is True
    assert _branch(repo) == "main" and _rev(repo, "HEAD") == _rev(repo, "origin/main")
    assert _rev(repo, f"refs/heads/{final['resgate']}") == local
    assert "stash@{0}" in _git(repo, "stash", "list").stdout
    assert not (atualizar._base() / "branch").exists()


def test_rollback_da_troca_volta_pra_branch_de_antes(repo, canal, monkeypatch):
    antes = _rev(repo, "HEAD")
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: False)
    final = atualizar.executar()
    assert final["ok"] is False and final["voltou"] is True
    assert _branch(repo) == "main" and _rev(repo, "HEAD") == antes
    assert not (atualizar._base() / "branch").exists()


def test_branch_local_do_alvo_com_commit_proprio_ganha_resgate(repo, canal):
    _git(repo, "checkout", "-q", "-b", "teste")
    _commit(repo, "x.txt", "historia propria\n")
    local = _rev(repo, "HEAD")
    _git(repo, "checkout", "-q", "main")
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == _rev(canal, "HEAD")
    resgates = [b for b in _branches(repo) if b.startswith("resgate/") and b.endswith("-teste")]
    assert resgates and _rev(repo, resgates[0]) == local


def test_fora_da_main_a_tela_e_compilada_e_nao_baixada(repo, monkeypatch):
    """O CI só publica o dist da main: baixá-lo numa branch de teste serviria a tela errada."""
    _git(repo, "checkout", "-q", "-b", "teste")
    chamadas = []
    monkeypatch.setattr(atualizar.shutil, "which", lambda n: "/usr/bin/npm" if n == "npm" else None)
    rodar_real = atualizar._rodar
    def _build(args, cwd=None, timeout=0, log=None):
        if args[0] != "/usr/bin/npm":
            return rodar_real(args, cwd=cwd, timeout=timeout, log=log)
        chamadas.append(args)
        (repo / "frontend" / "dist").mkdir(parents=True, exist_ok=True)
        (repo / "frontend" / "dist" / "index.html").write_text("local", encoding="utf-8")
        return subprocess.CompletedProcess(args, 0, "", "")
    monkeypatch.setattr(atualizar, "_rodar", _build)
    import urllib.request
    monkeypatch.setattr(urllib.request, "urlopen",
                        lambda *a, **k: (_ for _ in ()).throw(AssertionError("não devia baixar do CI")))
    assert atualizar._atualizar_dist() is None
    assert chamadas == [["/usr/bin/npm", "--prefix", "frontend", "run", "build"]]


def test_branch_configurada_sem_upstream_nao_perde_commit_local(repo, canal):
    """Sem upstream o cabeçalho do status dá ahead 0: sem resgate, o reset levava o commit."""
    _git(repo, "checkout", "-q", "--no-track", "-b", "teste", "origin/teste")
    _commit(repo, "local.txt", "commit local\n")
    local = _rev(repo, "HEAD")
    _commit(canal, "u.txt", "nova\n")
    _git(canal, "push", "-q", "origin", "teste")          # divergiu: o ff recusa e vem o reset
    assert atualizar.checar()["ahead"] == 1
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == _rev(canal, "HEAD")
    assert _rev(repo, f"refs/heads/{final['resgate']}") == local


def test_troca_que_falha_depois_diz_onde_o_checkout_ficou(repo, canal, monkeypatch):
    def _quebra():
        raise RuntimeError("passo quebrou")
    monkeypatch.setattr(atualizar, "_aplicar_passos", _quebra)
    final = atualizar.executar()
    assert final["ok"] is False and "passo quebrou" in final["erro"]
    assert "o checkout ficou na branch teste" in final["erro"]


def test_resgate_da_branch_local_aparece_nos_avisos(repo, canal):
    _git(repo, "checkout", "-q", "-b", "teste")
    _commit(repo, "x.txt", "historia propria\n")
    _git(repo, "checkout", "-q", "main")
    final = atualizar.executar()
    assert final["ok"] is True
    assert any("resgate/" in a and "teste" in a for a in final["avisos"])


def test_marca_da_branch_que_falha_nao_derruba_o_motor(repo):
    caminho = atualizar._caminho_canal()
    caminho.mkdir(parents=True)                         # diretório no lugar do arquivo: OSError
    atualizar._marcar_canal("teste")
    atualizar._marcar_canal("main")
    assert atualizar._canal() == ""
    avisos = atualizar.estado()["avisos"]
    assert len(avisos) == 2 and all("nao consegui gravar a branch" in a for a in avisos)


def _build_local(repo, monkeypatch, ok=True):
    _git(repo, "checkout", "-q", "-b", "teste")
    dist = repo / "frontend" / "dist"
    dist.mkdir(parents=True)
    (dist / "index.html").write_text("tela da main", encoding="utf-8")
    monkeypatch.setattr(atualizar.shutil, "which", lambda n: "/usr/bin/npm" if n == "npm" else None)
    rodar_real = atualizar._rodar
    def _build(args, cwd=None, timeout=0, log=None):
        if args[0] != "/usr/bin/npm":
            return rodar_real(args, cwd=cwd, timeout=timeout, log=log)
        (dist / "index.html").write_text("tela da teste", encoding="utf-8")
        return subprocess.CompletedProcess(args, 0 if ok else 1, "", "erro X")
    monkeypatch.setattr(atualizar, "_rodar", _build)
    return dist


def test_build_local_guarda_a_tela_anterior_pro_rollback(repo, monkeypatch):
    dist = _build_local(repo, monkeypatch)
    assert atualizar._atualizar_dist() is None
    assert (dist / "index.html").read_text(encoding="utf-8") == "tela da teste"
    assert (atualizar._DIST_VELHO() / "index.html").read_text(encoding="utf-8") == "tela da main"
    atualizar._dist_velho_voltar()
    assert (dist / "index.html").read_text(encoding="utf-8") == "tela da main"


def test_build_local_que_falha_devolve_a_tela_anterior(repo, monkeypatch):
    dist = _build_local(repo, monkeypatch, ok=False)
    aviso = atualizar._atualizar_dist()
    assert aviso and "erro X" in aviso
    assert (dist / "index.html").read_text(encoding="utf-8") == "tela da main"


# ─── Tela pela chave das fontes (release `dist-commits`) ─────────────────────────────────────────

def _fontes_da_tela(repo):
    for pasta, nome in (("frontend", "a.ts"), ("packages/core", "b.ts"),
                        ("messages", "en.json"), ("project.inlang", "settings.json")):
        (repo / pasta).mkdir(parents=True, exist_ok=True)
        (repo / pasta / nome).write_text(nome, encoding="utf-8")
    for nome in ("package.json", "package-lock.json"):
        (repo / nome).write_text("{}", encoding="utf-8")
    (repo / "frontend" / "dist").mkdir()
    (repo / "frontend" / "dist" / "index.html").write_text("velho", encoding="utf-8")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "fontes")


def _tar_da_tela(html: bytes = b"<html>novo</html>") -> bytes:
    import io
    import tarfile
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        info = tarfile.TarInfo("index.html")
        info.size = len(html)
        tar.addfile(info, io.BytesIO(html))
    return buf.getvalue()


def _npm_dublado(repo, monkeypatch):
    chamadas = []
    monkeypatch.setattr(atualizar.shutil, "which", lambda n: "/usr/bin/npm" if n == "npm" else None)
    rodar_real = atualizar._rodar
    def _build(args, cwd=None, timeout=0, log=None):
        if args[0] != "/usr/bin/npm":
            return rodar_real(args, cwd=cwd, timeout=timeout, log=log)
        chamadas.append(args)
        (repo / "frontend" / "dist" / "index.html").write_text("local", encoding="utf-8")
        return subprocess.CompletedProcess(args, 0, "", "")
    monkeypatch.setattr(atualizar, "_rodar", _build)
    return chamadas


@pytest.fixture
def release_dist_commits(repo, monkeypatch):
    """Servidor HTTP de verdade no lugar da release `dist-commits`: nome do arquivo → bytes."""
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    arquivos: dict[str, bytes] = {}

    class _H(BaseHTTPRequestHandler):
        def do_GET(self):
            corpo = arquivos.get(self.path.rsplit("/", 1)[1])
            if corpo is None:
                self.send_error(404)
                return
            self.send_response(200)
            # `_Cortado`: promete mais do que manda e fecha, como uma conexão que cai no meio.
            self.send_header("Content-Length", str(len(corpo) + (1000 if isinstance(corpo, _Cortado) else 0)))
            self.end_headers()
            self.wfile.write(corpo)
            self.close_connection = True

        def log_message(self, *a):
            pass

    srv = ThreadingHTTPServer(("127.0.0.1", 0), _H)
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    for v in ("http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"):
        monkeypatch.delenv(v, raising=False)
    monkeypatch.setattr(atualizar, "_DIST_COMMITS_URL", f"http://127.0.0.1:{srv.server_port}/dist-commits")
    monkeypatch.setattr(atualizar, "_behind_top", lambda: True)
    _fontes_da_tela(repo)
    yield arquivos
    srv.shutdown()
    srv.server_close()
    t.join(timeout=5)


class _Cortado(bytes):
    pass


def _publicar(arquivos, chave, tar, sha=None):
    import hashlib
    arquivos[f"frontend-dist-{chave}.tar.gz"] = tar
    arquivos[f"frontend-dist-{chave}.tar.gz.sha256"] = \
        f"{sha or hashlib.sha256(tar).hexdigest()}  frontend-dist-{chave}.tar.gz\n".encode()


def _diario(monkeypatch):
    eventos = []
    monkeypatch.setattr(atualizar.diag, "registrar",
                        lambda evento, nivel="ok", **campos: eventos.append((evento, nivel, campos)))
    return eventos


def test_dist_atras_do_topo_baixa_a_tela_da_chave_sem_npm(repo, release_dist_commits, monkeypatch):
    """Atrás do topo o `dist-latest` é de outro commit; a tela do commit alcançado vem pela chave."""
    chamadas = _npm_dublado(repo, monkeypatch)
    _publicar(release_dist_commits, atualizar._chave_da_tela("HEAD"), _tar_da_tela())
    assert atualizar._atualizar_dist() is None
    assert chamadas == []
    assert (repo / "frontend" / "dist" / "index.html").read_text() == "<html>novo</html>"
    assert (atualizar._DIST_VELHO() / "index.html").read_text() == "velho"
    assert not list((repo / "frontend").glob(".dist-baixado.*"))


def test_dist_atras_do_topo_sem_a_chave_publicada_compila_aqui(repo, release_dist_commits, monkeypatch):
    """Chave não publicada é o caso normal (CI cancelado): nem aviso na tela nem no diário."""
    eventos = _diario(monkeypatch)
    chamadas = _npm_dublado(repo, monkeypatch)
    assert atualizar._atualizar_dist() is None
    assert chamadas == [["/usr/bin/npm", "--prefix", "frontend", "run", "build"]]
    assert (repo / "frontend" / "dist" / "index.html").read_text() == "local"
    assert eventos == []


def _falha_da_tela_da_chave(repo, monkeypatch, arquivos, tar, sha=None):
    """Falha que não é 404: compila aqui, registra no diário e não vira aviso da tela."""
    eventos = _diario(monkeypatch)
    chamadas = _npm_dublado(repo, monkeypatch)
    _publicar(arquivos, atualizar._chave_da_tela("HEAD"), tar, sha=sha)
    try:
        resultado = atualizar._atualizar_dist()
    except Exception as e:
        pytest.fail(f"a falha do download derrubou a atualização: {e!r}")
    assert resultado is None
    assert len(chamadas) == 1
    assert (repo / "frontend" / "dist" / "index.html").read_text() == "local"
    assert not atualizar.estado().get("avisos")
    assert [(e, n) for e, n, _ in eventos] == [("atualizacao.tela_da_chave", "aviso")]
    return eventos[0][2]["codigo"]


def test_dist_com_sha256_divergente_e_descartado_e_compila_aqui(repo, release_dist_commits, monkeypatch):
    codigo = _falha_da_tela_da_chave(repo, monkeypatch, release_dist_commits, _tar_da_tela(), sha="0" * 64)
    assert codigo == "sha256_divergente"


def test_dist_com_a_conexao_cortada_no_meio_compila_aqui(repo, release_dist_commits, monkeypatch):
    tar = _tar_da_tela()
    codigo = _falha_da_tela_da_chave(repo, monkeypatch, release_dist_commits, _Cortado(tar),
                                     sha=__import__("hashlib").sha256(tar).hexdigest())
    assert codigo == "download_falhou"


def test_dist_com_o_tar_truncado_compila_aqui(repo, release_dist_commits, monkeypatch):
    tar = _tar_da_tela(b"x" * 50000)[:-30]
    codigo = _falha_da_tela_da_chave(repo, monkeypatch, release_dist_commits, tar)
    assert codigo == "download_falhou"


def test_chave_do_dist_so_muda_com_as_fontes_da_tela(repo):
    _fontes_da_tela(repo)
    antes = atualizar._chave_da_tela("HEAD")
    (repo / "a.txt").write_text("outro\n", encoding="utf-8")
    _git(repo, "commit", "-qam", "fora da tela")
    assert antes and atualizar._chave_da_tela("HEAD") == antes
    (repo / "messages" / "en.json").write_text("mudou", encoding="utf-8")
    _git(repo, "commit", "-qam", "texto")
    assert atualizar._chave_da_tela("HEAD") not in (None, antes)


def test_chave_do_dist_muda_com_o_lock_das_dependencias(repo):
    """Mesmas fontes compiladas com outras dependências dão outra tela."""
    _fontes_da_tela(repo)
    antes = atualizar._chave_da_tela("HEAD")
    (repo / "package-lock.json").write_text('{"lockfileVersion": 3}', encoding="utf-8")
    _git(repo, "commit", "-qam", "lock")
    assert atualizar._chave_da_tela("HEAD") not in (None, antes)


def test_chave_do_dist_e_a_mesma_conta_do_ci(repo):
    """O `publica-dist` do ci.yml e o motor têm de chegar à mesma chave, ou o download nunca acha."""
    import re
    import shutil as sh
    if not (sh.which("bash") and sh.which("sha256sum")):
        pytest.skip("sem bash/sha256sum")
    ci = (atualizar.Path(atualizar.__file__).resolve().parents[2]
          / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    linha = re.search(r"^\s*(chave=\$\(git rev-parse .*\))\s*$", ci, re.M)
    assert linha, "o ci.yml não calcula a chave da tela"
    _fontes_da_tela(repo)
    p = subprocess.run(["bash", "-c", f'c=HEAD; {linha.group(1)}; printf %s "$chave"'],
                       cwd=repo, capture_output=True, text=True)
    assert p.returncode == 0, p.stderr
    assert p.stdout == atualizar._chave_da_tela("HEAD")


def test_preparar_baixa_dist_e_binario_juntos_e_guarda_os_dois_avisos(repo, monkeypatch):
    import threading
    juntos = threading.Barrier(2, timeout=5)   # em fila, o primeiro esperaria o segundo para sempre
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: P())
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    monkeypatch.setattr(atualizar, "_renovar_chromium", lambda: None)
    monkeypatch.setattr(atualizar, "_atualizar_dist", lambda: (juntos.wait(), "tela não atualizada: X")[1])
    monkeypatch.setattr(atualizar.rust_release, "fetch", lambda **kw: (juntos.wait(), ["binários: Y"])[1])
    (repo / "backend").mkdir(exist_ok=True)
    atualizar._preparar("systemd")
    assert atualizar.estado()["avisos"] == ["tela não atualizada: X", "binários: Y"]


def test_preparar_com_dist_que_explode_guarda_o_aviso_do_binario(repo, monkeypatch):
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: P())
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    monkeypatch.setattr(atualizar, "_renovar_chromium", lambda: None)
    def _explode():
        raise RuntimeError("build travou")
    monkeypatch.setattr(atualizar, "_atualizar_dist", _explode)
    monkeypatch.setattr(atualizar.rust_release, "fetch", lambda **kw: ["binários: Y"])
    (repo / "backend").mkdir(exist_ok=True)
    with pytest.raises(RuntimeError, match="build travou"):
        atualizar._preparar("systemd")
    assert atualizar.estado()["avisos"] == ["binários: Y"]


@pytest.mark.parametrize("tela_explode", [True, False])
def test_binario_baixado_junto_so_e_gravado_com_a_tela_no_lugar(repo, monkeypatch, tela_explode):
    """Tela que falha leva à volta do checkout: o binário novo não pode ficar gravado sob o Python velho."""
    class P:
        returncode = 0
        stdout = ""
        stderr = ""
    gravados = []
    def _fetch(release=None, hold=None):
        (gravados if hold is None else hold).append("hangar-server")
        return []
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: P())
    monkeypatch.setattr(atualizar.shutil, "which", lambda nome: f"/bin/{nome}")
    monkeypatch.setattr(atualizar, "_renovar_chromium", lambda: None)
    def _tela():
        if tela_explode:
            raise RuntimeError("build travou")
    monkeypatch.setattr(atualizar, "_atualizar_dist", _tela)
    monkeypatch.setattr(atualizar.rust_release, "fetch", _fetch)
    monkeypatch.setattr(atualizar.rust_release, "install_held", lambda hold: (gravados.extend(hold), [])[1], raising=False)
    (repo / "backend").mkdir(exist_ok=True)
    if tela_explode:
        with pytest.raises(RuntimeError, match="build travou"):
            atualizar._preparar("systemd")
        assert gravados == []
    else:
        atualizar._preparar("systemd")
        assert gravados == ["hangar-server"]


def test_contagem_de_commits_que_falha_resgata_por_precaucao(repo, canal, monkeypatch):
    """Sem a contagem contra o alvo, o 0 do upstream é o caso que perde commit: o motor resgata."""
    monkeypatch.setattr(atualizar.config.settings, "update_branch", "")
    monkeypatch.setattr(atualizar, "_CONTAGEM_FALHOU", set())
    real = atualizar._git
    def _sem_contagem(*args, **kw):
        if args[0] == "rev-list":
            return subprocess.CompletedProcess(args, 128, "fatal: bad revision", "")
        return real(*args, **kw)
    monkeypatch.setattr(atualizar, "_git", _sem_contagem)
    pre = atualizar.checar()
    assert pre["ahead_incerto"] is True and pre["ahead"] == 0
    final = atualizar.executar()
    assert final["ok"] is True and final["resgate"]
    assert _git(repo, "rev-parse", "--verify", f"refs/heads/{final['resgate']}").returncode == 0


def test_copia_da_tela_que_falha_nao_deixa_metade_pro_rollback(repo, monkeypatch):
    dist = _build_local(repo, monkeypatch)
    def _copia_pela_metade(origem, destino):
        destino.mkdir(parents=True)
        (destino / "pedaco.js").write_text("x", encoding="utf-8")
        raise OSError("disco cheio")
    monkeypatch.setattr(atualizar.shutil, "copytree", _copia_pela_metade)
    aviso = atualizar._atualizar_dist()
    assert aviso and "disco cheio" in aviso
    assert not atualizar._DIST_VELHO().exists()
    assert (dist / "index.html").read_text(encoding="utf-8") == "tela da main"


def test_tela_anterior_que_nao_volta_aparece_no_aviso(repo, monkeypatch):
    _build_local(repo, monkeypatch, ok=False)
    def _nao_volta():
        raise OSError("arquivo preso")
    monkeypatch.setattr(atualizar, "_dist_velho_voltar", _nao_volta)
    aviso = atualizar._atualizar_dist()
    assert "erro X" in aviso and "a tela anterior não voltou: arquivo preso" in aviso


def test_build_que_estoura_o_prazo_devolve_a_tela_anterior(repo, monkeypatch):
    dist = _build_local(repo, monkeypatch)
    rodar_real = atualizar._rodar
    def _estoura(args, cwd=None, timeout=0, log=None):
        if args[0] != "/usr/bin/npm":
            return rodar_real(args, cwd=cwd, timeout=timeout, log=log)
        (dist / "index.html").write_text("meia tela", encoding="utf-8")
        raise subprocess.TimeoutExpired(args, timeout)
    monkeypatch.setattr(atualizar, "_rodar", _estoura)
    with pytest.raises(subprocess.TimeoutExpired):
        atualizar._atualizar_dist()
    assert (dist / "index.html").read_text(encoding="utf-8") == "tela da main"


# ─── Só até o commit cujo binário do Rust foi publicado ─────────────────────────────────────────

def _protocolo(d, n):
    (d / "backend" / "app").mkdir(parents=True, exist_ok=True)
    _commit(d, "backend/app/rust_server.py", f"X = 1\nRUST_SERVER_PROTOCOL = {n}\nY = 2\n")


@pytest.fixture
def rust(repo, tmp_path, monkeypatch):
    """Origin com a main em 35 → 35 (sem tocar no número) → 36; a release diz o commit do binário."""
    origem = tmp_path / "origem.git"
    _git(tmp_path, "clone", "-q", "--bare", str(repo), str(origem))
    _git(repo, "remote", "add", "origin", str(origem))
    _git(repo, "fetch", "-q", "origin")
    _git(repo, "branch", "-q", "--set-upstream-to", "origin/main")
    obra = tmp_path / "obra-rust"
    _git(tmp_path, "clone", "-q", str(origem), str(obra))
    _protocolo(obra, 35)
    binario_35 = _rev(obra, "HEAD")
    _commit(obra, "b.txt", "sem mexer no contrato\n")
    ultimo_35 = _rev(obra, "HEAD")
    _protocolo(obra, 36)
    _git(obra, "push", "-q", "origin", "main")
    pedidos, preparo = [], {}
    manifesto = {"commit": binario_35, "files": {
        f"linux-x86_64/{n}": {"name": f"{n}-linux-x86_64", "sha256": "0" * 64} for n in ("hangar-server", "hangar-cano")}}
    monkeypatch.setattr(atualizar.config.settings, "rust_server", True)
    monkeypatch.setattr(atualizar.rust_release, "platform_key", lambda: "linux-x86_64")
    monkeypatch.delenv("HANGAR_SERVER_RELEASE_URL", raising=False)
    monkeypatch.setattr(atualizar.rust_release, "read_manifest",
                        lambda url: (pedidos.append(url), manifesto)[1])
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda t, **k: preparo.update(k))
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t, *a, **k: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    return {"obra": obra, "manifesto": manifesto, "pedidos": pedidos, "preparo": preparo,
            "binario_35": binario_35, "ultimo_35": ultimo_35, "topo": _rev(obra, "HEAD")}


def test_topo_sem_binario_para_no_commit_do_binario_publicado(repo, rust):
    # Nem o commit seguinte de mesmo contrato: o código dele rodaria com o binário de antes.
    final = atualizar.executar()
    assert final["ok"] is True
    assert _rev(repo, "HEAD") == rust["binario_35"] != rust["ultimo_35"]
    # Build pendente no CI não é aviso: quem atualiza não tem o que fazer com ele.
    assert final["avisos"] == [] and final["waiting_for_server_build"] is False
    assert rust["pedidos"] == [f"{atualizar.rust_release.RELEASES_URL}/server-latest"]
    # O download usa o manifesto da escolha, e a tela (também no Reiniciar) é compilada aqui.
    assert rust["preparo"]["release"][2] is rust["manifesto"]
    assert atualizar._behind_top() is True


def _workflow_do_server(obra):
    """O `server.yml` só compila o binário quando o push muda um destes caminhos."""
    (obra / ".github" / "workflows").mkdir(parents=True, exist_ok=True)
    _commit(obra, ".github/workflows/server.yml",
            'on:\n  push:\n    branches: ["**"]\n    paths:\n      - "crates/**"\n'
            '      # o contrato interno\n      - "backend/app/rust_server.py"\n  workflow_dispatch:\n')


def test_commit_que_muda_o_rust_espera_o_binario_dele(repo, rust):
    _workflow_do_server(rust["obra"])
    _git(rust["obra"], "push", "-q", "origin", "main")
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["ultimo_35"]


def test_commit_so_de_python_depois_do_binario_chega(repo, rust):
    rust["manifesto"]["commit"] = rust["topo"]
    _workflow_do_server(rust["obra"])
    _commit(rust["obra"], "py.txt", "só Python\n")
    _git(rust["obra"], "push", "-q", "origin", "main")
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == _rev(rust["obra"], "HEAD") != rust["topo"]


@pytest.mark.parametrize("build_change", ["none", "pending", "reverted"])
def test_main_recebe_branch_que_mesclou_o_build_publicado(repo, rust, build_change):
    obra = rust["obra"]
    _workflow_do_server(obra)
    assert _git(obra, "branch", "mobile-lote").returncode == 0
    (obra / "crates").mkdir()
    _commit(obra, "crates/server.rs", "published\n")
    rust["manifesto"]["commit"] = _rev(obra, "HEAD")
    assert _git(obra, "checkout", "mobile-lote").returncode == 0
    _commit(obra, "mobile.txt", "mobile\n")
    assert _git(obra, "merge", "--no-ff", "main", "-m", "Merge main into mobile-lote").returncode == 0
    _commit(obra, "py.txt", "Python depois do merge\n")
    expected = _rev(obra, "HEAD")
    if build_change != "none":
        _commit(obra, "crates/server.rs", "unpublished\n")
        _commit(obra, "py.txt", "Python depois do Rust\n")
        if build_change == "reverted":
            _commit(obra, "crates/server.rs", "published\n")
            expected = _rev(obra, "HEAD")
    assert _git(obra, "push", "-q", "origin", "HEAD:main").returncode == 0

    final = atualizar.executar()

    assert final["ok"] is True
    assert _rev(repo, "HEAD") == expected
    assert final["waiting_for_server_build"] is False and final["avisos"] == []


def test_build_deste_sistema_atrasado_no_manifesto_por_sistema(repo, rust):
    """O topo publicado para outro sistema não leva este: vale a entrada dele em `platforms`."""
    rust["manifesto"]["commit"] = rust["topo"]
    rust["manifesto"]["platforms"] = {
        "linux-x86_64": {"commit": rust["binario_35"], "protocol": 35},
        "windows-x86_64": {"commit": rust["topo"], "protocol": 36}}
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["binario_35"]


@pytest.mark.parametrize("platforms", [{"windows-x86_64": {"protocol": 36}}, None, []])
def test_manifesto_por_sistema_sem_este_sistema_nao_confere(repo, rust, platforms):
    rust["manifesto"]["platforms"] = platforms
    final = atualizar.executar()
    assert _rev(repo, "HEAD") == rust["topo"]
    assert final["avisos"][0].startswith("não consegui conferir o binário do Rust")


def test_topo_com_binario_avanca_ate_o_topo(repo, rust):
    rust["manifesto"]["commit"] = rust["topo"]
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["topo"]
    assert final["avisos"] == [] and atualizar._behind_top() is False


def _http(code):
    def _falha(url):
        raise atualizar.urllib.error.HTTPError(url, code, "x", {}, None)
    return _falha


@pytest.mark.parametrize("caso", ["sem_rust", "branch_sem_release"])
def test_sem_binario_deste_sistema_vai_ao_topo_como_antes(repo, rust, monkeypatch, caso):
    if caso == "sem_rust":
        monkeypatch.setattr(atualizar.config.settings, "rust_server", False)
    else:
        _git(rust["obra"], "push", "-q", "origin", "main:teste")
        monkeypatch.setattr(atualizar.config.settings, "update_branch", "teste")
        monkeypatch.setattr(atualizar.rust_release, "read_manifest", _http(404))
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["topo"] and final["avisos"] == []


@pytest.mark.parametrize("caso", ["rede", "http_500", "commit_torto", "commit_ausente"])
def test_sem_como_conferir_vai_ao_topo_e_diz(repo, rust, monkeypatch, caso):
    if caso == "rede":
        monkeypatch.setattr(atualizar.rust_release, "read_manifest",
                            lambda url: (_ for _ in ()).throw(OSError("rede fora")))
    elif caso == "http_500":
        monkeypatch.setattr(atualizar.rust_release, "read_manifest", _http(500))
    elif caso == "commit_torto":
        rust["manifesto"]["commit"] = "--output=/tmp/x"
    else:
        rust["manifesto"]["commit"] = "f" * 40
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["topo"]
    assert final["avisos"][0].startswith("não consegui conferir o binário do Rust")
    assert atualizar.pinned_target("main")[0] is None


@pytest.mark.parametrize("caso", ["manifesto_da_main_regravando", "sem_build_deste_sistema", "rede", "commit_ausente"])
def test_sem_binario_conferido_nao_troca_o_contrato_do_checkout(repo, rust, monkeypatch, caso):
    """Checkout no 35 com o binário 35 instalado, topo no 36 ainda sem build conferido: o código
    só avança no 35, e o auto-update espera."""
    _git(repo, "fetch", "-q", "origin")
    _git(repo, "merge", "-q", "--ff-only", rust["binario_35"])
    if caso == "manifesto_da_main_regravando":   # o `--clobber` do server.yml apaga e sobe o arquivo
        monkeypatch.setattr(atualizar.rust_release, "read_manifest", _http(404))
    elif caso == "sem_build_deste_sistema":
        rust["manifesto"]["files"] = {k.replace("linux-x86_64", "windows-x86_64"): v
                                      for k, v in rust["manifesto"]["files"].items()}
    elif caso == "rede":
        monkeypatch.setattr(atualizar.rust_release, "read_manifest",
                            lambda url: (_ for _ in ()).throw(OSError("rede fora")))
    else:
        rust["manifesto"]["commit"] = "f" * 40
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["ultimo_35"]
    assert final["avisos"] == ["não consegui conferir o binário do Rust publicado para este sistema; o código "
                               "não troca de contrato com ele até conferir"]
    assert atualizar.pinned_target("main")[0] is None


def test_sem_binario_conferido_e_nada_no_contrato_do_checkout_fica_onde_esta(repo, rust, monkeypatch):
    _git(repo, "fetch", "-q", "origin")
    _git(repo, "merge", "-q", "--ff-only", rust["ultimo_35"])
    monkeypatch.setattr(atualizar.rust_release, "read_manifest", _http(500))
    monkeypatch.setattr(atualizar, "_newest_with_protocol", lambda topo, want: None)
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["ultimo_35"]


def test_ja_no_topo_com_binario_de_outro_contrato_nao_recua_e_avisa(repo, rust):
    """O caso que motivou a regra: a máquina já foi ao topo antes dela existir."""
    _git(repo, "fetch", "-q", "origin")
    _git(repo, "merge", "-q", "--ff-only", "origin/main")
    final = atualizar.executar()
    assert final["ok"] is True and _rev(repo, "HEAD") == rust["topo"]
    assert final["avisos"] == [f"o binário do Rust publicado para este sistema (commit {rust['binario_35'][:8]}) "
                               "fala outro contrato que este código; o Python atende sozinho até sair o binário novo"]


def test_commit_local_adiante_do_topo_nao_e_atras_do_topo(repo, rust):
    rust["manifesto"]["commit"] = rust["topo"]
    _git(repo, "fetch", "-q", "origin")
    _git(repo, "merge", "-q", "--ff-only", "origin/main")
    _commit(repo, "local.txt", "meu\n")
    final = atualizar.executar()
    assert final["ok"] is True and final["avisos"] == [] and atualizar._behind_top() is False


def test_branch_de_teste_nova_nasce_no_commit_escolhido_com_upstream(repo, rust, monkeypatch):
    obra = rust["obra"]
    _git(obra, "checkout", "-q", "-b", "teste")
    _workflow_do_server(obra)
    _commit(obra, "t.txt", "teste\n")
    teste_36 = _rev(obra, "HEAD")
    _protocolo(obra, 37)
    _git(obra, "push", "-q", "origin", "teste")
    rust["manifesto"]["commit"] = rust["topo"]       # binário da branch: protocolo 36
    monkeypatch.setattr(atualizar.config.settings, "update_branch", "teste")
    final = atualizar.executar()
    assert final["ok"] is True
    assert _branch(repo) == "teste" and _rev(repo, "HEAD") == teste_36
    assert _git(repo, "rev-parse", "--abbrev-ref", "teste@{upstream}").stdout.strip() == "origin/teste"
    assert rust["pedidos"] == [f"{atualizar.rust_release.RELEASES_URL}/server-teste"]


def test_merge_que_muda_o_contrato_conta_pelo_primeiro_pai(repo, rust):
    obra = rust["obra"]
    _git(obra, "checkout", "-q", "-b", "lado", rust["ultimo_35"])
    _protocolo(obra, 37)
    _git(obra, "checkout", "-q", "main")
    _git(obra, "merge", "-q", "--no-ff", "-m", "junta", "lado", "-X", "theirs")
    _git(obra, "push", "-q", "origin", "main")
    rust["manifesto"]["commit"] = rust["topo"]       # o 36, antes do merge
    final = atualizar.executar()
    assert _rev(repo, "HEAD") == rust["topo"] and final["avisos"] == []



@pytest.mark.parametrize("marker", [None, '{"sem_build": true}', '{"version": "0.1.0.1"}'])
@pytest.mark.parametrize("stage", ["reiniciar", "nativo"])
def test_motor_morto_pelo_proprio_reinicio_conclui_quando_o_servidor_e_o_novo(repo, stage, marker, tmp_path, monkeypatch):
    """Sem escopo próprio do systemd (o gerenciador do usuário recusa unidade transiente), o motor
    nasce no cgroup do backend e o restart que ele dispara o mata antes de gravar "pronto". Quem
    responde depois é o backend novo, com o disco no commit da atualização: isso é sucesso."""
    import os as _os
    home = tmp_path / "home"
    monkeypatch.setattr(atualizar.Path, "home", lambda: home)
    if marker:
        (home / ".hangar" / "native").mkdir(parents=True)
        (home / ".hangar" / "native" / "release.json").write_text(marker, encoding="utf-8")
    head = atualizar._git("rev-parse", "HEAD").stdout.strip()
    atualizar._escrever(fase="rodando", etapa=stage, passo=5, total=6, pid=_pid_morto(),
                        backend_pid=_os.getpid() + 1, commit_para=head, avisos=["aviso anterior"])
    d = atualizar.estado_para_tela()
    assert (d["fase"], d["ok"]) == ("pronto", True)
    native_warning = ([] if marker is None or "sem_build" in marker else [
        "app nativo nao atualizou: a atualizacao foi concluida pelo servidor novo" if stage == "reiniciar"
        else "app nativo nao atualizou: o processo que o atualizava nao existe mais"])
    assert d["avisos"] == ["aviso anterior", *native_warning]
    assert atualizar.estado_para_tela()["avisos"] == d["avisos"]


@pytest.mark.parametrize("stage", ["reiniciar", "nativo"])
def test_motor_morto_no_reinicio_sem_servidor_novo_ou_commit_novo_e_falha(repo, stage):
    import os as _os
    head = atualizar._git("rev-parse", "HEAD").stdout.strip()
    # O backend que responde é o mesmo de antes: o restart não aconteceu.
    atualizar._escrever(fase="rodando", etapa=stage, passo=5, total=6, pid=_pid_morto(),
                        backend_pid=_os.getpid(), commit_para=head)
    assert atualizar.estado_para_tela()["ok"] is False
    # Servidor novo, mas o disco não está no commit da atualização.
    atualizar._escrever(fase="rodando", etapa=stage, passo=5, total=6, pid=_pid_morto(),
                        backend_pid=_os.getpid() + 1, commit_para="0" * 40, ok=None, erro=None)
    assert atualizar.estado_para_tela()["ok"] is False


def test_reinicio_pede_o_frontend_antes_do_backend(monkeypatch):
    # O reinício do backend leva junto o motor e o `systemctl`: o frontend tem de ter sido pedido antes.
    pedidos = []
    monkeypatch.setattr(atualizar, "_rodar", lambda args, **kw: pedidos.append(args)
                        or subprocess.CompletedProcess(args, 0, "", ""))
    atualizar._reiniciar("systemd")
    assert pedidos[-1][-2:] == ["hangar-frontend.service", "hangar-backend.service"]


def test_motor_rodado_a_mao_nao_herda_o_backend_de_outra_rodada(monkeypatch):
    monkeypatch.delenv(atualizar._BACKEND_PID_ENV, raising=False)
    assert atualizar._launching_backend() is None
    monkeypatch.setenv(atualizar._BACKEND_PID_ENV, "4321")
    assert atualizar._launching_backend() == 4321


@pytest.fixture
def native_machine(repo, tmp_path, monkeypatch):
    import io
    import urllib.request

    home = tmp_path / "home"
    monkeypatch.setattr(atualizar.Path, "home", lambda: home)
    marker = home / ".hangar" / "native" / "release.json"
    marker.parent.mkdir(parents=True)
    marker.write_text('{"version": "0.1.0.1"}', encoding="utf-8")
    monkeypatch.setenv("HANGAR_NATIVE_UPDATE_URL", "https://native.example/release")
    monkeypatch.setattr(atualizar.config.settings, "auth_token", "local-owner")
    data = {"marker": marker, "manifest": {"version": "0.1.0.2", "requires_api": 2},
            "api": {"api": 2}, "requests": [], "commands": [], "events": [], "failure": None}

    def urlopen(request, **kwargs):
        url = request.full_url if isinstance(request, urllib.request.Request) else request
        data["requests"].append(url)
        if url.startswith("http://127.0.0.1:"):
            assert url == "http://127.0.0.1:9876/api/client-api"
            assert request.get_header("Authorization") == "Bearer local-owner"
            payload = data["api"]
        else:
            assert url == "https://native.example/release/native-latest.json"
            payload = data["manifest"]
        if isinstance(payload, Exception):
            raise payload
        return io.BytesIO(payload if isinstance(payload, bytes) else json.dumps(payload).encode())

    real_run = atualizar._rodar
    def run(args, **kwargs):
        if args[0] == "git":
            return real_run(args, **kwargs)
        data["commands"].append((args, kwargs))
        data["events"].append("nativo")
        if data["failure"] == "timeout":
            raise subprocess.TimeoutExpired(args, 900)
        if data["failure"] == "launch":
            raise OSError("powershell ausente")
        if data["failure"] == "unexpected":
            raise RuntimeError("fixture-unexpected")
        if data["failure"] == "exit":
            return subprocess.CompletedProcess(args, 1, "##HANGAR-FALHA## arquivo preso\nfim", "")
        return subprocess.CompletedProcess(args, 0, "instalado", "")

    monkeypatch.setattr(urllib.request, "urlopen", urlopen)
    monkeypatch.setattr(atualizar, "_rodar", run)
    return data


@pytest.mark.parametrize("marker", [None, '{"sem_build": true}', '[]', '{quebrado'])
def test_native_install_requires_installed_marker(repo, native_machine, marker):
    path = native_machine["marker"]
    if marker is None:
        path.unlink()
    else:
        path.write_text(marker, encoding="utf-8")
    atualizar._instalar_nativo("main", 9876)
    assert native_machine["commands"] == [] and native_machine["requests"] == []
    assert not atualizar.estado().get("avisos")


def test_native_install_skips_beta(repo, native_machine):
    atualizar._instalar_nativo("beta", 9876)
    assert native_machine["commands"] == [] and native_machine["requests"] == []
    assert not atualizar.estado().get("avisos")


@pytest.mark.parametrize("windows", [False, True])
@pytest.mark.parametrize("required,api", [(2, 2), (2, 3), (None, 1)])
def test_native_install_uses_platform_command_with_compatible_api(repo, native_machine, monkeypatch,
                                                                windows, required, api):
    monkeypatch.setattr(atualizar, "_E_WINDOWS", windows)
    if required is None:
        native_machine["manifest"].pop("requires_api")
    else:
        native_machine["manifest"]["requires_api"] = required
    native_machine["api"] = {"api": api}
    atualizar._instalar_nativo("main", 9876)
    expected = (["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
                 str(repo / "scripts" / "install-native.ps1")] if windows
                else [str(repo / "scripts" / "install-native.sh")])
    assert native_machine["commands"] == [(expected, {"cwd": repo, "timeout": 900})]
    assert not atualizar.estado().get("avisos")


@pytest.mark.parametrize("api", [1, None, "2", True, 0])
def test_native_install_skips_incompatible_or_unknown_api(repo, native_machine, monkeypatch, api):
    native_machine["api"] = {"api": api}
    records = []
    monkeypatch.setattr(atualizar.diag, "registrar", lambda *args, **kw: records.append((args, kw)))
    atualizar._instalar_nativo("main", 9876)
    assert native_machine["commands"] == [] and records
    assert not atualizar.estado().get("avisos")


@pytest.mark.parametrize("source", ["api", "manifest"])
@pytest.mark.parametrize("failure", ["503", "unreadable", "not-object"])
def test_native_install_unreadable_contract_only_records_diary(repo, native_machine, monkeypatch,
                                                              source, failure):
    import urllib.error
    native_machine[source] = (urllib.error.HTTPError("http://local", 503, "indisponivel", {}, None)
                              if failure == "503" else b"{quebrado" if failure == "unreadable" else [])
    records = []
    monkeypatch.setattr(atualizar.diag, "registrar", lambda *args, **kw: records.append((args, kw)))
    atualizar._instalar_nativo("main", 9876)
    assert native_machine["commands"] == [] and records
    assert not atualizar.estado().get("avisos")


@pytest.mark.parametrize("required", [None, "2", True, 0])
def test_native_install_rejects_invalid_required_api(repo, native_machine, required):
    native_machine["manifest"]["requires_api"] = required
    atualizar._instalar_nativo("main", 9876)
    assert native_machine["commands"] == [] and not atualizar.estado().get("avisos")


def test_native_install_old_server_404_means_api_one(repo, native_machine):
    import urllib.error
    native_machine["manifest"].pop("requires_api")
    native_machine["api"] = urllib.error.HTTPError("http://local", 404, "antigo", {}, None)
    atualizar._instalar_nativo("main", 9876)
    assert len(native_machine["commands"]) == 1


@pytest.mark.parametrize("topology", ["systemd", "windows", "manual"])
@pytest.mark.parametrize("failure", [None, "exit", "timeout", "launch", "unexpected"])
def test_native_install_after_restart_keeps_update_success(repo, native_machine, monkeypatch,
                                                          topology, failure):
    data = native_machine
    data["failure"] = failure
    monkeypatch.setattr(atualizar, "_E_WINDOWS", topology == "windows")
    monkeypatch.setattr(atualizar, "checar", lambda: {"pode": True, "branch": "main", "alvo": "main",
                                                     "commit": _rev(repo, "HEAD"), "topologia": topology})
    monkeypatch.setattr(atualizar, "resguardar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_shell_mudou", lambda *args: False)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_preparar", lambda *args, **kwargs: None)
    def restart(*args):
        data["events"].append("reiniciar")
        if topology == "manual":
            atualizar._escrever(reiniciar_manual=True)
    monkeypatch.setattr(atualizar, "_reiniciar", restart)
    monkeypatch.setattr(atualizar, "_subiu", lambda *args: data["events"].append("http") or True)
    pids = iter([100, 200])
    monkeypatch.setattr(atualizar, "_pid_do_servidor",
                        lambda *args: data["events"].append("pid") or next(pids))
    def clean_dist():
        assert atualizar.estado()["etapa"] == "nativo"
        data["events"].append("limpar_dist")
    monkeypatch.setattr(atualizar, "_dist_velho_apagar", clean_dist)
    final = atualizar.executar(9876)
    assert final["fase"] == "pronto" and final["ok"] is True and final["erro"] is None
    assert final["etapa"] == "nativo" and (final["passo"], final["total"]) == (6, 6)
    assert final["reiniciar_manual"] is (topology == "manual")
    expected = (["pid", "reiniciar"] if topology == "manual"
                else ["pid", "reiniciar", "http", "pid"])
    assert data["events"] == expected + ["nativo", "limpar_dist"]
    assert len(final["avisos"]) == (1 if failure else 0)
    if failure:
        assert final["avisos"][0].startswith("app nativo nao atualizou: ")
        if failure == "exit":
            assert final["avisos"] == ["app nativo nao atualizou: arquivo preso"]
        elif failure == "timeout":
            assert "900" in final["avisos"][0]
        elif failure == "unexpected":
            assert final["avisos"] == ["app nativo nao atualizou: fixture-unexpected"]
        else:
            assert "powershell ausente" in final["avisos"][0]
