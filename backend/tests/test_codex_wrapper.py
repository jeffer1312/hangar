"""O wrapper do `codex` do shell (scripts/hangar-codex) com o BACKEND DESLIGADO.

O que estes testes protegem: antes, toda decisao do wrapper era uma pergunta a API local — com o
serviço parado ele nao abria nada, enquanto `claude`, `pi` e `kimi` abriam. Agora a fonte de
verdade e o sidecar em disco, e a API so entra onde agrega algo.

O modulo e carregado por caminho porque `scripts/hangar-codex` nao tem extensao `.py` (e um
executavel do PATH, nao um pacote).
"""
import importlib.machinery
import importlib.util
import os
import signal
from pathlib import Path

import pytest
from unittest.mock import patch

from app.adapters.codex import sessions as codex_sessions

_CAMINHO = Path(__file__).resolve().parents[2] / "scripts" / "hangar-codex"


def _wrapper():
    # Loader EXPLICITO: sem extensao `.py`, `spec_from_file_location` devolve None (nao acha um
    # loader pelo sufixo) e o erro sai como um AttributeError sem relacao com o que aconteceu.
    loader = importlib.machinery.SourceFileLoader("hangar_codex_wrapper", str(_CAMINHO))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


@pytest.fixture
def w():
    return _wrapper()


@pytest.fixture(autouse=True)
def _sidecars_em_tmp(tmp_path):
    sdir = tmp_path / "codex-sessions"
    with patch.object(codex_sessions, "_dir", lambda: sdir):
        yield sdir


def _salva(nome, cwd, rollout, app_pid=None):
    codex_sessions.save(nome, f"tid-{nome}", str(rollout), cwd,
                        endpoint="ws://127.0.0.1:1", app_pid=app_pid)


def _sem_api(w):
    """A API fora do ar: e o estado que este ticket existe pra cobrir."""
    return patch.object(w, "_api", side_effect=RuntimeError("backend inacessivel"))


def test_escolhe_a_mais_recente_por_atividade_sem_tocar_na_api(w, tmp_path, capsys):
    velho, novo = tmp_path / "a.jsonl", tmp_path / "b.jsonl"
    velho.write_text("x")
    novo.write_text("x")
    os.utime(velho, (1000, 1000))
    os.utime(novo, (2000, 2000))
    _salva("proj", "/tmp/proj", velho)
    _salva("proj-2", "/tmp/proj", novo)

    with _sem_api(w), patch.object(w, "_tmux_vivas", return_value={"proj", "proj-2"}):
        assert w._resume_target("/tmp/proj", codex_sessions) == "proj-2"
    # As que ficaram de fora sao DITAS, nunca descartadas em silencio.
    assert "proj" in capsys.readouterr().err


def test_sidecar_sem_pane_vivo_nao_e_candidata(w, tmp_path):
    """Sidecar orfao (pane derrubado sem limpeza) sequestraria o `codex` daquela pasta pra sempre:
    o wrapper diria "retomando" e o attach falharia."""
    rollout = tmp_path / "a.jsonl"
    rollout.write_text("x")
    _salva("proj", "/tmp/proj", rollout)

    with _sem_api(w), patch.object(w, "_tmux_vivas", return_value=set()):
        assert w._resume_target("/tmp/proj", codex_sessions) is None


def test_sessao_de_outro_diretorio_nao_e_candidata(w, tmp_path):
    rollout = tmp_path / "a.jsonl"
    rollout.write_text("x")
    _salva("outro", "/tmp/outro", rollout)

    with _sem_api(w), patch.object(w, "_tmux_vivas", return_value={"outro"}):
        assert w._resume_target("/tmp/proj", codex_sessions) is None


