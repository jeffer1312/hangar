"""Catálogo de modelos do Codex: o que o `model/list` devolve vira lista da tela de abertura.

O que esta suíte trava: modelo escondido não aparece; os níveis de esforço saem POR MODELO (a lição
do Pi, medida de novo aqui — `gpt-5.6-luna` não tem `ultra` e `gpt-5.5` não tem `max`); e resposta
sem modelo nenhum é falha do provedor, não catálogo vazio.
"""
import io
import json

import pytest

from app import codex_models as cm


# Recorte real do `codex app-server` 0.151.0 nesta máquina (campos que não usamos foram cortados,
# os que usamos estão byte por byte). Fixture inventada aqui seria mentira sobre o formato — o
# mesmo cuidado que o ticket 06 cobrou.
RESPOSTA = {
    "data": [
        {
            "id": "gpt-5.6-sol", "model": "gpt-5.6-sol", "displayName": "GPT-5.6-Sol",
            "description": "Latest frontier agentic coding model.", "hidden": False,
            "supportedReasoningEfforts": [
                {"reasoningEffort": "low", "description": "Fast responses with lighter reasoning"},
                {"reasoningEffort": "medium", "description": "Balances speed and reasoning depth"},
                {"reasoningEffort": "high", "description": "Greater reasoning depth"},
                {"reasoningEffort": "xhigh", "description": "Extra high reasoning depth"},
                {"reasoningEffort": "max", "description": "Maximum reasoning depth"},
                {"reasoningEffort": "ultra", "description": "Maximum reasoning with delegation"},
            ],
            "defaultReasoningEffort": "low", "isDefault": True,
        },
        {
            "id": "gpt-5.5", "model": "gpt-5.5", "displayName": "GPT-5.5",
            "description": "Previous frontier model.", "hidden": False,
            "supportedReasoningEfforts": [
                {"reasoningEffort": "low", "description": ""},
                {"reasoningEffort": "medium", "description": ""},
                {"reasoningEffort": "high", "description": ""},
                {"reasoningEffort": "xhigh", "description": ""},
            ],
            "defaultReasoningEffort": "medium", "isDefault": False,
        },
        {
            "id": "gpt-5.6-codex-mini-internal", "model": "gpt-5.6-codex-mini-internal",
            "displayName": "Interno", "description": "", "hidden": True,
            "supportedReasoningEfforts": [], "defaultReasoningEffort": None, "isDefault": False,
        },
    ]
}


def test_parse_normaliza_para_o_formato_da_tela():
    modelos = cm.parse(RESPOSTA)
    assert modelos[0] == {
        "id": "gpt-5.6-sol", "name": "GPT-5.6-Sol",
        "desc": "Latest frontier agentic coding model.",
        "efforts": ["low", "medium", "high", "xhigh", "max", "ultra"],
        "default_effort": "low",
    }


def test_modelo_escondido_nao_entra():
    """`hidden` é o marcador do provedor pra modelo que não deve ser oferecido — oferecer um deles
    faria a sessão nascer num id que o plano do usuário não atende."""
    assert [m["id"] for m in cm.parse(RESPOSTA)] == ["gpt-5.6-sol", "gpt-5.5"]


def test_os_niveis_sao_por_modelo():
    """A razão de o catálogo existir: lista fechada no código não distingue os dois. Medido em
    30/08/2026 no codex-cli 0.151.0."""
    por_id = {m["id"]: m for m in cm.parse(RESPOSTA)}
    assert "ultra" in por_id["gpt-5.6-sol"]["efforts"]
    assert "ultra" not in por_id["gpt-5.5"]["efforts"]
    assert "max" not in por_id["gpt-5.5"]["efforts"]


def test_modelo_sem_id_e_pulado_sem_derrubar_a_lista():
    """Entrada torta de uma versão futura não pode cegar o seletor inteiro — mesma regra do
    `parse` do pi_catalog."""
    assert cm.parse({"data": [{"hidden": False}, RESPOSTA["data"][1]]}) == cm.parse(
        {"data": [RESPOSTA["data"][1]]})


def test_resposta_sem_modelo_nenhum_estoura():
    """rc=0 com zero modelo é falha do provedor, não catálogo vazio: virar lista vazia na tela
    diria "seu plano não tem modelo", que é outra afirmação."""
    with pytest.raises(RuntimeError):
        cm.parse({"data": []})


def test_listar_sem_o_binario_tem_erro_proprio(monkeypatch):
    """"não achei o codex" não é "o codex falhou" — mesma separação do PiAusente."""
    monkeypatch.setattr(cm.shutil, "which", lambda _: None)
    with pytest.raises(cm.CodexAusente):
        cm.listar(fresco=True)


