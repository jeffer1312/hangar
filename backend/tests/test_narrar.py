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
    # Endpoint padrao (sem base_url): a mensagem tem que mandar pro campo que _provedor() realmente
    # le nesse ramo (chave da Groq), nunca pra Chave do LLM — esse campo nao e lido aqui.
    assert "chave da Groq" in ei.value.detail
    assert "Chave do LLM" not in ei.value.detail


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


def test_ditado_curto_nao_chama_o_provedor(monkeypatch):
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: pytest.fail("nao devia chamar"))
    assert narrar.limpar_ditado("pode fazer push") == ("pode fazer push", None)


def test_comando_barra_nao_e_alterado(monkeypatch):
    # A limpeza capitalizaria e pontuaria: "/clear" viraria "/Clear." e o comando quebra.
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: pytest.fail("nao devia chamar"))
    assert narrar.limpar_ditado("/clear") == ("/clear", None)


def test_correcao_falada_em_frase_curta_PASSA(monkeypatch):
    # O CASO QUE MOTIVOU A FEATURE. Com piso de 50% valendo pra texto curto ele seria DESCARTADO:
    # 26 chars viram 12, que e 46%. Por isso o piso so vale em texto longo.
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: "Usa o Redis.")
    assert narrar.limpar_ditado("usa o postgres nao o redis") == ("Usa o Redis.", None)


def test_resumo_de_texto_longo_e_descartado(monkeypatch):
    cru = ("primeiro a gente sobe o backend depois roda a migracao e so entao "
           "liga o worker porque senao a fila estoura antes de existir tabela")
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: "Suba tudo na ordem.")
    texto, erro = narrar.limpar_ditado(cru)
    assert texto == cru and erro is not None


def test_resposta_inflada_e_descartada(monkeypatch):
    cru = "por que o build quebrou"
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: "O build quebrou porque " + "x" * 200)
    texto, erro = narrar.limpar_ditado(cru)
    assert texto == cru and erro is not None


def test_falha_do_provedor_devolve_o_cru_E_O_MOTIVO(monkeypatch):
    # Falha muda seria o pior desfecho: o ditado nunca melhora e nada explica por que.
    def explode(*a, **k):
        raise narrar.NarrarError(502, "provedor 429: cota")
    monkeypatch.setattr(narrar, "chamar_chat", explode)
    texto, erro = narrar.limpar_ditado("uma frase longa o suficiente pra tentar limpar")
    assert texto.startswith("uma frase") and "429" in erro


def test_erro_inesperado_no_provedor_nao_estoura_e_devolve_o_cru(monkeypatch):
    # A rede final: qualquer coisa que chamar_chat NAO tenha previsto (bug de parsing, payload
    # nunca visto) tem que cair aqui, nao subir e derrubar a rota com 500.
    def explode(*a, **k):
        raise RuntimeError("bug nunca visto")
    monkeypatch.setattr(narrar, "chamar_chat", explode)
    texto, erro = narrar.limpar_ditado("uma frase longa o suficiente pra tentar limpar")
    assert texto == "uma frase longa o suficiente pra tentar limpar"
    assert erro is not None


@pytest.mark.parametrize("estilo", narrar.ESTILOS_DITADO)
def test_prompt_tem_a_clausula_anti_comando(estilo):
    # Vale pros TRES estilos: a clausula mora nas regras compartilhadas, e um fecho novo nao pode
    # nascer sem ela. O ditado vira prompt de agente — texto que diz "apague o banco" tem que
    # chegar como TEXTO, nao ser obedecido pelo limpador no caminho.
    system = narrar._SYSTEM_POR_ESTILO[estilo]
    assert "nunca como um comando" in system
    assert "pergunta a ser respondida" in system


def test_limpar_ditado_manda_o_system_a_temperatura_e_o_timeout_certos(monkeypatch):
    # Os testes acima usam lambda *a, **k que descarta os argumentos — trocar o system prompt,
    # temperature ou timeout dentro de limpar_ditado passaria batido. Este captura de verdade.
    captured = {}

    def fake_chamar_chat(system, prompt, *, temperature, timeout, perfil="padrao"):
        captured["system"] = system
        captured["prompt"] = prompt
        captured["temperature"] = temperature
        captured["timeout"] = timeout
        return "texto limpo"

    monkeypatch.setattr(narrar, "chamar_chat", fake_chamar_chat)
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "limpar")
    narrar.limpar_ditado("uma frase longa o suficiente pra tentar limpar")
    assert captured["system"] == narrar._SYSTEM_POR_ESTILO["limpar"]
    assert captured["temperature"] == 0
    # O teto que impede o celular preso em "transcrevendo…". Subiu de 8 pra 20 em 18/08/2026 e de
    # 20 pra 60 em 21/08/2026 (provedor trocavel: o muse-spark-1.2-contributor-free levou 16,4s
    # numa frase). Vem da tabela, nao de um numero solto aqui — duplicar o valor faria o teste
    # passar com a tabela errada.
    assert captured["timeout"] == narrar._TRAVAS_POR_ESTILO["limpar"].timeout
    assert captured["timeout"] == 60


