"""agentes_sync: uma credencial do app espalhada pra config de cada agente.

Tudo roda contra um HOME falso (`tmp_path`) — nenhum teste aqui pode encostar no ~/.pi,
~/.kimi-code ou ~/.codex de verdade, que têm as chaves do usuário dentro.
"""
import os

import pytest
import json
import stat
import tomllib

from app import agentes_sync

MODELOS = [
    {"id": "k3", "context_length": 262144, "vision": True},
    {"id": "glm-5.2", "context_length": None, "vision": None},
]

CONFIG_KIMI = """\
[providers.apikey]
type = "kimi"
api_key = "sk-do-usuario"
base_url = "https://api.kimi.com/coding/v1"

[models."apikey/k3"]
provider = "apikey"
model = "k3"
display_name = "K3"

[[hooks]]
event = "Stop"
command = 'python hook.py'
"""


def _homes(tmp_path, *quais):
    """Cria as pastas dos agentes pedidos e devolve o mapa de homes pro sincronizar()."""
    for q in quais:
        {"pi": tmp_path / ".pi" / "agent", "kimi": tmp_path / ".kimi-code",
         "codex": tmp_path / ".codex"}[q].mkdir(parents=True, exist_ok=True)
    return {a: tmp_path for a in agentes_sync.ALVOS}


def test_pi_grava_e_rele(tmp_path):
    _homes(tmp_path, "pi")
    ok, motivo = agentes_sync.gravar_pi("cred", "https://x.dev/v1", "sk-abc", MODELOS,
                                        home=tmp_path)
    assert ok, motivo
    d = json.loads((tmp_path / ".pi" / "agent" / "models.json").read_text())
    p = d["providers"]["cred"]
    assert p["baseUrl"] == "https://x.dev/v1"
    assert p["apiKey"] == "sk-abc"
    assert p["api"] == "openai-completions"
    assert [m["id"] for m in p["models"]] == ["k3", "glm-5.2"]
    assert p["models"][0]["input"] == ["text", "image"]  # vision True
    assert p["models"][0]["contextWindow"] == 262144
    # vision/context desconhecidos NÃO viram chute
    assert p["models"][1]["input"] == ["text"]
    assert "contextWindow" not in p["models"][1]
    # custo e maxTokens o probe não informa: nem zero, nem nada
    assert "cost" not in p["models"][0] and "maxTokens" not in p["models"][0]


def test_pi_provedor_conhecido_recebe_so_a_credencial(tmp_path):
    # O caminho bom: o Pi tem catálogo próprio, então dar a lista de modelos por fora só piora.
    # Medido em 25/08/2026 com PI_CODING_AGENT_DIR num sandbox: auth.json de UMA linha
    # (opencode-go) + models.json vazio -> `pi --list-models opencode-go` lista 13 modelos com
    # contexto, raciocínio e imagem certos.
    _homes(tmp_path, "pi")
    ok, motivo = agentes_sync.gravar_pi("cred", "https://opencode.ai/zen/go", "sk-abc", MODELOS,
                                        home=tmp_path)
    assert ok, motivo
    auth = json.loads((tmp_path / ".pi" / "agent" / "auth.json").read_text())
    assert auth["opencode-go"] == {"type": "api_key", "key": "sk-abc"}
    # E NADA em models.json: uma lista nossa ali competiria com o catálogo do Pi.
    assert not (tmp_path / ".pi" / "agent" / "models.json").exists()


def test_pi_nao_desloga_assinatura_ja_conectada(tmp_path):
    # `oauth` no auth.json é assinatura logada (Claude Pro, ChatGPT, OpenRouter por OAuth).
    # Sobrescrever com uma chave desloga a pessoa daquele provedor dentro do Pi.
    _homes(tmp_path, "pi")
    auth = tmp_path / ".pi" / "agent" / "auth.json"
    auth.write_text(json.dumps({"anthropic": {"type": "oauth", "access": "token-da-assinatura"}}))
    ok, motivo = agentes_sync.gravar_pi("cred", "https://api.anthropic.com", "sk-ant", [],
                                        home=tmp_path)
    assert not ok and "nao-substituivel" in motivo
    assert json.loads(auth.read_text())["anthropic"]["access"] == "token-da-assinatura"


def test_pi_troca_chave_antiga_e_preserva_os_outros(tmp_path):
    _homes(tmp_path, "pi")
    auth = tmp_path / ".pi" / "agent" / "auth.json"
    auth.write_text(json.dumps({
        "groq": {"type": "api_key", "key": "sk-velha"},
        "openai-codex": {"type": "oauth", "access": "intacto"},
    }))
    assert agentes_sync.gravar_pi("cred", "https://api.groq.com/openai", "sk-nova", [],
                                  home=tmp_path)[0]
    d = json.loads(auth.read_text())
    assert d["groq"]["key"] == "sk-nova"
    assert d["openai-codex"]["access"] == "intacto"


