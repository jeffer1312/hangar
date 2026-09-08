"""Mesclagem de artefatos e configuração sem alterar entradas particulares."""
from copy import deepcopy
import json

import pytest

from app import codex_fragmentos
from app.codex_arquivos import AlteradoExternamente, hash_bytes
from app.codex_fragmentos import mesclar_config, reconciliar_arquivos


def test_arquivos_adotam_igual_criam_novo_e_preservam_colisao(tmp_path):
    igual, novo, particular = (tmp_path / nome for nome in ("igual.md", "novo.md", "particular.md"))
    igual.write_bytes(b"igual")
    particular.write_bytes(b"pessoal")
    fontes = {igual: b"igual", novo: b"novo", particular: b"importado"}
    manifesto, avisos = reconciliar_arquivos(fontes, {}, tmp_path / "backups")
    assert set(manifesto) == {str(igual), str(novo)}
    assert novo.read_bytes() == b"novo"
    assert particular.read_bytes() == b"pessoal"
    assert len(avisos) == 1
    assert not (tmp_path / "backups").exists()


def test_historico_nativo_permite_adotar_e_backup_e_so_da_primeira_mudanca(tmp_path):
    path, backups = tmp_path / "agent.md", tmp_path / "backups"
    path.write_bytes(b"nativo antigo")
    manifesto, avisos = reconciliar_arquivos({path: b"nova fonte"}, {}, backups, confiaveis={path})
    assert avisos == []
    assert path.read_bytes() == b"nova fonte"
    salvo = next(backups.iterdir())
    registro = salvo.read_bytes()
    assert bytes.fromhex(json.loads(registro)["conteudo_hex"]) == b"nativo antigo"
    manifesto, avisos = reconciliar_arquivos({path: b"mais nova"}, manifesto, backups)
    assert avisos == []
    assert path.read_bytes() == b"mais nova"
    assert salvo.read_bytes() == registro


def test_arquivo_gerenciado_reflete_claude_e_preserva_edicao_se_fonte_removida(tmp_path):
    path, backups = tmp_path / "agent.md", tmp_path / "backups"
    manifesto, _ = reconciliar_arquivos({path: b"fonte"}, {}, backups)
    path.write_bytes(b"edicao local")
    novo, avisos = reconciliar_arquivos({path: b"atualizacao"}, manifesto, backups)
    assert novo[str(path)]["hash"] == hash_bytes(b"atualizacao")
    assert avisos == []
    assert path.read_bytes() == b"atualizacao"
    assert bytes.fromhex(json.loads(next(backups.iterdir()).read_bytes())["conteudo_hex"]) == b"edicao local"
    path.write_bytes(b"nova edicao local")
    preservado, avisos = reconciliar_arquivos({}, novo, backups)
    assert preservado == novo
    assert avisos
    assert path.read_bytes() == b"nova edicao local"


def test_arquivo_obsoleto_so_e_removido_se_hash_corresponde(tmp_path):
    path, backups = tmp_path / "agent.md", tmp_path / "backups"
    manifesto, _ = reconciliar_arquivos({path: b"fonte"}, {}, backups)
    novo, avisos = reconciliar_arquivos({}, manifesto, backups)
    assert novo == {}
    assert avisos == []
    assert not path.exists()
    assert bytes.fromhex(json.loads(next(backups.iterdir()).read_bytes())["conteudo_hex"]) == b"fonte"


def test_edicao_concorrente_durante_backup_impede_remocao(tmp_path, monkeypatch):
    path, backups = tmp_path / "agent.md", tmp_path / "backups"
    manifesto, _ = reconciliar_arquivos({path: b"fonte"}, {}, backups)
    original = codex_fragmentos.backup
    def intercalar(*args):
        original(*args)
        path.write_bytes(b"nova edicao")
    monkeypatch.setattr(codex_fragmentos, "backup", intercalar)
    novo, avisos = reconciliar_arquivos({}, manifesto, backups)
    assert novo == manifesto
    assert avisos
    assert path.read_bytes() == b"nova edicao"


def test_conflito_de_escrita_nao_declara_conteudo_que_nao_foi_gravado(tmp_path, monkeypatch):
    path = tmp_path / "agent.md"
    def falhar(*args):
        raise AlteradoExternamente("Mudou")
    monkeypatch.setattr(codex_fragmentos, "gravar", falhar)
    novo, avisos = reconciliar_arquivos({path: b"fonte"}, {}, tmp_path / "backups")
    assert novo == {}
    assert avisos
    assert not path.exists()


def test_update_gerenciado_rele_bytes_apos_conflito_e_reaplica_claude(tmp_path, monkeypatch):
    path, backups = tmp_path / "agent.md", tmp_path / "backups"
    manifesto, _ = reconciliar_arquivos({path: b"antigo"}, {}, backups)
    original = codex_fragmentos.gravar
    esperados = []
    def intercalar(destino, data, esperado, pasta):
        esperados.append(esperado)
        if len(esperados) == 1:
            destino.write_bytes(b"concorrente")
            raise AlteradoExternamente("Mudou")
        return original(destino, data, esperado, pasta)
    monkeypatch.setattr(codex_fragmentos, "gravar", intercalar)
    novo, avisos = reconciliar_arquivos({path: b"Claude"}, manifesto, backups)
    assert avisos == []
    assert esperados == [b"antigo", b"concorrente"]
    assert path.read_bytes() == b"Claude"
    assert novo[str(path)]["hash"] == hash_bytes(b"Claude")


