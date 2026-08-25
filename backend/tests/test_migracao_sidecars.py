"""Migração dos sidecars .claude-pocket-* -> .hangar-* (app/migracao_sidecars.py)."""
import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app import migracao_sidecars as m  # noqa: E402


def test_renomeia_pasta_e_deixa_link_no_caminho_antigo(tmp_path):
    antigo = tmp_path / ".claude-pocket-state"
    antigo.mkdir()
    (antigo / "s1.json").write_text('{"state": "idle"}', encoding="utf-8")

    assert m.migrar_base(tmp_path) == 1

    novo = tmp_path / ".hangar-state"
    assert (novo / "s1.json").read_text(encoding="utf-8") == '{"state": "idle"}'
    # O que importa não é o link existir, e sim o processo VELHO (hook, extensão, statusline de
    # fora do repo) continuar acertando o alvo escrevendo no nome antigo.
    if antigo.is_symlink():
        (antigo / "s2.json").write_text("x", encoding="utf-8")
        assert (novo / "s2.json").exists()


def test_renomeia_json_solto(tmp_path):
    (tmp_path / ".claude-pocket-apelidos.json").write_text('{"a": "b"}', encoding="utf-8")
    assert m.migrar_base(tmp_path) == 1
    assert (tmp_path / ".hangar-apelidos.json").read_text(encoding="utf-8") == '{"a": "b"}'


def test_e_idempotente(tmp_path):
    (tmp_path / ".claude-pocket-pair").mkdir()
    assert m.migrar_base(tmp_path) == 1
    assert m.migrar_base(tmp_path) == 0      # 2a passada não mexe no link que ela mesma deixou


def test_nao_funde_quando_os_dois_existem(tmp_path):
    """Destino já existente PARA aquele item — mesclar escolheria um vencedor por arquivo e
    perderia estado calado. Fica como está, com aviso."""
    antigo = tmp_path / ".claude-pocket-queue"
    antigo.mkdir()
    (antigo / "velho.json").write_text("velho", encoding="utf-8")
    novo = tmp_path / ".hangar-queue"
    novo.mkdir()
    (novo / "novo.json").write_text("novo", encoding="utf-8")

    assert m.migrar_base(tmp_path) == 0
    assert (antigo / "velho.json").exists()
    assert (novo / "novo.json").exists()


def test_leitura_prefere_o_novo_mas_aceita_o_antigo(tmp_path):
    """O caminho dos `.json` soltos no Windows, onde não dá pra deixar link sem privilégio."""
    novo = tmp_path / ".hangar-runner.json"
    antigo = tmp_path / ".claude-pocket-runner.json"

    assert m.caminho_de_leitura(novo) == novo           # nenhum dos dois existe: o novo
    antigo.write_text("{}", encoding="utf-8")
    assert m.caminho_de_leitura(novo) == antigo         # só o antigo: cai nele
    novo.write_text("{}", encoding="utf-8")
    assert m.caminho_de_leitura(novo) == novo           # os dois: o novo vence


@pytest.mark.skipif(os.name == "nt", reason="link de arquivo no Windows exige privilégio")
def test_link_de_arquivo_no_posix_aponta_pro_novo(tmp_path):
    antigo = tmp_path / ".claude-pocket-conn.json"
    antigo.write_text('{"url": "ws://x"}', encoding="utf-8")
    m.migrar_base(tmp_path)
    assert antigo.is_symlink()
    assert antigo.read_text(encoding="utf-8") == '{"url": "ws://x"}'


def test_migrar_leva_a_pasta_de_dados_do_servidor(tmp_path, monkeypatch):
    """`~/.claude-pocket/` não está dentro de config dir nenhum — e é onde mora o cofre do sync."""
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    cofre = tmp_path / ".claude-pocket"
    cofre.mkdir()
    (cofre / "sync-vault.json").write_text('{"v": 1}', encoding="utf-8")

    m.migrar([tmp_path / ".claude"])

    assert (tmp_path / ".hangar" / "sync-vault.json").read_text(encoding="utf-8") == '{"v": 1}'


def test_upload_migra_pasta_do_projeto_na_primeira_leitura(tmp_path):
    """A pasta de anexos mora no cwd do PROJETO, fora do alcance da migração da subida."""
    from app.uploads import resolve_upload

    antigo = tmp_path / ".claude-pocket-uploads"
    antigo.mkdir()
    (antigo / "1234-abcdef.png").write_bytes(b"png")

    # Uma mensagem antiga cita o caminho velho; o anexo tem que continuar servível.
    assert resolve_upload(str(tmp_path), "1234-abcdef.png").endswith("1234-abcdef.png")
    assert (tmp_path / ".hangar-uploads" / "1234-abcdef.png").exists()
