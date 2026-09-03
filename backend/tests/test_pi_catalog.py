"""Parse do `pi --list-models`.

Por que parse de tabela e não JSON: o `pi --list-models` não tem modo JSON (medido — `--json` não é
flag dele; `--mode json` é do agente). E por que ele e não o sidecar da extensão: medido em
10/08/2026, as duas fontes trazem os MESMOS 384 modelos, mas só esta traz contexto e imagem. O
sidecar continua sendo a fonte de `levels` por modelo, que esta não tem.
"""
import subprocess

import pytest

from app import pi_catalog

SAIDA = """provider          model                                    context  max-out  thinking  images
cline             deepseek/deepseek-v4-flash               1.0M     131.1K   yes       no
kimi-coding       k3                                       1.0M     131.1K   yes       yes
clinepass         cline-pass/glm-5.2                       200K     131.1K   yes       no
"""

_PI_FALSO = "/opt/bin/pi"


@pytest.fixture(autouse=True)
def _pi_no_path(monkeypatch):
    """O `pi` REAL da maquina nao pode decidir se o teste roda.

    `listar()` resolve o binario com `shutil.which` (o `pi` do npm e um `.CMD` no Windows, que o
    `CreateProcess` nao acha pelo nome cru) — sem este fixture, quem nao tem o Pi instalado veria
    `PiAusente` antes de chegar no `subprocess.run` mockado, e o caso testaria a maquina.
    """
    monkeypatch.setattr(pi_catalog.shutil, "which", lambda nome: _PI_FALSO)


def test_cabecalho_e_descartado():
    assert all(m["id"] != "model" for m in pi_catalog.parse(SAIDA))


def test_le_provedor_e_id():
    ms = pi_catalog.parse(SAIDA)
    assert {"provider": "kimi-coding", "id": "k3"}.items() <= ms[1].items()


def test_id_com_barra_dentro_sobrevive():
    """`clinepass` + `cline-pass/glm-5.2`: o Pi corta na PRIMEIRA barra (medido), então o id tem que
    manter a barra interna."""
    ms = pi_catalog.parse(SAIDA)
    assert ms[2]["id"] == "cline-pass/glm-5.2"
    assert ms[2]["provider"] == "clinepass"


def test_etiquetas_de_contexto_e_imagem():
    ms = pi_catalog.parse(SAIDA)
    assert ms[1]["context"] == "1.0M"
    assert ms[1]["images"] is True
    assert ms[0]["images"] is False


def test_linha_malformada_e_pulada_sem_derrubar_a_lista():
    assert len(pi_catalog.parse(SAIDA + "lixo\n")) == 3


def test_tabela_com_coluna_a_mais_nao_vira_lista_vazia(monkeypatch):
    """Mudança de formato do pi tem que virar erro visível, não seletor vazio dito completo."""
    nova = ("provider model context max-out thinking images cost\n"
            "cline deepseek/v4 1.0M 131.1K yes no 0.10\n")
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, nova, ""))
    pi_catalog._cache.clear()
    with pytest.raises(RuntimeError):
        pi_catalog.listar()


def test_falha_nao_fica_no_cache(monkeypatch):
    """O vazio não pode sobreviver ao conserto do pi."""
    pi_catalog._cache.clear()
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, "", ""))
    with pytest.raises(RuntimeError):
        pi_catalog.listar()
    assert "pi" not in pi_catalog._cache
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, SAIDA, ""))
    assert len(pi_catalog.listar()) == 3


def test_id_com_byte_ilegivel_e_falha_do_provedor(monkeypatch):
    """`errors="replace"` troca byte ruim por `�` — e o `id` daqui é DIGITADO na TUI depois
    (`/cp-model <provider> <id>`), então um id com `�` é troca de modelo que falha sem explicação.

    Vale a mesma doutrina da tabela irreconhecível: erro visível, e sem cachear.
    """
    ruim = SAIDA.replace("k3", "k�3")
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, ruim, ""))
    pi_catalog._cache.clear()
    with pytest.raises(RuntimeError, match="ilegivel"):
        pi_catalog.listar()
    assert "pi" not in pi_catalog._cache