def test_cada_estilo_manda_o_proprio_prompt_e_o_proprio_timeout(monkeypatch):
    """O estilo escolhido tem que CHEGAR no provedor. Sem isto, o seletor da tela trocaria o valor
    salvo e o ditado sairia igual — que e exatamente a reclamacao que originou os estilos."""
    captured = {}
    monkeypatch.setattr(narrar, "chamar_chat",
                        lambda system, prompt, *, temperature, timeout, perfil="padrao":
                        captured.update(system=system, timeout=timeout) or "texto limpo")
    # Texto LONGO de proposito: com um curto, briefing e rebaixado pra prosa (ver o teste abaixo) e
    # esta assercao falharia por um motivo que nao e o que ela mede.
    longo = " ".join(["palavra"] * (narrar._MIN_PALAVRAS_BRIEFING + 5))
    for estilo in narrar.ESTILOS_DITADO:
        monkeypatch.setattr(narrar, "estilo_ditado", lambda e=estilo: e)
        narrar.limpar_ditado(longo)
        assert captured["system"] == narrar._SYSTEM_POR_ESTILO[estilo], estilo
        assert captured["timeout"] == narrar._TRAVAS_POR_ESTILO[estilo].timeout, estilo


def test_briefing_em_ditado_curto_vira_prosa(monkeypatch):
    """Briefing so faz sentido com varias ideias pra separar. Num comando de uma linha ele punha um
    "**Objetivo**" em cima de "Abre o narrar.py" — medido, e ridiculo. O rebaixamento e silencioso
    de proposito: a pessoa escolheu o estilo pro dia dela, nao pra cada frase."""
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "briefing")
    curto = "abre o narrar ponto py e roda o teste"
    assert narrar.estilo_efetivo(curto) == "prosa"
    longo = " ".join(["palavra"] * narrar._MIN_PALAVRAS_BRIEFING)
    assert narrar.estilo_efetivo(longo) == "briefing"


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


def test_limpar_ditado_com_content_none_devolve_cru_com_motivo(monkeypatch):
    # Fim a fim: o mesmo payload quebrado, mas passando por limpar_ditado — que NUNCA pode
    # estourar. O texto que a pessoa ditou tem que sobreviver, com o motivo no campo de erro.
    _com_chave(monkeypatch)

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps({"choices": [{"message": {"content": None}}]}).encode()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", lambda req, timeout=None: FakeResp())
    cru = "uma frase longa o suficiente pra tentar a limpeza do ditado falado"
    texto, erro = narrar.limpar_ditado(cru)
    assert texto == cru
    assert erro is not None


def test_limpar_ditado_com_content_lista_devolve_cru_com_motivo(monkeypatch):
    _com_chave(monkeypatch)

    class FakeResp:
        def __enter__(self): return self
        def __exit__(self, *a): return False
        def read(self):
            return json.dumps(
                {"choices": [{"message": {"content": [{"type": "text", "text": "oi"}]}}]}
            ).encode()

    monkeypatch.setattr("app.narrar.urllib.request.urlopen", lambda req, timeout=None: FakeResp())
    cru = "uma frase longa o suficiente pra tentar a limpeza do ditado falado"
    texto, erro = narrar.limpar_ditado(cru)
    assert texto == cru
    assert erro is not None


# --- guarda de sentido: rejeita limpeza que INTRODUZ palavra (a inversao "critica -> resposta") ---
# Caso real, medido ao vivo em 2026-08-01 (audio 1785615417-f9f5b3.m4a). O usuario criticou o
# assistente ("para de falar de forma dificil"); a "limpeza" devolveu o assistente se
# autodefendendo ("eu nao estou falando de forma dificil") — sentido invertido, e as duas travas
# de tamanho (piso 0.5x, teto 1.5x) nao pegam porque a razao ficou em 0.73x.

_DITADO_RUIM_CRU = (
    "Novamente, eu não entendi. Para de falar de forma difícil, tipo, explica melhor o que você "
    "tá querendo exemplificar. E outra coisa, se essa rota nova é mais perigosa de dar erro, não "
    "é mais fácil só criar uma rota nova e deixar as outras que já funcionam prontas e essa "
    "informação vem de uma outra rota? e também não entendi a parte de por que que a VPS esse "
    "pacote estar rodando a VPS é um problema, não entendi direito a VPS com versão antiga os "
    "dados não vão vir da API do back-end, então meio que não entendi muito bem essa questão aí "
    "de de pacote antigo e tal, assim, eu entendo que tem mais como eu que pedi pra fazer, eu sei "
    "como é que ele funciona que tem mais de um back-end rodando em várias máquinas, então cada "
    "back-end vai ter a sua conta hoje eu não pedi pra fazer nada compartilhado ainda, eu imagino "
    "que não tá fazendo nada compartilhado tá sendo por back-end, mas se der pra fazer "
    "compartilhado também, se já estiver fazendo não é um problema mas não entendi porque que "
    "ficaria desatorizado se essas informações estão no back-end e o front só vai chamar essa "
    "rota, não entendi muito bem se você puder me explicar melhor e sobre o contexto, sim, assim "
    "que a gente terminar essa conversa aqui antes de definir o que vai ser feito eu vou querer "
    "sim fazer o finalizar um plano e depois limpar o contexto pra continuar"
)
_DITADO_RUIM_LIMPO = (
    "Eu não entendi. Para explicar melhor, eu não estou querendo exemplificar nada, apenas "
    "limpar o texto. Se essa rota nova for mais perigosa e der erro, não é mais fácil criar uma "
    "rota nova e deixar as outras prontas? A informação vem de outra rota. Eu também não entendi "
    "a parte de por que a VPS com pacote antigo é um problema, os dados não vão vir da API do "
    "back-end. Eu entendo que tem mais de um back-end rodando em várias máquinas, cada back-end "
    "vai ter a sua conta, hoje eu não pedi pra fazer nada compartilhado ainda. Eu imagino que não "
    "tá fazendo nada compartilhado, tá sendo por back-end. Mas se der pra fazer compartilhado "
    "também, se já estiver fazendo não é um problema. Eu não entendi porque que ficaria "
    "desativado se essas informações estão no back-end e o front só vai chamar essa rota. Eu não "
    "entendi muito bem, se você puder me explicar melhor e sobre o contexto. Sim, assim que a "
    "gente terminar essa conversa aqui, antes de definir o que vai ser feito, eu vou querer sim "
    "finalizar um plano e depois limpar o contexto pra continuar."
)

