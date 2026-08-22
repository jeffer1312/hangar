"""Criar/retomar sessão com motor.

O motor é aplicado prefixando o comando com `cp-engine --exec`, não com `tmux -e`: assim a key não
aparece em /proc/<pid>/cmdline (legível por qualquer usuário) e o tmux.py não muda.
E o motor tem que sobreviver aos DOIS resumes — senão uma sessão Kimi ressuscita na conta Anthropic
continuando um transcript de Kimi, calado.
"""
import os

import pytest

from app import engines as eng
from app import procinfo
from app import registry as reg


# Capturado no IMPORT, antes de qualquer fixture: o `_isola` abaixo troca `reg._exigir_cp_engine`
# por um no-op, entao la dentro nao ha mais como alcancar a funcao de verdade.
_EXIGIR_ORIGINAL = reg._exigir_cp_engine


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(eng, "caminho", lambda: tmp_path / "engines.json")
    # A guarda que recusa quando o `cp-engine` nao esta no PATH e sobre o AMBIENTE do servidor, nao
    # sobre a montagem do comando, que e o assunto deste arquivo. Sem desliga-la aqui, todos estes
    # casos passariam a depender de o lancador estar instalado na maquina que roda a suite — verde
    # no Linux (onde o install-claude-wrapper.sh o poe no PATH) e vermelho no Windows, pelo
    # ambiente e nao pelo codigo. A guarda tem teste proprio, logo abaixo.
    monkeypatch.setattr(reg, "_exigir_cp_engine", lambda: None)
    yield


def test_recusa_alto_quando_o_cp_engine_nao_esta_no_path(monkeypatch):
    """A guarda em si — o unico caso que NAO desliga o `_exigir_cp_engine`.

    Sem ela o pane nasce rodando um comando que nao existe, morre no ato, e o `tmux new-session`
    devolve 0 assim mesmo (medido no psmux: rc=0 e, 3s depois, `has-session` ja responde 1). O app
    entao reporta "sessao criada" pra uma sessao que evaporou.
    """
    monkeypatch.setattr(reg.shutil, "which", lambda nome: None)
    with pytest.raises(ValueError, match="cp-engine"):
        _EXIGIR_ORIGINAL()

    monkeypatch.setattr(reg.shutil, "which", lambda nome: "/qualquer/cp-engine")
    _EXIGIR_ORIGINAL()           # com o lancador no PATH, passa calado


def _motor():
    eng.salvar("kimi", {"base_url": "https://api.kimi.com/coding",
                        "api_key": "sk-kimi-1234", "model": "k3"})


def _reg(tmp_path, monkeypatch, visto):
    def _fake_new(name, cwd, command, config_dir=None):
        visto["command"] = command
        return True

    monkeypatch.setattr(reg.tmux, "new_session", _fake_new)
    monkeypatch.setattr(reg.tmux, "has_session", lambda n: False)
    monkeypatch.setattr(reg, "_pretrust_cwd", lambda cwd, cfg: None)
    return reg.SessionRegistry(projects_dir=tmp_path)


def test_create_com_motor_prefixa_o_comando(tmp_path, monkeypatch):
    _motor()
    visto = {}
    info = _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), engine="kimi")
    assert visto["command"].startswith("cp-engine --exec kimi -- claude --session-id ")
    assert info.engine == "kimi"


def test_create_com_motor_nao_poe_a_key_no_comando(tmp_path, monkeypatch):
    _motor()
    visto = {}
    _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), engine="kimi")
    assert "sk-kimi" not in visto["command"]


def test_create_sem_motor_nao_muda_o_comando(tmp_path, monkeypatch):
    visto = {}
    info = _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path))
    assert visto["command"].startswith("claude --session-id ")
    assert info.engine is None


def test_create_com_motor_inexistente_estoura(tmp_path, monkeypatch):
    visto = {}
    r = _reg(tmp_path, monkeypatch, visto)
    with pytest.raises(ValueError, match="motor"):
        r.create("s", str(tmp_path), engine="fantasma")
    assert "command" not in visto


@pytest.mark.skipif(os.name != "posix",
                    reason="exercita o ramo /proc do _engine_of contra um /proc de mentira; no "
                           "Windows o despacho vai pro psutil e nao ha o que este caso testa")
