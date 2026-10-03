"""Ponte do plugin de function hooks: a regra de quem fica com o pedido de permissão e a ida e volta
da resposta do app. O resto (envio por `fill`) depende de tmux e é conferido no uso real."""
import asyncio
import json
import threading
from pathlib import Path

import pytest
from fastapi import HTTPException

from app import plugin_bridge as pb


# Conversa que o Hangar acompanha em toda sessão dos testes; o plugin certo manda este id.
UUID = "0b6e5c1a-1111-4222-8333-444455556666"
_REAL_TRACKED = pb.tracked_session_id


@pytest.fixture(autouse=True)
def _limpa(monkeypatch):
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: UUID)
    yield
    for d in (pb._perguntas, pb._waiters, pb._estados, pb._batidas, pb._eventos, pb._fechadas,
              pb._donos, pb._recusas):
        d.clear()
    pb._apps_abertos = 0


def _corpo(id: str, **extra) -> pb.AskBody:
    return pb.AskBody(sessao="s1", token=pb.mint("s1"), id=id, **extra)


def test_token_e_por_sessao_e_recusa_o_de_outra():
    assert pb.mint("s1") == pb.mint("s1") and pb.mint("s1") != pb.mint("s2")
    with pytest.raises(HTTPException) as e:
        asyncio.run(pb.ask(pb.AskBody(sessao="s1", token=pb.mint("s2"), id="x")))
    assert e.value.status_code == 403


def test_permissao_sem_ninguem_no_app_volta_pro_terminal(monkeypatch):
    # Segurar o `ask` esconde o diálogo do terminal: sem app aberto, não há quem responda.
    monkeypatch.setattr(pb, "terminal_preso", lambda name: False)
    assert asyncio.run(pb.ask(_corpo("perm:t1", tool="Bash"))) == {"soltar": True}
    assert pb.pergunta_pendente("s1") is None


def test_permissao_com_terminal_preso_volta_pro_terminal(monkeypatch):
    monkeypatch.setattr(pb, "terminal_preso", lambda name: True)
    pb.app_entrou()
    assert asyncio.run(pb.ask(_corpo("perm:t1", tool="Bash"))) == {"soltar": True}


def test_resposta_do_app_chega_ao_hook_e_so_vale_com_o_aviso_dele(monkeypatch):
    monkeypatch.setattr(pb, "terminal_preso", lambda name: False)
    monkeypatch.setattr(pb, "CONFIRMA_S", 3.0)
    pb.app_entrou()
    resultado: dict = {}

    async def cena():
        espera = asyncio.create_task(pb.ask(_corpo("perm:t1", tool="Bash", resumo="echo ok")))
        while pb.pergunta_pendente("s1") is None:
            await asyncio.sleep(0.01)
        assert pb.pergunta_pendente("s1")["resumo"] == "echo ok"
        # A rota do app roda em thread do pool, nunca no loop.
        t = threading.Thread(
            target=lambda: resultado.update(ok=pb.responder_pergunta("s1", {"permitir": True})))
        t.start()
        recebido = await espera
        await pb.ask_fim(pb.AskFimBody(sessao="s1", token=pb.mint("s1"), id="perm:t1", vencedor="app"))
        await asyncio.to_thread(t.join)
        return recebido

    assert asyncio.run(cena()) == {"permitir": True}
    assert resultado["ok"] is True
    assert pb.pergunta_pendente("s1") is None


def test_portao_desligado_nao_poe_nada_na_sessao_e_ligado_poe_o_plugin(monkeypatch):
    monkeypatch.setattr(pb, "plugin_in_skills_dir", lambda config_dir=None: False)
    # Desligado, a sessão nasce byte a byte como antes: sem flag, sem env. É a promessa do fallback.
    from app.adapters import get_adapter
    monkeypatch.setattr(pb, "ligado", lambda: False)
    assert pb.raizes_dos_plugins() == [] and pb.env_da_sessao("s1") == {}
    assert get_adapter("claude").spawn_command("/tmp/p", "sid") == ["claude", "--session-id", "sid"]

    monkeypatch.setattr(pb, "ligado", lambda: True)
    (raiz,) = pb.raizes_dos_plugins()
    assert Path(raiz).parts[-2:] == ("plugins", "hangar")
    assert get_adapter("claude").spawn_command("/tmp/p", "sid")[:5] == [
        "claude", "--session-id", "sid", "--plugin-dir", raiz]
    env = pb.env_da_sessao("s1")
    assert env["HANGAR_PLUGIN_TOKEN"] == pb.mint("s1") and env["HANGAR_PLUGIN_URL"].endswith("/api/plugin")


