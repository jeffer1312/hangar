"""Instalar um harness pelo botão (app/harness_install.py)."""
import asyncio
import subprocess

import pytest

from app import harness_install as hi
from app import harness_saude


def _rodar(inst, cli):
    """A instalação inteira, direto — o que a thread executaria, sem thread nem loop no caminho."""
    inst._estado = inst._zerado(fase="rodando", harness=cli, etapa=hi.ETAPAS[0], passo=1)
    inst._executar(cli, hi._COMANDOS[cli])
    return inst.status()


@pytest.fixture
def inst(monkeypatch):
    i = hi.Instalador()
    monkeypatch.setattr(hi, "_resolver", lambda argv: argv)
    # A releitura do wrapper depois do instalador olha o HOME REAL: sem fixar, o resultado do teste
    # dependeria de a máquina de quem roda ter os wrappers instalados. Quem exercita esse ramo
    # sobrescreve com o valor que quer.
    monkeypatch.setattr(hi.harness_saude, "_wrapper", lambda cli: {"ok": True, "params": {}})
    return i


def test_comando_que_falha_para_ali_e_guarda_a_saida(inst, monkeypatch):
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda argv, cwd=None, timeout=0, log=None: (
        log("npm ERR! 404 Not Found") or subprocess.CompletedProcess(argv, 1, "", "")))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar",
                        lambda: pytest.fail("não pode conferir depois de o comando falhar"))
    e = _rodar(inst, "codex")
    assert e["fase"] == "pronto" and e["ok"] is False
    assert e["etapa"] == "comando" and "saiu com 1" in e["erro"]
    assert "npm ERR! 404 Not Found" in e["log"]


def test_rc_zero_nao_prova_instalacao(inst, monkeypatch):
    """A regra que o desenho pediu: quem diz se instalou é o disco, não o `rc` do comando."""
    monkeypatch.setattr(hi.atualizar, "_rodar",
                        lambda argv, cwd=None, timeout=0, log=None: subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar",
                        lambda: [{"id": "codex", "instalado": False, "itens": []}])
    e = _rodar(inst, "codex")
    assert e["ok"] is False and e["etapa"] == "conferir"


def test_depois_de_instalar_roda_os_consertos_do_card_na_ordem(inst, monkeypatch):
    monkeypatch.setattr(hi.atualizar, "_rodar",
                        lambda argv, cwd=None, timeout=0, log=None: subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar", lambda: [{
        "id": "pi", "instalado": True, "itens": [
            {"id": "credenciais", "conserto": "sync:pi"},
            {"id": "mcp", "conserto": None},                 # informativo: não roda nada
            {"id": "extensoes", "conserto": "extensoes:pi"},
        ]}])
    feitos = []
    monkeypatch.setattr(hi.harness_saude, "consertar", lambda i: feitos.append(i) or f"{i} ok")
    e = _rodar(inst, "pi")
    assert e["ok"] is True and feitos == ["sync:pi", "extensoes:pi"]
    assert "$ extensoes:pi" in e["log"] and "extensoes:pi ok" in e["log"]


def test_o_wrapper_roda_depois_de_conferir_e_antes_dos_ajustes(inst, monkeypatch):
    """Instalar o CLI não basta: sem o wrapper do Hangar ele sobe fora do tmux e o app não o vê."""
    monkeypatch.setattr(hi.harness_saude, "cmd_instalador", lambda: ["bash", "instalador"])
    ordem = []

    def _fake(argv, cwd=None, timeout=0, log=None):
        ordem.append(argv[-1])
        return subprocess.CompletedProcess(argv, 0, "", "")

    monkeypatch.setattr(hi.atualizar, "_rodar", _fake)
    monkeypatch.setattr(hi.harness_saude, "diagnosticar", lambda: [{
        "id": "codex", "instalado": True, "itens": [{"id": "credenciais", "conserto": "sync:codex"}]}])
    monkeypatch.setattr(hi.harness_saude, "consertar", lambda i: ordem.append(i) or "ok")
    e = _rodar(inst, "codex")
    assert e["ok"] is True
    assert ordem == ["@openai/codex", "instalador", "sync:codex"]


