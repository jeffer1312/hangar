"""A borda HTTP das contas.

O que esta suíte trava: conta recém-criada aparece na lista ANTES do /login (senão o usuário não
tem onde abrir a sessão pra rodar o /login — impasse); com CP_CLAUDE_CONFIG_DIRS setado o POST
recusa em vez de devolver 200 pra uma conta que nunca vai aparecer no seletor; e apagar só aceita
pasta carimbada — recusando também o config dir ativo do backend, a conta da lista fixa, sessão
viva na conta, processo vivo fora do tmux e varredura não confiável. Checagem e rmtree do apagar
rodam sob a MESMA trava da criação de sessão, e as validações da criação rodam ANTES da
reconciliação (pedido rejeitado não toca a conta no disco).
"""
import json
import threading
import time

import pytest
from fastapi.testclient import TestClient

from app import contas, procinfo
from app.api import app, registry
from app.config import list_config_dirs, settings

# Convenção da casa (ver test_engines_api.py): cada arquivo declara o próprio token.
TOKEN = "t-contas"
AUTH = {"Authorization": f"Bearer {TOKEN}"}


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    compartilhado = tmp_path / ".claude"
    (compartilhado / "projects").mkdir(parents=True)
    (compartilhado / "skills").mkdir()
    (compartilhado / ".credentials.json").write_text("{}", encoding="utf-8")
    (tmp_path / ".claude.json").write_text(json.dumps({"oauthAccount": {}}), encoding="utf-8")
    return tmp_path


@pytest.fixture
def sem_processos_usando(monkeypatch):
    """O DELETE varre /proc inteiro procurando processos com CLAUDE_CONFIG_DIR. O teste finge a
    resposta feliz (nenhum) pra não depender da máquina onde roda; a varredura em si é coberta
    pelo test_procinfo."""
    monkeypatch.setattr(procinfo, "pids_com_config_dir", lambda alvo: [])


class _SessaoFake:
    def __init__(self, name: str):
        self.name = name


def test_conta_sem_credencial_ainda_aparece_na_lista(casa):
    """Impasse que isto evita: sem credencial a pasta não passava no filtro, então a conta sumia
    justamente entre criar e logar — e o /login só pode ser rodado DENTRO de uma sessão dela."""
    contas.criar("conta2")
    assert str(casa / ".claude-conta2") in {c.path for c in list_config_dirs()}


def test_pasta_parecida_sem_marcador_e_sem_credencial_nao_entra(casa):
    (casa / ".claude-backup").mkdir()
    assert str(casa / ".claude-backup") not in {c.path for c in list_config_dirs()}


def test_pasta_conta_symlinkada_nao_entra(casa):
    """Raiz symlink com marcador plantado no alvo: sem a guarda do e_conta, um
    ~/.claude-evil -> /tmp/fora entraria na lista como conta e a reconciliação remexeria fora."""
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    (casa / ".claude-evil").symlink_to(fora, target_is_directory=True)
    assert str(casa / ".claude-evil") not in {c.path for c in list_config_dirs()}


def test_marcador_symlink_nao_carimba_conta(casa):
    """Marcador symlink não carimba: seguir o link aceitaria conta cujo carimbo aponta pra fora."""
    contas.criar("conta2")
    marcador = casa / ".claude-conta2" / contas.MARCADOR
    marcador.unlink()
    marcador.symlink_to(casa / ".claude" / "settings.json")
    assert str(casa / ".claude-conta2") not in {c.path for c in list_config_dirs()}


def test_criar_conta_pela_api(casa):
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 200
    assert r.json()["path"] == str(casa / ".claude-conta2")
    assert r.json()["label"] == "conta2"


def test_criar_conta_repetida_devolve_409(casa):
    cli = TestClient(app)
    r1 = cli.post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    # Pré-condição afirmada, não assumida: uma implementação que devolvesse 409 pra TODO POST
    # passaria sem ela.
    assert r1.status_code == 200
    assert r1.json()["path"] == str(casa / ".claude-conta2")
    assert (casa / ".claude-conta2").is_dir()
    r = cli.post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 409