@pytest.mark.parametrize("url,esperado", [
    ("https://opencode.ai/zen/go", "opencode-go"),
    ("https://opencode.ai/zen/go/v1", "opencode-go"),
    ("https://opencode.ai/zen", "opencode"),          # prefixo do /go: a ordem da tabela decide
    ("https://api.kimi.com/coding", "kimi-coding"),
    ("https://openrouter.ai/api/v1/", "openrouter"),
    ("https://api.groq.com/openai", "groq"),
    ("https://api.deepseek.com", "deepseek"),
    ("https://api.anthropic.com", "anthropic"),
    ("https://ai.omniwise.com.br", None),             # gateway próprio: o Pi não conhece
    ("https://api.commandcode.ai/provider", None),
])
def test_reconhece_o_provedor_pela_url(url, esperado):
    assert agentes_sync.provedor_embutido_do_pi(url) == esperado


def test_base_url_sai_no_dialeto_openai_nos_tres_alvos(tmp_path):
    # Caso relatado em 25/08/2026: a conta do OpenCode Zen cadastrada pelo app aparecia no Pi com o
    # endereço errado. O app guarda a RAIZ (é o que o Claude Code e o probe pedem), e os três alvos
    # daqui montam `{base}/chat/completions` — sem o `/v1` a chamada cai fora da API.
    # Gateway próprio de propósito: um endereço que o Pi CONHECE não passa mais pelo models.json
    # (vai só a credencial pro auth.json), e é o caminho da lista escrita por nós que este caso mede.
    _homes(tmp_path, "pi", "kimi", "codex")
    raiz = "https://ai.omniwise.com.br"
    assert agentes_sync.gravar_pi("cred", raiz, "sk-abc", [], home=tmp_path)[0]
    assert agentes_sync.gravar_kimi("cred", raiz, "sk-abc", [], home=tmp_path)[0]
    assert agentes_sync.gravar_codex("cred", raiz, "sk-abc", [], home=tmp_path)[0]

    pi = json.loads((tmp_path / ".pi" / "agent" / "models.json").read_text())
    assert pi["providers"]["cred"]["baseUrl"] == "https://ai.omniwise.com.br/v1"
    kimi = tomllib.loads((tmp_path / ".kimi-code" / "config.toml").read_text())
    assert kimi["providers"]["cred"]["base_url"] == "https://ai.omniwise.com.br/v1"
    codex = tomllib.loads((tmp_path / ".codex" / "config.toml").read_text())
    assert codex["model_providers"]["cred"]["base_url"] == "https://ai.omniwise.com.br/v1"


def test_base_url_ja_com_v1_nao_duplica():
    # O campo aceita as duas formas (dá pra colar a URL como o provedor documenta), então quem já
    # veio com /v1 não pode virar /v1/v1.
    assert agentes_sync.base_openai("https://x.dev/v1") == "https://x.dev/v1"
    assert agentes_sync.base_openai("https://x.dev/v1/") == "https://x.dev/v1"
    assert agentes_sync.base_openai("  https://x.dev  ") == "https://x.dev/v1"


def test_pi_recupera_visao_do_catalogo_do_proprio_pi(tmp_path):
    # Rede de segurança do caminho de gateway PRÓPRIO, onde a lista tem de sair de nós: o
    # `/v1/models` costuma devolver o shape OpenAI pelado (sem capacidade nenhuma), e foi assim que
    # o muse-spark, que LÊ IMAGEM, chegou ao Pi como texto puro. A tabela que o próprio Pi mantém
    # sabe a resposta. Provedor CONHECIDO nem chega aqui — ver o caso da credencial sozinha.
    pi = tmp_path / ".pi" / "agent"
    pi.mkdir(parents=True)
    (pi / "models-store.json").write_text(json.dumps({
        "opencode-go": {"models": [
            {"id": "muse-spark-1.2-contributor", "name": "Muse Spark 1.2 Contributor",
             "input": ["text", "image"], "contextWindow": 1048576, "reasoning": True},
        ]},
    }), encoding="utf-8")
    modelos = [{"id": "muse-spark-1.2-contributor", "context_length": None, "vision": None},
               {"id": "modelo-fora-do-catalogo", "context_length": None, "vision": None}]
    assert agentes_sync.gravar_pi("cred", "https://ai.omniwise.com.br", "sk-abc", modelos,
                                  home=tmp_path)[0]
    ms = json.loads((pi / "models.json").read_text())["providers"]["cred"]["models"]
    assert ms[0]["input"] == ["text", "image"]
    assert ms[0]["contextWindow"] == 1048576
    assert ms[0]["reasoning"] is True
    assert ms[0]["name"] == "Muse Spark 1.2 Contributor"
    # Fora do catálogo segue no mínimo honesto — o catálogo preenche buraco, não inventa.
    assert ms[1]["input"] == ["text"] and "contextWindow" not in ms[1]