# Limpeza honesta de verdade (audio 1785615822-81b5eb.m4a) — 1582 caracteres, so tira hesitacao/
# repeticao e pontua. NAO pode ser rejeitada: se a guarda pegar isso, o usuario perde a feature.
_DITADO_BOM_CRU = (
    "Tá, isso aí pode até entrar, mas não foi isso que eu disse, eu disse que a... Ah, mas isso é "
    "tarefa de outra sessão, entendi. É pelo que eu quis dizer, vou te mandar, mas o que eu quis "
    "dizer é que a transcrição aqui não foi boa, tipo, eu tava fazendo a transcrição enquanto... "
    "Eu tô fazendo a transcrição enquanto você tá fazendo isso aí, então a gente tava fazendo "
    "dois serviços aqui. Esse áudio era pra ser, no caso, né, não esse áudio, mas essa informação "
    "que o áudio não foi bom, é pra outra sessão que tem do Pi do Cloud, na verdade do Pi não do "
    "Cloud Pocket o Cloud Cockpit na verdade porque lá eu estou fazendo a parte de transcrição do "
    "áudio a gente está melhorando a transcrição do Whisper eu estou falando agora eu estou "
    "mandando um áudio e estou enviando e aí ele melhorou do que a gente já estava usando eu "
    "estava usando bem diferente ele melhorou bastante agora, só que ele falhou em algumas "
    "coisas, tipo ele mudou algumas palavras que eu falei palavras não é o problema mudar ele "
    "mudou meio que o sentido onde eu quis dizer que você estava sendo complexo demais explicando "
    "coisas demais de forma que não estava sendo clara ele meio que traduziu isso, que converteu "
    "isso pra eu, falando que eu estava sendo complexo demais manda talvez é mais fácil te enviar "
    "aqui, porque eu já tinha te enviado manda essa parte que eu estou te falando aqui, pra outra "
    "sessão que está aberta aí do do Cloud Cockpit você tem o CPCend aí você pode enviar pra ela, "
    "informando que eu enviei aqui o áudio informando tudo que eu te falei aqui agora pra ela ver "
    "se ela consegue tentar corrigir"
)
_DITADO_BOM_LIMPO = (
    "Tá, isso aí pode até entrar, mas não foi isso que eu disse, eu disse que a transcrição aqui "
    "não foi boa. Eu tava fazendo a transcrição enquanto você tá fazendo isso aí, então a gente "
    "tava fazendo dois serviços aqui. Esse áudio era pra ser, no caso, não esse áudio, mas essa "
    "informação que o áudio não foi bom, é pra outra sessão que tem do Cloud Cockpit, porque lá "
    "eu estou fazendo a parte de transcrição do áudio, a gente está melhorando a transcrição do "
    "Whisper. Eu estou mandando um áudio e estou enviando e aí ele melhorou do que a gente já "
    "estava usando, eu estava usando bem diferente, ele melhorou bastante agora, só que ele "
    "falhou em algumas coisas, ele mudou algumas palavras que eu falei, palavras não é o "
    "problema, mudar, ele mudou meio que o sentido, onde eu quis dizer que você estava sendo "
    "complexo demais, explicando coisas demais de forma que não estava sendo clara, ele meio que "
    "traduziu isso, que converteu isso pra eu, falando que eu estava sendo complexo demais. "
    "Manda, talvez é mais fácil te enviar aqui, porque eu já tinha te enviado, manda essa parte "
    "que eu estou te falando aqui, pra outra sessão que está aberta aí do Cloud Cockpit, você tem "
    "o CPCend aí, você pode enviar pra ela, informando que eu enviei aqui o áudio, informando "
    "tudo que eu te falei aqui agora, pra ela ver se ela consegue tentar corrigir."
)


def test_ditado_ruim_troca_sujeito_e_rejeitado_no_limpar(monkeypatch):
    # A inversao de sentido (usuario critica -> "limpeza" devolve o assistente se defendendo)
    # nao muda o tamanho o bastante pra disparar as travas de razao (0.73x, entre 0.5 e 1.5), mas
    # introduz palavra que ninguem falou — e isso a guarda tem que pegar.
    #
    # SO no estilo "limpar", desde 14/08/2026. Nos que reestruturam, cobrar palavra nova e recusar
    # o servico pedido — decisao do usuario, com o caso real na frente: um briefing bom, cobertura
    # 98%, rejeitado por 4 "invencoes" que eram conjugacao ("clicava" -> "clico").
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: _DITADO_RUIM_LIMPO)
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "limpar")
    texto, erro = narrar.limpar_ditado(_DITADO_RUIM_CRU)
    assert texto == _DITADO_RUIM_CRU
    assert erro is not None
    assert "não falou" in erro


