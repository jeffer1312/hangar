"""A borda HTTP das contas.

O que esta suíte trava: conta recém-criada aparece na lista ANTES do /login (senão o usuário não
tem onde abrir a sessão pra rodar o /login — impasse); com CP_CLAUDE_CONFIG_DIRS setado o POST
recusa em vez de devolver 200 pra uma conta que nunca vai aparecer no seletor; apagar só aceita
pasta carimbada; e a borda destrutiva (DELETE) é fechada: configuração ativa do backend, conta da
lista fixa, sessão viva, processo vivo com o config dir e resolução não confiável recusam antes
do rmtree, e a trava do ciclo de abertura cobre a janela reconciliação→registry.create.
"""
import json
import threading
import time

import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import contas
from app.api import app
from app.config import list_config_dirs, settings
from app.models import SessionInfo

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


def test_conta_sem_credencial_ainda_aparece_na_lista(casa):
    """Impasse que isto evita: sem credencial a pasta não passava no filtro, então a conta sumia
    justamente entre criar e logar — e o /login só pode ser rodado DENTRO de uma sessão dela."""
    contas.criar("conta2")
    assert str(casa / ".claude-conta2") in {c.path for c in list_config_dirs()}


def test_pasta_parecida_sem_marcador_e_sem_credencial_nao_entra(casa):
    (casa / ".claude-backup").mkdir()
    assert str(casa / ".claude-backup") not in {c.path for c in list_config_dirs()}


def test_pasta_conta_por_symlink_nao_entra_na_lista(casa):
    """~/.claude-evil -> /tmp/fora com marcador do lado de lá não é conta: a reconciliação e o
    apagar remexeriam — e destruiriam — um diretório externo."""
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    (casa / ".claude-evil").symlink_to(fora, target_is_directory=True)
    assert str(casa / ".claude-evil") not in {c.path for c in list_config_dirs()}


def test_marcador_por_symlink_nao_entra_na_lista(casa):
    conta = casa / ".claude-conta2"
    conta.mkdir()
    alvo = casa / "marcador-fora"
    alvo.write_text("", encoding="utf-8")
    (conta / contas.MARCADOR).symlink_to(alvo)
    assert str(conta) not in {c.path for c in list_config_dirs()}


def test_criar_conta_pela_api(casa):
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 200
    assert r.json()["path"] == str(casa / ".claude-conta2")
    assert r.json()["label"] == "conta2"


def test_criar_conta_repetida_devolve_409(casa):
    cli = TestClient(app)
    r1 = cli.post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    # Pré-condição afirmada de verdade: um POST que devolvesse 409 pra tudo passaria no teste
    # antigo (que descartava o resultado da criação).
    assert r1.status_code == 200
    assert r1.json()["path"] == str(casa / ".claude-conta2")
    assert (casa / ".claude-conta2").is_dir()
    r = cli.post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 409


def test_nome_fora_do_alfabeto_devolve_422(casa):
    """O ContaBody valida o alfabeto no schema (pattern), antes do módulo: \n, /, .. e nomes
    longos são rejeitados pelo pydantic com 422. Sem o pattern, o re.match com $ do módulo
    aceitaria 'conta2\n' (o $ casa antes da quebra final) e a pasta nasceria com controle de
    linha no nome."""
    cli = TestClient(app)
    for nome in ("conta2\n", "conta/2", "..", "x" * 33, "Conta 2"):
        r = cli.post("/api/claude-configs", json={"nome": nome}, headers=AUTH)
        assert r.status_code == 422, nome
    assert not (casa / ".claude-conta2\n").exists()


def test_com_lista_fixa_de_config_dirs_o_post_recusa(casa, monkeypatch):
    """Com CP_CLAUDE_CONFIG_DIRS setado, list_config_dirs ignora o auto-scan: a conta seria criada,
    nunca apareceria no seletor, e mandar o path mesmo assim daria 400 na criação de sessão.
    Recusar aqui, com o motivo, é a única saída que não mente."""
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS", f"padrao:{casa / '.claude'}")
    r = TestClient(app).post("/api/claude-configs", json={"nome": "conta2"}, headers=AUTH)
    assert r.status_code == 409
    assert "CP_CLAUDE_CONFIG_DIRS" in r.json()["detail"]["msg"]


