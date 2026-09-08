"""Provas de proveniência e atualização de skills em homes temporárias."""
import json
import os
from pathlib import Path
import shutil

import pytest

from app import codex_skills, skill_bridge
from app.codex_arquivos import hash_bytes
from app.codex_arquivos import AlteradoExternamente
from app.codex_skills import reconciliar_skills


def _skill(path, texto="instruções", *, nome=None):
    path.mkdir(parents=True, exist_ok=True)
    nome = nome or path.name
    (path / "SKILL.md").write_text(f"---\nname: {nome}\ndescription: Teste\n---\n{texto}\n", encoding="utf-8")
    return path


@pytest.fixture
def ambiente(tmp_path, monkeypatch):
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(skill_bridge, "_REPO", tmp_path / "repo")
    return home, home / ".codex", home / "backups"


def _rodar(ambiente, plugins=None, registro=None, **kw):
    home, codex, backups = ambiente
    return reconciliar_skills(home, codex, plugins or {}, registro or {}, backups, **kw)


def test_skill_pessoal_symlink_idempotente_e_sistema_preservado(ambiente):
    home, codex, _ = ambiente
    origem = _skill(home / ".claude/skills/pessoal")
    _skill(codex / "skills/.system")
    _skill(codex / "skills/exclusiva")
    registro, avisos = _rodar(ambiente)
    assert avisos == []
    assert (codex / "skills/pessoal").resolve() == origem
    assert registro["pessoal"]["mode"] == "symlink"
    assert (codex / "skills/.system/SKILL.md").is_file()
    assert (codex / "skills/exclusiva/SKILL.md").is_file()
    assert _rodar(ambiente, registro=registro) == (registro, [])


def test_usuario_tem_precedencia_sobre_plugin_com_mesmo_nome(ambiente):
    home, codex, _ = ambiente
    origem = _skill(home / ".claude/skills/skill", "pessoal diferente")
    _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill", "plugin")
    plugin = codex / "plugins/cache/market/plugin/1"
    _skill(plugin / "skills/skill", "plugin")
    registro, avisos = _rodar(ambiente, {"plugin@market": {"path": str(plugin)}})
    assert avisos == []
    assert (codex / "skills/skill").resolve() == origem
    assert registro["skill"]["mode"] == "symlink"


def test_plugin_equivalente_retira_ponte_gerenciada_e_cria_backup(ambiente):
    home, codex, backups = ambiente
    origem = _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill")
    _rodar(ambiente)
    plugin = codex / "plugins/cache/market/plugin/2"
    _skill(plugin / "skills/skill", "nova versão")
    registro, avisos = _rodar(ambiente, {"plugin@market": {"path": str(plugin)}})
    assert avisos == []
    assert not (codex / "skills/skill").is_symlink()
    assert registro["skill"]["mode"] == "native"
    assert registro["skill"]["source"] == str(origem)
    assert list(backups.glob("*.json"))


def test_plugin_de_outro_marketplace_nao_remove_ponte(ambiente):
    home, codex, _ = ambiente
    origem = _skill(home / ".claude/plugins/cache/market-a/plugin/1/skills/skill")
    plugin = codex / "plugins/cache/market-b/plugin/1"
    _skill(plugin / "skills/skill")
    _rodar(ambiente, {"plugin@market-b": {"path": str(plugin)}})
    assert (codex / "skills/skill").resolve() == origem


def test_exemplos_e_skill_ilegivel_nao_comprovam_plugin(ambiente):
    home, codex, _ = ambiente
    origem = _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill")
    plugin = codex / "plugins/cache/market/plugin/1"
    _skill(plugin / "tests/examples/skill")
    registro, _ = _rodar(ambiente, {"plugin@market": {"path": str(plugin)}})
    assert registro["skill"]["mode"] == "symlink"
    assert (codex / "skills/skill").resolve() == origem
    nativa = _skill(plugin / "skills/skill")
    (nativa / "SKILL.md").write_bytes(b"\xff")
    registro, avisos = _rodar(ambiente, {"plugin@market": {"path": str(plugin)}})
    assert registro["skill"]["mode"] == "symlink"
    assert avisos