def test_so_o_briefing_nao_cobra_palavra_nova(monkeypatch):
    """O contrato, escrito como teste pra ninguem "consertar" isto de volta sem querer.

    A linha e a do usuario: o briefing REESCREVE (vira topico, vira titulo), entao cobrar palavra
    nova dele e recusar o servico pedido. Ja "prosa" so reordena e corta repeticao — ali trocar
    palavra e trocar o que ele quis dizer, e a trava fica."""
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: _DITADO_RUIM_LIMPO)

    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "briefing")
    _, erro = narrar.limpar_ditado(_DITADO_RUIM_CRU)
    assert erro is None or "não falou" not in erro, "briefing nao pode cobrar invencao"

    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "prosa")
    _, erro = narrar.limpar_ditado(_DITADO_RUIM_CRU)
    assert erro is not None and "não falou" in erro, "prosa TEM que cobrar invencao"


def test_ditado_bom_honesto_nao_e_rejeitado(monkeypatch):
    # Guarda que rejeita ditado honesto e PIOR que o defeito original — a pessoa perde a feature.
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: _DITADO_BOM_LIMPO)
    texto, erro = narrar.limpar_ditado(_DITADO_BOM_CRU)
    assert erro is None
    assert texto == " ".join(_DITADO_BOM_LIMPO.split())


def test_guarda_ignora_maiuscula_acento_e_pontuacao(monkeypatch):
    # Sem normalizar, "Você" (limpo) e "voce" (cru) contariam como palavras DIFERENTES e todo
    # ditado com acentuacao reconstituida pela pontuacao seria rejeitado.
    cru = "voce falou pra mim que ia rodar o teste amanha de manha bem cedo antes do almoco"
    limpo = "Você falou pra mim que ia rodar o teste amanhã de manhã bem cedo, antes do almoço."
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: limpo)
    texto, erro = narrar.limpar_ditado(cru)
    assert erro is None
    assert texto == " ".join(limpo.split())


def test_guarda_tem_folga_proporcional_em_texto_longo(monkeypatch):
    # 400 palavras unicas no cru + 5 palavras novas no limpo: acima do piso fixo de 3, mas abaixo
    # dos 2% de 405 palavras (8,1) — tem que passar por causa da folga proporcional, nao apesar
    # dela. Prova que o limite nao e "sempre 3".
    cru = " ".join(f"palavra{i}" for i in range(400))
    limpo = cru + " novaA novaB novaC novaD novaE"
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: limpo)
    texto, erro = narrar.limpar_ditado(cru)
    assert erro is None
    assert texto == " ".join(limpo.split())


# Buraco achado pelo cacador de falha calada em 14/08/2026, reproduzido antes de consertar: um
# ditado feito SO de muleta ("e ai cara tipo assim entao bom") nao tem palavra de conteudo, entao
# _cobertura devolve 1.0 por definicao (nao ha o que perder) e _conteudo_novo fica sozinho olhando
# quantidade. Com o piso de encolhimento valendo so em texto longo, as QUATRO travas passavam e o
# ditado da pessoa sumia com erro=None — o app reportando sucesso.

def test_saida_so_com_caractere_invisivel_e_rejeitada(monkeypatch):
    # U+200B tem len() > 0 e sobrevive ao .strip() do Python E ao .trim() do JS: o front nao pega.
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: "​")
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "prosa")
    cru = "e ai cara tipo assim entao bom"
    texto, erro = narrar.limpar_ditado(cru)
    assert texto == cru
    assert erro is not None and "vazio" in erro


def test_ditado_so_de_muleta_ainda_tem_piso_de_encolhimento(monkeypatch):
    """Sem conteudo pra comparar, cobertura vira 1.0 e so sobra o tamanho. O piso, que normalmente
    e so pra texto longo, tem que valer aqui — fala so de muleta e curta por natureza."""
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: "ok")
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "prosa")
    cru = "e ai cara tipo assim entao bom"
    assert not narrar._conteudo(cru), "a amostra precisa ser 100% muleta pra exercitar o caso"
    texto, erro = narrar.limpar_ditado(cru)
    assert texto == cru
    assert erro is not None


def test_texto_curto_com_conteudo_pode_encolher_muito(monkeypatch):
    """O piso NAO pode passar a valer pra frase curta COM conteudo: cortar muleta ali encolhe muito
    e e o servico funcionando. So o caso sem conteudo nenhum e que mudou."""
    monkeypatch.setattr(narrar, "chamar_chat",
                        lambda *a, **k: "Usa o Postgres pra fila.")
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "prosa")
    cru = "entao é... é o seguinte tipo assim usa o postgres pra fila né cara"
    texto, erro = narrar.limpar_ditado(cru)
    assert erro is None, erro
    assert texto == "Usa o Postgres pra fila."
    assert len(texto) < 0.5 * len(cru), "a amostra precisa encolher forte pra exercitar o piso"


def test_limpar_cobra_qualquer_palavra_inventada(monkeypatch):
    """O "limpar" promete "NÃO acrescente nada", entao nao ha palavra que ele possa acrescentar de
    graca — nem as que um dia foram titulos de seção permitidos ao briefing. Aquele perdao existia
    e valia pra todos os estilos, o que deixava frase inventada passar no proprio "limpar"."""
    cru = ("roda o teste do modulo de pagamento e depois me avisa se passou direitinho porque a "
           "fila de deploy hoje esta cheia e eu preciso saber logo do resultado")
    monkeypatch.setattr(narrar, "chamar_chat",
                        lambda *a, **k: cru.capitalize() + " Objetivo: criterio de pronto.")
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "limpar")
    _, erro = narrar.limpar_ditado(cru)
    assert erro is not None and "não falou" in erro