def test_resposta_sem_ninguem_segurando_nao_e_entrega():
    assert pb.responder_pergunta("s1", {"permitir": True}) is False


def test_resposta_repetida_de_pergunta_que_o_app_ja_fechou_conta_como_entregue():
    # Toque duplo no app: a segunda chega com a pergunta já fechada. Devolver False a mandaria de
    # novo pela tecla — a mesma resposta duas vezes.
    pb._fechadas["s1"] = ("toolu_1", "app")
    assert pb.responder_pergunta("s1", {"answers": {}}, "toolu_1") is True
    assert pb.responder_pergunta("s1", {"answers": {}}, "toolu_2") is False
    pb._fechadas["s1"] = ("toolu_1", "terminal")
    assert pb.responder_pergunta("s1", {"answers": {}}, "toolu_1") is False


def test_entrega_roda_sob_a_trava_de_envio_da_sessao(monkeypatch):
    # O Enter, a conferência e a limpeza tocam o mesmo composer do `send_prompt`.
    from app import terminal_input
    visto: dict = {}
    monkeypatch.setattr(pb, "_entregar",
                        lambda n, t, m, j: visto.update(travada=terminal_input._send_lock(n).locked()))
    pb.entregar("s1", "oi")
    assert visto["travada"] is True
    assert terminal_input._send_lock("s1").locked() is False


class _Saida:
    def __init__(self, stdout: str):
        self.stdout = stdout


def test_versao_do_cli_diz_se_os_mods_vem_ligados(monkeypatch):
    pb.esquecer_capacidade()
    monkeypatch.setattr(pb.shutil, "which", lambda n: "/usr/bin/claude")
    monkeypatch.setattr(pb.subprocess, "run", lambda *a, **k: _Saida("2.1.287 (Claude Code)\n"))
    assert pb.cli_version() == (2, 1, 287) and pb.mods_by_default()
    pb.esquecer_capacidade()
    monkeypatch.setattr(pb.subprocess, "run", lambda *a, **k: _Saida("2.1.286 (Claude Code)\n"))
    assert not pb.mods_by_default()
    pb.esquecer_capacidade()
    monkeypatch.setattr(pb.subprocess, "run", lambda *a, **k: _Saida("lixo"))
    assert pb.cli_version() is None and not pb.mods_by_default()
    pb.esquecer_capacidade()


def test_plugin_na_pasta_de_skills_da_conta_dispensa_o_plugin_dir(monkeypatch, tmp_path):
    monkeypatch.setattr(pb, "ligado", lambda: True)
    monkeypatch.setattr(pb, "mods_by_default", lambda: True)
    assert pb.raizes_dos_plugins(tmp_path) == [str(pb.PLUGIN_SRC)]
    manifesto = tmp_path / "skills" / "hangar" / ".claude-plugin" / "plugin.json"
    manifesto.parent.mkdir(parents=True)
    manifesto.write_text('{"name": "outro"}', encoding="utf-8")
    assert pb.raizes_dos_plugins(tmp_path) == [str(pb.PLUGIN_SRC)]
    manifesto.write_text('{"name": "hangar"}', encoding="utf-8")
    assert pb.raizes_dos_plugins(tmp_path) == []