def test_o_que_o_provedor_disse_vence_o_catalogo(tmp_path):
    # O provedor reflete ESTA chave e ESTE deployment; o catálogo é tabela geral. Um deployment que
    # declara não ler imagem não pode ser sobrescrito por uma tabela dizendo que o modelo lê.
    pi = tmp_path / ".pi" / "agent"
    pi.mkdir(parents=True)
    (pi / "models-store.json").write_text(json.dumps({
        "qualquer": {"models": [{"id": "m1", "input": ["text", "image"], "contextWindow": 999}]},
    }), encoding="utf-8")
    modelos = [{"id": "m1", "context_length": 128000, "vision": False}]
    assert agentes_sync.gravar_pi("cred", "https://x.dev", "sk-abc", modelos, home=tmp_path)[0]
    m1 = json.loads((pi / "models.json").read_text())["providers"]["cred"]["models"][0]
    assert m1["input"] == ["text"]
    assert m1["contextWindow"] == 128000


def test_catalogo_do_pi_ausente_ou_quebrado_nao_derruba(tmp_path):
    pi = tmp_path / ".pi" / "agent"
    pi.mkdir(parents=True)
    (pi / "models-store.json").write_text("{ isto nao e json", encoding="utf-8")
    modelos = [{"id": "m1", "context_length": None, "vision": None}]
    assert agentes_sync.gravar_pi("cred", "https://x.dev", "sk-abc", modelos, home=tmp_path)[0]
    m1 = json.loads((pi / "models.json").read_text())["providers"]["cred"]["models"][0]
    assert m1["input"] == ["text"] and m1["name"] == "m1"


def test_pi_preserva_provedor_do_usuario(tmp_path):
    _homes(tmp_path, "pi")
    cfg = tmp_path / ".pi" / "agent" / "models.json"
    cfg.write_text(json.dumps({"providers": {"hcn": {"apiKey": "sk-do-usuario"}}}))
    assert agentes_sync.gravar_pi("cred", "https://x.dev", "sk-abc", [], home=tmp_path)[0]
    d = json.loads(cfg.read_text())
    assert d["providers"]["hcn"]["apiKey"] == "sk-do-usuario"
    assert set(d["providers"]) == {"hcn", "cred"}


def test_kimi_preserva_o_resto_e_nao_duplica(tmp_path):
    _homes(tmp_path, "kimi")
    cfg = tmp_path / ".kimi-code" / "config.toml"
    cfg.write_text(CONFIG_KIMI)
    for _ in range(2):  # duas gravações: idempotente
        ok, motivo = agentes_sync.gravar_kimi("cred", "https://x.dev/v1", "sk-abc", MODELOS,
                                              home=tmp_path)
        assert ok, motivo
    texto = cfg.read_text()
    assert texto.count('[providers."cred"]') == 1
    assert texto.count('[models."cred/k3"]') == 1
    # o arquivo do usuário continua abrindo, e o que era dele continua lá
    d = tomllib.loads(texto)
    assert d["providers"]["apikey"]["api_key"] == "sk-do-usuario"
    assert d["models"]["apikey/k3"]["display_name"] == "K3"
    assert d["hooks"][0]["event"] == "Stop"
    # e o nosso entrou com a forma do Kimi
    assert d["providers"]["cred"] == {"type": "kimi", "api_key": "sk-abc",
                                      "base_url": "https://x.dev/v1"}
    assert d["models"]["cred/k3"]["max_context_size"] == 262144
    assert "max_context_size" not in d["models"]["cred/glm-5.2"]  # context_length None
    assert (tmp_path / ".kimi-code" / "config.toml.bak-hangar").read_text() == CONFIG_KIMI


def test_kimi_recusa_provedor_feito_a_mao(tmp_path):
    """Apendar uma tabela que já existe fora do nosso bloco quebraria o TOML do usuário."""
    _homes(tmp_path, "kimi")
    cfg = tmp_path / ".kimi-code" / "config.toml"
    cfg.write_text('[providers.cred]\ntype = "kimi"\napi_key = "sk-do-usuario"\n')
    ok, motivo = agentes_sync.gravar_kimi("cred", "https://x.dev", "sk-abc", [], home=tmp_path)
    assert (ok, motivo) == (False, "ja-existe-fora-do-bloco")
    assert tomllib.loads(cfg.read_text())["providers"]["cred"]["api_key"] == "sk-do-usuario"


