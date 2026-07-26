"""Normalização do /v1/models: os provedores concordam em `id` e `context_length` e divergem no resto."""
import io
import json
import urllib.error
import urllib.request

import pytest

from app import engine_probe


def test_normaliza_o_formato_do_kimi(monkeypatch):
    # Medido em api.kimi.com/coding: capability vem como supports_image_in.
    bruto = {"data": [{"id": "k3", "context_length": 262144, "supports_image_in": True}]}
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: bruto)
    assert engine_probe.listar_modelos("https://api.kimi.com/coding", "sk-x") == [
        {"id": "k3", "context_length": 262144, "vision": True}
    ]


def test_normaliza_o_formato_do_omniroute(monkeypatch):
    # Medido em ai.omniwise.com.br: capability vem dentro de "capabilities" e sem flag de imagem.
    bruto = {"data": [{"id": "cx/gpt-5.6-sol", "context_length": 500000,
                       "capabilities": {"tool_calling": True, "thinking": True}}]}
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: bruto)
    assert engine_probe.listar_modelos("https://ai.omniwise.com.br", "sk-x") == [
        {"id": "cx/gpt-5.6-sol", "context_length": 500000, "vision": None}
    ]


def test_modelo_sem_id_e_ignorado(monkeypatch):
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: {"data": [{"context_length": 1}]})
    assert engine_probe.listar_modelos("https://x.y", "sk-x") == []


def test_resposta_sem_data_estoura_com_mensagem(monkeypatch):
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: {"nada": 1})
    with pytest.raises(RuntimeError, match="inesperada"):
        engine_probe.listar_modelos("https://x.y", "sk-x")


# As 3 testes abaixo exercitam _buscar de verdade (não mockado) — os testes acima mockam _buscar,
# então nunca passavam pela tradução real de urllib.error para RuntimeError. É essa tradução que
# faz uma key errada chegar na tela como "401 Invalid Authentication" em vez de "não respondeu".


def test_buscar_traduz_http_error_com_a_mensagem_do_provedor(monkeypatch):
    # Corpo real visto contra api.kimi.com/coding com uma key errada.
    corpo = json.dumps({"error": {"message": "The API Key appears to be invalid or may have expired."}}).encode()

    def _explode(req, timeout=None):
        raise urllib.error.HTTPError(req.full_url, 401, "Unauthorized", None, io.BytesIO(corpo))

    monkeypatch.setattr(urllib.request, "urlopen", _explode)
    with pytest.raises(RuntimeError) as exc:
        engine_probe._buscar("https://api.kimi.com/coding", "sk-errada")
    assert "401" in str(exc.value)
    assert "API Key appears to be invalid" in str(exc.value)


def test_buscar_traduz_url_error_em_nao_foi_possivel_falar(monkeypatch):
    def _explode(req, timeout=None):
        raise urllib.error.URLError("nome não resolve")

    monkeypatch.setattr(urllib.request, "urlopen", _explode)
    with pytest.raises(RuntimeError, match="não foi possível falar com o provedor"):
        engine_probe._buscar("https://x.y", "sk-x")


def test_buscar_caminho_feliz_com_bytes_invalidos_nao_estoura_unicodedecodeerror(monkeypatch):
    # UnicodeDecodeError é ValueError, não URLError/OSError/TimeoutError: sem errors="replace" ela
    # escapa do except e vira 500 com traceback em vez do 502 com a mensagem do provedor.
    corpo = b'{"data": [{"id": "k3"}]}\xff'

    class _Resposta:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def read(self):
            return corpo

    monkeypatch.setattr(urllib.request, "urlopen", lambda req, timeout=None: _Resposta())
    with pytest.raises(RuntimeError, match="não-JSON"):
        engine_probe._buscar("https://x.y", "sk-x")


# ---------------------------------------------------------------------------
# Fix wave (pré-push), item 2: `-> dict[str, Any]` é só o type hint. Um provedor que devolve um
# array ou escalar no topo satisfaz a assinatura e não o runtime — listar_modelos() chamaria
# `.get("data")` num objeto que não tem `.get`, e isso é um 500 com traceback, não o 502 com a
# mensagem do provedor que este módulo promete em todo outro caminho malformado.
# ---------------------------------------------------------------------------

def test_resposta_top_level_lista_estoura_runtimeerror_nao_attributeerror(monkeypatch):
    corpo = json.dumps([1, 2, 3]).encode()

    class _Resposta:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def read(self):
            return corpo

    monkeypatch.setattr(urllib.request, "urlopen", lambda req, timeout=None: _Resposta())
    with pytest.raises(RuntimeError, match="inesperado"):
        engine_probe._buscar("https://x.y", "sk-x")


def test_resposta_top_level_escalar_estoura_runtimeerror_nao_attributeerror(monkeypatch):
    corpo = json.dumps("oops").encode()

    class _Resposta:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def read(self):
            return corpo

    monkeypatch.setattr(urllib.request, "urlopen", lambda req, timeout=None: _Resposta())
    with pytest.raises(RuntimeError, match="inesperado"):
        engine_probe._buscar("https://x.y", "sk-x")


# ---------------------------------------------------------------------------
# Fix wave (pré-push), item 3: a key vai pro header Authorization sem checar \r/\n. urllib recusa
# o header ("Invalid header value b'Bearer sk-x\r\n...'") mas a mensagem ECOA a key crua, e isso
# vira traceback no log do uvicorn/journal (POST /api/engines/modelos só pega RuntimeError e
# relança pra logar). Validar ANTES de montar o Request barra o vazamento na origem.
# ---------------------------------------------------------------------------

def test_buscar_recusa_key_com_crlf_antes_de_montar_o_request():
    with pytest.raises(ValueError) as exc:
        engine_probe._buscar("https://x.y", "sk-secreta\r\nX-Evil: 1")
    assert "sk-secreta" not in str(exc.value)


def test_buscar_recusa_base_url_com_crlf_antes_de_montar_o_request():
    with pytest.raises(ValueError) as exc:
        engine_probe._buscar("https://x.y\r\nEvil: 1", "sk-x")
    assert "sk-x" not in str(exc.value)


def test_buscar_caminho_feliz_monta_o_header_e_le_o_json(monkeypatch):
    corpo = json.dumps({"data": [{"id": "k3"}]}).encode()

    class _Resposta:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def read(self):
            return corpo

    def _ok(req, timeout=None):
        assert req.get_header("Authorization") == "Bearer sk-x"
        assert req.full_url == "https://api.kimi.com/coding/v1/models"
        return _Resposta()

    monkeypatch.setattr(urllib.request, "urlopen", _ok)
    assert engine_probe._buscar("https://api.kimi.com/coding", "sk-x") == {"data": [{"id": "k3"}]}
