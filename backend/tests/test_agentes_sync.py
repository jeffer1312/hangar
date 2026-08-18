"""agentes_sync: uma credencial do app espalhada pra config de cada agente.

Tudo roda contra um HOME falso (`tmp_path`) — nenhum teste aqui pode encostar no ~/.pi,
~/.kimi-code ou ~/.codex de verdade, que têm as chaves do usuário dentro.
"""
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
