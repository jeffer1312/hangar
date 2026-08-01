import time
from pathlib import Path

from app.adapters.pi import sessions as pi_sessions


def test_cwd_slug_matches_pi_layout():
    # Medido na maquina alvo: /home/jefferson -> --home-jefferson--
    assert pi_sessions.cwd_slug("/home/jefferson") == "--home-jefferson--"
    assert (pi_sessions.cwd_slug("/home/jefferson/Projetos/claude-cockpit")
            == "--home-jefferson-Projetos-claude-cockpit--")


def test_cwd_slug_keeps_spaces_accents_and_underscores():
    # Diretorios REAIS em ~/.pi/agent/sessions (Pi 0.82.1): so o separador de caminho vira '-'.
    # Trocar todo nao-alfanumerico devolvia `--home-jefferson--rea-de-trabalho-...--`, que nao
    # existe — e ai transcript_path() nao achava nada e o fallback por CP_PI_SESSION ficava morto.
    assert (pi_sessions.cwd_slug("/home/jefferson/Área de trabalho/repos/servicos-api")
            == "--home-jefferson-Área de trabalho-repos-servicos-api--")
    assert (pi_sessions.cwd_slug("/tmp/claude-1000/-home-jefferson-Projetos/scratchpad/piprobe")
            == "--tmp-claude-1000--home-jefferson-Projetos-scratchpad-piprobe--")


def test_root_transcript_climbs_from_a_subagent_run(tmp_path):
    # A raiz esta no proprio caminho: o diretorio dos runs se chama igual ao .jsonl da conversa.
    raiz = tmp_path / "2026-07-30T20-29-24-651Z_18e48e08.jsonl"
    raiz.write_text("")
    run = tmp_path / "2026-07-30T20-29-24-651Z_18e48e08" / "44bad0fb" / "run-2"
    run.mkdir(parents=True)
    assert pi_sessions.root_transcript(str(run / "session.jsonl")) == str(raiz)
    # Sem irmao .jsonl nao ha o que provar — "" e nao um chute no arquivo mais proximo.
    orfao = tmp_path / "orfa" / "t" / "run-0"
    orfao.mkdir(parents=True)
    assert pi_sessions.root_transcript(str(orfao / "session.jsonl")) == ""


def test_transcript_path_globs_past_the_timestamp_prefix(tmp_path, monkeypatch):
    # O nome do arquivo do Pi tem timestamp na frente, entao o path NAO e derivavel so do
    # session-id: precisa casar pelo sufixo. Sem isto o tail nunca acha o transcript.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    d = tmp_path / pi_sessions.cwd_slug("/w")
    d.mkdir(parents=True)
    target = d / "2026-07-27T13-48-54-772Z_019fa3d5-f074-707b-92a8-1ca7f1d99ec9.jsonl"
    target.write_text("")
    (d / "2026-07-27T10-00-00-000Z_00000000-0000-0000-0000-000000000000.jsonl").write_text("")

    got = pi_sessions.transcript_path("/w", "019fa3d5-f074-707b-92a8-1ca7f1d99ec9")
    assert got == str(target)


def test_transcript_path_empty_when_session_not_created_yet(tmp_path, monkeypatch):
    # Sessao recem-spawnada: o arquivo so nasce no primeiro turno. "" (nao excecao) porque o
    # registry chama isto ANTES de a TUI escrever qualquer coisa.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    assert pi_sessions.transcript_path("/w", "nao-existe") == ""


def test_is_subagent_transcript_separates_the_task_runs_from_the_conversation():
    # Caminhos REAIS medidos numa sessao real (2026-07-30): o subagente grava dentro de um
    # diretorio com o stem da sessao, a conversa fica no irmao .jsonl.
    raiz = ("/home/jefferson/.pi/agent/sessions/--home-jefferson-Projetos-x--/"
            "2026-07-30T20-29-24-651Z_18e48e08-4ef3-4c39-bec3-3fcbb5999b46")
    assert pi_sessions.is_subagent_transcript(f"{raiz}/44bad0fb/run-2/session.jsonl")
    assert not pi_sessions.is_subagent_transcript(f"{raiz}.jsonl")
    # Cada sinal sozinho basta: nome fixo sem o run-<n>, e run-<n> com outro nome.
    assert pi_sessions.is_subagent_transcript(f"{raiz}/44bad0fb/session.jsonl")
    assert pi_sessions.is_subagent_transcript(f"{raiz}/44bad0fb/run-0/outro.jsonl")
    # "run-" sem numero e um cwd chamado run-2 nao contam — o arquivo e que decide.
    assert not pi_sessions.is_subagent_transcript(
        "/home/jefferson/.pi/agent/sessions/--home-run-2--/2026-07-30T20-29-24-651Z_abc.jsonl")


def test_transcript_path_picks_newest_on_duplicate_id(tmp_path, monkeypatch):
    # `pi --session-id X` reusa o id; se por qualquer motivo houver dois arquivos com o mesmo
    # sufixo, o mais NOVO e a sessao viva. Escolher o mais velho mostraria historico congelado.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    d = tmp_path / pi_sessions.cwd_slug("/w")
    d.mkdir(parents=True)
    old = d / "2026-07-01T00-00-00-000Z_abc.jsonl"
    new = d / "2026-07-27T00-00-00-000Z_abc.jsonl"
    old.write_text("")
    new.write_text("")
    import os
    os.utime(old, (1, 1))
    os.utime(new, (time.time(), time.time()))
    assert pi_sessions.transcript_path("/w", "abc") == str(new)
