"""Arquivo de conversas mortas: listagem (preview/cwd/live) e validacao anti-traversal do path."""
import json
import os

import pytest

from app import archive
from app import worktrees


SID = "11111111-1111-1111-1111-111111111111"


@pytest.fixture(autouse=True)
def _tmp_projects(tmp_path, monkeypatch):
    # Sem zerar list_config_dirs o teste varreria as contas REAIS da maquina de quem roda (o
    # archive agora enxerga todas), e o resultado dependeria de quantos ~/.claude* existem la.
    monkeypatch.setattr(archive, "list_config_dirs", lambda: [])
    # Mesma razao: sem zerar, o teste enxergaria as conversas REAIS de Pi/Kimi/Codex da maquina.
    monkeypatch.setattr(archive, "_conversas_de_outros_providers", lambda: [])
    monkeypatch.setattr(archive.settings, "projects_dir", tmp_path)
    return tmp_path


@pytest.fixture(autouse=True)
def _isolated_removed(tmp_path, monkeypatch):
    # `archive_cwd` consulta o mapa de worktrees removidas; nunca o ~/.hangar real.
    monkeypatch.setattr(worktrees, "REMOVED_FILE", tmp_path / "removidas.json")


def _write_transcript(projdir, sid=SID, text="oi arquivo", cwd="/home/u/proj"):
    projdir.mkdir(parents=True, exist_ok=True)
    j = projdir / f"{sid}.jsonl"
    j.write_text(json.dumps({
        "type": "user", "uuid": "u1", "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z",
        "message": {"role": "user", "content": text},
    }) + "\n", encoding="utf-8")
    return j


def test_list_folders_aggregates(tmp_path):
    _write_transcript(tmp_path / "-home-u-proj")
    _write_transcript(tmp_path / "-home-u-proj", sid="33333333-3333-3333-3333-333333333333",
                      text="segunda conversa")
    # Cópia renomeada ao apagar uma conta não é conversa retomável: não entra na contagem.
    _write_transcript(tmp_path / "-home-u-proj", sid=f"{SID}.from-work", text="copia")
    folders = archive.list_folders()
    assert [(f.project, f.cwd, f.count) for f in folders] == [
        ("-home-u-proj", "/home/u/proj", 2),
    ]


def test_previa_sai_sem_marcacao_de_markdown():
    # A previa e UMA linha de lista: renderizar nao e opcao, e `**assim**` na tela e ruido.
    assert archive._texto_simples("**O backend responde certo** — veja `app.py`") == \
        "O backend responde certo — veja app.py"
    assert archive._texto_simples("## Titulo\n- item um\n- item dois") == "Titulo item um item dois"
    assert archive._texto_simples("veja [o guia](http://x/y) agora") == "veja o guia agora"


def _conta(tmp_path, monkeypatch, nome, rotulo):
    """Uma 2a conta (config dir proprio) visivel pro archive, alem do projects_dir do processo."""
    cdir = tmp_path / nome
    (cdir / "projects").mkdir(parents=True)
    from app.config import ConfigDirInfo
    monkeypatch.setattr(archive, "list_config_dirs",
                        lambda: [ConfigDirInfo(path=str(cdir), label=rotulo, active=False)])
    return cdir


OUTRO_SID = "44444444-4444-4444-4444-444444444444"


def test_varre_todas_as_contas(tmp_path, monkeypatch):
    # A conversa que vive em OUTRA conta era invisivel no Arquivo, e retomar a partir dela morria
    # com "No conversation found with session ID" -- o resume nascia na conta do backend.
    cdir = _conta(tmp_path, monkeypatch, "conta-b", "Trabalho")
    _write_transcript(tmp_path / "-home-u-proj", text="da conta do backend")
    _write_transcript(cdir / "projects" / "-home-u-proj", sid=OUTRO_SID, text="da outra conta")

    assert [(f.project, f.count) for f in archive.list_folders()] == [("-home-u-proj", 2)]

    # Link direto (so projeto + uuid) continua abrindo conversa de outra conta.
    assert archive.archive_jsonl("-home-u-proj", OUTRO_SID).parent.parent.parent == cdir