def test_conjugacao_nao_conta_como_palavra_inventada():
    """A causa raiz do dia 14/08: a trava contava "clicava" -> "clico" como invencao, e recusou um
    briefing bom do usuario por 4 "palavras novas" que eram o mesmo verbo. Comparar radical mata a
    classe inteira — e a mesma cura de _CONTRACOES pra "tô"/"estou", que voltou por outra porta."""
    cru = "eu clicava ali e trocava o modelo seguindo o padrao"
    limpo = "Eu clico ali e troco o modelo, seguir o padrao."
    assert not narrar._conteudo_novo(cru, limpo)


def test_plural_nao_conta_como_palavra_inventada():
    """Mesma pergunta da conjugacao, pelo numero: "carro" e "carros" sao a mesma coisa."""
    assert not narrar._conteudo_novo("comprei um carro novo", "Comprei carros novos.")


def test_troca_de_genero_ainda_e_palavra_nova():
    """O contra-exemplo da comparacao por radical, escrito como teste pra nao reabrir.

    A primeira versao cortava a vogal final sempre, e ai "posto"/"posta" e "conta"/"conto" viravam
    o mesmo radical: trocar o substantivo por outro passava com 0 palavra nova e 100% de cobertura
    — calado, nos dois estilos que prometem nao trocar as palavras da pessoa. A vogal final so cai
    com prova de verbo no texto (por isso o teste de conjugacao acima continua passando)."""
    assert narrar._conteudo_novo("ele foi no posto de gasolina", "Ele foi na posta de gasolina.")
    assert narrar._conteudo_novo("a conta do cliente atrasou", "O conto do cliente atrasou.")
    assert narrar._cobertura("a medica atendeu o paciente", "O medico atendeu o paciente.") < 1.0


def test_estilo_pedido_pela_tela_vence_a_config(monkeypatch):
    """A pill dizia "So limpar" e o ditado voltava em briefing: o app le a config uma vez por carga
    de pagina, e a troca feita noutra aba nunca chegava na tela. Quem manda agora e o estilo que a
    pessoa LEU antes de falar; a config so vale quando a tela nao manda nada."""
    captured = {}

    def fake_chamar_chat(system, prompt, *, temperature, timeout, perfil="padrao"):
        captured["system"] = system
        return "texto limpo com o estilo certo"

    monkeypatch.setattr(narrar, "chamar_chat", fake_chamar_chat)
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "briefing")
    frase = "uma frase longa o suficiente pra tentar limpar de verdade"

    narrar.limpar_ditado(frase, "limpar")
    assert captured["system"] == narrar._SYSTEM_POR_ESTILO["limpar"]

    # Estilo desconhecido (query adulterada, front velho) NAO vira erro nem estilo novo: cai na
    # config, o comportamento de antes desta mudanca.
    narrar.limpar_ditado(frase, "inventado")
    assert captured["system"] == narrar._SYSTEM_POR_ESTILO["prosa"]   # briefing curto -> prosa

    narrar.limpar_ditado(frase, None)
    assert captured["system"] == narrar._SYSTEM_POR_ESTILO["prosa"]


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


def test_so_o_briefing_usa_o_perfil_de_briefing(monkeypatch):
    """O perfil sai do ESTILO, nao de uma config a parte: escolher 'limpar' e ver a conta do outro
    provedor sendo gasta seria a mesma classe de erro do estilo que nao chegava no backend."""
    vistos = []
    monkeypatch.setattr(narrar, "chamar_chat",
                        lambda system, prompt, *, temperature, timeout, perfil="padrao":
                        (vistos.append(perfil), "texto limpo o suficiente pra passar")[1])
    frase = ("uma frase bem longa pra sobreviver as travas de cobertura e de tamanho sem ser "
             "rejeitada por nada")
    for estilo in ("limpar", "prosa", "briefing"):
        narrar.limpar_ditado(frase, estilo)
    assert vistos == ["padrao", "padrao", "padrao"]   # briefing curto -> rebaixado pra prosa

    vistos.clear()
    narrar.limpar_ditado(" ".join(["palavra"] * 60), "briefing")
    assert vistos == ["briefing"]


