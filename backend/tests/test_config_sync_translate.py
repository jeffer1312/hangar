import json

from app import config_sync_translate as tr
from app.narrar import NarrarError


def _fake(monkeypatch, tmp_path, answers):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(tr, "_ESPERA_429", (0, 0))
    calls = []

    def chat(system, prompt, **kw):
        calls.append(json.loads(prompt))
        answer = answers.pop(0)
        if isinstance(answer, Exception):
            raise answer
        return answer(calls[-1])
    monkeypatch.setattr(tr, "chamar_chat", chat)
    return calls


def test_translates_once_and_skips_text_already_in_the_language(monkeypatch, tmp_path):
    calls = _fake(monkeypatch, tmp_path, [lambda batch: json.dumps([f"pt:{t}" for t in batch])])
    texts = ["Reads the file when the hook runs.", "Lê o arquivo quando o hook roda.", ""]
    out, error = tr.translate(texts, "pt")
    assert out == ["pt:Reads the file when the hook runs.", "Lê o arquivo quando o hook roda.", ""]
    assert error == "" and calls == [["Reads the file when the hook runs."]]
    assert tr.translate(texts, "pt") == (out, "") and len(calls) == 1   # veio do cache em disco


def test_rate_limit_waits_and_then_reports_how_far_it_got(monkeypatch, tmp_path):
    limit = NarrarError(502, "provedor 429: rate limit")
    calls = _fake(monkeypatch, tmp_path, [limit, lambda b: json.dumps(b), limit, limit, limit])
    monkeypatch.setattr(tr, "_LOTE", 1)
    out, error = tr.translate(["Runs the tests for the file.", "Opens the browser for the user."], "pt")
    assert out[0] == "Runs the tests for the file." and "traduzi 1 de 2" in error
    assert len(calls) == 5


def test_wrong_sized_answer_keeps_the_originals(monkeypatch, tmp_path):
    _fake(monkeypatch, tmp_path, [lambda batch: "[]"])
    out, error = tr.translate(["Runs the tests for the file."], "pt")
    assert out == ["Runs the tests for the file."] and error