def test_nome_invalido_devolve_422(casa):
    """O pattern do schema (mesma regra do contas._NOME_OK) recusa na borda, antes do módulo."""
    r = TestClient(app).post("/api/claude-configs", json={"nome": "Conta 2"}, headers=AUTH)
    assert r.status_code == 422


def test_nome_com_quebra_de_linha_devolve_422(casa):
    """O pattern do schema usa \Z (o $ aceita \n final, e o pydantic casa com re.match). Sem isto
    a pasta nascia com quebra de linha no nome."""
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2\n"}, headers=AUTH)
    assert r.status_code == 422
    assert not (casa / ".claude-conta2\n").exists()


def test_com_lista_fixa_de_config_dirs_o_post_recusa(casa, monkeypatch):
    """Com CP_CLAUDE_CONFIG_DIRS setado, list_config_dirs ignora o auto-scan: a conta seria criada,
    nunca apareceria no seletor, e mandar o path mesmo assim daria 400 na criação de sessão.
    Recusar aqui, com o motivo, é a única saída que não mente."""
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", f"padrao:{casa / '.claude'}")
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 409
    assert "CP_CLAUDE_CONFIG_DIRS" in r.json()["detail"]


def test_apagar_conta(casa, sem_processos_usando):
    cli = TestClient(app)
    r1 = cli.post("/api/claude-configs", json={"nome": "cotna2"}, headers=AUTH)
    assert r1.status_code == 200
    assert (casa / ".claude-cotna2").is_dir()
    r = cli.delete("/api/claude-configs/cotna2", headers=AUTH)
    assert r.status_code == 200
    assert r.json() == {"ok": True}
    assert not (casa / ".claude-cotna2").exists()


def test_apagar_pasta_nao_carimbada_devolve_404(casa):
    (casa / ".claude-backup").mkdir()
    assert TestClient(app).delete("/api/claude-configs/backup", headers=AUTH).status_code == 404
    assert (casa / ".claude-backup").is_dir()