def test_cli_sem_mods_por_padrao_mantem_o_plugin_dir_mesmo_com_o_plugin_nas_skills(monkeypatch, tmp_path):
    # A pasta de skills só foi medida carregando o plugin no CLI com mods por padrão.
    monkeypatch.setattr(pb, "ligado", lambda: True)
    monkeypatch.setattr(pb, "mods_by_default", lambda: False)
    manifesto = tmp_path / "skills" / "hangar" / ".claude-plugin" / "plugin.json"
    manifesto.parent.mkdir(parents=True)
    manifesto.write_text('{"name": "hangar"}', encoding="utf-8")
    assert pb.raizes_dos_plugins(tmp_path) == [str(pb.PLUGIN_SRC)]


def test_interruptor_desligado_tira_o_plugin_mesmo_com_mods_por_padrao(monkeypatch):
    from app import runtime_config
    monkeypatch.setattr(pb, "mods_by_default", lambda: True)
    monkeypatch.setattr(runtime_config, "get", lambda k: False if k == "claude_function_hooks" else None)
    assert pb._ligado_de_verdade() is False


async def _ate(cond):
    while not cond():
        await asyncio.sleep(0.01)


def _pull(sessao="s1", instance="a", modos=("fill",), session_id=UUID):
    return pb.PullBody(sessao=sessao, token=pb.mint(sessao), instance=instance, modos=list(modos),
                       session_id=session_id)


def test_segunda_instancia_com_dono_vivo_recebe_409(monkeypatch):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)

    async def cena():
        primeira = asyncio.create_task(pb.pull(_pull(instance="a")))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        with pytest.raises(HTTPException) as e:
            await pb.pull(_pull(instance="b"))
        await primeira
        return e.value.status_code

    assert asyncio.run(cena()) == 409


def test_esquecer_libera_a_sessao_mesmo_com_o_pull_antigo_terminando(monkeypatch):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)

    async def cena():
        antiga = asyncio.create_task(pb.pull(_pull(instance="a")))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        pb.esquecer("s1")
        await asyncio.gather(antiga, pb.pull(_pull(instance="b")))

    asyncio.run(cena())
    assert pb._donos["s1"][0] == "b"


def test_pull_antigo_nao_recria_o_dono_esquecido(monkeypatch):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)

    async def cena():
        antiga = asyncio.create_task(pb.pull(_pull(instance="a")))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        pb.esquecer("s1")
        await antiga

    asyncio.run(cena())
    assert "s1" not in pb._donos


def test_dono_sem_batida_expira_e_outra_instancia_assume(monkeypatch):
    import time
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)
    pb._donos["s1"] = ("a", {"fill"}, time.monotonic() - (pb.ESPERA_S + 10) - 1)
    asyncio.run(pb.pull(_pull(instance="b")))
    assert pb._donos["s1"][0] == "b"


def test_modos_declarados_ficam_com_o_dono(monkeypatch):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)
    asyncio.run(pb.pull(_pull(instance="a", modos=("fill", "user"))))
    assert pb.declared_modes("s1") == {"fill", "user"}


def test_endereco_da_maquina_e_gravado_sem_o_bearer(tmp_path):
    pb._publish_address(tmp_path)
    texto = pb.machine_file(tmp_path).read_text(encoding="utf-8")
    from app.config import settings
    assert json.loads(texto)["url"].endswith("/api/plugin")
    assert json.loads(texto)["chave"] == pb.machine_key()
    assert not settings.auth_token or settings.auth_token not in texto


@pytest.fixture
def ligado(monkeypatch):
    monkeypatch.setattr(pb, "ligado", lambda: True)


def test_whoami_resolve_pelo_pane(monkeypatch, ligado):
    from app import quem_chama
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1" if pane == "%3" else None)
    r = asyncio.run(pb.whoami(pb.WhoamiBody(chave=pb.machine_key(), pane="%3", session_id=UUID)))
    assert r == {"sessao": "s1", "token": pb.mint("s1"), "origem": "pane"}


def test_whoami_com_interruptor_desligado_nao_resolve_nem_pelo_pane(monkeypatch):
    from app import quem_chama
    monkeypatch.setattr(pb, "ligado", lambda: False)
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1")
    assert asyncio.run(pb.whoami(pb.WhoamiBody(chave=pb.machine_key(), pane="%3"))) == {"sessao": None}