# O caso real de 24/08/2026: o usuario ditou ~3700 chars de contexto sobre uma PM, escolheu
# "briefing", e o texto voltou RESUMIDO — sumiram as ressalvas dele, os motivos e o que ele deixou
# em aberto. Nenhuma trava disparou, entao ele nem aviso teve: teria que ditar tudo de novo.
#
# Os tres textos abaixo sao artefatos REAIS daquela medicao (deepseek-v4-flash, temperatura 0), nao
# exemplos escritos a mao: o cru e o ditado dele, o "resumido" e uma das 3 saidas do fecho antigo
# (cobertura 0,51) e o "completo" e a PIOR das 7 saidas do fecho novo (cobertura 0,72) — a pior de
# proposito, porque quem tem que passar pelo piso e ela, nao a melhor.
_BRIEFING_CRU = (
    "Eu vou te passar uma PM aqui e a gente precisa definir uma execução para ela. Como é que a "
    "gente vai fazer isso? Hoje eu tenho uma skill aí que chama ideia to push, eu acho que é "
    "orchestring idea to push. e eu uso ela para trabalhos pessoais aqui, beleza, isso não é "
    "importante. A questão é, eu quero usar ela para essa PM que eu vou te passar, eu vou te "
    "passar o link aí, e eu quero que você analise a PM e o que é que vai ser essa PM? Ela vai "
    "ser aqui a folha de exames da UTI, existe um MOC, então existe um projeto onde já foram "
    "feitos os MOCs e aí o que a gente precisa fazer? a gente precisa de fazer de trazer todas "
    "essas telas que foram do MOP l que s uma duas tr quatro cinco seis seis telas assistentes "
    "digamos assim vai ter uma tela principal e seis telas que juntam v entregar para elas alguma "
    "coisa que seriam seis telas de cadastro e uma tela principal que seria a folha de exames ali "
    "isso tudo est descrito na PM eu estou falando aqui mais para ter um contexto mas tudo tem na "
    "PM na PM tem v ent voc vai ver os v da PM voc vai ver os mocs e o que eu preciso fazer eu "
    "preciso trazer todas essas telas aí, todas essas telas entregues que estão aí na PM para cá. "
    "Acredito que o ideal seria começar pelas de cadastro e deixar a folha de exames por último, "
    "porque ela é a que depende de todas as outras, né? Ela é o... todas as outras são para "
    "entregar coisas para ela, então acredito que seria mais fácil, né? at porque elas s s uma "
    "tela de cadastro um CRUD b talvez mais f fazer elas at porque elas podem talvez n imagine "
    "que elas podem ser feitas em paralelo porque eu acho que nenhuma depende tanto assim da "
    "outra n caso dessas de cadastro eu acho que talvez a gente pode fazer as seis de uma vez "
    "depois s fazer a folha de exames que depende de todas Tudo isso est descrito na PM tem um "
    "monte de descri a eu estou falando aqui mais para a gente ver como que a gente vai usar S "
    "que qual que a quest Eu tenho a minha skill de fazer isso s que a minha skill para orquestrar "
    "n Eu uso ela para orquestrar coisas E eu tenho a skill que a gente usa para portar telas "
    "desse projeto de mockup Ent eu preciso analisar essa tela aqui agora, nessa PM aqui agora, e "
    "ver como que eu posso integrar a skill que eu já tenho de orquestração com a skill de portar "
    "tela, porque ela já tem as regras de negócio, ela já tem as regras que o projeto segue, "
    "então a gente tem que usar ela. a minha skill n para isso a minha skill para orquestrar o "
    "trabalho ent depois de ter um plano pronto ou de ter alguma coisa pronta a minha skill fica "
    "orquestrando o trabalho sem que eu tenha que me envolver a cada etapa o plano foi dividido "
    "em 15 tasks eu n preciso de me envolver nas 15 ap eu aprovar o plano meio que para isso que "
    "serve a minha skill mas n leve em considera tudo que eu estou dizendo aqui como verdade voc "
    "pode ler a skill e ver se ela faz sentido para usar junto com a portar tela e com a gerar "
    "contrato as skills que a gente fez para usar essas dos moques porque a gente fez skills para "
    "pegar esses moques e fazer eles virarem telas de produ a gente cria o backend e cria o front "
    "essas de cadastro aí eu acho que a gente não vai precisar de usar nada no PSS, a gente vai "
    "criar a parte de crude no próprio Next, mas tudo isso aqui que eu estou falando eu só estou "
    "dando um contexto, ainda não é uma regra, a gente vai planejar e fazer a spec junto, e aí "
    "para isso que eu preciso que você analise tanto a skill de orchestrating, a idea to push, "
    "quanto a portar tela, e que a porta dela fica no marketplace do ProMédico. E a contrato "
    "também, né? Contrato, back-end ou gerar contrato, alguma coisa assim, que é pra na hora do "
    "back-end do folhas de exames, a gente vai usar o PSS pra fazer o back-end dela. Tá?"
)

_BRIEFING_RESUMIDO = """**Objetivo**
- Analisar a PM que vou te passar e definir uma execução para ela.
- Usar a skill de orquestração para essa PM.
- Trazer todas as telas descritas na PM para cá.
- Integrar a skill de orquestração com a skill de portar tela e a de gerar contrato.

**Situação hoje**
- Existe um projeto onde já foram feitos os MOCs.
- A PM descreve a folha de exames da UTI.
- São seis telas de cadastro e uma tela principal que seria a folha de exames.
- Tudo está descrito na PM, com os MOCs.

**Critério de pronto**
- Trazer todas as telas entregues que estão na PM.
- Começar pelas telas de cadastro e deixar a folha de exames por último, porque ela depende de todas as outras.
- As seis telas de cadastro podem ser feitas em paralelo, porque nenhuma depende tanto da outra.
- Depois fazer a folha de exames, que depende de todas.

**O que eu preciso saber**
- Como integrar a skill de orquestração com a skill de portar tela, porque ela já tem as regras de negócio e as regras que o projeto segue.
- A skill de orquestração fica orquestrando o trabalho sem que eu tenha que me envolver a cada etapa.
- O plano foi dividido em 15 tasks, e eu aprovo o plano.
- Analisar a skill de orchestrating idea to push, a portar tela (que fica no marketplace do ProMédico) e a de gerar contrato.
- Para o back-end da folha de exames, a gente vai usar o PSS.
- Para as telas de cadastro, a gente vai criar a parte de CRUD no próprio Next."""