def test_arquivo_particular_criado_durante_primeira_escrita_nao_e_adotado(tmp_path, monkeypatch):
    path = tmp_path / "agent.md"
    def intercalar(*args):
        path.write_bytes(b"particular")
        raise AlteradoExternamente("Outro processo criou")
    monkeypatch.setattr(codex_fragmentos, "gravar", intercalar)
    manifesto, avisos = reconciliar_arquivos({path: b"Claude"}, {}, tmp_path / "backups")
    assert manifesto == {}
    assert avisos
    assert path.read_bytes() == b"particular"


def test_symlink_de_artefato_e_substituido_sem_escrever_na_fonte(tmp_path):
    fonte, destino = tmp_path / "particular.md", tmp_path / "agent.md"
    fonte.write_bytes(b"fonte")
    destino.symlink_to(fonte)
    anterior = {str(destino): {"hash": hash_bytes(b"fonte")}}
    novo, avisos = reconciliar_arquivos({destino: b"atualizado"}, anterior, tmp_path / "backups")
    assert avisos == []
    assert not destino.is_symlink()
    assert fonte.read_bytes() == b"fonte"
    assert novo[str(destino)]["hash"] == hash_bytes(b"atualizado")


def test_config_adota_igual_cria_novo_e_preserva_colisao_sem_proveniencia():
    atual = {"igual": {"command": "a"}, "colisao": {"command": "meu"}, "pessoal": {"url": "local"}}
    fonte = {"igual": {"command": "a"}, "colisao": {"command": "novo"}, "novo": {"command": "b"}}
    original = deepcopy(atual)
    novo, manifesto, avisos = mesclar_config(atual, fonte, {})
    assert novo == {**original, "novo": {"command": "b"}}
    assert manifesto == {"igual": {"command": "a"}, "novo": {"command": "b"}}
    assert len(avisos) == 1
    assert atual == original
    novo["igual"]["command"] = "mudado"
    assert fonte["igual"]["command"] == "a"
    assert manifesto["igual"]["command"] == "a"


def test_config_historico_adota_preservando_campos_complementares_recursivos():
    atual = {"mcp": {"command": "velho", "env": {"LOCAL": "meu", "FONTE": "velho"},
                     "startup_timeout_sec": 60}}
    fonte = {"mcp": {"command": "novo", "env": {"FONTE": "novo"}}}
    novo, manifesto, avisos = mesclar_config(atual, fonte, {}, confiaveis={"mcp"})
    assert avisos == []
    assert novo["mcp"] == {"command": "novo", "env": {"LOCAL": "meu", "FONTE": "novo"},
                            "startup_timeout_sec": 60}
    assert manifesto == fonte
    assert mesclar_config(novo, fonte, manifesto) == (novo, manifesto, [])


def test_config_remove_campos_antigos_apenas_se_ainda_iguais():
    antigo = {"mcp": {"command": "velho", "env": {"RETIRADO": "x", "EDITADO": "x"}, "args": ["old"]}}
    atual = {"mcp": {"command": "velho", "env": {"RETIRADO": "x", "EDITADO": "local", "LOCAL": "meu"},
                     "args": ["old"], "timeout": 10}}
    fonte = {"mcp": {"command": "novo", "env": {"NOVO": "y"}}}
    novo, manifesto, avisos = mesclar_config(atual, fonte, antigo)
    assert avisos == []
    assert novo == {"mcp": {"command": "novo", "env": {"EDITADO": "local", "LOCAL": "meu", "NOVO": "y"},
                            "timeout": 10}}
    assert manifesto == fonte


def test_config_remove_entrada_gerenciada_e_preserva_complementos_de_outra():
    anterior = {"retirar": {"command": "a"}, "complemento": {"command": "b", "env": {"OLD": "x"}}}
    atual = {**deepcopy(anterior), "pessoal": {"command": "c"}}
    atual["complemento"]["env"]["LOCAL"] = "meu"
    novo, manifesto, avisos = mesclar_config(atual, {}, anterior)
    assert novo == {"complemento": {"env": {"LOCAL": "meu"}}, "pessoal": {"command": "c"}}
    assert manifesto == {}
    assert len(avisos) == 1


@pytest.mark.parametrize("atual,fonte,anterior", [(None, {}, {}), ({}, [], {}), ({}, {}, None)])
def test_config_invalida_falha_sem_mutacao(atual, fonte, anterior):
    with pytest.raises(ValueError, match="precisam ser objetos"):
        mesclar_config(atual, fonte, anterior)


def test_config_suporta_entradas_escalares_e_valores_nulos():
    novo, manifesto, avisos = mesclar_config({"scalar": "old", "null": None}, {"scalar": "new"},
                                            {"scalar": "old", "null": None})
    assert novo == manifesto == {"scalar": "new"}
    assert avisos == []
