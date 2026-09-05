"""orq_politica: tabela de contas liberadas, seção gerada, id de cota e a regra `permitido`."""
from pathlib import Path

import pytest

from app import orq_md, orq_politica as pol
from app.config import ConfigDirInfo

ARQUIVO_VIVO = """# Contas e modelos desta máquina — política de uso

**Quem decide é o usuário.** Prosa que o app NUNCA toca.

## O que pode

| conta | provider | apelido | modelos | trocar? |
|---|---|---|---|---|
| `200-01` | claude | Rafael e Viana | opus[1m], opus | sim |
| apikey | kimi | Kimi da casa | * | sim |
| opencode-go | pi | Deepseek Claude | deepseek-v4-flash | não |

## O que NÃO pode

- `openrouter` (pi) — não liberada

## Como levantar isto em OUTRA máquina

texto do usuário.
"""


@pytest.fixture
def maquina(tmp_path, monkeypatch):
    home = tmp_path / "home"
    (home / ".claude").mkdir(parents=True)
    (home / ".claude-200-01").mkdir()
    (home / ".claude-claude-200-2").mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    monkeypatch.setattr(pol.contas, "compartilhado", lambda: home / ".claude")
    (home / ".hangar").mkdir()
    (home / ".hangar" / "orquestracao-contas.md").write_text(ARQUIVO_VIVO, encoding="utf-8")
    # inventário determinístico
    monkeypatch.setattr(pol.config, "list_config_dirs", lambda: [
        ConfigDirInfo(path=str(home / ".claude"), label="Felizardo", active=True),
        ConfigDirInfo(path=str(home / ".claude-200-01"), label="Rafael", active=False),
        ConfigDirInfo(path=str(home / ".claude-claude-200-2"), label="Ricardo", active=False)])
    monkeypatch.setattr(pol.cotas, "_providers_kimi", lambda: [("apikey", "k", "https://x")])
    monkeypatch.setattr(pol.kimi_models, "read_catalog", lambda path=None: {"default": None, "models": [
        {"alias": "apikey/k3", "provider": "apikey", "id": "k3", "name": "K3",
         "context_length": 1000000, "efforts": ["low", "high", "max"], "default_effort": "high"}]})
    monkeypatch.setattr(pol.pi_catalog, "listar", lambda fresco=False: [
        {"provider": "opencode-go", "id": "deepseek-v4-flash", "context": "128k", "thinking": True},
        {"provider": "openrouter", "id": "x/y", "context": "8k", "thinking": False}])
    monkeypatch.setattr(pol.apelidos, "ler", lambda: {})
    return home


def test_le_tabela_do_arquivo_vivo(maquina):
    lida = pol.ler()
    assert [(c.conta, c.provider) for c in lida] == [("200-01", "claude"), ("apikey", "kimi"), ("opencode-go", "pi")]
    assert lida[0].modelos == ("opus[1m]", "opus")
    assert lida[1].modelos == ("*",) and lida[1].trocar
    assert not lida[2].trocar


def test_id_cota_aceita_as_duas_grafias(maquina):
    home = maquina
    assert pol.id_cota("claude", "200-01") == f"claude:{(home / '.claude-200-01').resolve()}"
    assert pol.id_cota("claude", "claude-200-2") == f"claude:{(home / '.claude-claude-200-2').resolve()}"
    assert pol.id_cota("claude", "padrao") == f"claude:{(home / '.claude').resolve()}"
    assert pol.id_cota("kimi", "apikey") == "kimi:apikey"
    assert pol.id_cota("pi", "clinepass") is None
    assert pol.nome_conta_claude(home / ".claude-claude-200-2") == "claude-200-2"
    assert pol.nome_conta_claude(home / ".claude") == "padrao"


def test_inventario_junta_os_quatro_providers(maquina):
    inv = pol.inventario()
    chaves = [(i.provider, i.conta) for i in inv]
    assert chaves == [("claude", "padrao"), ("claude", "200-01"), ("claude", "claude-200-2"),
                      ("kimi", "apikey"), ("pi", "opencode-go"), ("pi", "openrouter"), ("codex", "openai-codex")]
    kimi = inv[3]
    assert kimi.modelos[0]["id"] == "apikey/k3" and kimi.modelos[0]["efforts"] == ["low", "high", "max"]
    # Claude sem cache do picker → lista reduzida
    assert [m["id"] for m in inv[0].modelos] == ["opus", "sonnet", "haiku"]


def test_inventario_sobrevive_a_pi_ausente(maquina, monkeypatch):
    monkeypatch.setattr(pol.pi_catalog, "listar", lambda fresco=False: (_ for _ in ()).throw(RuntimeError("sem pi")))
    assert [i.provider for i in pol.inventario()] == ["claude", "claude", "claude", "kimi", "codex"]


def test_gravar_conta_troca_so_as_duas_secoes(maquina):
    pol.gravar_conta(pol.ContaPolitica("claude-200-2", "claude", "Ricardo", ("opus",), True))
    texto = pol.caminho().read_text(encoding="utf-8")
    assert "Prosa que o app NUNCA toca." in texto and "texto do usuário." in texto
    assert "| claude-200-2 | claude | Ricardo | opus | sim |" in texto
    nao_pode = texto.split("## O que NÃO pode")[1].split("## Como")[0]
    assert "`padrao` (claude)" in nao_pode and "`openrouter` (pi)" in nao_pode
    assert "`claude-200-2`" not in nao_pode and "`200-01`" not in nao_pode
    assert [c.conta for c in pol.ler()] == ["200-01", "apikey", "opencode-go", "claude-200-2"]


