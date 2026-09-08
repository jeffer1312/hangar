"""O importador nativo do Codex copia hook que é ARQUIVO e pula hook que é SYMLINK — mas reescreve
os dois comandos pra `<codex>/hooks/`. Medido em 07/09/2026 (codex-cli 0.153.4) numa HOME
descartável: `real.sh` foi copiado, `linkado.sh` não, e o hook quebrado bloqueava a ferramenta
("python3: can't open file ... No such file or directory") porque um PreToolUse que falha barra a
chamada.
"""
import json
import os

import pytest

from app import codex_hooks_arquivos as mod


def _doc(*comandos: str) -> dict:
    return {"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": c}] } for c in comandos]}}


def test_materializa_o_que_o_importador_pulou(tmp_path):
    home, codex = tmp_path / "home", tmp_path / "home" / ".codex"
    (home / ".claude" / "hooks").mkdir(parents=True)
    (codex / "hooks").mkdir(parents=True)
    alvo = tmp_path / "repo" / "hooks" / "linkado.py"
    alvo.parent.mkdir(parents=True)
    alvo.write_text("print('alvo')\n")
    (home / ".claude" / "hooks" / "linkado.py").symlink_to(alvo)
    (codex / "hooks" / "real.sh").write_text("echo real\n")     # o importador copiou este
    doc = _doc(f"sh '{codex}/hooks/real.sh'", f"python3 '{codex}/hooks/linkado.py'")
    (codex / "hooks.json").write_text(json.dumps(doc))

    criados, orfaos = mod.materializar(codex, home)

    assert criados == ["linkado.py"] and orfaos == []
    destino = codex / "hooks" / "linkado.py"
    assert destino.exists() and destino.read_text() == "print('alvo')\n"
    # Symlink pro arquivo REAL, não pro link do meio: uma fonte só, e editar o script continua
    # valendo pros dois agentes sem cópia pra envelhecer.
    if os.name != "nt":
        assert destino.is_symlink() and destino.resolve() == alvo.resolve()
    # O que já existia não é tocado.
    assert (codex / "hooks" / "real.sh").read_text() == "echo real\n"


def test_sem_equivalente_no_claude_vira_aviso_e_nao_arquivo_vazio(tmp_path):
    home, codex = tmp_path / "home", tmp_path / "home" / ".codex"
    (home / ".claude" / "hooks").mkdir(parents=True)
    (codex / "hooks").mkdir(parents=True)
    (codex / "hooks.json").write_text(json.dumps(_doc(f"python3 '{codex}/hooks/sumiu.py'")))

    criados, orfaos = mod.materializar(codex, home)

    assert criados == [] and orfaos == ["sumiu.py"]
    assert not (codex / "hooks" / "sumiu.py").exists()   # nunca um arquivo vazio no lugar


def test_ignora_caminho_fora_da_pasta_de_hooks_do_codex(tmp_path):
    """Comando que aponta pro repo (o caso dos hooks do próprio Hangar) não é assunto desta etapa —
    ela só preenche o que o importador prometeu em `<codex>/hooks/`."""
    home, codex = tmp_path / "home", tmp_path / "home" / ".codex"
    (home / ".claude" / "hooks").mkdir(parents=True)
    codex.mkdir(parents=True)
    doc = _doc(f"python3 '{tmp_path}/repo/backend/hooks/state.py'")
    assert mod.faltantes(doc, codex) == []
    assert mod.materializar(codex, home, doc) == ([], [])


def test_citacao_com_dotdot_nao_escapa_da_pasta_de_hooks(tmp_path):
    """`<codex>/hooks/../../fora.py` não é um hook do Codex — e o caminho que entra na lista é o
    RESOLVIDO, não o cru, pra a escrita não depender de o SO normalizar o `..` de novo."""
    home, codex = tmp_path / "home", tmp_path / "home" / ".codex"
    (home / ".claude" / "hooks").mkdir(parents=True)
    (codex / "hooks").mkdir(parents=True)
    doc = _doc(f"python3 '{codex}/hooks/../../fora.py'")
    assert mod.faltantes(doc, codex) == []
    dentro = _doc(f"python3 '{codex}/hooks/./x.py'")
    assert mod.faltantes(dentro, codex) == [(codex / "hooks" / "x.py").resolve()]


def test_windows_refaz_a_copia_que_ficou_velha(tmp_path, monkeypatch):
    """No Windows o hook é CÓPIA, e cópia envelhece: sem isto, editar o hook em ~/.claude/hooks
    nunca chegava ao Codex — calado. Compara bytes, não mtime (cópia e original nascem com datas
    diferentes)."""
    # `mod._WINDOWS`, não `os.name`: trocar `os.name` num teste leva o `pathlib` junto e o caso
    # estoura no andaime, não no código (está registrado no CLAUDE.md).
    monkeypatch.setattr(mod, "_WINDOWS", True)
    home, codex = tmp_path / "home", tmp_path / "home" / ".codex"
    (home / ".claude" / "hooks").mkdir(parents=True)
    (codex / "hooks").mkdir(parents=True)
    (home / ".claude" / "hooks" / "x.py").write_text("novo\n")
    (codex / "hooks" / "x.py").write_text("velho\n")
    doc = _doc(f"python3 '{codex}/hooks/x.py'")

    criados, orfaos = mod.materializar(codex, home, doc)

    assert criados == ["x.py"] and orfaos == []
    assert (codex / "hooks" / "x.py").read_text() == "novo\n"
    # Conteúdo igual não reescreve (senão toda reconciliação mexeria no arquivo à toa).
    assert mod.materializar(codex, home, doc) == ([], [])


@pytest.mark.skipif(os.name == "nt", reason="symlink pendurado exige privilégio no Windows")
def test_link_pendurado_e_refeito(tmp_path):
    home, codex = tmp_path / "home", tmp_path / "home" / ".codex"
    (home / ".claude" / "hooks").mkdir(parents=True)
    (codex / "hooks").mkdir(parents=True)
    alvo = tmp_path / "repo" / "x.py"
    alvo.parent.mkdir(parents=True)
    alvo.write_text("ok\n")
    (home / ".claude" / "hooks" / "x.py").symlink_to(alvo)
    (codex / "hooks" / "x.py").symlink_to(tmp_path / "nao-existe")   # link morto de uma rodada velha

    criados, orfaos = mod.materializar(codex, home, _doc(f"python3 '{codex}/hooks/x.py'"))

    assert criados == ["x.py"] and orfaos == []
    assert (codex / "hooks" / "x.py").read_text() == "ok\n"