def test_apagar_config_dir_ativo_do_backend_recusa(casa, monkeypatch):
    """Com CLAUDE_CONFIG_DIR apontando pra conta e nenhuma sessão tmux aberta, o DELETE antigo
    devolvia 200 e removia projects/ que o backend continua usando."""
    contas.criar("conta2")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(casa / ".claude-conta2"))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_conta_da_lista_fixa_recusa(casa, monkeypatch):
    """Com CP_CLAUDE_CONFIG_DIRS setado, a conta apagada voltaria da variável: o seletor mostraria
    uma conta fantasma e uma sessão nova recriaria a pasta sem marcador nem atalhos."""
    contas.criar("conta2")
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS",
                       f"padrao:{casa / '.claude'},conta2:{casa / '.claude-conta2'}")
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_recusa_sessao_viva_na_conta(casa, monkeypatch, sem_processos_usando):
    contas.criar("conta2")
    monkeypatch.setattr(registry, "list", lambda: [_SessaoFake("sessao-x")])
    import app.tmux as tmux_mod
    monkeypatch.setattr(tmux_mod, "pane_pid", lambda name: 12345)
    monkeypatch.setattr(procinfo, "_config_dir_confiavel",
                        lambda pid: (casa / ".claude-conta2", True))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert "sessao-x" in r.json()["detail"]
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_recusa_quando_resolucao_de_sessao_falha(casa, monkeypatch, sem_processos_usando):
    """Falha de leitura do environ não é 'conta padrão': o fallback silencioso do
    _session_config_dir trataria o processo como se não usasse a conta e o DELETE apagaria debaixo
    de um CLI vivo."""
    contas.criar("conta2")
    monkeypatch.setattr(registry, "list", lambda: [_SessaoFake("sessao-y")])
    import app.tmux as tmux_mod
    monkeypatch.setattr(tmux_mod, "pane_pid", lambda name: 12345)
    monkeypatch.setattr(procinfo, "_config_dir_confiavel", lambda pid: (None, False))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_recusa_processo_vivo_fora_do_tmux(casa, monkeypatch):
    """claude mantido fora do tmux não aparece no registry.list: só a varredura de /proc acha."""
    contas.criar("conta2")
    monkeypatch.setattr(procinfo, "pids_com_config_dir", lambda alvo: [9876])
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_recusa_quando_varredura_nao_e_confiavel(casa, monkeypatch):
    """Varredura com falha (None) = não dá pra garantir que ninguém usa: falha fechado."""
    contas.criar("conta2")
    monkeypatch.setattr(procinfo, "pids_com_config_dir", lambda alvo: None)
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_trava_do_delete_e_da_criacao_e_a_mesma(casa):
    """A trava pública que a criação segura (reconcile→registry.create) e o DELETE
    (checagem→rmtree) é a MESMA: enquanto uma thread a segura, a outra espera. Sem isto, uma
    sessão recém-criada ainda invisível ao registry teria a pasta apagada debaixo dela."""
    contas.criar("conta2")
    entrou = threading.Event()
    solta = threading.Event()

    def segura():
        with contas.travada("conta2"):
            entrou.set()
            solta.wait(5)

    t = threading.Thread(target=segura)
    t.start()
    assert entrou.wait(5)

    acabou = threading.Event()

    def tenta():
        with contas.travada("conta2"):
            pass
        acabou.set()

    t2 = threading.Thread(target=tenta)
    t2.start()
    time.sleep(0.2)
    assert not acabou.is_set()      # a segunda operação fica esperando a primeira
    solta.set()
    t.join(5)
    assert acabou.wait(5)


def test_engine_invalido_nao_toca_a_conta(casa, monkeypatch):
    """Validações rodam ANTES da reconciliação: pedido rejeitado não altera a conta no disco."""
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas, "reconciliar",
                        lambda *a, **k: reconciliou.append(1) or [])
    r = TestClient(app).post("/api/sessions",
                             json={"name": "x", "cwd": "/tmp",
                                   "config_dir": str(casa / ".claude-conta2"),
                                   "provider": "claude", "engine": "nao-existe"},
                             headers=AUTH)
    assert r.status_code == 400
    assert reconciliou == []


def test_provider_invalido_nao_toca_a_conta(casa, monkeypatch):
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas, "reconciliar",
                        lambda *a, **k: reconciliou.append(1) or [])
    r = TestClient(app).post("/api/sessions",
                             json={"name": "x", "cwd": "/tmp",
                                   "config_dir": str(casa / ".claude-conta2"),
                                   "provider": "outro"},
                             headers=AUTH)
    assert r.status_code == 400
    assert reconciliou == []


def test_falha_na_reconciliacao_aborta_a_criacao(casa, monkeypatch):
    """Falha de filesystem na reconciliação devolve o status/detail do módulo e NÃO cria a
    sessão — a abertura aborta com o motivo, não com 500 traceback."""
    contas.criar("conta2")
    chamou_create = []

    def estoura(*a, **k):
        raise contas.ContaError(500, "atalho recusado (Windows sem Modo Desenvolvedor?)")

    monkeypatch.setattr(contas, "reconciliar", estoura)
    monkeypatch.setattr(registry, "create", lambda *a, **k: chamou_create.append(1))
    r = TestClient(app).post("/api/sessions",
                             json={"name": "x", "cwd": "/tmp",
                                   "config_dir": str(casa / ".claude-conta2"),
                                   "provider": "claude"},
                             headers=AUTH)
    assert r.status_code == 500
    assert "atalho recusado" in r.json()["detail"]
    assert chamou_create == []
