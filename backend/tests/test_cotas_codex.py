"""Cota da conta do Codex no painel (app/cotas.py, fonte `codex`).

Por que a fonte não é HTTP como as outras: a credencial do Codex é um par OAuth do ChatGPT em
`~/.codex/auth.json`, e o endpoint que a traduz em cota não é público. Quem sabe fazer essa conta é
o próprio binário — e ele responde `account/rateLimits/read` num app-server efêmero em stdio, sem
sessão viva e sem pane (medido em 30/08/2026, codex-cli 0.151.0, 1,2s). É o mesmo mecanismo do
catálogo de modelos, então a I/O trocada aqui é o `codex_appserver.perguntar`.
"""
import json

import pytest

from app import codex_appserver, cotas

# Cópia da resposta real desta máquina em 30/08/2026 (campos que não usamos foram cortados).
# Detalhes que quebram parser ingênuo: o percentual já vem PRONTO (`usedPercent`, não used/cap), a
# janela se identifica pela DURAÇÃO em minutos, e `resetsAt` é epoch em SEGUNDOS — não em
# milissegundos como no CommandCode.
_RATE_LIMITS = {
    "rateLimits": {
        "limitId": "codex",
        "primary": {"usedPercent": 5, "windowDurationMins": 300, "resetsAt": 1788107727},
        "secondary": {"usedPercent": 1, "windowDurationMins": 10080, "resetsAt": 1788655220},
        "credits": {"hasCredits": False, "unlimited": False, "balance": "0"},
        "planType": "plus",
    },
}


@pytest.fixture(autouse=True)
def _cache_limpo():
    """A presença da credencial é cacheada pelo mtime (custo do tick do SSE). Sem zerar, um caso
    que muda o HOME herdaria a resposta do anterior e passaria por acidente."""
    cotas._cred_codex_cache = None
    yield
    cotas._cred_codex_cache = None


def _home(monkeypatch, alvo):
    """Quem resolve a pasta do Codex é o `codex_appserver`, não o `cotas` — patchar ali é o que diz
    a verdade sobre o caminho testado (`cotas.Path` é a mesma classe e funcionaria por acidente)."""
    monkeypatch.setattr(codex_appserver.Path, "home", staticmethod(lambda: alvo))


def _auth(home, tokens=True):
    d = home / ".codex"
    d.mkdir(parents=True, exist_ok=True)
    corpo = {"auth_mode": "chatgpt", "OPENAI_API_KEY": None}
    if tokens:
        corpo["tokens"] = {"access_token": "at-x", "refresh_token": "rt-x", "account_id": "acc-1"}
    (d / "auth.json").write_text(json.dumps(corpo), encoding="utf-8")
    return d


def test_le_as_duas_janelas(monkeypatch):
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw:_RATE_LIMITS)
    estado, janelas, motivo = cotas._ler_codex()
    assert (estado, motivo) == ("lida", None)
    assert [(j.rotulo, j.pct) for j in janelas] == [("5h", 5.0), ("7d", 1.0)]
    # Segundos, não milissegundos: dividir por 1000 aqui poria o reset em 1970.
    assert janelas[0].reset_ts == 1788107727


def test_pergunta_o_metodo_de_cota(monkeypatch):
    vistos = []
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda m, **kw:(vistos.append(m), _RATE_LIMITS)[1])
    cotas._ler_codex()
    assert vistos == ["account/rateLimits/read"]


def test_janela_ausente_some_em_vez_de_zerar(monkeypatch):
    """Conta sem a janela semanal não pode desenhar 0% — 0% é uma afirmação, e falsa."""
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw:{
        "rateLimits": {"primary": _RATE_LIMITS["rateLimits"]["primary"], "secondary": None}})
    estado, janelas, _ = cotas._ler_codex()
    assert (estado, [j.rotulo for j in janelas]) == ("lida", ["5h"])


def test_resposta_sem_janela_nenhuma_nao_e_lida(monkeypatch):
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw:{"rateLimits": {}})
    assert cotas._ler_codex() == ("indisponivel", [], "formato-desconhecido")