class _FakeProc:
    """O app-server em stdio: escreve o que mandaram nele e devolve as linhas combinadas."""

    def __init__(self, argv, linhas, erro=""):
        self.argv = argv
        self.escrito = io.StringIO()
        self.stdin = self.escrito
        self.stdout = iter(linhas)
        self.stderr = io.StringIO(erro)
        self.morto = False

    def kill(self):
        self.morto = True

    def wait(self, timeout=None):
        return 0


def _fake_popen(monkeypatch, linhas, erro=""):
    criados = []

    def popen(argv, **kw):
        p = _FakeProc(argv, linhas, erro)
        criados.append(p)
        return p

    monkeypatch.setattr(cm.shutil, "which", lambda _: "/usr/bin/codex")
    monkeypatch.setattr(cm.subprocess, "Popen", popen)
    cm._cache = None
    return criados


def test_listar_fala_json_rpc_e_cacheia(monkeypatch):
    criados = _fake_popen(monkeypatch, [
        json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}}) + "\n",
        json.dumps({"jsonrpc": "2.0", "method": "remoteControl/status/changed"}) + "\n",
        json.dumps({"jsonrpc": "2.0", "id": 2, "result": RESPOSTA}) + "\n",
    ])
    assert [m["id"] for m in cm.listar(fresco=True)] == ["gpt-5.6-sol", "gpt-5.5"]
    assert criados[0].argv[:2] == ["/usr/bin/codex", "app-server"]
    # initialize ANTES do model/list: sem o handshake o app-server recusa o pedido.
    pedidos = [json.loads(l) for l in criados[0].escrito.getvalue().splitlines()]
    assert [p["method"] for p in pedidos] == ["initialize", "model/list"]
    cm.listar()
    assert len(criados) == 1


def test_o_stdin_so_fecha_depois_da_resposta(monkeypatch):
    """A causa que só apareceu ao vivo (30/08/2026): com `subprocess.run(input=...)` o stdin fecha
    junto com a entrada, o app-server responde o `initialize` e SAI (rc=0, 0,25s) sem chegar no
    segundo pedido — a lista voltava vazia com sucesso aparente."""
    criados = _fake_popen(monkeypatch, [
        json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}}) + "\n",
        json.dumps({"jsonrpc": "2.0", "id": 2, "result": RESPOSTA}) + "\n",
    ])
    cm.listar(fresco=True)
    assert not criados[0].escrito.closed


def test_saida_sem_a_resposta_do_pedido_estoura(monkeypatch):
    """Subir o app-server e não achar a resposta do `model/list` é falha do provedor: sem isto o
    catálogo voltaria vazio e a tela diria que não há modelo."""
    _fake_popen(monkeypatch, ["ruido\n"], erro="codex: login expirado")
    with pytest.raises(RuntimeError, match="login expirado"):
        cm.listar(fresco=True)


def test_checar_escolha_recusa_nivel_que_o_modelo_nao_lista(monkeypatch):
    """Medido em 30/08/2026: pedir `ultra` a um `gpt-5.5` NÃO mata o arranque — o binário segue com
    o dele. Sem esta recusa a sessão nasceria com a escolha descartada em silêncio."""
    _fake_popen(monkeypatch, [json.dumps({"jsonrpc": "2.0", "id": 2, "result": RESPOSTA}) + "\n"])
    with pytest.raises(ValueError, match="ultra"):
        cm.checar_escolha("gpt-5.5", "ultra")
    cm.checar_escolha("gpt-5.6-sol", "ultra")   # o mesmo nível, no modelo que o lista


def test_checar_escolha_recusa_modelo_fora_do_catalogo(monkeypatch):
    _fake_popen(monkeypatch, [json.dumps({"jsonrpc": "2.0", "id": 2, "result": RESPOSTA}) + "\n"])
    with pytest.raises(ValueError, match="fora do catalogo"):
        cm.checar_escolha("gpt-9", None)


def test_checar_escolha_sem_modelo_nao_pergunta_nada(monkeypatch):
    """Nível sem modelo não é checável: o modelo é o do `~/.codex/config.toml`, que este catálogo
    não diz qual é. Não pode virar recusa nem subprocess à toa."""
    criados = _fake_popen(monkeypatch, [])
    cm.checar_escolha(None, "high")
    assert criados == []


def test_processo_sempre_morre(monkeypatch):
    """O app-server fica vivo enquanto o stdin estiver aberto — sair sem matá-lo deixaria um
    processo por abertura de tela. Vale inclusive quando a leitura falha."""
    criados = _fake_popen(monkeypatch, ["ruido\n"])
    with pytest.raises(RuntimeError):
        cm.listar(fresco=True)
    assert criados[0].morto