def test_desligar_remove_a_linha_e_lista_em_nao_pode(maquina):
    pol.desligar("apikey")
    texto = pol.caminho().read_text(encoding="utf-8")
    assert [c.conta for c in pol.ler()] == ["200-01", "opencode-go"]
    assert "`apikey` (kimi) — não liberada" in texto


def test_gravar_com_mtime_velho_e_conflito(maquina):
    import os
    mt = pol.caminho().stat().st_mtime
    os.utime(pol.caminho(), (1, 1))
    with pytest.raises(orq_md.Conflito):
        pol.gravar_conta(pol.ContaPolitica("x", "codex"), mt)


def test_migrar_leva_o_arquivo_do_config_dir_pro_cofre(maquina):
    pol.caminho().unlink()
    antigo = Path.home() / ".claude" / "orquestracao-contas.md"
    antigo.write_text(ARQUIVO_VIVO, encoding="utf-8")
    assert pol.migrar() is True
    assert not antigo.exists() and pol.caminho().read_text(encoding="utf-8") == ARQUIVO_VIVO
    assert pol.migrar() is False          # roda de novo sem efeito


def test_migrar_nao_funde_quando_os_dois_existem(maquina):
    antigo = Path.home() / ".claude" / "orquestracao-contas.md"
    antigo.write_text("antigo", encoding="utf-8")
    assert pol.migrar() is False
    assert antigo.read_text(encoding="utf-8") == "antigo"
    assert pol.caminho().read_text(encoding="utf-8") == ARQUIVO_VIVO


def test_migrar_nao_move_link_nem_derruba_a_subida(maquina, monkeypatch):
    pol.caminho().unlink()
    antigo = Path.home() / ".claude" / "orquestracao-contas.md"
    alvo = Path.home() / "fora.md"
    alvo.write_text("x", encoding="utf-8")
    antigo.symlink_to(alvo)
    assert pol.migrar() is False and antigo.is_symlink()
    antigo.unlink()
    antigo.write_text(ARQUIVO_VIVO, encoding="utf-8")
    monkeypatch.setattr(pol.shutil, "move", lambda *a: (_ for _ in ()).throw(PermissionError("nope")))
    assert pol.migrar() is False          # fail-soft: avisa no log, nao levanta
    assert antigo.exists() and not pol.caminho().exists()


def test_arquivo_ausente_nasce_com_as_duas_secoes(maquina):
    pol.caminho().unlink()
    pol.gravar_conta(pol.ContaPolitica("padrao", "claude", "F", ("*",), True))
    texto = pol.caminho().read_text(encoding="utf-8")
    assert texto.startswith("## O que pode\n\n| conta |")
    assert "## O que NÃO pode" in texto


def test_politica_vazia_nao_proibe_nada():
    assert pol.permitido("claude", "qualquer", "sonnet", "high", politica=[]) is None
    assert pol.permitido("claude", "qualquer", "sonnet", "turbo", politica=[]) == "erro_orq_esforco_invalido"


def test_tabela_recusa_linha_omp(maquina):
    # `permitido()` normaliza omp->pi antes de procurar: uma linha `omp` gravada aqui nunca seria
    # consultada, e a tela mostraria uma liberacao que nao vale nada.
    with pytest.raises(ValueError, match="política do omp é a do Pi"):
        pol.gravar_conta(pol.ContaPolitica("opencode-go", "omp"))
    texto = ARQUIVO_VIVO.replace("| opencode-go | pi |", "| opencode-go | omp |")
    assert [c.provider for c in pol.ler(texto)] == ["claude", "kimi"], "linha omp e ignorada na leitura"


def test_permitido_omp_reusa_a_linha_do_pi(maquina):
    # omp e o MESMO credencial do pi (sem linha propria na tabela) — uma conta liberada pra "pi"
    # libera "omp" tambem, e uma conta pi nao liberada tambem barra o omp.
    assert pol.permitido("omp", "opencode-go", "deepseek-v4-flash", "max") is None
    assert pol.permitido("omp", "openrouter", "x/y", "high") == "erro_orq_conta_nao_liberada"


def test_permitido(maquina):
    assert pol.permitido("claude", "200-01", "opus[1m]", "medium") is None
    assert pol.permitido("claude", "padrao", "opus", "high") == "erro_orq_conta_nao_liberada"
    assert pol.permitido("claude", "200-01", "sonnet", "high") == "erro_orq_modelo_nao_liberado"
    assert pol.permitido("pi", "opencode-go", "glm", "high") == "erro_orq_conta_travada"
    assert pol.permitido("pi", "opencode-go", "deepseek-v4-flash", "max") is None
    assert pol.permitido("claude", "200-01", "opus", "ultracode") == "erro_orq_esforco_invalido"
    assert pol.permitido("kimi", "apikey", "apikey/k3", "medium") == "erro_orq_esforco_invalido"
    assert pol.permitido("kimi", "apikey", "apikey/k3", "max") is None
    assert pol.permitido("kimi", "apikey", "apikey/desconhecido", "qualquer") is None