def test_binario_ausente_tem_motivo_proprio(monkeypatch):
    """"não achei o codex" não é "o codex falhou" — e nenhum dos dois pode derrubar a lista das
    outras contas."""
    def some(m, **kw):
        raise codex_appserver.CodexAusente("nao achei o executavel `codex`")
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", some)
    assert cotas._ler_codex() == ("indisponivel", [], "codex-ausente")


@pytest.mark.parametrize("erro", [RuntimeError("nao respondeu"), OSError("boom")])
def test_falha_de_leitura_nao_levanta(monkeypatch, erro):
    def quebra(m, **kw):
        raise erro
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", quebra)
    estado, janelas, motivo = cotas._ler_codex()
    assert (estado, janelas) == ("indisponivel", [])
    assert motivo == "sem-resposta"


def test_credencial_nova_no_disco_e_notada(monkeypatch, tmp_path):
    """O cache é pelo mtime, e a leitura roda por sessão a cada varredura: fazer login no Codex
    não pode exigir reiniciar o backend pra a linha aparecer."""
    _home(monkeypatch, tmp_path)
    cotas._cred_codex_cache = None
    assert cotas.id_conta_codex() is None
    _auth(tmp_path)
    assert cotas.id_conta_codex() is not None


def test_o_teto_de_tempo_e_o_das_outras_fontes(monkeypatch):
    """`_atualizar` espera TODAS as leituras juntas: uma fonte com teto maior que as outras vira o
    tempo de resposta do `/api/cotas` inteiro. O padrão do módulo (30s) é do catálogo, que é tela
    aberta por gente."""
    vistos = []
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda m, timeout: (vistos.append(timeout), _RATE_LIMITS)[1])
    cotas._ler_codex()
    assert vistos == [cotas._HTTP_TIMEOUT]


def test_sem_credencial_no_disco_nem_pergunta(monkeypatch, tmp_path):
    """Cobre a corrida (logout entre montar a fonte e ler): perguntar custa um processo de ~1,2s, e
    sem `auth.json` com tokens a linha diz "não há credencial" em vez de "falhou"."""
    _auth(tmp_path, tokens=False)
    _home(monkeypatch, tmp_path)
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda m, **kw:pytest.fail("nao podia perguntar sem credencial"))
    assert cotas._ler_codex() == ("sem_credencial", [], None)


def test_o_id_da_conta_e_o_mesmo_da_fonte(monkeypatch, tmp_path):
    """A pílula do topo procura no `/api/cotas` a linha do `conta` da sessão. Ids diferentes nos
    dois lugares fariam ela cair no pior-geral numa sessão cuja cota o app sabe ler."""
    _home(monkeypatch, tmp_path)
    assert cotas.id_conta_codex() is None
    _auth(tmp_path)
    fonte = next(f for f in cotas._fontes() if f.provedor == "codex")
    assert fonte.chave == cotas.id_conta_codex()


def test_codex_home_manda_no_caminho(monkeypatch, tmp_path):
    """Quem move a pasta do Codex move a credencial junto — o mesmo `CODEX_HOME` que o lançador
    respeita."""
    outro = tmp_path / "alhures"
    _auth(outro)
    _home(monkeypatch, tmp_path / "vazio")
    monkeypatch.setenv("CODEX_HOME", str(outro / ".codex"))
    assert cotas.id_conta_codex() == f"codex:{outro / '.codex'}"


def test_a_fonte_so_existe_com_credencial(monkeypatch, tmp_path):
    """Quem não usa Codex não ganha uma linha vazia no painel — nem paga o processo."""
    _home(monkeypatch, tmp_path)
    assert not [f for f in cotas._fontes() if f.provedor == "codex"]
    _auth(tmp_path)
    fontes = [f for f in cotas._fontes() if f.provedor == "codex"]
    assert len(fontes) == 1
    assert fontes[0].chave.startswith("codex:")
