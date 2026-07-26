"""Motores de modelo: um JSON por baixo, env derivado por cima.

O que esta suíte trava: valor com quebra de linha não entra (ele vira `export` no shell), os 6 nomes
de modelo andam JUNTOS (faltar um quebra subagent, calado), a janela usa MAX_CONTEXT_TOKENS (a outra
var é inerte — medido nos dois provedores), base_url insegura não entra (a key sairia em claro) e
motor desconhecido estoura em vez de devolver env vazio (a sessão subiria na conta Anthropic achando
que é o motor pedido).
"""
import ast
import os
import pathlib
import subprocess
import sys

import pytest

from app import engines as eng

CLI = pathlib.Path(__file__).resolve().parents[2] / "scripts" / "cp-engine"


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(eng, "caminho", lambda: tmp_path / "engines.json")
    yield


def _kimi() -> dict:
    return {
        "label": "Kimi Code · K3",
        "base_url": "https://api.kimi.com/coding",
        "api_key": "sk-kimi-abcdefgh1234",
        "model": "k3",
        "context_window": 262144,
        "vision": True,
    }


def test_env_repete_o_modelo_nas_seis_vars():
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    seis = [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ]
    assert [env[k] for k in seis] == ["k3"] * 6


def test_env_usa_auth_token_e_nunca_api_key():
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    assert env["ANTHROPIC_AUTH_TOKEN"] == "sk-kimi-abcdefgh1234"
    assert "ANTHROPIC_API_KEY" not in env


def test_env_marca_o_motor_para_o_proc_reconhecer():
    eng.salvar("kimi", _kimi())
    assert eng.env_de("kimi")["CP_ENGINE"] == "kimi"


def test_janela_usa_max_context_tokens_e_nao_a_var_inerte():
    # Medido nos dois provedores: AUTO_COMPACT_WINDOW não move a janela (o /context seguia em 200k) e
    # MAX_CONTEXT_TOKENS move. Var de doc de terceiro sem efeito medido não entra.
    eng.salvar("kimi", _kimi())
    env = eng.env_de("kimi")
    assert env["CLAUDE_CODE_MAX_CONTEXT_TOKENS"] == "262144"
    assert "CLAUDE_CODE_AUTO_COMPACT_WINDOW" not in env


def test_sem_janela_nao_inventa_a_var():
    d = _kimi()
    del d["context_window"]
    eng.salvar("kimi", d)
    assert "CLAUDE_CODE_MAX_CONTEXT_TOKENS" not in eng.env_de("kimi")


def test_motor_desconhecido_estoura_em_vez_de_env_vazio():
    with pytest.raises(KeyError):
        eng.env_de("nao-existe")


def test_valor_com_quebra_de_linha_e_recusado():
    # `cp-engine --env` imprime CHAVE=VALOR por linha e o shell dá export nisso: um \n na key
    # exportaria uma variável arbitrária (ex: PATH) no shell que vai rodar o claude.
    for campo, valor in (("api_key", "sk-x\nPATH=/tmp/evil"),
                         ("model", "k3\nHOME=/tmp"),
                         ("base_url", "https://a.b\nX=1")):
        d = _kimi()
        d[campo] = valor
        with pytest.raises(ValueError, match="linha"):
            eng.salvar("x", d)


def test_http_publico_e_recusado():
    d = _kimi()
    d["base_url"] = "http://api.kimi.com/coding"
    with pytest.raises(ValueError, match="https"):
        eng.salvar("kimi", d)


def test_http_em_loopback_e_em_rede_privada_e_aceito():
    # É o caso do proxy tradutor local (LiteLLM) e de um gateway na LAN.
    for url in ("http://127.0.0.1:4000", "http://192.168.1.50:4000", "http://localhost:8080"):
        d = _kimi()
        d["base_url"] = url
        eng.salvar("proxy", d)
        assert eng.env_de("proxy")["ANTHROPIC_BASE_URL"] == url


def test_base_url_perde_a_barra_final():
    # O CC monta {base}/v1/messages; barra sobrando geraria //v1/messages.
    d = _kimi()
    d["base_url"] = "https://api.kimi.com/coding/"
    eng.salvar("kimi", d)
    assert eng.env_de("kimi")["ANTHROPIC_BASE_URL"] == "https://api.kimi.com/coding"


def test_campo_obrigatorio_faltando_estoura():
    for faltando in ("base_url", "api_key", "model"):
        d = _kimi()
        del d[faltando]
        with pytest.raises(ValueError, match=faltando):
            eng.salvar("x", d)


def test_campo_desconhecido_e_descartado():
    d = _kimi()
    d["rode_isto"] = "rm -rf /"
    eng.salvar("kimi", d)
    assert "rode_isto" not in eng.listar()["kimi"]


def test_nome_de_motor_e_sanitizado():
    for ruim in ("../fuga", "COM MAIUSCULA", "", "com espaco"):
        with pytest.raises(ValueError, match="nome"):
            eng.salvar(ruim, _kimi())


def test_remover_devolve_se_existia():
    eng.salvar("kimi", _kimi())
    assert eng.remover("kimi") is True
    assert eng.remover("kimi") is False


def test_arquivo_nasce_0600():
    eng.salvar("kimi", _kimi())
    assert (eng.caminho().stat().st_mode & 0o777) == 0o600


def test_arquivo_corrompido_nao_derruba_a_leitura():
    eng.caminho().write_text("{lixo", encoding="utf-8")
    assert eng.listar() == {}


def test_gravar_nao_perde_o_motor_do_vizinho():
    eng.salvar("kimi", _kimi())
    d = _kimi()
    d["model"] = "codex/gpt-5.6-sol"
    d["base_url"] = "https://ai.omniwise.com.br"
    eng.salvar("omniroute", d)
    assert set(eng.listar()) == {"kimi", "omniroute"}


def test_modulo_e_stdlib_pura():
    # O cp-engine importa este módulo com o python3 do SISTEMA (sem venv). Um import de app.config
    # puxaria pydantic e quebraria o terminal, deixando só o app funcionando — falha assimétrica,
    # chata de diagnosticar. Sentinela, não prova: barra os culpados conhecidos.
    fonte = pathlib.Path(eng.__file__).read_text(encoding="utf-8")
    arvore = ast.parse(fonte)
    importados = {
        n.module.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.ImportFrom) and n.module
    } | {
        a.name.split(".")[0] for n in ast.walk(arvore)
        if isinstance(n, ast.Import) for a in n.names
    }
    assert not (importados & {"app", "pydantic", "fastapi", "httpx", "httpx2"})
