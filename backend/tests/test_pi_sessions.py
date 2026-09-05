import os
import time
from pathlib import Path

import pytest

from app.adapters.pi import sessions as pi_sessions

# Os dois casos de slug abaixo espelham diretorios REAIS de `~/.pi/agent/sessions` na maquina
# Linux. O slug e o caminho ABSOLUTO com os separadores virando `-`, entao caminho POSIX literal
# so faz sentido la: no Windows o `os.path.abspath("/home/jefferson")` ganha a letra do drive e o
# esperado passaria a medir o abspath, nao a regra. O gemeo logo abaixo cobre o Windows com o
# valor medido no disco DESTA VM.
so_posix = pytest.mark.skipif(os.name != "posix", reason="slug de caminho POSIX; ver o gemeo do Windows")


@so_posix
def test_cwd_slug_matches_pi_layout():
    # Medido na maquina alvo: /home/jefferson -> --home-jefferson--
    assert pi_sessions.cwd_slug("/home/jefferson") == "--home-jefferson--"
    assert (pi_sessions.cwd_slug("/home/jefferson/Projetos/hangar")
            == "--home-jefferson-Projetos-hangar--")


@so_posix
def test_cwd_slug_keeps_spaces_accents_and_underscores():
    # Diretorios REAIS em ~/.pi/agent/sessions (Pi 0.82.1): so o separador de caminho vira '-'.
    # Trocar todo nao-alfanumerico devolvia `--home-jefferson--rea-de-trabalho-...--`, que nao
    # existe — e ai transcript_path() nao achava nada e o fallback por CP_PI_SESSION ficava morto.
    assert (pi_sessions.cwd_slug("/home/jefferson/Área de trabalho/repos/servicos_api")
            == "--home-jefferson-Área de trabalho-repos-servicos_api--")
    assert (pi_sessions.cwd_slug("/tmp/claude-1000/-home-jefferson-Projetos/scratchpad/piprobe")
            == "--tmp-claude-1000--home-jefferson-Projetos-scratchpad-piprobe--")


@pytest.mark.skipif(os.name != "nt", reason="slug de caminho Windows; ver os gemeos POSIX")
def test_cwd_slug_no_windows_leva_a_letra_do_drive():
    """Medido em 22/08/2026 na VM Windows: com o Pi 0.84.2 rodando em `C:\\cockpit`, o diretorio
    que ele criou em `~/.pi/agent/sessions` chama-se `--C--cockpit--` — a letra do drive entra
    como se fosse o primeiro componente, e as duas barras viram `-`. As duas grafias de entrada
    (barra e contrabarra) tem que cair no MESMO slug, senao o transcript_path erra conforme quem
    chamou."""
    assert pi_sessions.cwd_slug("C:\\cockpit") == "--C--cockpit--"
    assert pi_sessions.cwd_slug("C:/cockpit") == "--C--cockpit--"
    assert pi_sessions.cwd_slug("C:\\cockpit\\backend") == "--C--cockpit-backend--"


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

    got = pi_sessions.transcript_path("/w", "019fa3d5-f074-707b-92a8-1ca7f1d99ec9", "pi")
    assert got == str(target)


def test_omp_transcript_fora_da_pasta_do_cwd_e_achado_pelo_nome(tmp_path, monkeypatch):
    # omp 18.1.6 grava o transcript principal em `sessions/-/<nome>` e nao no diretorio do
    # `--session`; o Pi nao faz isso, entao no Pi a busca fica restrita a pasta do cwd.
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    raiz = tmp_path / "sessions"
    (raiz / "-").mkdir(parents=True)
    alvo = raiz / "-" / "2026-09-04T19-42-03-000Z_d5bb540a-af7b-45a7-ad80-e7de61326ead.jsonl"
    alvo.write_text("")
    assert pi_sessions.transcript_path("/tmp", "d5bb540a-af7b-45a7-ad80-e7de61326ead", "omp") == str(alvo)
    assert pi_sessions.localizar_na_raiz(alvo.name, "omp") == str(alvo)
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(raiz))
    assert pi_sessions.transcript_path("/tmp", "d5bb540a-af7b-45a7-ad80-e7de61326ead", "pi") == ""


def test_transcript_path_empty_when_session_not_created_yet(tmp_path, monkeypatch):
    # Sessao recem-spawnada: o arquivo so nasce no primeiro turno. "" (nao excecao) porque o
    # registry chama isto ANTES de a TUI escrever qualquer coisa.
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path))
    assert pi_sessions.transcript_path("/w", "nao-existe", "pi") == ""


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
    assert pi_sessions.transcript_path("/w", "abc", "pi") == str(new)


def test_sessions_root_por_provider(monkeypatch, tmp_path):
    monkeypatch.delenv("PI_CODING_AGENT_SESSION_DIR", raising=False)
    monkeypatch.delenv("PI_CODING_AGENT_DIR", raising=False)
    assert pi_sessions.sessions_root("pi") == Path.home() / ".pi" / "agent" / "sessions"
    assert pi_sessions.sessions_root("omp") == Path.home() / ".omp" / "agent" / "sessions"
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path / "omp-agent"))
    assert pi_sessions.sessions_root("omp") == tmp_path / "omp-agent" / "sessions"
    with pytest.raises(ValueError):
        pi_sessions.sessions_root("kimi")


def test_transcript_path_le_a_raiz_do_provider_pedido(monkeypatch, tmp_path):
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path / "omp"))
    d = tmp_path / "omp" / "sessions" / pi_sessions.cwd_slug("/w")
    d.mkdir(parents=True)
    f = d / "2026-09-03T15-51-00-640Z_01a067f7-6120-700b-b71d-6a6092e0c720.jsonl"
    f.write_text("")
    assert pi_sessions.transcript_path("/w", "01a067f7-6120-700b-b71d-6a6092e0c720", "omp") == str(f)
    monkeypatch.setenv("PI_CODING_AGENT_SESSION_DIR", str(tmp_path / "pi"))
    assert pi_sessions.transcript_path("/w", "01a067f7-6120-700b-b71d-6a6092e0c720", "pi") == ""


def test_transcript_alvo_fica_no_layout_normal(monkeypatch, tmp_path):
    import re
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path / "omp"))
    alvo = Path(pi_sessions.transcript_alvo("/w", "abc-uuid", "omp"))
    assert alvo.parent == tmp_path / "omp" / "sessions" / pi_sessions.cwd_slug("/w")
    assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z_abc-uuid\.jsonl", alvo.name)
    alvo.parent.mkdir(parents=True)
    alvo.write_text("")
    assert pi_sessions.transcript_path("/w", "abc-uuid", "omp") == str(alvo)


def test_subagente_do_omp_mora_direto_sob_o_stem():
    stem = "2026-09-03T16-17-19-304Z_01a0680f-77c8-7397-805f-c8651e6051f1"
    assert pi_sessions.is_subagent_transcript(f"/r/--w--/{stem}/ContarLinhas.jsonl")
    assert pi_sessions.is_subagent_transcript(f"/r/--w--/{stem}/44bad0fb/run-2/session.jsonl")
    assert not pi_sessions.is_subagent_transcript(f"/r/--w--/{stem}.jsonl")
    assert not pi_sessions.is_subagent_transcript("/r/--w--/notas/qualquer.jsonl")