def test_engine_of_le_o_cp_engine_do_proc(tmp_path, monkeypatch):
    # Mesmo truque do _config_dir_of: o env do processo VIVO é o registro autoritativo — um sidecar
    # em disco pode divergir do que está de fato rodando no pane.
    environ = tmp_path / "environ"
    environ.write_bytes(b"PATH=/usr/bin\x00CP_ENGINE=kimi\x00HOME=/home/x\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(environ))
    assert reg._engine_of(1234) == "kimi"


def test_engine_of_sem_a_marca_e_none(tmp_path, monkeypatch):
    environ = tmp_path / "environ"
    environ.write_bytes(b"PATH=/usr/bin\x00")
    monkeypatch.setattr(procinfo, "_proc_environ_path", lambda pid: str(environ))
    assert reg._engine_of(1234) is None


def _prep_resume(tmp_path, monkeypatch, visto, motor):
    sid = "11111111-2222-3333-4444-555555555555"
    proj = tmp_path / "projects" / "-tmp"
    proj.mkdir(parents=True)
    (proj / f"{sid}.jsonl").write_text("", encoding="utf-8")

    def _fake_new(name, cwd, command, config_dir=None):
        visto["command"] = command
        return True

    monkeypatch.setattr(reg, "_engine_of", lambda pid: motor)
    monkeypatch.setattr(reg, "_config_dir_of", lambda pid: None)
    monkeypatch.setattr(reg, "sanitize_cwd", lambda cwd: "-tmp")
    monkeypatch.setattr(reg.tmux, "kill_session", lambda n: None)
    monkeypatch.setattr(reg.tmux, "new_session", _fake_new)
    r = reg.SessionRegistry(projects_dir=tmp_path / "projects")
    monkeypatch.setattr(r, "_pane_of", lambda name: {"cwd": "/tmp", "pid": 4242})
    monkeypatch.setattr(r, "_forget", lambda name: None)
    return r, sid


def test_resume_de_pane_vivo_preserva_o_motor(tmp_path, monkeypatch):
    _motor()
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "kimi")
    info = r.resume("s", sid)
    assert visto["command"] == f"cp-engine --exec kimi -- claude --resume {sid}"
    assert info.engine == "kimi"


def test_resume_de_motor_removido_nao_trava_a_sessao(tmp_path, monkeypatch):
    # Motor apagado no app depois da sessão nascer: melhor ressuscitar na conta Anthropic (e o badge
    # mostrar isso) do que recusar o resume e deixar a sessão inacessível.
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "sumiu")
    info = r.resume("s", sid)
    assert visto["command"] == f"claude --resume {sid}"
    assert info.engine is None


def test_resume_do_arquivo_aceita_motor(tmp_path, monkeypatch):
    # api.py:1942 usa create(resume_session_id=...): o pane morreu, não há /proc para ler, então o
    # motor vem do cliente. Sem isto, retomar do Arquivo troca de motor calado.
    _motor()
    visto = {}
    r = _reg(tmp_path, monkeypatch, visto)
    sid = "11111111-2222-3333-4444-555555555555"
    info = r.create("s", str(tmp_path), resume_session_id=sid, engine="kimi")
    assert visto["command"] == f"cp-engine --exec kimi -- claude --resume {sid}"
    assert info.engine == "kimi"


# ── Task 3: escolha de modelo/janela entra no prefixo cp-engine ────────────────────────────────


def test_create_com_escolha_poe_modelo_e_esforco_no_comando(tmp_path, monkeypatch):
    # A flag do modelo chega ao claude; o uuid é aleatório, então confere por prefixo.
    visto = {}
    _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), model="k3-256k", effort="high")
    assert visto["command"].startswith(
        "claude --session-id ") and visto["command"].endswith("--model k3-256k --effort high")


def test_create_com_motor_e_escolha_remonta_o_prefixo_com_modelo_e_janela(tmp_path, monkeypatch):
    """O prefixo do motor leva modelo E janela: a flag sozinha ganharia só de ANTHROPIC_MODEL, e o
    ambiente (aliases, subagente, janela) voltaria pro modelo do motor — o cenário "motor de 1M
    com modelo de 262k"."""
    _motor()
    visto = {}
    _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), engine="kimi",
                                              model="k3-256k", context_window=262144)
    assert visto["command"].startswith(
        "cp-engine --exec kimi --model k3-256k --context 262144 -- claude --session-id ")


def test_create_com_motor_e_modelo_sem_janela_omite_o_context(tmp_path, monkeypatch):
    # Provedor que não reporta context_length: sem o número do modelo, o --context não pode sair
    # (exportar a janela do MOTOR com outro modelo é o bug de volta).
    _motor()
    visto = {}
    _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), engine="kimi",
                                              model="k3-256k", context_window=None)
    assert visto["command"].startswith(
        "cp-engine --exec kimi --model k3-256k -- claude --session-id ")