def test_whoami_nome_so_vale_sem_pane(monkeypatch, ligado):
    from app import quem_chama
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: None)
    monkeypatch.setattr(quem_chama, "_por_nome", lambda nome: nome)
    com_pane = asyncio.run(pb.whoami(pb.WhoamiBody(chave=pb.machine_key(), pane="%0", nome="s1")))
    sem_pane = asyncio.run(pb.whoami(pb.WhoamiBody(chave=pb.machine_key(), nome="s1", session_id=UUID)))
    assert com_pane == {"sessao": None} and sem_pane["sessao"] == "s1"


def test_whoami_de_outro_servidor_tmux_nao_resolve(monkeypatch, ligado):
    from app import quem_chama
    monkeypatch.setattr(pb, "_socket_do_tmux", lambda: "/tmp/tmux-1000/default")
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1")
    corpo = pb.WhoamiBody(chave=pb.machine_key(), pane="%3", tmux="/tmp/tmux-1000/outro,42,0")
    assert asyncio.run(pb.whoami(corpo)) == {"sessao": None}


def test_whoami_sem_socket_conhecido_nao_compara(monkeypatch, ligado):
    from app import quem_chama
    monkeypatch.setattr(pb, "_socket_do_tmux", lambda: None)
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1")
    corpo = pb.WhoamiBody(chave=pb.machine_key(), pane="%3", tmux="/tmp/tmux-1000/outro,42,0",
                          session_id=UUID)
    assert asyncio.run(pb.whoami(corpo))["sessao"] == "s1"


PSMUX_TMUX = "/tmp/psmux-{}/default,59317,0"


@pytest.fixture
def psmux(monkeypatch):
    import subprocess
    from app import quem_chama, tmux
    monkeypatch.setattr(tmux, "_run", lambda args, input=None: subprocess.CompletedProcess(
        args, 0, stdout="3568 cx-a\n17392 cx-b\n", stderr=""))
    # `%1` existe em toda sessão do psmux; se o pane fosse consultado, cairia em cx-a.
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "cx-a")
    monkeypatch.setattr(quem_chama, "_por_nome", lambda nome: nome if nome == "cx-c" else None)


def test_whoami_psmux_resolve_pelo_pid_do_servidor_e_ignora_o_pane(psmux, ligado):
    corpo = pb.WhoamiBody(chave=pb.machine_key(), pane="%1", tmux=PSMUX_TMUX.format(17392),
                          session_id=UUID)
    assert asyncio.run(pb.whoami(corpo)) == {"sessao": "cx-b", "token": pb.mint("cx-b"),
                                             "origem": "psmux-pid"}


def test_whoami_psmux_pid_desconhecido_cai_no_nome(psmux, ligado):
    def corpo(nome):
        return pb.WhoamiBody(chave=pb.machine_key(), pane="%1", nome=nome, tmux=PSMUX_TMUX.format(999),
                             session_id=UUID)
    assert asyncio.run(pb.whoami(corpo("cx-c")))["sessao"] == "cx-c"
    assert asyncio.run(pb.whoami(corpo(None))) == {"sessao": None}
    assert asyncio.run(pb.whoami(corpo("morta"))) == {"sessao": None}


def test_whoami_recusa_chave_errada(ligado):
    with pytest.raises(HTTPException) as e:
        asyncio.run(pb.whoami(pb.WhoamiBody(chave="x", pane="%3")))
    assert e.value.status_code == 403


def test_rotas_novas_do_plugin_recusam_cliente_de_fora(monkeypatch, ligado):
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import quem_chama
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1")
    app = FastAPI()
    app.include_router(pb.plugin_router)
    corpos = {"/api/plugin/whoami": {"chave": pb.machine_key(), "pane": "%3", "session_id": UUID},
              "/api/plugin/submitted": {"sessao": "s1", "token": pb.mint("s1"), "ok": True}}
    for rota, corpo in corpos.items():
        assert TestClient(app, client=("100.64.0.9", 5000)).post(rota, json=corpo).status_code == 403, rota
        assert TestClient(app, client=("127.0.0.1", 5000)).post(rota, json=corpo).status_code == 200, rota


