"""Arquivo de conversas mortas: listagem (preview/cwd/live) e validacao anti-traversal do path."""
import json
import os

import pytest

from app import archive


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
    folders = archive.list_folders()
    assert [(f.project, f.cwd, f.count) for f in folders] == [
        ("-home-u-proj", "/home/u/proj", 2),
    ]


def test_list_conversations_preview_cwd_live(tmp_path):
    j = _write_transcript(tmp_path / "-home-u-proj")
    entries = archive.list_conversations("-home-u-proj", set())
    assert [(e.project, e.session_id, e.cwd, e.preview, e.live) for e in entries] == [
        ("-home-u-proj", SID, "/home/u/proj", "oi arquivo", False),
    ]
    # live: o realpath do jsonl em uso marca a entrada
    entries = archive.list_conversations("-home-u-proj", {os.path.realpath(str(j))})
    assert entries[0].live is True
    # validacao: projeto fora do alfabeto / inexistente
    import pytest as _pytest
    with _pytest.raises(ValueError):
        archive.list_conversations("../fora", set())
    with _pytest.raises(FileNotFoundError):
        archive.list_conversations("nao-existe", set())


def test_ultima_msg_vem_do_fim_do_arquivo(tmp_path):
    # A ultima msg e o que identifica a conversa meses depois -- e ela e lida por seek do FIM,
    # nunca varrendo o arquivo: o recheio abaixo passa do span da 1a passada de proposito.
    projdir = tmp_path / "-home-u-proj"
    j = _write_transcript(projdir)
    linhas = [json.dumps({"type": "assistant", "uuid": f"a{i}", "timestamp": "2026-01-01T00:00:01Z",
                          "message": {"role": "assistant", "content": [
                              {"type": "text", "text": "x" * 2000}]}})
              for i in range(60)]
    linhas.append(json.dumps({"type": "assistant", "uuid": "fim",
                              "timestamp": "2026-01-01T00:00:02Z",
                              "message": {"role": "assistant", "content": [
                                  {"type": "text", "text": "ultima resposta"}]}}))
    with open(j, "a", encoding="utf-8") as fh:
        fh.write("\n".join(linhas) + "\n")
    assert j.stat().st_size > archive._TAIL_BYTES
    e = archive.list_conversations("-home-u-proj", set())[0]
    assert e.ultima == "ultima resposta"
    assert e.preview == "oi arquivo"      # a 1a msg continua servindo pro titulo


def test_tail_events_cresce_a_janela_ate_juntar_as_msgs(tmp_path):
    # As ultimas msgs de texto podem estar MUITO atras do fim: transcript real enche o final de
    # `attachment` e saida de ferramenta (medido: 300 linhas nas ultimas 2MB, com 6 msgs de texto).
    # Uma janela fixa devolveria a previa quase vazia justo na conversa longa.
    projdir = tmp_path / "-home-u-proj"
    j = _write_transcript(projdir, text="primeira")
    linhas = [json.dumps({"type": "user", "uuid": f"u{i}", "timestamp": "2026-01-01T00:00:01Z",
                          "message": {"role": "user", "content": f"msg {i}"}})
              for i in range(4)]
    # Recheio SEM msg de texto, grande o bastante pra empurrar as msgs pra fora da 1a janela.
    entulho = [json.dumps({"type": "attachment", "uuid": f"a{i}", "content": "x" * 4000})
               for i in range(400)]
    with open(j, "a", encoding="utf-8") as fh:
        fh.write("\n".join(linhas + entulho) + "\n")
    assert j.stat().st_size > archive._TAIL_BYTES * 4

    evs = archive.tail_events("-home-u-proj", SID, 5)
    assert [e.text for e in evs] == ["primeira", "msg 0", "msg 1", "msg 2", "msg 3"]
    assert [e.text for e in archive.tail_events("-home-u-proj", SID, 2)] == ["msg 2", "msg 3"]


def test_previa_sai_sem_marcacao_de_markdown():
    # A previa e UMA linha de lista: renderizar nao e opcao, e `**assim**` na tela e ruido.
    assert archive._texto_simples("**O backend responde certo** — veja `app.py`") == \
        "O backend responde certo — veja app.py"
    assert archive._texto_simples("## Titulo\n- item um\n- item dois") == "Titulo item um item dois"
    assert archive._texto_simples("veja [o guia](http://x/y) agora") == "veja o guia agora"


def test_ultima_msg_com_arquivo_menor_que_o_span(tmp_path):
    # Arquivo curto: o seek pega o inicio junto e a 1a linha NAO pode ser descartada como parcial.
    _write_transcript(tmp_path / "-home-u-proj", text="unica msg")
    assert archive.list_conversations("-home-u-proj", set())[0].ultima == "unica msg"


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

    entries = archive.list_conversations("-home-u-proj", set())
    assert {(e.session_id, e.config_dir, e.conta) for e in entries} == {
        (SID, None, ""),
        (OUTRO_SID, str(cdir), "Trabalho"),
    }
    # Retomar precisa da conta certa, e ela e descoberta no disco quando ninguem a informa.
    assert archive.conta_de("-home-u-proj", OUTRO_SID) == str(cdir)
    assert archive.conta_de("-home-u-proj", SID) is None
    # Link direto (so projeto + uuid) continua abrindo conversa de outra conta.
    assert archive.archive_jsonl("-home-u-proj", OUTRO_SID).parent.parent.parent == cdir


def test_teto_corta_as_mais_VELHAS_dos_outros_providers(tmp_path, monkeypatch):
    # A ordem que vem do disco e a do glob/indice, nao a de recencia. Cortar antes de ordenar
    # descartava conversa NOVA e mantinha velha, calado, na lista que promete "mais recentes
    # primeiro".
    # Sem transcript do Claude na pasta de proposito: o real teria mtime de AGORA e mascararia a
    # ordem dos falsos. Pasta que so existe pra outro agente tambem nao pode virar 404.
    from app.archive_providers import Conversa
    fora_de_ordem = [2.0, 5.0, 1.0, 4.0, 3.0]
    monkeypatch.setattr(archive, "_conversas_de_outros_providers", lambda: [
        Conversa("pi", "/home/u/proj", f"{i}", tmp_path / f"p{i}.jsonl", mt)
        for i, mt in enumerate(fora_de_ordem)
    ])
    entries = archive.list_conversations("-home-u-proj", set(), cap=2)
    assert [e.mtime for e in entries] == [5.0, 4.0]


def test_filtra_por_conta_quando_pedido(tmp_path, monkeypatch):
    # E o que o modal de sessao nova usa: so a conta escolhida no seletor.
    cdir = _conta(tmp_path, monkeypatch, "conta-b", "Trabalho")
    _write_transcript(tmp_path / "-home-u-proj", text="da conta do backend")
    _write_transcript(cdir / "projects" / "-home-u-proj", sid=OUTRO_SID, text="da outra conta")
    entries = archive.list_conversations("-home-u-proj", set(), config_dir=str(cdir))
    assert [e.session_id for e in entries] == [OUTRO_SID]


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