def test_roots_declarados_symlink_local_e_nome_frontmatter(ambiente):
    home, codex, _ = ambiente
    _skill(home / ".claude/plugins/cache/market/plugin/1/skills/antigo", nome="nome-real")
    plugin = codex / "plugins/cache/market/plugin/local"
    (plugin / ".codex-plugin").mkdir(parents=True)
    (plugin / ".codex-plugin/plugin.json").write_text(json.dumps({"skills": ["custom"]}))
    skill_local = _skill(home / "plugin-local/custom/novo", nome="nome-real")
    (plugin / "custom").symlink_to(skill_local.parent, target_is_directory=True)
    registro, avisos = _rodar(ambiente, {"plugin@market": {"path": str(plugin)}})
    assert avisos == []
    assert registro["antigo"]["mode"] == "native"
    assert not (codex / "skills/antigo").exists()


def test_link_pessoal_e_diretorio_real_homonimos_preservados(ambiente):
    home, codex, _ = ambiente
    _skill(home / ".claude/skills/link")
    _skill(home / ".claude/skills/real")
    externo = _skill(home / "externo")
    (codex / "skills").mkdir(parents=True)
    (codex / "skills/link").symlink_to(externo)
    _skill(codex / "skills/real", "conteúdo exclusivo")
    registro, avisos = _rodar(ambiente)
    assert registro == {}
    assert len(avisos) == 2
    assert (codex / "skills/link").resolve() == externo
    assert "conteúdo exclusivo" in (codex / "skills/real/SKILL.md").read_text()


def test_falha_criando_novo_symlink_mantem_link_anterior(ambiente, monkeypatch):
    home, codex, _ = ambiente
    velho = _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill")
    _rodar(ambiente)
    _skill(home / ".claude/plugins/cache/market/plugin/2/skills/skill", "nova")
    def falhar(*args, **kw):
        raise OSError("Sem permissão")
    monkeypatch.setattr(Path, "symlink_to", falhar)
    _, avisos = _rodar(ambiente, windows=True)
    assert avisos
    assert (codex / "skills/skill").resolve() == velho
    assert not list((codex / "skills").glob(".*.hangar-*"))


def test_copia_windows_reflete_claude_remove_so_gerenciados_e_preserva_exclusivos(ambiente, monkeypatch):
    home, codex, backups = ambiente
    origem = _skill(home / ".claude/skills/skill", "v1")
    (origem / "remover.txt").write_text("antigo")
    (origem / "alterado.txt").write_text("base")
    def falhar(*args, **kw):
        raise OSError("Symlink indisponível")
    monkeypatch.setattr(Path, "symlink_to", falhar)
    registro, avisos = _rodar(ambiente, windows=True)
    assert avisos == []
    assert registro["skill"]["mode"] == "copy"
    destino = codex / "skills/skill"
    (destino / "exclusivo.txt").write_text("meu")
    (destino / "alterado.txt").write_text("alteração local")
    (origem / "SKILL.md").write_text("v2")
    (origem / "remover.txt").unlink()
    (origem / "alterado.txt").write_text("v2")
    registro, avisos = _rodar(ambiente, registro=registro, windows=True)
    assert (destino / "SKILL.md").read_text() == "v2"
    assert not (destino / "remover.txt").exists()
    assert (destino / "exclusivo.txt").read_text() == "meu"
    assert (destino / "alterado.txt").read_text() == "v2"
    assert "remover.txt" not in registro["skill"]["files"]
    assert avisos == []
    backups_antes = {p.name: p.read_bytes() for p in backups.iterdir()}
    _rodar(ambiente, registro=registro, windows=True)
    assert {p.name: p.read_bytes() for p in backups.iterdir()} == backups_antes


def test_sem_plugin_a_copia_pessoal_em_agents_fica_e_a_ponte_nao_linka(ambiente):
    # ~/.agents/skills é fonte do Pi/Kimi/omp e o Codex já a lê: nada a apagar, nada a duplicar.
    home, codex, backups = ambiente
    origem = _skill(home / ".claude/skills/skill")
    (origem / "helper.py").write_text("print('teste')")
    duplicata = home / ".agents/skills/skill"
    shutil.copytree(origem, duplicata)
    registro, avisos = _rodar(ambiente)
    assert avisos == []
    assert duplicata.exists()
    assert not os.path.lexists(codex / "skills/skill")
    assert registro["skill"]["mode"] == "native"
    assert list(backups.glob("*.json")) == []