def test_pull_semeia_o_estado_so_quando_o_state_nao_disse_nada(monkeypatch):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)
    asyncio.run(pb.pull(pb.PullBody(sessao="s1", token=pb.mint("s1"), instance="a", estado="idle",
                                    session_id=UUID)))
    assert pb.estado_recente("s1") == ("idle", None)
    pb.esquecer("s1")
    asyncio.run(pb.state(pb.StateBody(sessao="s1", token=pb.mint("s1"), estado="working"), None))
    asyncio.run(pb.pull(pb.PullBody(sessao="s1", token=pb.mint("s1"), instance="a", estado="idle",
                                    session_id=UUID)))
    assert pb.estado_recente("s1") == ("working", None)


def test_modo_user_so_com_dono_que_declarou_sessao_parada_e_texto_simples(monkeypatch):
    monkeypatch.setattr(pb, "mods_by_default", lambda: True)
    monkeypatch.setattr(pb, "declared_modes", lambda name: {"fill", "user"})
    monkeypatch.setattr(pb, "estado_recente", lambda name: ("idle", None))
    assert pb.choose_mode("s1", "roda os testes\ne commita") == "user"
    assert pb.choose_mode("s1", "manda pro a@b.com") == "user"
    for texto in ("olha @src/app.py", "@README.md resume", "!ls", "/model"):
        assert pb.choose_mode("s1", texto) == "fill", texto
    monkeypatch.setattr(pb, "estado_recente", lambda name: ("working", None))
    assert pb.choose_mode("s1", "oi") == "fill"
    monkeypatch.setattr(pb, "estado_recente", lambda name: ("idle", None))
    monkeypatch.setattr(pb, "declared_modes", lambda name: {"fill"})
    assert pb.choose_mode("s1", "oi") == "fill"
    monkeypatch.setattr(pb, "declared_modes", lambda name: {"fill", "user"})
    monkeypatch.setattr(pb, "mods_by_default", lambda: False)
    assert pb.choose_mode("s1", "oi") == "fill"


def _entrega_user(monkeypatch, confirma: bool | None, no_transcript: set[str] | None,
                  antes: set[str] | None = frozenset(), confirma_s: float = 0.3, leitura=None):
    monkeypatch.setattr(pb, "CONFIRMA_S", confirma_s)
    monkeypatch.setattr(pb, "PROVA_TRANSCRIPT_S", 0.3)
    from app import pqueue, tmux
    monkeypatch.setattr(tmux, "send_keys", lambda *a, **k: pytest.fail("modo user não aperta tecla"))
    # A primeira leitura é a foto de antes da entrega; as seguintes, o transcript depois dela.
    leituras = iter([antes])
    monkeypatch.setattr(pqueue, "committed_user_lines",
                        leitura or (lambda jsonl, provider="claude": next(leituras, no_transcript)))

    async def cena():
        pull = asyncio.create_task(pb.pull(pb.PullBody(sessao="s1", token=pb.mint("s1"), instance="a",
                                                       modos=["fill", "user"], session_id=UUID)))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        entrega = asyncio.create_task(asyncio.to_thread(pb._entregar, "s1", "oi", "user", "/x.jsonl"))
        assert (await asyncio.wait_for(pull, 5)) == {"text": "oi", "modo": "user"}
        if confirma is not None:
            await pb.submitted(pb.SubmittedBody(sessao="s1", token=pb.mint("s1"), ok=confirma))
        return await asyncio.wait_for(entrega, 5)

    return asyncio.run(cena())


def test_modo_user_confirmado_entrega_sem_tecla(monkeypatch):
    assert _entrega_user(monkeypatch, True, set()) is True


def test_modo_user_sem_confirmacao_mas_no_transcript_conta_como_entregue(monkeypatch):
    assert _entrega_user(monkeypatch, None, {"oi"}) is True


def test_modo_user_sem_confirmacao_e_fora_do_transcript_volta_pro_tmux(monkeypatch):
    assert _entrega_user(monkeypatch, None, set()) is False