def test_apagar_conta(casa):
    cli = TestClient(app)
    r = cli.post("/api/claude-configs", json={"nome": "cotna2"}, headers=AUTH)
    assert r.status_code == 200
    assert (casa / ".claude-cotna2").is_dir()
    resp = cli.delete("/api/claude-configs/cotna2", headers=AUTH)
    assert resp.status_code == 200
    assert resp.json() == {"ok": True}
    assert not (casa / ".claude-cotna2").exists()


def test_apagar_pasta_nao_carimbada_devolve_404(casa):
    (casa / ".claude-backup").mkdir()
    assert TestClient(app).delete("/api/claude-configs/backup", headers=AUTH).status_code == 404
    assert (casa / ".claude-backup").is_dir()


def test_apagar_a_config_ativa_do_backend_devolve_409(casa, monkeypatch):
    """Com CLAUDE_CONFIG_DIR apontando pra conta, o backend mora DENTRO dela (settings, custos,
    transcripts): apagar deixaria o app escrevendo num caminho que sumiu."""
    contas.criar("conta2")
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(casa / ".claude-conta2"))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_conta_da_lista_fixa_de_config_dirs_devolve_409(casa, monkeypatch):
    """O GET continua devolvendo esta conta mesmo apagada (a lista fixa por env não muda): sobraria
    um fantasma no seletor, e a próxima sessão recriaria a pasta sem marcador nem atalhos."""
    contas.criar("conta2")
    monkeypatch.setenv("CP_CLAUDE_CONFIG_DIRS",
                       f"padrao:{casa / '.claude'},conta2:{casa / '.claude-conta2'}")
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert "CP_CLAUDE_CONFIG_DIRS" in r.json()["detail"]["msg"]
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_com_sessao_viva_usando_a_conta_devolve_409(casa, monkeypatch):
    contas.criar("conta2")

    class S:
        name = "sessao-x"

    monkeypatch.setattr(api_mod.registry, "list", lambda: [S()])
    monkeypatch.setattr(api_mod, "_session_config_dir_strict",
                        lambda name: (casa / ".claude-conta2", True))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert "sessao-x" in r.json()["detail"]["msg"]
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_com_resolucao_de_sessao_nao_confiavel_devolve_409(casa, monkeypatch):
    """Falha ao resolver o config dir de uma sessão viva NÃO libera o apagar: na dúvida, recusa.
    (A leitura do /proc/<pid>/environ pode falhar — processo morreu no meio, permissão.)"""
    contas.criar("conta2")

    class S:
        name = "sessao-x"

    monkeypatch.setattr(api_mod.registry, "list", lambda: [S()])
    monkeypatch.setattr(api_mod, "_session_config_dir_strict", lambda name: (None, False))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_com_processo_vivo_usando_a_conta_devolve_409(casa, monkeypatch):
    """Um `claude` aberto FORA do tmux não aparece no registry: a consulta por CLAUDE_CONFIG_DIR
    no /proc é quem segura o apagar debaixo dele."""
    contas.criar("conta2")
    monkeypatch.setattr(api_mod.procinfo, "_pids_com_config_dir", lambda alvo: ([999], True))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert (casa / ".claude-conta2").is_dir()


def test_apagar_espera_o_ciclo_de_abertura_da_mesma_conta(casa):
    """A trava do ciclo cobre o intervalo reconciliação→registry.create: o DELETE espera a
    abertura terminar em vez de apagar a pasta embaixo dela."""
    contas.criar("conta2")
    adquiriu = threading.Event()
    erros = []

    def abre_em_thread():
        try:
            with contas.ciclo_conta("conta2"):
                adquiriu.set()
                time.sleep(0.3)
                # Se o DELETE não esperasse o ciclo, a pasta sumiria aqui — e a sessão que está
                # subindo escreveria num caminho que não existe.
                if not (casa / ".claude-conta2").is_dir():
                    erros.append("pasta sumiu durante o ciclo")
        except Exception as e:
            erros.append(str(e))

    t = threading.Thread(target=abre_em_thread)
    t.start()
    assert adquiriu.wait(2), "a thread não adquiriu o ciclo"
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    t.join()
    assert erros == []
    assert r.status_code == 200
    assert not (casa / ".claude-conta2").exists()