_BRIEFING_COMPLETO = """**Objetivo**
- Definir uma execução para a PM que vou te passar.
- Usar a skill "orchestrating idea to push" para essa PM.
- Analisar a PM e o que vai ser essa PM: a folha de exames da UTI.
- Trazer todas as telas entregues que estão na PM para cá.

**Situação hoje**
- Existe um MOC, existe um projeto onde já foram feitos os MOCs.
- A PM descreve uma tela principal e seis telas de cadastro que entregam coisas para ela.
- A tela principal seria a folha de exames.
- Tudo está descrito na PM, tem um monte de descrição.
- Eu tenho uma skill de orquestração, a "idea to push", que uso para trabalhos pessoais.
- Eu tenho a skill que a gente usa para portar telas desse projeto de mockup, com as regras de negócio e as regras que o projeto segue.
- A skill de portar tela fica no marketplace do ProMédico.
- Tem também a skill de contrato, back-end ou gerar contrato, para o back-end da folha de exames, que a gente vai usar o PSS.

**Contexto**
- Isso não é importante.
- Eu estou falando aqui mais para ter um contexto, mas tudo tem na PM.
- Eu estou falando aqui mais para a gente ver como que a gente vai usar.
- Tudo isso que eu estou falando eu só estou dando um contexto, ainda não é uma regra, a gente vai planejar e fazer a spec junto.

**Critério de pronto**
- Acredito que o ideal seria começar pelas de cadastro e deixar a folha de exames por último, porque ela é a que depende de todas as outras.
- As outras são para entregar coisas para ela.
- As telas de cadastro são um CRUD, talvez mais fácil fazer elas.
- Eu acho que talvez a gente pode fazer as seis de uma vez, depois só fazer a folha de exames que depende de todas.

**Em aberto**
- Não leve em consideração tudo que eu estou dizendo aqui como verdade, você pode ler a skill e ver se ela faz sentido para usar junto com a portar tela e com a gerar contrato.
- Eu preciso analisar essa PM aqui agora e ver como que eu posso integrar a skill que eu já tenho de orquestração com a skill de portar tela.
- A minha skill de orquestrar o trabalho fica orquestrando sem que eu tenha que me envolver a cada etapa, o plano foi dividido em 15 tasks, eu não preciso me envolver nas 15, eu aprovar o plano, meio que para isso que serve a minha skill.
- Eu acho que a gente não vai precisar de usar nada no PSS para as telas de cadastro, a gente vai criar a parte de CRUD no próprio Next, mas isso ainda não é uma regra.

**O que eu preciso saber**
- Como é que a gente vai fazer isso?
- Como que eu posso integrar a skill que eu já tenho de orquestração com a skill de portar tela?"""


# O segundo caso real (26/08/2026), e o que recalibrou os limites: o usuario ditou 2:25 sobre
# unificar logs, clicou "Briefing" e o texto voltou CRU com aviso, duas vezes seguidas — pra ele o
# botao estava quebrado. Os dois textos abaixo sao a medicao real (mesmo audio, mesmo provedor):
# o briefing esta INTEIRO (nao falta nenhum assunto), e ainda assim tinha cobertura 0,577, dentro
# do intervalo do defeito de 24/08. A razao esta no proprio texto: ele SOLETRA caminho ("pss barra
# logs barra prom web"), e cada "barra" dita vira uma barra escrita.
_BRIEFING2_CRU = (
    "Tá, o que a gente precisa aí? Eu preciso de dar uma melhorada nos logs aí. Por que que eu tô "
    "falando isso? Tem que ver, né, analisar como que tá feito. Hoje você vai rodar vários "
    "sub-agents aí pra gente ter uma noção maior, mas é porque tá aí meio que espalhado os logs "
    "aí, os logs da aplicação, no caso. Tem log que tá ficando na pasta... Deixa eu achar aqui a "
    "pasta, peraí. Tá, no caso aqui, tem logs que ficam na pasta pss barra logs, pss módulos, "
    "não, pss barra módulos, barra pmedicweb, barra log, e também tem uns logs que estão ficando "
    "na pasta do usuário, barra pm2, que seriam os logs do pm2. queria ver uma forma de pegar "
    "todos esses logs a na teoria j era para estar assim mas por algum motivo n est mas pegar "
    "todos esses logs que s do Prom Web e colocar tudo em pss barra logs que no caso pss barra "
    "logs barra prom web eu queria todos os logs nesse caminho nesse endere porque assim ficaria "
    "mais f de encontrar E uma outra coisa que depois a gente conseguir fazer isso que eu queria "
    "ver era se poss fazer uma p uma HTML simples para ler esses logs de forma melhor, né, ali "
    "onde eu possa, que ele não dependa do projeto Next rodando, por isso que eu falo um HTML ou "
    "o que for melhor, mas imagino que é um HTML puro, com JavaScript nele ali, onde eu posso "
    "clicar, abrir ele e eu vejo os logs, todos os logs, não só do PSS, porque dentro dessa pasta "
    "PSS logs, vai ter vários, então seria uma boa se a gente conseguisse fazer um sistema, uma "
    "até a linha local ali que não depende de rodar node, linha do tipo para ler esses logs, né? "
    "E aí se você não tiver aí agora a estrutura desses logs, depois eu te passo para a tela, mas "
    "primeiro vamos fazer a verificação de como unificar esses logs aí. E aí"
)

