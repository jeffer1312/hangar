import json

import pytest

from app import runtime_config, narrar
from app.narrar import NarrarError


@pytest.fixture(autouse=True)
def _config_isolada(monkeypatch, tmp_path):
    """Toda a suite le config de um diretorio VAZIO, nunca do ~/.claude da maquina.

    Sem isto, `limpar_ditado` -> `estilo_efetivo` -> `estilo_ditado` -> `runtime_config.get`
    abria o runtime-config.json de PRODUCAO desta maquina, e as travas testadas eram as do estilo
    que estivesse salvo ali. Os testes de guarda passavam porque o valor no disco era `prosa`, igual
    ao padrao — coincidencia de ambiente, nao algo que a suite fixa: trocando pra `limpar` (que sobe
    a cobertura de 0.60 pra 0.80), um deles passava a ser rejeitado por OUTRO motivo e a assercao
    quebrava. Mesmo isolamento que test_runtime_config.py ja fazia."""
    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: str(tmp_path))




def _com_chave(monkeypatch):
    monkeypatch.setattr(runtime_config, "get", lambda campo: "k" if campo == "groq_api_key" else None)


def _sem_chave(monkeypatch):
    monkeypatch.setattr(runtime_config, "get", lambda campo: "")


def _config(monkeypatch, valores: dict):
    """Fake runtime_config.get orientado a dict, pra testar _provedor() sem tocar no arquivo real."""
    monkeypatch.setattr(runtime_config, "get", lambda campo: valores.get(campo))


def test_eh_instrucao_padrao():
    assert narrar.eh_instrucao_padrao("")
    assert narrar.eh_instrucao_padrao("  ")
    assert narrar.eh_instrucao_padrao("Ler como está")
    assert not narrar.eh_instrucao_padrao("explica o código")


def test_sem_instrucao_nao_chama_a_groq(monkeypatch):
    # CRITICO: o caminho comum (sem instrucao) nao pode gastar token nem latencia na Groq.
    def _explode(*a, **k):
        raise AssertionError("urlopen nao deveria ter sido chamado")
    monkeypatch.setattr("app.narrar.urllib.request.urlopen", _explode)
    assert narrar.narrar("texto original", [], "") == "texto original"
    assert narrar.narrar("texto original", [], "ler como está") == "texto original"


def test_sem_chave_levanta_503(monkeypatch):
    _sem_chave(monkeypatch)
    with pytest.raises(NarrarError) as ei:
        narrar.narrar("texto", [], "explique o código")
    assert ei.value.status == 503
    assert "Configuracoes -> Voz" in ei.value.detail
    assert "Groq" not in ei.value.detail


def test_sem_chave_endpoint_custom_levanta_503(monkeypatch):
    # Endpoint proprio: agora e a Chave do LLM que falta, nao a da Groq.
    _config(monkeypatch, {"llm_base_url": "https://outro.provedor/v1"})
    with pytest.raises(NarrarError) as ei:
        narrar.narrar("texto", [], "explique o código")
    assert ei.value.status == 503
    assert "Chave do LLM" in ei.value.detail
    assert "chave da Groq" not in ei.value.detail


def test_prompt_narrar_manda_instrucao_como_dado_no_prompt_do_usuario():
    prompt = narrar.prompt_narrar("texto sel", ["const x = 1;"], "explica isso")
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
    r = narrar.narrar("texto sel", [], "explica isso")
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
    narrar.narrar("texto", [], "explica isso")
    req = captured["req"]
    assert req.full_url.endswith("/chat/completions")
    assert req.headers["User-agent"] == "hangar/1.0"
    assert req.headers["Authorization"].startswith("Bearer ")


def test_sem_plano_b_a_falha_do_provedor_vira_502_sem_claude_p(monkeypatch):
    # Quem chama em rajada (traducao do pensamento) nao pode subir um `claude -p` por falha:
    # foi isso, com o provedor em 500, que saturou o backend (dezenas de processos em paralelo).
    _com_chave(monkeypatch)
    import urllib.error
    def fake_urlopen(req, timeout=None):
        raise urllib.error.HTTPError(req.full_url, 500, "Internal", {}, None)
    monkeypatch.setattr("app.narrar.urllib.request.urlopen", fake_urlopen)
    with pytest.raises(NarrarError) as e:
        narrar.chamar_chat("s", "p", temperature=0.1, timeout=5)
    assert e.value.status == 502 and "500" in e.value.detail


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
    narrar.narrar("texto", [], "explica isso")
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
        narrar.narrar("texto", [], "explica isso")
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
    narrar.narrar("texto", [], "explica isso")
    assert captured["req"].headers["Authorization"] == "Bearer chave-groq"