def test_codex_guarda_env_key_e_nao_a_chave(tmp_path):
    _homes(tmp_path, "codex")
    cfg = tmp_path / ".codex" / "config.toml"
    cfg.write_text('[projects."/tmp"]\ntrust_level = "trusted"\n')
    ok, motivo = agentes_sync.gravar_codex("cred", "https://x.dev/v1", "sk-segredo", MODELOS,
                                           home=tmp_path)
    assert ok, motivo
    texto = cfg.read_text()
    assert "sk-segredo" not in texto
    assert "HANGAR_CRED_API_KEY" in motivo  # o relatório diz qual variável precisa existir
    d = tomllib.loads(texto)
    assert d["model_providers"]["cred"] == {"name": "cred", "base_url": "https://x.dev/v1",
                                            "env_key": "HANGAR_CRED_API_KEY", "wire_api": "chat"}
    assert d["projects"]["/tmp"]["trust_level"] == "trusted"


def test_agente_nao_instalado_nao_e_erro(tmp_path):
    r = agentes_sync.sincronizar("cred", "https://x.dev", "sk-abc", MODELOS,
                                 homes={a: tmp_path for a in agentes_sync.ALVOS})
    assert set(r) == set(agentes_sync.ALVOS)
    assert all(v == {"ok": False, "motivo": "nao-instalado"} for v in r.values()), r
    assert list(tmp_path.iterdir()) == []  # não criou config pra agente que não existe


# LACUNA VISIVEL: estes arquivos guardam a CHAVE de API do provedor, e no Windows eles NAO
# ficam protegidos — nao ha bit de modo la (quem decide e a ACL) e a ACL equivalente ainda nao
# esta implementada. Mesmo tratamento do peers.json e do arquivo de conexao do Pi: a falta fica
# escrita, em vez de sumir atras de um assert que nao roda.
@pytest.mark.skipif(os.name != "posix",
                    reason="modo 0600 nao existe no Windows; a protecao por ACL ainda nao existe")
def test_modo_0600_continua_0600(tmp_path):
    """O config do Kimi tem API key em texto puro; virar 0644 pela nossa escrita é vazamento."""
    _homes(tmp_path, "kimi", "pi")
    kimi = tmp_path / ".kimi-code" / "config.toml"
    kimi.write_text(CONFIG_KIMI)
    kimi.chmod(0o600)
    pi = tmp_path / ".pi" / "agent" / "models.json"
    pi.write_text("{}")
    pi.chmod(0o600)
    agentes_sync.sincronizar("cred", "https://x.dev", "sk-abc", MODELOS,
                             homes={a: tmp_path for a in agentes_sync.ALVOS})
    for p in (kimi, pi):
        assert stat.S_IMODE(p.stat().st_mode) == 0o600, p
        bak = p.with_name(p.name + ".bak-hangar")
        assert stat.S_IMODE(bak.stat().st_mode) == 0o600, bak  # o backup também tem a chave


def test_config_corrompido_nao_derruba_os_outros(tmp_path):
    _homes(tmp_path, "pi", "kimi", "codex")
    (tmp_path / ".kimi-code" / "config.toml").write_text("[providers.\nisso não é toml")
    r = agentes_sync.sincronizar("cred", "https://x.dev", "sk-abc", MODELOS,
                                 homes={a: tmp_path for a in agentes_sync.ALVOS})
    assert r["kimi"] == {"ok": False, "motivo": "config-invalido"}
    assert r["pi"]["ok"] and r["codex"]["ok"], r
    # e o arquivo quebrado do usuário não foi tocado
    assert (tmp_path / ".kimi-code" / "config.toml").read_text() == "[providers.\nisso não é toml"


def test_nome_invalido_recusado(tmp_path):
    _homes(tmp_path, "pi", "kimi", "codex")
    r = agentes_sync.sincronizar("Nome Com Espaço", "https://x.dev", "sk", MODELOS,
                                 homes={a: tmp_path for a in agentes_sync.ALVOS})
    assert all(v == {"ok": False, "motivo": "nome-invalido"} for v in r.values()), r


def test_aspas_no_id_do_modelo_nao_quebram_o_toml(tmp_path):
    _homes(tmp_path, "kimi")
    ok, motivo = agentes_sync.gravar_kimi(
        "cred", "https://x.dev", 'sk-com"aspas\\barra', [{"id": 'mo"del'}], home=tmp_path)
    assert ok, motivo
    d = tomllib.loads((tmp_path / ".kimi-code" / "config.toml").read_text())
    assert d["providers"]["cred"]["api_key"] == 'sk-com"aspas\\barra'
    assert d["models"]['cred/mo"del']["model"] == 'mo"del'
