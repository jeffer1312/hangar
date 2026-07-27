import time
from pathlib import Path

from app.adapters.pi import sessions as pi_sessions


def test_cwd_slug_matches_pi_layout():
    # Medido na maquina alvo: /home/jefferson -> --home-jefferson--
    assert pi_sessions.cwd_slug("/home/jefferson") == "--home-jefferson--"
    assert (pi_sessions.cwd_slug("/home/jefferson/Projetos/claude-cockpit")
            == "--home-jefferson-Projetos-claude-cockpit--")


def test_transcript_path_globs_past_the_timestamp_prefix(tmp_path, monkeypatch):
    # O nome do arquivo do Pi tem timestamp na frente, entao o path NAO e derivavel so do
    # session-id: precisa casar pelo sufixo. Sem isto o tail nunca acha o transcript.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    d = tmp_path / pi_sessions.cwd_slug("/w")
    d.mkdir(parents=True)
    alvo = d / "2026-07-27T13-48-54-772Z_019fa3d5-f074-707b-92a8-1ca7f1d99ec9.jsonl"
    alvo.write_text("")
    (d / "2026-07-27T10-00-00-000Z_00000000-0000-0000-0000-000000000000.jsonl").write_text("")

    got = pi_sessions.transcript_path("/w", "019fa3d5-f074-707b-92a8-1ca7f1d99ec9")
    assert got == str(alvo)


def test_transcript_path_empty_when_session_not_created_yet(tmp_path, monkeypatch):
    # Sessao recem-spawnada: o arquivo so nasce no primeiro turno. "" (nao excecao) porque o
    # registry chama isto ANTES de a TUI escrever qualquer coisa.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    assert pi_sessions.transcript_path("/w", "nao-existe") == ""


def test_transcript_path_picks_newest_on_duplicate_id(tmp_path, monkeypatch):
    # `pi --session-id X` reusa o id; se por qualquer motivo houver dois arquivos com o mesmo
    # sufixo, o mais NOVO e a sessao viva. Escolher o mais velho mostraria historico congelado.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    d = tmp_path / pi_sessions.cwd_slug("/w")
    d.mkdir(parents=True)
    velho = d / "2026-07-01T00-00-00-000Z_abc.jsonl"
    novo = d / "2026-07-27T00-00-00-000Z_abc.jsonl"
    velho.write_text("")
    novo.write_text("")
    import os
    os.utime(velho, (1, 1))
    os.utime(novo, (time.time(), time.time()))
    assert pi_sessions.transcript_path("/w", "abc") == str(novo)