def test_wrapper_que_falha_para_ali(inst, monkeypatch):
    monkeypatch.setattr(hi.harness_saude, "cmd_instalador", lambda: ["bash", "instalador"])
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda argv, cwd=None, timeout=0, log=None:
                        subprocess.CompletedProcess(argv, 0 if argv[-1] != "instalador" else 3, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar",
                        lambda: [{"id": "codex", "instalado": True, "itens": []}])
    monkeypatch.setattr(hi.harness_saude, "consertar", lambda i: pytest.fail("não pode ajustar sem wrapper"))
    e = _rodar(inst, "codex")
    assert e["ok"] is False and e["etapa"] == "wrapper" and "saiu com 3" in e["erro"]


def test_sem_bash_o_wrapper_e_anotado_e_a_instalacao_segue(inst, monkeypatch):
    """Windows: os wrappers de lá são do install.ps1. A instalação deu certo — o que falta é dito."""
    def _sem_bash():
        raise ValueError("sem bash nesta máquina")

    monkeypatch.setattr(hi.harness_saude, "cmd_instalador", _sem_bash)
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda argv, cwd=None, timeout=0, log=None:
                        subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar",
                        lambda: [{"id": "codex", "instalado": True, "itens": []}])
    e = _rodar(inst, "codex")
    assert e["ok"] is True
    assert any("wrapper pulado" in l and "sem bash" in l for l in e["log"])


def test_conserto_que_falha_para_ali_sem_derrubar_a_thread(inst, monkeypatch):
    monkeypatch.setattr(hi.atualizar, "_rodar",
                        lambda argv, cwd=None, timeout=0, log=None: subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar", lambda: [{
        "id": "pi", "instalado": True,
        "itens": [{"id": "skills", "conserto": "skills"}, {"id": "x", "conserto": "extensoes:pi"}]}])
    chamados = []

    def _consertar(i):
        chamados.append(i)
        raise ValueError("a ponte do kimi quebrou")

    monkeypatch.setattr(hi.harness_saude, "consertar", _consertar)
    e = _rodar(inst, "pi")
    assert e["ok"] is False and e["etapa"] == "ajustes"
    assert "a ponte do kimi quebrou" in e["erro"] and chamados == ["skills"]


def test_pedir_outro_harness_com_um_em_curso_nao_devolve_o_estado_do_primeiro(inst):
    """Devolver o estado do outro fazia a tela ler `ok=True` de uma instalação que nunca começou."""
    inst._estado = inst._zerado(fase="rodando", harness="codex", etapa="ajustes", passo=4, ok=None)
    with pytest.raises(hi.EmCurso):
        asyncio.run(inst.iniciar("pi"))
    # O MESMO harness pode reperguntar: é o polling da tela, e a resposta é o estado dele mesmo.
    assert asyncio.run(inst.iniciar("codex"))["harness"] == "codex"


def test_rodada_anterior_ja_terminada_nao_e_devolvida_como_desta(inst, monkeypatch):
    """Entre `fase=pronto` e o Task fechar, `done()` ainda é falso — e o pedido novo levava o `ok`
    da rodada passada."""
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda *a, **k: (_ for _ in ()).throw(AssertionError))
    inst._estado = inst._zerado(fase="pronto", harness="codex", ok=True)

    class _Pendente:
        def done(self): return False
        def add_done_callback(self, _): pass

    monkeypatch.setattr(hi.asyncio, "create_task", lambda coro: (coro.close(), _Pendente())[1])
    e = asyncio.run(inst.iniciar("pi"))
    assert e["harness"] == "pi" and e["fase"] == "rodando" and e["ok"] is None


def test_callback_da_rodada_velha_nao_derruba_a_nova(inst):
    """Ele pintava a instalação nova como interrompida — e, como a vez é da fase, soltava a tranca
    com a thread nova ainda trabalhando."""
    velha = asyncio.Future(loop=asyncio.new_event_loop())
    velha.set_result(None)
    inst._task = object()  # a instalação ATUAL é outra
    inst._estado = inst._zerado(fase="rodando", harness="pi", etapa="comando", passo=1)
    inst._encerrou(velha)
    assert inst._estado["fase"] == "rodando" and inst._estado["ok"] is None