def test_duplicata_nativa_diferente_preservada_sem_proveniencia(ambiente):
    home, codex, _ = ambiente
    _skill(home / ".claude/skills/skill", "atual")
    duplicata = _skill(home / ".agents/skills/skill", "outra")
    _, avisos = _rodar(ambiente)
    assert avisos and "difere" in avisos[0]
    assert duplicata.exists()
    assert not os.path.lexists(codex / "skills/skill")


def _plugin_com_skill(home, codex):
    _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill")
    plugin = codex / "plugins/cache/market/plugin/1"
    _skill(plugin / "skills/skill")
    return {"plugin@market": {"path": str(plugin)}}


def test_plugin_confirmado_nao_apaga_copia_pessoal_identica(ambiente):
    home, codex, _ = ambiente
    plugins = _plugin_com_skill(home, codex)
    origem = home / ".claude/plugins/cache/market/plugin/1/skills/skill"
    duplicata = home / ".agents/skills/skill"
    shutil.copytree(origem, duplicata)
    _, avisos = _rodar(ambiente, plugins)
    assert duplicata.exists()
    assert avisos and "preservada" in avisos[0]


def test_plugin_confirmado_retira_so_a_copia_que_o_hangar_registrou(ambiente):
    home, codex, _ = ambiente
    plugins = _plugin_com_skill(home, codex)
    origem = home / ".claude/plugins/cache/market/plugin/1/skills/skill"
    duplicata = _skill(home / ".agents/skills/skill", "versão antiga")
    registro = {"skill": {"path": str(duplicata), "mode": "copy", "source": str(origem),
                           "files": {"SKILL.md": hash_bytes((duplicata / "SKILL.md").read_bytes())}}}
    novo, avisos = _rodar(ambiente, plugins, registro)
    assert avisos == []
    assert not duplicata.exists()
    assert novo["skill"]["mode"] == "native"


def test_skill_ja_nativa_em_agents_nao_ganha_ponte_duplicada(ambiente):
    home, codex, _ = ambiente
    skill = _skill(home / ".agents/skills/nativa")
    registro, avisos = _rodar(ambiente)
    assert avisos == []
    assert registro["nativa"]["mode"] == "native"
    assert not (codex / "skills/nativa").exists()
    assert skill.exists()


def test_plugin_nativo_retira_copia_windows_sem_apagar_arquivo_exclusivo(ambiente, monkeypatch):
    home, codex, _ = ambiente
    _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill")
    def falhar(*args, **kw):
        raise OSError("Symlink indisponível")
    monkeypatch.setattr(Path, "symlink_to", falhar)
    registro, _ = _rodar(ambiente, windows=True)
    destino = codex / "skills/skill"
    (destino / "exclusivo.txt").write_text("meu")
    plugin = codex / "plugins/cache/market/plugin/1"
    _skill(plugin / "skills/skill")
    registro, avisos = _rodar(ambiente, {"plugin@market": {"path": str(plugin)}}, registro, windows=True)
    assert not (destino / "SKILL.md").exists()
    assert (destino / "exclusivo.txt").read_text() == "meu"
    assert registro["skill"]["files"] == {}
    assert avisos


def test_fonte_removida_retira_so_copia_gerenciada(ambiente, monkeypatch):
    home, codex, _ = ambiente
    origem = _skill(home / ".claude/skills/skill")
    _skill(home / ".claude/skills/outra")
    def falhar(*args, **kw):
        raise OSError("Symlink indisponível")
    monkeypatch.setattr(Path, "symlink_to", falhar)
    registro, _ = _rodar(ambiente, windows=True)
    destino = codex / "skills/skill"
    (destino / "exclusivo.txt").write_text("meu")
    shutil.rmtree(origem)
    registro, avisos = _rodar(ambiente, registro=registro, windows=True)
    assert not (destino / "SKILL.md").exists()
    assert (destino / "exclusivo.txt").read_text() == "meu"
    assert registro["skill"]["files"] == {}
    assert avisos