def test_falha_na_reconciliacao_devolve_erro_e_nao_cria_sessao(casa, monkeypatch):
    """ContaError do reconciliar (ex: Windows sem Modo Desenvolvedor) e OSError de filesystem
    viram HTTPException com o motivo, e a sessão NÃO é criada — abertura abortada, não 500 com
    traceback."""
    contas.criar("conta2")

    def boom_conta(self, projeto=None):
        raise contas.ContaError(500, "não consegui criar o atalho")

    def boom_os(self, projeto=None):
        raise OSError("permissão negada")

    criados = []
    monkeypatch.setattr(api_mod.registry, "create",
                        lambda *a, **k: criados.append(a) or object())
    cli = TestClient(app)
    # ContaError: o detail passa como STRING (e.detail) — o texto do motivo chega direto.
    monkeypatch.setattr(contas._Ciclo, "reconciliar", boom_conta)
    r = cli.post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude"}, headers=AUTH)
    assert r.status_code == 500
    assert "não consegui criar o atalho" in r.json()["detail"]
    # OSError: vira o envelope erro_conta_reconciliacao_falhou — o contrato code/params tem que
    # ser afirmado (B4 do parecer task 11): um envelope com o MESMO msg mas code errado reprova.
    monkeypatch.setattr(contas._Ciclo, "reconciliar", boom_os)
    r = cli.post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude"}, headers=AUTH)
    assert r.status_code == 500
    d = r.json()["detail"]
    assert d["code"] == "erro_conta_reconciliacao_falhou"
    assert d["params"]["nome_conta"] == "conta2"
    assert d["params"]["erro"] == "permissão negada"
    assert "permissão negada" in d["msg"]
    assert criados == []


def test_codex_com_config_dir_nao_reconcilia(casa, monkeypatch):
    """Provider codex não consome config dir (o create_codex nem recebe ele): a reconciliação —
    efeito no disco — não pode rodar num pedido que vai criar uma sessão codex."""
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas._Ciclo, "reconciliar",
                        lambda self, projeto=None: reconciliou.append(1) or [])

    async def create_codex_fake(*a, **k):
        return SessionInfo(name="s1", provider="codex")

    monkeypatch.setattr(api_mod.registry, "create_codex", create_codex_fake)
    r = TestClient(app).post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "codex"}, headers=AUTH)
    assert reconciliou == []


def test_engine_invalido_rejeita_antes_de_reconciliar(casa, monkeypatch):
    """Validação de engine vem ANTES do toque no disco: pedido que vai ser rejeitado não pode
    ter movido deriva nem criado memória na conta."""
    contas.criar("conta2")
    reconciliou = []
    monkeypatch.setattr(contas._Ciclo, "reconciliar",
                        lambda self, projeto=None: reconciliou.append(1) or [])
    monkeypatch.setattr(api_mod.engines, "listar", lambda: {})
    r = TestClient(app).post("/api/sessions", json={
        "name": "s1", "cwd": str(casa), "config_dir": str(casa / ".claude-conta2"),
        "provider": "claude", "engine": "naoexiste"}, headers=AUTH)
    assert r.status_code == 400
    assert reconciliou == []


def test_apagar_recusa_quando_a_varredura_de_processos_falha(casa, monkeypatch):
    """"Nao consegui olhar" nao pode sair igual a "olhei e nao achei".

    `_pids_com_config_dir` devolvia `[]` tanto quando terminava a varredura sem achar nada quanto
    quando ela morria no meio (psutil.Error no laco, /proc ilegivel). O DELETE seguia com o rmtree e
    apagava a pasta debaixo de um `claude` vivo que a varredura nem chegou a enxergar. Agora o
    segundo elemento diz se a varredura completou, e o DELETE recusa quando nao completou.
    """
    contas.criar("conta2")
    monkeypatch.setattr(api_mod.registry, "list", lambda: [])
    monkeypatch.setattr(api_mod.procinfo, "_pids_com_config_dir", lambda alvo: ([], False))
    r = TestClient(app).delete("/api/claude-configs/conta2", headers=AUTH)
    assert r.status_code == 409
    assert "varrer os processos" in r.json()["detail"]["msg"]
    assert (casa / ".claude-conta2").is_dir(), "apagou mesmo sem conseguir varrer os processos"