def test_resume_preserva_modelo_e_janela_do_pane(tmp_path, monkeypatch):
    """Dívida de teste do caminho de resume: sem o par procinfo._model_of/_env_var_of aplicado, a
    sessão ressuscita com a flag num modelo e o AMBIENTE noutro (as cinco chaves, o subagente e a
    janela voltariam pro modelo do motor) — e nada na tela acusa."""
    _motor()
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: ("k3-256k", "high"))
    monkeypatch.setattr(procinfo, "_env_var_of", lambda pid, nome: "262144")
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "kimi")
    info = r.resume("s", sid)
    assert visto["command"] == (
        f"cp-engine --exec kimi --model k3-256k --context 262144 -- "
        f"claude --resume {sid} --model k3-256k --effort high")
    assert info.engine == "kimi"


def test_resume_com_modelo_marcado_de_janela_nao_estoura(tmp_path, monkeypatch):
    """`opus[1m]` é o que o próprio Claude Code anexa ao nome do modelo, e está no settings.json das
    contas do usuário. Com o colchete fora da whitelist, retomar uma sessão dessas estourava."""
    _motor()
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: ("opus[1m]", None))
    monkeypatch.setattr(procinfo, "_env_var_of", lambda pid, nome: None)
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, None)
    r.resume("s", sid)
    assert visto["command"] == f"claude --resume {sid} --model 'opus[1m]'"


def test_resume_com_modelo_invalido_nao_mata_o_pane(tmp_path, monkeypatch):
    """O comando é montado ANTES do kill. Montar depois trocava 'o resume falhou' por 'a sessão foi
    destruída e não relançada' — o pane já não existia quando a validação estourava."""
    _motor()
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: ("opus; rm -rf /", None))
    monkeypatch.setattr(procinfo, "_env_var_of", lambda pid, nome: None)
    visto = {}
    mortes = []
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, None)
    monkeypatch.setattr(reg.tmux, "kill_session", lambda n: mortes.append(n))
    with pytest.raises(ValueError):
        r.resume("s", sid)
    assert mortes == []                 # a sessão continua de pé
    assert "command" not in visto       # e nada foi relançado


def test_resume_com_modelo_sem_janela_omite_o_context(tmp_path, monkeypatch):
    _motor()
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: ("k3-256k", "high"))
    monkeypatch.setattr(procinfo, "_env_var_of", lambda pid, nome: None)
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "kimi")
    r.resume("s", sid)
    assert visto["command"] == (
        f"cp-engine --exec kimi --model k3-256k -- claude --resume {sid} --model k3-256k --effort high")


def test_resume_sem_modelo_no_pane_nao_poe_flags_nem_context(tmp_path, monkeypatch):
    # Sessão que subiu sem escolha: o resume tem que continuar byte por byte o de hoje.
    _motor()
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: (None, None))
    monkeypatch.setattr(procinfo, "_env_var_of", lambda pid, nome: "262144")
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "kimi")
    r.resume("s", sid)
    assert visto["command"] == f"cp-engine --exec kimi -- claude --resume {sid}"


def test_resume_le_a_janela_antes_de_matar_o_pane(tmp_path, monkeypatch):
    """B2 da revisão final. A janela mora no /proc/<pid>/environ do processo que está no pane:
    lê-la DEPOIS do kill_session devolve nada (o /proc some junto), e a sessão ressuscita sem
    --context — compactando em ~167k com um modelo de 262k, calado. O fake deixa o environ
    ILEGÍVEL depois do kill: se a ordem voltar a errar, o --context 262144 some do comando."""
    _motor()
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: ("k3-256k", "high"))
    morto = {"sim": False}

    def _env(pid, nome):
        return None if morto["sim"] else "262144"

    def _kill(nome):
        morto["sim"] = True

    monkeypatch.setattr(procinfo, "_env_var_of", _env)
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "kimi")
    monkeypatch.setattr(reg.tmux, "kill_session", _kill)
    r.resume("s", sid)
    assert visto["command"] == (
        f"cp-engine --exec kimi --model k3-256k --context 262144 -- "
        f"claude --resume {sid} --model k3-256k --effort high")


def test_resume_de_motor_removido_descarta_a_escolha_no_fallback(tmp_path, monkeypatch):
    """B3 (versão estreita) da revisão final. Motor apagado cai pro fallback da conta Anthropic —
    decisão anterior a esta branch, não se mexe. O que ESTA branch piorou é carregar junto o
    modelo/esforço do MOTOR: `claude --resume … --model k3-256k --effort high` na conta Anthropic
    é sessão inviável (id que ela não conhece). No fallback, descartar modelo, esforço e janela:
    resume pelado, como antes desta branch."""
    monkeypatch.setattr(procinfo, "_model_of", lambda pid: ("k3-256k", "high"))
    monkeypatch.setattr(procinfo, "_env_var_of", lambda pid, nome: "262144")
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "sumiu")
    info = r.resume("s", sid)
    assert visto["command"] == f"claude --resume {sid}"
    assert info.engine is None
