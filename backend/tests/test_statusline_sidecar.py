import json
import time

from app import statusline


def _publica(tmp_path, stem, line, ts=None):
    d = tmp_path / ".claude-pocket-status"
    d.mkdir(exist_ok=True)
    (d / f"{stem}.json").write_text(json.dumps(
        {"line": line, "ts": time.time() if ts is None else ts}))


def _dirs(monkeypatch, tmp_path):
    monkeypatch.setattr(statusline, "_dirs", lambda: [tmp_path])


def test_reads_the_full_line_published_by_the_session(monkeypatch, tmp_path):
    # E o ponto da feature: a linha do sidecar tem o que o pane de 99 colunas cortou fora.
    _dirs(monkeypatch, tmp_path)
    inteira = "🤖 k3 (max) │ 💬 ctx 151k/262k │ ⚡5h:51% ↻54m │ 💵 $0.00 │ 🕐 19:04"
    _publica(tmp_path, "2026_abc", inteira)
    assert statusline.read("2026_abc") == inteira


def test_missing_or_empty_sidecar_falls_back_to_the_pane(monkeypatch, tmp_path):
    # Sessao sem a extensao instrumentada nao pode ficar SEM statusline — None e o sinal de
    # "usa o pane", que e o comportamento de sempre.
    _dirs(monkeypatch, tmp_path)
    assert statusline.read("nao-existe") is None
    assert statusline.read(None) is None
    _publica(tmp_path, "vazio", "   ")
    assert statusline.read("vazio") is None


def test_ignores_a_sidecar_older_than_a_day(monkeypatch, tmp_path):
    # Marcador esquecido de sessao antiga cujo stem voltou a existir: melhor o pane que uma linha
    # de ontem apresentada como atual.
    _dirs(monkeypatch, tmp_path)
    _publica(tmp_path, "velho", "linha de ontem", ts=time.time() - 90000)
    assert statusline.read("velho") is None


def test_broken_json_does_not_raise(monkeypatch, tmp_path):
    # Escrita parcial: o publisher usa tmp+rename, mas o contrato aqui e nao explodir de qualquer
    # jeito — sidecar e conveniencia, o pane e a rede.
    _dirs(monkeypatch, tmp_path)
    d = tmp_path / ".claude-pocket-status"
    d.mkdir(exist_ok=True)
    (d / "meio.json").write_text('{"line": "cor')
    assert statusline.read("meio") is None