def test_falha_na_ponte_nao_remove_duplicata_que_ainda_serve_ao_codex(ambiente, monkeypatch):
    home, _, _ = ambiente
    origem = _skill(home / ".claude/skills/skill")
    duplicata = home / ".agents/skills/skill"
    shutil.copytree(origem, duplicata)
    def falhar(*args, **kw):
        raise OSError("Symlink indisponível")
    monkeypatch.setattr(Path, "symlink_to", falhar)
    registro, avisos = _rodar(ambiente)
    assert avisos == [], "a ponte nem tenta linkar o que o Codex já lê em ~/.agents/skills"
    assert registro["skill"]["mode"] == "native"
    assert duplicata.exists()


def test_troca_concorrente_para_link_pessoal_e_preservada(ambiente, monkeypatch):
    home, codex, _ = ambiente
    _skill(home / ".claude/plugins/cache/market/plugin/1/skills/skill")
    _rodar(ambiente)
    _skill(home / ".claude/plugins/cache/market/plugin/2/skills/skill")
    pessoal = _skill(home / "skill-pessoal")
    destino = codex / "skills/skill"
    original = Path.symlink_to
    def intercalar(self, target, **kwargs):
        original(self, target, **kwargs)
        if self.name.startswith(".skill.hangar-"):
            destino.unlink()
            original(destino, pessoal)
    monkeypatch.setattr(Path, "symlink_to", intercalar)
    _, avisos = _rodar(ambiente)
    assert avisos
    assert destino.resolve() == pessoal


def test_copia_windows_rele_arquivo_gerenciado_apos_conflito(ambiente, monkeypatch):
    home, codex, _ = ambiente
    origem = _skill(home / ".claude/skills/skill")
    def sem_link(*args, **kwargs):
        raise OSError("Symlink indisponível")
    monkeypatch.setattr(Path, "symlink_to", sem_link)
    registro, _ = _rodar(ambiente, windows=True)
    destino = codex / "skills/skill/SKILL.md"
    antigo = destino.read_bytes()
    (origem / "SKILL.md").write_bytes(b"Claude")
    original = codex_skills.gravar
    esperados = []
    def intercalar(path, data, esperado, backups):
        esperados.append(esperado)
        if len(esperados) == 1:
            path.write_bytes(b"concorrente")
            raise AlteradoExternamente("Mudou")
        return original(path, data, esperado, backups)
    monkeypatch.setattr(codex_skills, "gravar", intercalar)
    registro, avisos = _rodar(ambiente, registro=registro, windows=True)
    assert avisos == []
    assert esperados == [antigo, b"concorrente"]
    assert destino.read_bytes() == b"Claude"
    assert registro["skill"]["files"]["SKILL.md"] == hash_bytes(b"Claude")


@pytest.mark.parametrize("alvo,raiz,gerenciado", [
    (r"\\?\C:\Users\Pessoa\.claude\skills\probe", r"C:\Users\Pessoa\.claude\skills", True),
    (r"\\?\c:\users\pessoa\.CLAUDE\skills\probe", r"C:\Users\Pessoa\.claude\skills", True),
    (r"C:\Users\Pessoa\.claude\skills\probe", r"\\?\C:\Users\Pessoa\.claude\skills", True),
    (r"\\?\UNC\server\share\.claude\skills\probe", r"\\server\share\.claude\skills", True),
    (r"\\server\share\.claude\skills\probe", r"\\?\UNC\server\share\.claude\skills", True),
    (r"\\?\C:\Users\Pessoa\.claude\skills-pessoal\probe", r"C:\Users\Pessoa\.claude\skills", False),
    (r"\\?\C:\backup\Users\Pessoa\.claude\skills\probe", r"C:\Users\Pessoa\.claude\skills", False),
    (r"\\?\D:\Users\Pessoa\.claude\skills\probe", r"C:\Users\Pessoa\.claude\skills", False),
    (r"\\?\C:\Users\Pessoa\.claude\skills\..\externa\probe", r"C:\Users\Pessoa\.claude\skills", False),
    (r"\\?\UNC\outro\share\.claude\skills\probe", r"\\server\share\.claude\skills", False),
])
def test_proveniencia_windows_normaliza_prefixo_sem_adotar_externos(tmp_path, monkeypatch, alvo, raiz, gerenciado):
    entrada = tmp_path / "ponte"
    monkeypatch.setattr(Path, "is_symlink", lambda self: self == entrada)
    monkeypatch.setattr(codex_skills.os, "readlink", lambda path: alvo)
    assert codex_skills._link_gerenciado(entrada, (raiz,), windows=True) is gerenciado
