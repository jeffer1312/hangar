"""Motor da atualização: pré-voo, resgate, ordem das etapas, estado.

Todo teste roda contra um repo git DE VERDADE criado em `tmp_path` — o módulo conversa com o git
por subprocess, e um mock de `subprocess.run` testaria o mock, não o comportamento que importa
(saber distinguir arquivo rastreado de solto, saber que uma branch de resgate ficou mesmo criada).
"""
import json
import subprocess
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
    # Sem escopo systemd nos testes: `_scope_prefix` sonda o systemd com um `subprocess.run` de
    # verdade, e quem troca o `Popen` por um dublê acaba interceptando a sonda em vez do
    # lançamento. O que o prefixo faz está coberto por `test_lancamento_usa_escopo_systemd`.
    monkeypatch.setattr(atualizar.tmux, "_scope_prefix", lambda: [])
    # Nos testes o processo FAZ o papel do motor. Em produção esta chave só liga dentro de
    # `executar()`, pra o backend (que roda os mesmos `_git` no endpoint) não escrever no log e
    # sobrescrever o que o motor gravou.
    monkeypatch.setattr(atualizar, "_SOU_O_MOTOR", True)
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
    def _instalar_quebra(topologia):
        chamou.append("instalar")
        raise RuntimeError("build quebrou")
    monkeypatch.setattr(atualizar, "_reaplicar", _instalar_quebra)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t: chamou.append("reiniciar"))

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


def test_backend_que_nao_sobe_volta_pro_commit_anterior(repo, monkeypatch):
    antes = _git(repo, "rev-parse", "HEAD").stdout.strip()

    def _puxar_avanca(pre):
        (repo / "c.txt").write_text("versao nova\n", encoding="utf-8")
        _git(repo, "add", "c.txt")
        _git(repo, "commit", "-m", "versao nova")

    monkeypatch.setattr(atualizar, "_puxar", _puxar_avanca)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t: None)
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: False)

    final = atualizar.executar()
    assert _git(repo, "rev-parse", "HEAD").stdout.strip() == antes
    assert final["ok"] is False and "nao respondeu" in final["erro"]


def test_pronto_marca_ok(repo, monkeypatch):
    for nome in ("_puxar", "_aplicar_passos", "_reaplicar", "_reiniciar"):
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
    trava.write_text("999999", encoding="utf-8")
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

def test_avisa_as_sessoes_antes_de_reiniciar(repo, monkeypatch):
    ordem = []
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: None)
    monkeypatch.setattr(atualizar, "_avisar_sessoes", lambda: ordem.append("avisou"))
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t: ordem.append("reiniciou"))
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    atualizar.executar()
    assert ordem == ["avisou", "reiniciou"]


def test_hangar_send_ausente_nao_derruba_a_atualizacao(repo, monkeypatch):
    """Aviso é cortesia: máquina sem `hangar-send` não pode ficar sem atualizar por causa dele."""
    def _sem_binario(*a, **kw):
        raise OSError("hangar-send: not found")
    monkeypatch.setattr(atualizar, "_rodar", _sem_binario)
    atualizar._avisar_sessoes()   # não levanta


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


def test_sem_marca_nao_inventa_aviso(repo, monkeypatch):
    class P:
        returncode = 0
        stdout = "tudo certo por aqui\n"
        stderr = ""
    monkeypatch.setattr(atualizar, "_rodar", lambda *a, **kw: P())
    atualizar._reaplicar("systemd")
    assert not atualizar.estado().get("avisos")


def test_avisa_as_sessoes_antes_do_instalador(repo, monkeypatch):
    """No Windows quem reinicia é o próprio installer — avisar depois dele é avisar tarde."""
    ordem = []
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_avisar_sessoes", lambda: ordem.append("avisou"))
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: ordem.append("instalou"))
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t: ordem.append("reiniciou"))
    monkeypatch.setattr(atualizar, "_subiu", lambda porta, teto=0: True)
    atualizar.executar()
    assert ordem == ["avisou", "instalou", "reiniciou"]


def test_windows_nao_pede_reinicio_manual(repo):
    """No Windows o restart já aconteceu na etapa anterior, dentro do `install.ps1 -Update`.

    Ele derruba a instância velha e chama `Start-ScheduledTask` (bloco que o modo `-Update` NÃO
    pula), e ainda há o `hangar-vigia` de rede. Marcar "falta reiniciar" aqui fazia a tela pedir um
    passo que já tinha sido dado — medido na máquina Windows em 25/08/2026.
    """
    atualizar._reiniciar("windows")
    assert not atualizar.estado().get("reiniciar_manual")


def test_instalacao_na_mao_tambem_pede_reinicio(repo):
    atualizar._reiniciar("manual")
    assert atualizar.estado()["reiniciar_manual"] is True


def test_sem_restart_nao_cobra_prova_de_vida(repo, monkeypatch):
    """O backend velho continua respondendo: um `_subiu` verde ali não provaria nada."""
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: None)
    monkeypatch.setattr(atualizar, "_avisar_sessoes", lambda: None)
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t: atualizar._escrever(reiniciar_manual=True))
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
    atualizar._escrever(fase="rodando", passo=4, total=5, pid=999999)
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
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: pytest.fail("nao pode reinstalar"))
    monkeypatch.setattr(atualizar, "_reiniciar", lambda t: pytest.fail("nao pode reiniciar"))

    final = atualizar._voltar("abc123", "o servidor caiu", "systemd", 1)
    assert final["ok"] is False and final["voltou"] is False
    assert final["no_ar"] is False and "nao consegui voltar" in final["erro"]


def test_restart_que_falha_vai_pro_rollback(repo, monkeypatch):
    """`systemctl restart` já derrubou o processo antigo: isto NÃO é "está tudo como estava"."""
    monkeypatch.setattr(atualizar, "_puxar", lambda pre: None)
    monkeypatch.setattr(atualizar, "_aplicar_passos", lambda: None)
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: None)
    monkeypatch.setattr(atualizar, "_avisar_sessoes", lambda: None)
    def _quebra(t):
        raise RuntimeError("systemctl nao subiu")
    monkeypatch.setattr(atualizar, "_reiniciar", _quebra)
    voltou = []
    monkeypatch.setattr(atualizar, "_voltar",
                        lambda c, m, t, p: (voltou.append(m), {"ok": False})[1])
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


def test_rollback_que_nao_sobe_nao_diz_que_esta_no_ar(repo, monkeypatch):
    """O restart que motivou o rollback já matou o processo antigo; se o do rollback também falha,
    não há nada rodando — dizer "no ar" manda a pessoa embora de um servidor morto."""
    monkeypatch.setattr(atualizar, "_reaplicar", lambda t: None)
    def _quebra(t):
        raise RuntimeError("nao subiu nem na volta")
    monkeypatch.setattr(atualizar, "_reiniciar", _quebra)
    final = atualizar._voltar(atualizar._git("rev-parse", "HEAD").stdout.strip(),
                              "o servidor caiu", "systemd", 1)
    assert final["ok"] is False and final["no_ar"] is False


def test_lock_de_processo_morto_nao_trava_a_maquina(repo):
    trava = atualizar._base() / "rodando.lock"
    trava.parent.mkdir(parents=True, exist_ok=True)
    trava.write_text("999999", encoding="utf-8")     # pid que não existe
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


def test_lancamento_escolhe_o_modo_do_sistema(repo, monkeypatch):
    """POSIX sai do grupo de processos com `setsid`; Windows usa DETACHED_PROCESS."""
    capturado: dict = {}
    class P:
        pid = 1
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
    assert capturado.get("creationflags")
    assert "start_new_session" not in capturado