def test_archive_jsonl_valid_path(tmp_path):
    j = _write_transcript(tmp_path / "-home-u-proj")
    assert archive.archive_jsonl("-home-u-proj", SID) == j


def test_archive_jsonl_rejects_traversal_and_missing(tmp_path):
    _write_transcript(tmp_path / "-home-u-proj")
    with pytest.raises(ValueError):
        archive.archive_jsonl("../fora", SID)          # projeto fora do alfabeto
    with pytest.raises(ValueError):
        archive.archive_jsonl("-home-u-proj", "../../x")  # sid nao-uuid
    with pytest.raises(FileNotFoundError):
        archive.archive_jsonl("-home-u-proj", "22222222-2222-2222-2222-222222222222")


def test_archive_cwd_reads_from_header(tmp_path):
    # usado por "Retomar conversa": precisa do cwd real pra subir a sessao nova no lugar certo.
    _write_transcript(tmp_path / "-home-u-proj", cwd="/home/u/proj")
    assert archive.archive_cwd("-home-u-proj", SID) == "/home/u/proj"
    with pytest.raises(FileNotFoundError):
        archive.archive_cwd("-home-u-proj", "22222222-2222-2222-2222-222222222222")



# ── Transcript sem mensagem do usuário (reserva pré-aquecida) ─────────────────

RESERVA = "55555555-5555-5555-5555-555555555555"


def _write_so_attachments(projdir, sid=RESERVA, cwd="/home/u/proj"):
    """O que o Claude Code grava antes da 1ª mensagem: só os `attachment` dos hooks de SessionStart.
    O texto do hook cita "user" entre aspas de propósito: não pode passar por linha de usuário."""
    projdir.mkdir(parents=True, exist_ok=True)
    j = projdir / f"{sid}.jsonl"
    linhas = [json.dumps({
        "parentUuid": None, "isSidechain": False,
        "attachment": {"type": "hook_success", "hookName": "SessionStart:startup",
                       "content": 'contexto do hook: "user" e "type"'},
        "type": "attachment", "uuid": f"a{i}", "cwd": cwd, "timestamp": "2026-01-01T00:00:00Z",
    }) for i in range(3)]
    j.write_text("\n".join(linhas) + "\n", encoding="utf-8")
    return j


def test_jsonl_sem_linha_de_usuario_fica_fora_do_arquivo(tmp_path):
    projdir = tmp_path / "-home-u-proj"
    normal = _write_transcript(projdir)
    reserva = _write_so_attachments(projdir)
    # A reserva é a mais nova: com cap=1 ela não pode roubar a vaga da conversa de verdade.
    os.utime(normal, (1000, 1000))
    assert reserva.stat().st_mtime > 1000

    assert [(f.project, f.count) for f in archive.list_folders()] == [("-home-u-proj", 1)]


def test_pasta_so_com_reserva_nao_aparece(tmp_path):
    _write_so_attachments(tmp_path / "-home-u-proj")
    assert archive.list_folders() == []


def test_linha_de_usuario_sem_previa_continua_listada(tmp_path):
    # Linha `type: "user"` que o chat não mostra (isMeta) não dá prévia, mas é conversa: fica.
    projdir = tmp_path / "-home-u-proj"
    _write_so_attachments(projdir, sid=SID)
    with open(projdir / f"{SID}.jsonl", "a", encoding="utf-8") as fh:
        fh.write(json.dumps({"type": "user", "uuid": "m1", "isMeta": True, "cwd": "/home/u/proj",
                             "message": {"role": "user", "content": "Caveat: meta"}}) + "\n")
    assert [f.count for f in archive.list_folders()] == [1]


def test_previa_do_indice_dispensa_a_leitura(tmp_path, monkeypatch):
    # Com a 1ª msg já no cabeçalho do índice, saber que há usuário não abre o arquivo de novo.
    j = _write_transcript(tmp_path / "-home-u-proj")
    monkeypatch.setattr(archive, "_heads", lambda: {str(j): ("oi arquivo", "/home/u/proj")})
    abertos = []
    original = open

    def espia(path, *a, **kw):
        abertos.append(str(path))
        return original(path, *a, **kw)

    monkeypatch.setattr("builtins.open", espia)
    assert [f.count for f in archive.list_folders()] == [1]
    assert str(j) not in abertos