def test_endpoint_padrao_ignora_llm_api_key_de_outro_provedor(monkeypatch):
    # CRITICO: o caso que motivou a mudanca. Uma llm_api_key sobrando de configuracao anterior
    # (endpoint custom) NUNCA pode vazar pra api.groq.com — quem vai no Authorization com endpoint
    # padrao e sempre a chave da Groq, mesmo com llm_api_key preenchida.
    _config(monkeypatch, {"llm_api_key": "chave-de-outro-provedor", "groq_api_key": "chave-groq"})
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
    narrar.narrar("texto", [], "explica isso")
    assert captured["req"].headers["Authorization"] == "Bearer chave-groq"


def test_transcricao_custom_nao_envia_sua_chave_ao_llm_padrao(monkeypatch):
    """Serviços independentes: a chave do áudio nunca pode vazar para o host do LLM padrão."""
    _config(monkeypatch, {
        "transcription_base_url": "https://fala.exemplo/v1",
        "groq_api_key": "chave-da-transcricao-custom",
    })
    assert narrar._provedor() == (narrar.PADRAO_BASE_URL, "", narrar.PADRAO_MODELO)


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
    narrar.narrar("texto", [], "explica isso")
    req = captured["req"]
    assert req.full_url == "https://outro.provedor/v1/chat/completions"
    assert req.headers["Authorization"] == "Bearer chave-custom"
    corpo = json.loads(req.data)
    assert corpo["model"] == "modelo-custom"


def _corpo_enviado(monkeypatch) -> dict:
    """Dispara uma narracao e devolve o JSON que foi pro provedor."""
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
    narrar.narrar("texto", [], "explica isso")
    return json.loads(captured["req"].data)


def test_sem_esforco_configurado_o_payload_nao_muda(monkeypatch):
    # `reasoning_effort` nao e universal: mandar a chave pra um provedor que nao a conhece e um 400
    # que derruba a limpeza inteira. Vazio (o padrao) tem que sair do payload por completo — nao
    # basta ir como "" ou null.
    _config(monkeypatch, {"groq_api_key": "k"})
    assert "reasoning_effort" not in _corpo_enviado(monkeypatch)


def test_esforco_configurado_vai_no_payload(monkeypatch):
    # "none" e o valor que importa: desliga o raciocinio num modelo que raciocina, e foi o que fez
    # o deepseek-v4-flash sair de 6,4s (3/15 estourando o timeout de 8s) pra 1,8s sem estouro.
    _config(monkeypatch, {"groq_api_key": "k", "llm_reasoning_effort": "none"})
    assert _corpo_enviado(monkeypatch)["reasoning_effort"] == "none"


def test_resposta_sem_texto_esperado_levanta_502(monkeypatch):
    _com_chave(monkeypatch)

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self): return json.dumps({"choices": []}).encode()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", lambda req, timeout=None: FakeResp())
    with pytest.raises(NarrarError) as ei:
        narrar.narrar("texto", [], "explica")
    assert ei.value.status == 502


def _provedor_500(monkeypatch):
    import urllib.error

    def _erro(req, timeout=None):
        raise urllib.error.HTTPError(req.full_url, 500, "Internal Server Error", {}, None)

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", _erro)


def _claude_responde(monkeypatch, texto: str, visto: dict):
    class R:
        returncode, stdout, stderr = 0, texto, ""

    def _run(cmd, **k):
        visto["cmd"], visto["input"], visto["cwd"] = cmd, k.get("input"), k.get("cwd")
        return R()

    monkeypatch.setattr("subprocess.run", _run)


def test_external_failure_never_spends_a_claude_account(monkeypatch):
    _com_chave(monkeypatch)
    _provedor_500(monkeypatch)
    _claude_responde(monkeypatch, "Resposta de outra conta.", {})
    with pytest.raises(NarrarError) as error:
        narrar.chamar_chat("sys", "texto cru", temperature=0, timeout=60)
    assert error.value.status == 502
    assert "provedor 500" in error.value.detail






def test_sem_chave_nao_gasta_a_assinatura(monkeypatch):
    # 503 e config ausente: e pra corrigir na tela, nao pra mascarar gastando cota da assinatura.
    _sem_chave(monkeypatch)
    monkeypatch.setattr("subprocess.run",
                        lambda *a, **k: pytest.fail("nao devia chamar o claude"))
    with pytest.raises(NarrarError) as ei:
        narrar.chamar_chat("sys", "texto", temperature=0, timeout=60)
    assert ei.value.status == 503


