def test_modo_user_sem_confirmacao_e_transcript_ilegivel_nao_volta_pra_tecla(monkeypatch):
    assert _entrega_user(monkeypatch, None, None) is pb.INCERTO


def test_modo_user_texto_no_transcript_responde_antes_da_confirmacao(monkeypatch):
    import time
    inicio = time.monotonic()
    assert _entrega_user(monkeypatch, None, {"oi"}, confirma_s=30.0) is True
    assert time.monotonic() - inicio < 2.0


def test_modo_user_texto_que_chega_no_fim_do_prazo_nao_volta_pra_tecla(monkeypatch):
    # O texto só aparece depois da última leitura do laço: sem a releitura no prazo, iria pra tecla.
    import time
    from app import pqueue
    prazo = []

    def leitura(jsonl, provider="claude"):
        if not prazo:
            prazo.append(time.monotonic() + pb.CONFIRMA_S + pb.PROVA_TRANSCRIPT_S)
            return set()
        return {"oi"} if time.monotonic() >= prazo[0] else set()

    assert _entrega_user(monkeypatch, None, set(), leitura=leitura) is True


def test_modo_user_texto_igual_a_um_anterior_nao_prova_a_entrega(monkeypatch):
    # O transcript é conferido por conjunto: o "oi" de antes não diz que este chegou.
    assert _entrega_user(monkeypatch, None, {"oi"}, antes={"oi"}) is pb.INCERTO
    assert _entrega_user(monkeypatch, True, {"oi"}, antes={"oi"}) is True


def test_whoami_so_entrega_a_ponte_a_conversa_que_o_hangar_acompanha(monkeypatch, ligado):
    # Um segundo `claude` num split resolve para a mesma sessão pelo pane; só a conversa o separa.
    from app import quem_chama
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1")

    def pergunta(session_id):
        return asyncio.run(pb.whoami(pb.WhoamiBody(chave=pb.machine_key(), pane="%3",
                                                   session_id=session_id)))
    assert pergunta(UUID)["sessao"] == "s1"
    assert pergunta("outra-conversa") == {"sessao": None}
    assert pergunta(None) == {"sessao": None}
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: None)
    assert pergunta(UUID) == {"sessao": None}


@pytest.mark.parametrize("enviado,acompanhado,motivo", [
    ("outra-conversa", UUID, "uuid-diferente"),
    (UUID, None, "uuid-desconhecido"),
    (None, UUID, "uuid-ausente"),
])
def test_pull_de_outra_conversa_recebe_409_e_nao_vira_dono(monkeypatch, enviado, acompanhado, motivo):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: acompanhado)
    with pytest.raises(HTTPException) as e:
        asyncio.run(pb.pull(_pull(session_id=enviado)))
    assert e.value.status_code == 409 and motivo in e.value.detail
    assert "s1" not in pb._donos and not pb.aguardando("s1")


def test_dono_recusado_volta_quando_a_conversa_converge(monkeypatch):
    # Depois do `/clear` o plugin manda o id novo antes de o Hangar segui-lo: recusa, e não para sempre.
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: UUID)
    with pytest.raises(HTTPException):
        asyncio.run(pb.pull(_pull(session_id="nova")))
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: "nova")
    asyncio.run(pb.pull(_pull(session_id="nova")))
    assert pb._donos["s1"][0] == "a"


def test_conversa_acompanhada_so_vale_com_vinculo_certo(monkeypatch):
    # Palpite por mtime (tracked=False) não identifica conversa: aceitá-lo devolveria o risco.
    from app import api, tmux
    monkeypatch.setattr(tmux, "list_panes_all", lambda: {"s1": [{"name": "s1", "cwd": "/w",
                                                                 "pid": None, "active": True}]})
    resolucao = {"v": ("/c/projects/-w/abc.jsonl", True)}
    monkeypatch.setattr(api.registry, "resolve_tracked",
                        lambda name, cwd: resolucao["v"] if cwd == "/w" else (None, False))
    assert _REAL_TRACKED("s1") == "abc"
    resolucao["v"] = ("/c/projects/-w/abc.jsonl", False)
    assert _REAL_TRACKED("s1") is None
    resolucao["v"] = (None, False)
    assert _REAL_TRACKED("s1") is None