def test_cria_direto_no_tmux_com_o_comando_do_lancador(w):
    from app.adapters.codex import lancador
    chamadas = []

    class _R:
        returncode = 0
        stderr = ""

    with patch.object(w, "_tmux_vivas", return_value=set()), \
         patch.object(w.subprocess, "run", lambda *a, **k: chamadas.append(a[0]) or _R()):
        nome = w._create_local("/tmp/proj", "revise", lancador)

    assert nome == "proj"
    argv = chamadas[0]
    assert argv[:4] == ["tmux", "new-session", "-d", "-s"]
    # A identidade vai por env, que e de onde o lancador a le — nao repetida no comando.
    assert f"CP_SESSION_NAME={nome}" in argv
    assert "hangar-codex-tui --cwd /tmp/proj --prompt revise" in argv[-1]


def test_nome_livre_pula_o_que_o_tmux_ja_tem(w):
    from app.adapters.codex import lancador

    class _R:
        returncode = 0
        stderr = ""

    with patch.object(w, "_tmux_vivas", return_value={"proj", "proj-2"}), \
         patch.object(w.subprocess, "run", lambda *a, **k: _R()):
        assert w._create_local("/tmp/proj", None, lancador) == "proj-3"


def test_limpeza_sem_api_mata_o_app_server_e_apaga_o_sidecar(w, tmp_path):
    """Sem backend, era aqui que sobrava sidecar orfao mais um servidor escutando em loopback."""
    rollout = tmp_path / "a.jsonl"
    rollout.write_text("x")
    _salva("proj", "/tmp/proj", rollout, app_pid=4242)
    mortos = []

    with patch.object(w.os, "kill", lambda pid, sig: mortos.append((pid, sig))):
        w._limpar_local("proj", codex_sessions)

    assert mortos == [(4242, signal.SIGTERM)]
    assert codex_sessions.load("proj") is None


def test_limpeza_sobrevive_a_app_server_ja_morto(w, tmp_path):
    """O caso COMUM: o lancador chegou primeiro. Nao pode virar erro na saida do terminal."""
    rollout = tmp_path / "a.jsonl"
    rollout.write_text("x")
    _salva("proj", "/tmp/proj", rollout, app_pid=4242)

    def _morto(pid, sig):
        raise ProcessLookupError

    with patch.object(w.os, "kill", _morto):
        w._limpar_local("proj", codex_sessions)

    assert codex_sessions.load("proj") is None


def test_carregar_os_modulos_nao_puxa_o_pacote_dos_adapters(w):
    """`from app.adapters.codex import sessions` executa `app/adapters/__init__.py`, que instancia
    os quatro adapters e puxa `app.config` -> pydantic. Este wrapper roda no `python3` do SISTEMA:
    numa maquina sem pydantic instalado ali, aquele import falharia e o wrapper voltaria a depender
    da API — que e justamente o que ele nao pode fazer. Aqui passaria por acidente (esta maquina tem
    pydantic no sistema), entao o que se afirma e o FATO que segura a garantia: o pacote nao e
    tocado."""
    import sys as _sys
    for nome in ("app.adapters", "app.adapters.codex"):
        _sys.modules.pop(nome, None)

    sessions, lancador = w._do_backend()

    assert sessions is not None and lancador is not None
    assert "app.adapters" not in _sys.modules
    assert callable(lancador.comando_do_lancador)


def test_api_fora_do_ar_nao_impede_de_abrir(w):
    """Sem disco E sem API, "nao sei se ja existe sessao" tem que virar "nao ha" e seguir pra
    criacao. Deixar o erro subir encerrava o `codex` sem abrir nada."""
    with _sem_api(w):
        assert w._resume_target("/tmp/proj", None) is None


def test_sem_os_modulos_do_backend_a_api_decide(w, tmp_path):
    """Plano B honesto: sem conseguir ler o disco, o wrapper volta a perguntar a API — o
    comportamento de antes deste ticket, e nao um silencio."""
    with patch.object(w, "_api", return_value=[
            {"provider": "codex", "cwd": "/tmp/proj", "name": "proj", "last_activity": 5.0}]):
        assert w._resume_target("/tmp/proj", None) == "proj"