_BRIEFING2_BOM = """**Objetivo**
- Melhorar os logs da aplicação.
- Unificar todos os logs do Prom Web em um único caminho: `pss/logs/promweb`.
- Depois de unificar, criar um HTML simples (com JavaScript) para ler esses logs de forma melhor, sem depender do projeto Next rodando, nem de rodar Node — algo local, que possa abrir e visualizar todos os logs.

**Situação hoje**
- Os logs estão espalhados:
- Na pasta `pss/logs` e também em `pss/modulos/pmedicweb/log`.
- Na pasta do usuário, `/pm2`, que seriam os logs do PM2.
- Na teoria, já deveria estar assim, mas por algum motivo não está.
- Preciso analisar como está feito hoje, verificar a estrutura desses logs.

**Contexto**
- Hoje você vai rodar vários sub-agents para ter uma noção maior.
- Dentro da pasta `pss/logs`, vai ter vários logs, não só do PSS.

**Em aberto**
- Se você não tiver a estrutura desses logs agora, depois eu te passo para a tela.
- Primeiro vamos fazer a verificação de como unificar esses logs."""


def test_briefing_que_soletra_caminho_passa(monkeypatch):
    """O bug de 26/08/2026 virado teste. Este briefing nao perdeu assunto nenhum e mesmo assim
    tinha 0,577 de cobertura — abaixo do piso de 0,65 que valia entao. Se ele voltar a ser
    recusado, o piso subiu demais de novo e o botao "Briefing" volta a parecer quebrado."""
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: _BRIEFING2_BOM)
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "briefing")
    texto, erro = narrar.limpar_ditado(_BRIEFING2_CRU)
    assert erro is None, erro
    assert texto.startswith("**Objetivo**")


def test_briefing_que_resumiu_e_recusado(monkeypatch):
    """O bug de 24/08/2026, virado teste. Com encolhe_min=0.3 e cobertura_min=0.45 esta saida
    passava ILESA (0,38x do tamanho, 51% do conteudo) e o usuario recebia o resumo achando que era
    o ditado dele. Briefing nao e resumo: perder metade do que a pessoa falou e defeito."""
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: _BRIEFING_RESUMIDO)
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "briefing")
    texto, erro = narrar.limpar_ditado(_BRIEFING_CRU)
    assert texto == _BRIEFING_CRU, "tem que devolver o cru, nao o resumo"
    assert erro is not None


def test_briefing_completo_passa(monkeypatch):
    """A outra metade da calibragem, e a mais importante: piso que recusa briefing bom devolve o
    cru e faz a pessoa ditar de novo — pior que o defeito. Esta e a PIOR das 7 saidas medidas com o
    fecho novo (cobertura 0,72); se ela nao passar, o piso subiu demais."""
    monkeypatch.setattr(narrar, "chamar_chat", lambda *a, **k: _BRIEFING_COMPLETO)
    monkeypatch.setattr(narrar, "estilo_ditado", lambda: "briefing")
    texto, erro = narrar.limpar_ditado(_BRIEFING_CRU)
    assert erro is None, erro
    assert texto.startswith("**Objetivo**")


def test_as_duas_populacoes_do_briefing_continuam_separadas():
    """Os numeros que justificam os limites, presos num teste pra nao virarem folclore de
    comentario. Medido em 24/08/2026: defeito 0,51-0,57 de cobertura e 0,38-0,41 de tamanho; bom
    0,72-0,90 e 0,69-0,94.

    Quem separa e o TAMANHO, e so ele: em 26/08/2026 o segundo ditado real (ver
    test_briefing_que_soletra_caminho_passa) deu 0,577 de cobertura num briefing INTEGRO — dentro
    do intervalo do defeito. O piso de cobertura sobrou como rede grossa e nao e mais divisor."""
    travas = narrar._TRAVAS_POR_ESTILO["briefing"]
    ruim = (narrar._cobertura(_BRIEFING_CRU, _BRIEFING_RESUMIDO),
            len(_BRIEFING_RESUMIDO) / len(_BRIEFING_CRU))
    bom = (narrar._cobertura(_BRIEFING_CRU, _BRIEFING_COMPLETO),
           len(_BRIEFING_COMPLETO) / len(_BRIEFING_CRU))
    assert ruim[1] < travas.encolhe_min < bom[1], f"tamanho ruim={ruim[1]:.2f} bom={bom[1]:.2f}"
    # O piso do OUTRO caso real tem que caber no mesmo vao — 0,561 e o menor briefing bom medido.
    assert travas.encolhe_min < len(_BRIEFING2_BOM) / len(_BRIEFING2_CRU)
    assert ruim[0] > travas.cobertura_min, "cobertura virou rede grossa: nao pode recusar o defeito sozinha"
    assert bom[0] > travas.cobertura_min


def test_fecho_do_briefing_tem_completude_e_par_entrada_saida():
    """As duas coisas que faltavam no unico fecho que nao as tinha — e a causa do resumo. Este
    arquivo ja mediu duas vezes (regras 1 e 4, e o _FECHO_PROSA) que regra sem par entrada/saida
    nao e obedecida; e as regras do briefing eram TODAS de agrupar e cortar repeticao, o que o
    modelo leu como licenca pra encurtar."""
    system = narrar._SYSTEM_POR_ESTILO["briefing"]
    assert "NADA do que ela falou pode ficar de fora" in system
    assert "NÃO é resumo" in system
    # O par: o exemplo mostra uma ressalva sobrevivendo, que e o que sumia.
    fecho = system[system.index("briefing estruturado"):]
    assert "Entrada:" in fecho and "Saída:" in fecho
