import json

import pytest

from app import runtime_config
from app.narrar import narrar, prompt_narrar, eh_instrucao_padrao, NarrarError


def _com_chave(monkeypatch):
    monkeypatch.setattr(runtime_config, "get", lambda campo: "k" if campo == "groq_api_key" else None)


def _sem_chave(monkeypatch):
    monkeypatch.setattr(runtime_config, "get", lambda campo: "")


def _config(monkeypatch, valores: dict):
    """Fake runtime_config.get orientado a dict, pra testar _provedor() sem tocar no arquivo real."""
    monkeypatch.setattr(runtime_config, "get", lambda campo: valores.get(campo))


def test_eh_instrucao_padrao():
    assert eh_instrucao_padrao("")
    assert eh_instrucao_padrao("  ")
    assert eh_instrucao_padrao("Ler como está")
    assert not eh_instrucao_padrao("explica o código")


def test_sem_instrucao_nao_chama_a_groq(monkeypatch):
    # CRITICO: o caminho comum (sem instrucao) nao pode gastar token nem latencia na Groq.
    def _explode(*a, **k):
        raise AssertionError("urlopen nao deveria ter sido chamado")
    monkeypatch.setattr("app.narrar.urllib.request.urlopen", _explode)
    assert narrar("texto original", [], "") == "texto original"
    assert narrar("texto original", [], "ler como está") == "texto original"


def test_sem_chave_levanta_503(monkeypatch):
    _sem_chave(monkeypatch)
    with pytest.raises(NarrarError) as ei:
        narrar("texto", [], "explique o código")
    assert ei.value.status == 503


def test_prompt_narrar_manda_instrucao_como_dado_no_prompt_do_usuario():
    prompt = prompt_narrar("texto sel", ["const x = 1;"], "explica isso")
    assert "explica isso" in prompt
    assert "const x = 1;" in prompt
    assert "texto sel" in prompt


def test_narrar_com_instrucao_chama_a_groq_e_devolve_o_texto(monkeypatch):
    _com_chave(monkeypatch)
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": "  texto tratado  "}}]}).encode()

    def fake_urlopen(req, timeout=None):
        captured["body"] = req.data
        return FakeResp()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", fake_urlopen)
    r = narrar("texto sel", [], "explica isso")
    assert r == "texto tratado"          # strip() aplicado
    assert b"explica isso" in captured["body"]


def test_request_vai_pra_url_certa_e_com_user_agent(monkeypatch):
    # O Cloudflare da Groq bane o UA padrao do urllib com 403 code 1010. Perder este header da
    # producao quebrada com a suite verde — por isso ele tem teste proprio ANTES da refatoracao.
    _com_chave(monkeypatch)
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": "ok"}}]}).encode()

    def fake_urlopen(req, timeout=None):
        captured["req"] = req
        return FakeResp()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", fake_urlopen)
    narrar("texto", [], "explica isso")
    req = captured["req"]
    assert req.full_url.endswith("/chat/completions")
    assert req.headers["User-agent"] == "claude-pocket/1.0"
    assert req.headers["Authorization"].startswith("Bearer ")


def test_corpo_manda_modelo_e_temperatura(monkeypatch):
    _com_chave(monkeypatch)
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": "ok"}}]}).encode()

    def fake_urlopen(req, timeout=None):
        captured["req"] = req
        return FakeResp()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", fake_urlopen)
    narrar("texto", [], "explica isso")
    corpo = json.loads(captured["req"].data)
    assert corpo["model"]
    assert corpo["temperature"] == 0.3


def test_endpoint_custom_nao_herda_a_chave_da_groq(monkeypatch):
    # CRITICO: a chave da Groq so pode ir pro endpoint da Groq. Sem essa amarra, um llm_base_url
    # custom sem llm_api_key preenchida mandaria o segredo da Groq pra um host que nao o emitiu.
    _config(monkeypatch, {"llm_base_url": "https://outro.provedor/v1", "groq_api_key": "chave-groq"})

    def _explode(*a, **k):
        raise AssertionError("urlopen nao deveria ter sido chamado sem chave efetiva")
    monkeypatch.setattr("app.narrar.urllib.request.urlopen", _explode)
    with pytest.raises(NarrarError) as ei:
        narrar("texto", [], "explica isso")
    assert ei.value.status == 503


def test_endpoint_padrao_herda_a_chave_da_groq(monkeypatch):
    _config(monkeypatch, {"groq_api_key": "chave-groq"})
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": "ok"}}]}).encode()

    def fake_urlopen(req, timeout=None):
        captured["req"] = req
        return FakeResp()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", fake_urlopen)
    narrar("texto", [], "explica isso")
    assert captured["req"].headers["Authorization"] == "Bearer chave-groq"


def test_base_url_e_modelo_custom_chegam_na_request(monkeypatch):
    _config(monkeypatch, {
        "llm_base_url": "https://outro.provedor/v1",
        "llm_api_key": "chave-custom",
        "llm_model": "modelo-custom",
    })
    captured = {}

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": "ok"}}]}).encode()

    def fake_urlopen(req, timeout=None):
        captured["req"] = req
        return FakeResp()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", fake_urlopen)
    narrar("texto", [], "explica isso")
    req = captured["req"]
    assert req.full_url == "https://outro.provedor/v1/chat/completions"
    assert req.headers["Authorization"] == "Bearer chave-custom"
    corpo = json.loads(req.data)
    assert corpo["model"] == "modelo-custom"


def test_resposta_sem_texto_esperado_levanta_502(monkeypatch):
    _com_chave(monkeypatch)

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self): return json.dumps({"choices": []}).encode()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", lambda req, timeout=None: FakeResp())
    with pytest.raises(NarrarError) as ei:
        narrar("texto", [], "explica")
    assert ei.value.status == 502