def test_argv_leva_o_caminho_RESOLVIDO_e_nao_o_nome_cru(monkeypatch):
    """No Windows o `pi` do npm global e um `pi.CMD`, e o `CreateProcess` so completa `.exe`:
    `subprocess.run(["pi", …])` levanta FileNotFoundError com o pi instalado e no PATH (medido
    22/08/2026 — a tela de abertura dava 502 enquanto o `cli_probe`, que usa `which`, dizia que o
    pi existia). Quem resolve e o `shutil.which`, que aplica o PATHEXT."""
    visto = {}

    def falso_run(argv, **k):
        visto["argv"] = argv
        return subprocess.CompletedProcess(argv, 0, SAIDA, "")

    monkeypatch.setattr(pi_catalog.subprocess, "run", falso_run)
    pi_catalog._cache.clear()
    pi_catalog.listar()
    assert visto["argv"][0] == _PI_FALSO
    assert visto["argv"][0] != "pi"


def test_pi_fora_do_path_e_erro_PROPRIO(monkeypatch):
    """"Nao achei o pi" nao e "o pi falhou": antes isso chegava na tela como
    `[WinError 2] O sistema nao pode encontrar o arquivo especificado` dentro da mensagem de falha
    do comando, e mandava procurar defeito num pi que nem estava instalado."""
    monkeypatch.setattr(pi_catalog.shutil, "which", lambda nome: None)
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: pytest.fail("nao pode nem tentar rodar sem binario"))
    pi_catalog._cache.clear()
    with pytest.raises(pi_catalog.PiAusente, match="PATH"):
        pi_catalog.listar()
    assert "pi" not in pi_catalog._cache
    # Subclasse de RuntimeError de proposito: a rota ja captura RuntimeError, entao um backend
    # antigo (ou outro chamador) nunca deixa isso virar 500 cru.
    assert issubclass(pi_catalog.PiAusente, RuntimeError)


def test_rotulo_ilegivel_em_coluna_de_leitura_nao_derruba_a_lista(monkeypatch):
    """`�` em coluna que só se lê (contexto) é feio, não errado — seletor vazio seria pior."""
    ruim = SAIDA.replace("200K", "20�K")
    monkeypatch.setattr(pi_catalog.subprocess, "run",
                        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, ruim, ""))
    pi_catalog._cache.clear()
    assert len(pi_catalog.listar()) == 3


OMP_JSON = '''{"models":[{"provider":"opencode-go","id":"deepseek-v4-flash","selector":"opencode-go/deepseek-v4-flash",
"name":"DeepSeek V4 Flash","contextWindow":1000000,"maxTokens":384000,"reasoning":true,
"thinking":["low","high","max"],"input":["text"],"cost":{"input":0.22}},
{"provider":"opencode-go","id":"deepseek-v4-flash-vision-exp","name":"x","contextWindow":1000000,
"maxTokens":384000,"reasoning":true,"thinking":[],"input":["text","image"]}]}'''


def test_parse_omp_devolve_o_mesmo_shape_do_pi():
    from app import pi_catalog
    ms = pi_catalog.parse_omp(OMP_JSON)
    assert ms[0] == {"provider": "opencode-go", "id": "deepseek-v4-flash", "context": "1M",
                     "max_out": "384K", "thinking": True, "images": False}
    assert ms[1]["images"] is True and ms[1]["thinking"] is False


def test_listar_omp_chama_omp_models_json_e_cacheia_por_provider(monkeypatch):
    from app import pi_catalog
    pi_catalog._cache.clear()
    chamadas = []

    class R:
        returncode = 0; stderr = ""
        def __init__(self, out): self.stdout = out
    def fake_run(cmd, **kw):
        chamadas.append(cmd)
        return R(OMP_JSON if cmd[0].endswith("omp") else "provider id ctx max thinking images\npi-prov m1 128k 8k yes no\n")
    monkeypatch.setattr(pi_catalog.subprocess, "run", fake_run)
    monkeypatch.setattr(pi_catalog.shutil, "which", lambda b: f"/bin/{b}")
    omp = pi_catalog.listar("omp")
    pi = pi_catalog.listar("pi")
    assert chamadas[0][:3] == ["/bin/omp", "models", "--json"]
    assert chamadas[1] == ["/bin/pi", "--list-models"]
    assert omp[0]["id"] == "deepseek-v4-flash" and pi[0]["id"] == "m1"
    assert pi_catalog.listar("omp") is omp