def test_sem_bash_o_pulo_do_wrapper_vira_aviso_no_estado(inst, monkeypatch):
    """No log ele some entre 400 linhas, enquanto a manchete promete 'ligado ao app'."""
    def _sem_bash():
        raise ValueError("sem bash nesta máquina")

    monkeypatch.setattr(hi.harness_saude, "cmd_instalador", _sem_bash)
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda argv, cwd=None, timeout=0, log=None:
                        subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar",
                        lambda: [{"id": "codex", "instalado": True, "itens": []}])
    e = _rodar(inst, "codex")
    assert e["ok"] is True
    assert any("wrapper foi pulada" in a for a in e["avisos"])


def test_instalador_que_sai_zero_sem_ligar_o_wrapper_e_falha(inst, monkeypatch):
    """`rc==0` não prova: o instalador só escreve nos shells que ELE detecta."""
    monkeypatch.setattr(hi.harness_saude, "cmd_instalador", lambda: ["bash", "instalador"])
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda argv, cwd=None, timeout=0, log=None:
                        subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "diagnosticar",
                        lambda: [{"id": "pi", "instalado": True, "itens": []}])
    monkeypatch.setattr(hi.harness_saude, "_wrapper",
                        lambda cli: {"ok": False, "params": {"lista": "fish"}})
    e = _rodar(inst, "pi")
    assert e["ok"] is False and e["etapa"] == "wrapper" and "continua faltando em: fish" in e["erro"]


def test_conserto_que_falha_diz_o_que_nao_chegou_a_rodar(inst, monkeypatch):
    monkeypatch.setattr(hi.harness_saude, "cmd_instalador", lambda: ["bash", "instalador"])
    monkeypatch.setattr(hi.atualizar, "_rodar", lambda argv, cwd=None, timeout=0, log=None:
                        subprocess.CompletedProcess(argv, 0, "", ""))
    monkeypatch.setattr(hi.harness_saude, "_wrapper", lambda cli: {"ok": True, "params": {}})
    monkeypatch.setattr(hi.harness_saude, "diagnosticar", lambda: [{
        "id": "pi", "instalado": True, "itens": [
            {"id": "a", "conserto": "sync:pi"}, {"id": "b", "conserto": "extensoes:pi"},
            {"id": "c", "conserto": "skills"}]}])
    monkeypatch.setattr(hi.harness_saude, "consertar",
                        lambda i: (_ for _ in ()).throw(ValueError("quebrou")))
    e = _rodar(inst, "pi")
    assert "não cheguei a rodar: extensoes:pi, skills" in e["erro"]


def test_harness_sem_comando_conferido_nao_instala(inst):
    with pytest.raises(ValueError):
        asyncio.run(inst.iniciar("claude"))
    # Sem comando conferido pra ESTE sistema o harness some da lista de botões — e o link do
    # fornecedor continua lá, que é o que a tela mostra no lugar.
    assert set(hi.comandos()) == {k for k, v in hi._COMANDOS.items() if v}
    assert set(hi.MANUAL) >= set(hi._COMANDOS)


def test_o_comando_exibido_e_o_do_fornecedor_sem_o_embrulho():
    assert hi._exibir(hi._sh("curl -fsSL https://omp.sh/install | sh")) == \
        "curl -fsSL https://omp.sh/install | sh"
    assert hi._exibir(["npm", "install", "-g", "@openai/codex"]) == "npm install -g @openai/codex"


def test_pipefail_faz_o_curl_que_falha_derrubar_o_pipe():
    """Sem ele o `curl … | sh` mente: o sh lê stdin vazio e sai 0, e a instalação passaria por feita."""
    argv = hi._sh("exit 7 | cat")
    assert subprocess.run(argv, capture_output=True).returncode == 7
    assert subprocess.run(["bash", "-c", "exit 7 | cat"], capture_output=True).returncode == 0


def test_esquecer_versao_derruba_o_cache_de_10_min(monkeypatch):
    harness_saude._versoes["zzz"] = (9e9, "1.0")
    assert harness_saude._versao("zzz") == "1.0"
    harness_saude.esquecer_versao("zzz")
    monkeypatch.setattr(harness_saude.shutil, "which", lambda _: None)
    assert harness_saude._versao("zzz") is None