def test_cwd_da_conversa_vem_do_pane_do_agente_e_nao_do_split_ativo(monkeypatch):
    # No Windows a pasta do projeto sai do cwd: um shell ativo noutra pasta apontaria outro transcript.
    from app import agentpane, api, tmux
    monkeypatch.setattr(tmux, "list_panes_all", lambda: {"s1": [
        {"name": "s1", "cwd": "/outra", "pid": 1, "active": True},
        {"name": "s1", "cwd": "/w", "pid": 2, "active": False}]})
    monkeypatch.setattr(agentpane, "_pane_do_agente", lambda pid, children: pid == 2)
    monkeypatch.setattr(api.registry, "resolve_tracked",
                        lambda name, cwd: ("/c/projects/-w/abc.jsonl", True) if cwd == "/w" else (None, False))
    assert _REAL_TRACKED("s1") == "abc"


def test_recusado_com_o_dono_esperando_nao_mexe_no_dono(monkeypatch):
    monkeypatch.setattr(pb, "ESPERA_S", 0.2)

    async def cena():
        dono = asyncio.create_task(pb.pull(_pull(instance="a")))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        antes = pb._donos["s1"]
        with pytest.raises(HTTPException) as e:
            await pb.pull(_pull(instance="b", session_id="outra-conversa"))
        assert e.value.status_code == 409
        assert pb.aguardando("s1") and pb._donos["s1"] == antes
        await dono

    asyncio.run(cena())


def test_recusa_loga_uma_vez_por_instancia_mesmo_com_o_dono_puxando(monkeypatch, caplog):
    monkeypatch.setattr(pb, "ESPERA_S", 0.05)
    caplog.set_level("INFO", logger="hangar.plugin_bridge")
    for _ in range(3):
        asyncio.run(pb.pull(_pull(instance="a")))
        with pytest.raises(HTTPException):
            asyncio.run(pb.pull(_pull(instance="b", session_id="outra-conversa")))
    assert sum("pull recusado" in r.getMessage() for r in caplog.records) == 1


@pytest.mark.parametrize("stdout,returncode,expected", [
    ("", 0, False),
    ("1\t\n", 0, False),
    ("1\t\n1\t\n", 0, False),
    ("0\t/dev/pts/7\n", 0, True),
    ("1\t\n0\t/dev/pts/7\n", 0, True),
    ("\t/dev/pts/7\n", 0, True),
    ("\t\n", 0, True),
    ("#{client_control_mode}\t\n", 0, True),
    ("1\n", 0, True),
    ("", 1, True),
])
def test_terminal_presenca_filtra_somente_controle_comprovado(monkeypatch, stdout, returncode, expected):
    from app import tmux
    from types import SimpleNamespace
    def run(args):
        assert args == ["tmux", "list-clients", "-t", "=s1", "-F", "#{client_control_mode}\t#{client_tty}"]
        return SimpleNamespace(stdout=stdout, returncode=returncode)
    monkeypatch.setattr(tmux, "_run", run)
    assert pb.terminal_preso("s1") is expected


def test_terminal_presenca_erro_do_multiplexador_e_conservador(monkeypatch):
    from app import tmux
    def run(args):
        raise OSError("fixture")
    monkeypatch.setattr(tmux, "_run", run)
    assert pb.terminal_preso("s1") is True


@pytest.mark.parametrize("stdout,expected", [(b"1\t\n", False), (b"\xff\t\n", True), (None, True), ("\n", True), ("1\t\textra\n", True)])
def test_terminal_presenca_bytes_e_formato_invalido(monkeypatch, stdout, expected):
    from app import tmux
    from types import SimpleNamespace
    monkeypatch.setattr(tmux, "_run", lambda args: SimpleNamespace(stdout=stdout, returncode=0))
    assert pb.terminal_preso("s1") is expected
