import json
import os

import pytest

from app import pqueue
from app.adapters.codex import rollout


def line(text):
    return json.dumps({
        "type": "response_item", "timestamp": "2026-01-01T00:00:00Z",
        "payload": {"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": text}]},
    }) + "\n"


def read(path):
    return pqueue.committed_user_lines(str(path), "codex")


def test_append_repetido_e_linha_parcial(tmp_path, monkeypatch):
    path = tmp_path / "rollout.jsonl"
    path.write_text(line("oi"))
    parsed = []
    original = rollout.parse_rollout_obj

    def parse(obj):
        parsed.append(obj)
        return original(obj)

    monkeypatch.setattr(rollout, "parse_rollout_obj", parse)
    assert read(path) == {"oi"}
    assert read(path) == {"oi"}
    assert len(parsed) == 1
    next_line = line("pode seguir")
    with path.open("a") as fh:
        fh.write(next_line[:-1])
    assert read(path) == {"oi"}
    assert len(parsed) == 1
    with path.open("a") as fh:
        fh.write("\n" + line("oi"))
    assert read(path) == {"oi", "pode seguir"}
    assert len(parsed) == 3
    result = read(path)
    result.clear()
    assert read(path) == {"oi", "pode seguir"}


@pytest.mark.parametrize("change", ["truncate", "replace", "same_size", "rewrite_larger"])
def test_mudanca_do_arquivo_descarta_confirmacoes_antigas(tmp_path, change):
    path = tmp_path / "rollout.jsonl"
    path.write_text(line("antigo"))
    assert read(path) == {"antigo"}
    old_mtime = path.stat().st_mtime_ns
    if change == "truncate":
        path.write_text("")
        assert read(path) == set()
        path.write_text(line("novo"))
        expected = {"novo"}
    elif change == "replace":
        replacement = tmp_path / "replacement"
        replacement.write_text(line("substituto maior"))
        replacement.replace(path)
        expected = {"substituto maior"}
    elif change == "same_size":
        path.write_text(line("fresco"))
        os.utime(path, ns=(old_mtime + 1_000_000, old_mtime + 1_000_000))
        expected = {"fresco"}
    else:
        path.write_text(line("novo maior") + line("outra mensagem"))
        expected = {"novo maior", "outra mensagem"}
    assert read(path) == expected


def test_erro_na_fonte_nao_devolve_cache(tmp_path):
    path = tmp_path / "rollout.jsonl"
    path.write_text(line("oi"))
    assert read(path) == {"oi"}
    path.unlink()
    assert read(path) is None
    path.mkdir()
    assert read(path) is None


def test_cache_limitado_e_separado_por_caminho(tmp_path, monkeypatch):
    monkeypatch.setattr(pqueue, "_indices", {})
    monkeypatch.setattr(pqueue, "_INDICES_MAX", 2)
    paths = [tmp_path / str(i) for i in range(3)]
    for i, path in enumerate(paths):
        path.write_text(line(str(i)))
        assert read(path) == {str(i)}
    assert set(pqueue._indices) == {(str(p), "codex") for p in paths[1:]}
    monkeypatch.setattr(pqueue, "_INDICE_CHARS", 1)
    paths[2].write_text(line("texto grande"))
    assert read(paths[2]) == {"texto grande"}
    assert (str(paths[2]), "codex") not in pqueue._indices


# Claude: a confirmação roda a cada envio e durante turno longo. Reler o transcript inteiro (MBs)
# a cada vez prendia um núcleo; o índice só lê o que foi acrescentado.
def claude_user(text):
    return json.dumps({"type": "user", "message": {"role": "user", "content": text}}) + "\n"


def claude_queue(op, text=None):
    obj = {"type": "queue-operation", "operation": op}
    if text is not None:
        obj["content"] = text
    return json.dumps(obj) + "\n"


def test_claude_le_so_o_acrescentado(tmp_path, monkeypatch):
    path = tmp_path / "t.jsonl"
    path.write_text(claude_user("oi") + claude_user("tudo certo"), encoding="utf-8")
    vistos = []
    original = pqueue._CommittedIndex._alimenta_claude
    monkeypatch.setattr(pqueue._CommittedIndex, "_alimenta_claude",
                        lambda self, obj: (vistos.append(obj), original(self, obj)))
    assert {"oi", "tudo certo"} <= pqueue.committed_user_lines(str(path))
    assert len(vistos) == 2
    assert {"oi", "tudo certo"} <= pqueue.committed_user_lines(str(path))
    assert len(vistos) == 2                      # nada novo: nenhuma linha relida
    with path.open("a", encoding="utf-8") as fh:
        fh.write(claude_user("pode seguir"))
    assert "pode seguir" in pqueue.committed_user_lines(str(path))
    assert len(vistos) == 3                      # só a linha acrescentada


def test_claude_fila_interna_acompanha_o_acrescentado(tmp_path):
    path = tmp_path / "t.jsonl"
    path.write_text(claude_queue("enqueue", "primeira") + claude_queue("enqueue", "segunda"),
                    encoding="utf-8")
    assert pqueue.fila_interna_pendente(str(path)) == {"primeira", "segunda"}
    with path.open("a", encoding="utf-8") as fh:
        fh.write(claude_queue("dequeue"))
    assert pqueue.fila_interna_pendente(str(path)) == {"segunda"}
    assert "primeira" in pqueue.committed_user_lines(str(path))


def test_claude_truncado_relê_do_inicio(tmp_path):
    path = tmp_path / "t.jsonl"
    path.write_text(claude_user("antigo") + claude_queue("enqueue", "presa"), encoding="utf-8")
    assert "antigo" in pqueue.committed_user_lines(str(path))
    assert pqueue.fila_interna_pendente(str(path)) == {"presa"}
    path.write_text(claude_user("novo"), encoding="utf-8")
    lines = pqueue.committed_user_lines(str(path))
    assert "novo" in lines and "antigo" not in lines
    assert pqueue.fila_interna_pendente(str(path)) == set()