def test_content_none_vira_502_honesto_nao_attributeerror(monkeypatch):
    # Payload real de gateway compativel com OpenAI: content nulo quando o modelo so devolveu
    # tool_calls, ou foi filtrado. .strip() em None e AttributeError, fora do tuple antigo.
    _com_chave(monkeypatch)

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": None}}]}).encode()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", lambda req, timeout=None: FakeResp())
    with pytest.raises(NarrarError) as ei:
        narrar.narrar("texto", [], "explica")
    assert ei.value.status == 502


def test_content_lista_de_partes_vira_502_honesto_nao_attributeerror(monkeypatch):
    # Payload real de varios proxies: content como lista de partes, nao string. .strip() numa
    # lista tambem e AttributeError.
    _com_chave(monkeypatch)

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps(
                {"choices": [{"message": {"content": [{"type": "text", "text": "oi"}]}}]}
            ).encode()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", lambda req, timeout=None: FakeResp())
    with pytest.raises(NarrarError) as ei:
        narrar.narrar("texto", [], "explica")
    assert ei.value.status == 502






# --- guarda de sentido: rejeita limpeza que INTRODUZ palavra (a inversao "critica -> resposta") ---
# Caso real, medido ao vivo em 2026-08-01 (audio 1785615417-f9f5b3.m4a). O usuario criticou o
# assistente ("para de falar de forma dificil"); a "limpeza" devolveu o assistente se
# autodefendendo ("eu nao estou falando de forma dificil") — sentido invertido, e as duas travas
# de tamanho (piso 0.5x, teto 1.5x) nao pegam porque a razao ficou em 0.73x.


# Limpeza honesta de verdade (audio 1785615822-81b5eb.m4a) — 1582 caracteres, so tira hesitacao/
# repeticao e pontua. NAO pode ser rejeitada: se a guarda pegar isso, o usuario perde a feature.












# Buraco achado pelo cacador de falha calada em 14/08/2026, reproduzido antes de consertar: um
# ditado feito SO de muleta ("e ai cara tipo assim entao bom") nao tem palavra de conteudo, entao
# _cobertura devolve 1.0 por definicao (nao ha o que perder) e _conteudo_novo fica sozinho olhando
# quantidade. Com o piso de encolhimento valendo so em texto longo, as QUATRO travas passavam e o
# ditado da pessoa sumia com erro=None — o app reportando sucesso.

















def test_briefing_pode_ter_provedor_proprio(monkeypatch):
    """Limpar e prosa querem rapidez; o briefing quer o modelo que estrutura melhor, e pode demorar
    mais. Endpoint de briefing VAZIO tem que continuar caindo no provedor de sempre — senao quem
    nunca configurou isso perderia a limpeza."""
    cfg = {"llm_briefing_base_url": "https://opencode.ai/zen/v1",
           "llm_briefing_api_key": "sk-briefing",
           "llm_briefing_model": "muse-spark-1.2-contributor-free",
           "groq_api_key": "sk-groq"}
    monkeypatch.setattr(narrar.runtime_config, "get", lambda campo: cfg.get(campo))
    assert narrar._provedor() == (narrar.PADRAO_BASE_URL, "sk-groq", narrar.PADRAO_MODELO)
    assert narrar._provedor("briefing") == (
        "https://opencode.ai/zen/v1", "sk-briefing", "muse-spark-1.2-contributor-free")

    cfg["llm_briefing_base_url"] = ""
    assert narrar._provedor("briefing") == narrar._provedor()




# O caso real de 24/08/2026: o usuario ditou ~3700 chars de contexto sobre uma PM, escolheu
# "briefing", e o texto voltou RESUMIDO — sumiram as ressalvas dele, os motivos e o que ele deixou
# em aberto. Nenhuma trava disparou, entao ele nem aviso teve: teria que ditar tudo de novo.
#
# Os tres textos abaixo sao artefatos REAIS daquela medicao (deepseek-v4-flash, temperatura 0), nao
# exemplos escritos a mao: o cru e o ditado dele, o "resumido" e uma das 3 saidas do fecho antigo
# (cobertura 0,51) e o "completo" e a PIOR das 7 saidas do fecho novo (cobertura 0,72) — a pior de
# proposito, porque quem tem que passar pelo piso e ela, nao a melhor.




# O segundo caso real (26/08/2026), e o que recalibrou os limites: o usuario ditou 2:25 sobre
# unificar logs, clicou "Briefing" e o texto voltou CRU com aviso, duas vezes seguidas — pra ele o
# botao estava quebrado. Os dois textos abaixo sao a medicao real (mesmo audio, mesmo provedor):
# o briefing esta INTEIRO (nao falta nenhum assunto), e ainda assim tinha cobertura 0,577, dentro
# do intervalo do defeito de 24/08. A razao esta no proprio texto: ele SOLETRA caminho ("pss barra
# logs barra prom web"), e cada "barra" dita vira uma barra escrita.
