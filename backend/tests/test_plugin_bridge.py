"""Ponte do plugin de function hooks: a regra de quem fica com o pedido de permissão e a ida e volta
da resposta do app. O resto (envio por `fill`) depende de tmux e é conferido no uso real."""
import asyncio
import json
import threading
import time
from pathlib import Path

import pytest
from fastapi import HTTPException

from app import plugin_bridge as pb


# Conversa que o Hangar acompanha em toda sessão dos testes; o plugin certo manda este id.
UUID = "0b6e5c1a-1111-4222-8333-444455556666"
_REAL_TRACKED = pb.tracked_session_id
_REAL_MODO_SEM_DIALOGO = pb.modo_sem_dialogo


async def _com_dialogo(name: str) -> bool:
    return False


@pytest.fixture(autouse=True)
def _limpa(monkeypatch):
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: UUID)
    # Sem isto, cada pedido de permissão dos testes capturaria um pane de verdade.
    monkeypatch.setattr(pb, "modo_sem_dialogo", _com_dialogo)
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


def test_token_com_chave_separa_processos_do_mesmo_nome_e_confere_pelo_nome():
    # Dois processos que nasceram `s1` (um renomeado, outro novo) têm tokens diferentes; a chave vai no
    # token, e a conferência refaz o HMAC sem nada guardado.
    a, b = pb.mint("s1", "k1"), pb.mint("s1", "k2")
    assert a.startswith("k1.") and a != b
    pb._confere("s1", a)
    pb._confere("s1", pb.mint("s1"))     # processo lançado antes da chave
    for nome, token in (("s2", a), ("s1", "k1." + "0" * 32), ("s1", "k2" + a[2:])):
        with pytest.raises(HTTPException) as e:
            pb._confere(nome, token)
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


@pytest.mark.parametrize("rodape,segura", [
    ("⏵⏵ auto mode on (shift+tab to cycle) · ← for agents", False),
    ("⏵⏵ don't ask on (shift+tab to cycle)", False),
    ("⏸ manual mode on · ← for agents", True),
    ("⏵⏵ accept edits on (shift+tab to cycle)", True),
    ("", True),
])
def test_permissao_so_vai_ao_app_quando_o_modo_pergunta(monkeypatch, rodape, segura):
    from app import state

    async def quadro(name, max_age):
        return f"❯ \n{rodape}\n"

    monkeypatch.setattr(pb, "modo_sem_dialogo", _REAL_MODO_SEM_DIALOGO)
    monkeypatch.setattr(state, "shared_capture", quadro)
    monkeypatch.setattr(pb, "terminal_preso", lambda name: False)
    pb.app_entrou()
    resposta = asyncio.run(pb.ask(_corpo("perm:t1", tool="Bash", janela_ms=50)))
    assert resposta == ({"answers": None} if segura else {"soltar": True})
    assert (pb.pergunta_pendente("s1") is not None) is segura


def test_pergunta_do_hook_morto_sai_sem_esperar_o_teto_do_long_poll(monkeypatch):
    # Item 16: o Esc no terminal interrompe o AskUserQuestion e o hook morre sem o `/ask-fim`. A
    # pergunta seguia "aberta" por 35 s e o /clear do app era adiado nesse intervalo.
    monkeypatch.setattr(pb, "terminal_preso", lambda name: False)
    pb.app_entrou()
    asyncio.run(pb.ask(_corpo("ask:t1", questions=[{"question": "A ou B?"}], janela_ms=20)))
    assert pb.pergunta_pendente("s1") is not None
    pb._perguntas["s1"]["visto"] -= pb.SEM_POLL_S + 1
    assert pb.pergunta_pendente("s1") is None
    # Com o long-poll aberto a pergunta vale até o teto de sempre.
    pb._perguntas["s1"]["fila"] = asyncio.Queue()
    assert pb.pergunta_pendente("s1") is not None


def test_interrupcao_pelo_app_solta_a_pergunta_na_hora_e_acorda_o_long_poll(monkeypatch):
    # O /interrupt do app é o Esc que fecha o diálogo; o long-poll do hook morto ficava aberto até a
    # janela de 25 s fechar, segurando a pergunta.
    monkeypatch.setattr(pb, "terminal_preso", lambda name: False)
    pb.app_entrou()

    async def cena():
        espera = asyncio.create_task(pb.ask(_corpo("ask:t1", questions=[{"question": "A ou B?"}])))
        while pb.pergunta_pendente("s1") is None:
            await asyncio.sleep(0.01)
        pb.interrompeu("s1", "ask:t1")
        assert pb.pergunta_pendente("s1") is None
        return await asyncio.wait_for(espera, 2)

    assert asyncio.run(cena()) == {"answers": None}
    # O hook que refaz o poll logo depois do Esc ainda pode estar morrendo: não volta.
    asyncio.run(pb.ask(_corpo("ask:t1", questions=[{"question": "A ou B?"}], janela_ms=20)))
    assert pb.pergunta_pendente("s1") is None
    # Ainda perguntando depois de HOOK_VIVO_S, ele sobreviveu ao Esc: a pergunta volta a contar.
    pb._perguntas["s1"]["interrompida"] -= pb.HOOK_VIVO_S + 1
    asyncio.run(pb.ask(_corpo("ask:t1", questions=[{"question": "A ou B?"}], janela_ms=20)))
    assert pb.pergunta_pendente("s1") is not None
    # Interrupção de outra pergunta (lida antes do Esc) não marca a que abriu depois.
    pb.interrompeu("s1", "ask:t0")
    assert pb.pergunta_pendente("s1") is not None


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
    # Desligado, a sessão nasce byte a byte como antes: sem flag, sem env. É a promessa do fallback.
    from app.adapters import get_adapter
    monkeypatch.setattr(pb, "ligado", lambda: False)
    assert pb.raizes_dos_plugins() == [] and pb.env_da_sessao("s1", "k1") == {}
    assert get_adapter("claude").spawn_command("/tmp/p", "sid") == ["claude", "--session-id", "sid"]

    monkeypatch.setattr(pb, "ligado", lambda: True)
    # Os outros mods do repo vêm depois; a ordem completa é do teste com pasta temporária.
    raiz = pb.raizes_dos_plugins()[0]
    assert Path(raiz).parts[-2:] == ("plugins", "hangar")
    assert get_adapter("claude").spawn_command("/tmp/p", "sid")[:5] == [
        "claude", "--session-id", "sid", "--plugin-dir", raiz]
    env = pb.env_da_sessao("s1", "k1")
    assert env["HANGAR_PLUGIN_TOKEN"] == pb.mint("s1", "k1") and env["HANGAR_PLUGIN_URL"].endswith("/api/plugin")


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


def test_plugin_entra_por_plugin_dir_mesmo_com_mods_por_padrao(monkeypatch):
    # Pela pasta de skills ele ficaria abaixo dos plugins do marketplace e não veria a faixa deles.
    monkeypatch.setattr(pb, "ligado", lambda: True)
    for mods in (True, False):
        monkeypatch.setattr(pb, "mods_by_default", lambda mods=mods: mods)
        assert pb.raizes_dos_plugins()[0] == str(pb.PLUGIN_SRC)


def test_hangar_abre_a_lista_dos_mods_e_pasta_sem_manifesto_fica_fora(tmp_path, monkeypatch):
    # O do Hangar fica por fora na cadeia e repassa a faixa ao app: "aaa" vem antes dele no
    # alfabeto e ainda assim entra depois.
    for nome in ("zzz", "aaa", "hangar"):
        (tmp_path / nome / ".claude-plugin").mkdir(parents=True)
        (tmp_path / nome / ".claude-plugin" / "plugin.json").write_text("{}")
    (tmp_path / "sem-manifesto" / "hooks").mkdir(parents=True)
    monkeypatch.setattr(pb, "PLUGINS_ROOT", tmp_path)
    monkeypatch.setattr(pb, "PLUGIN_SRC", tmp_path / "hangar")
    monkeypatch.setattr(pb, "ligado", lambda: True)
    raizes = [str(tmp_path / n) for n in ("hangar", "aaa", "zzz")]
    assert pb.raizes_dos_plugins() == raizes

    from app.adapters import get_adapter
    argv = get_adapter("claude").spawn_command("/tmp/p", "sid")
    assert argv[:9] == ["claude", "--session-id", "sid",
                        "--plugin-dir", raizes[0], "--plugin-dir", raizes[1], "--plugin-dir", raizes[2]]

    home = tmp_path / "home"
    pb._publish_plugin_dir(home)
    assert pb.plugin_dir_file(home).read_text(encoding="utf-8") == "".join(f"{r}\n" for r in raizes)


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


def test_plugin_dir_is_published_for_the_shell_wrapper(tmp_path, monkeypatch):
    monkeypatch.setattr(pb, "raizes_dos_plugins", lambda: ["/repo com espaço/plugins/hangar"])
    pb._publish_plugin_dir(tmp_path)
    # Uma linha só e sem BOM: o wrapper lê com `read` do shell.
    assert pb.plugin_dir_file(tmp_path).read_bytes() == "/repo com espaço/plugins/hangar\n".encode()


def test_plugin_dir_file_is_removed_when_mods_are_off(tmp_path, monkeypatch):
    monkeypatch.setattr(pb, "raizes_dos_plugins", lambda: ["/repo/plugins/hangar"])
    pb._publish_plugin_dir(tmp_path)
    monkeypatch.setattr(pb, "raizes_dos_plugins", lambda: [])
    pb._publish_plugin_dir(tmp_path)
    assert not pb.plugin_dir_file(tmp_path).exists()


@pytest.fixture
def ligado(monkeypatch):
    monkeypatch.setattr(pb, "ligado", lambda: True)
    # Sem vínculo terminal provado nos testes: o token sai só do nome.
    monkeypatch.setattr(pb, "_terminal_key", lambda nome: None)


def test_whoami_poe_no_token_a_chave_do_vinculo_terminal(monkeypatch, ligado):
    # A ponte do Rust acha a sessão aberta pelo wrapper do shell pela chave com que ele a abre.
    from app import quem_chama
    monkeypatch.setattr(quem_chama, "_por_pane", lambda pane: "s1" if pane == "%3" else None)
    monkeypatch.setattr(pb, "_terminal_key", lambda nome: "terminal_ab" if nome == "s1" else None)
    r = asyncio.run(pb.whoami(pb.WhoamiBody(chave=pb.machine_key(), pane="%3", session_id=UUID)))
    assert r == {"sessao": "s1", "token": pb.mint("s1", "terminal_ab"), "origem": "pane"}


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


def test_state_publishes_measured_context_for_its_conversation(tmp_path, monkeypatch):
    from app import api, tmux
    from app.registry import SessionRegistry
    transcript = tmp_path / f"{UUID}.jsonl"
    transcript.write_text("", encoding="utf-8")
    monkeypatch.setattr(tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(api.registry, "resolve_tracked", lambda *args: (str(transcript), True))
    monkeypatch.setattr(SessionRegistry, "_context_cache", {"s1": (0, str(transcript), None, None)})
    body = pb.StateBody(sessao="s1", token=pb.mint("s1"), estado="idle", session_id=UUID,
                        context={"used": 76_604, "window": 1_000_000})
    asyncio.run(pb.state(body, None))
    assert json.loads(transcript.with_suffix(".context.json").read_text()) == {"used": 76_604, "window": 1_000_000}
    assert "s1" not in SessionRegistry._context_cache


def test_state_rejects_context_from_another_conversation(tmp_path, monkeypatch):
    from app import api, tmux
    transcript = tmp_path / f"{UUID}.jsonl"
    transcript.write_text("", encoding="utf-8")
    monkeypatch.setattr(tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(api.registry, "resolve_tracked", lambda *args: (str(transcript), True))
    body = pb.StateBody(sessao="s1", token=pb.mint("s1"), estado="idle", session_id="outro",
                        context={"used": 76_604, "window": 1_000_000})
    with pytest.raises(HTTPException) as error:
        asyncio.run(pb.state(body, None))
    assert error.value.status_code == 409
    assert not transcript.with_suffix(".context.json").exists()


def test_rejected_measurement_still_updates_state_and_wakes_listeners(tmp_path, monkeypatch):
    from app import api, tmux
    transcript = tmp_path / f"{UUID}.jsonl"
    transcript.write_text("", encoding="utf-8")
    monkeypatch.setattr(tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(api.registry, "resolve_tracked", lambda *args: (str(transcript), True))
    wakeups, notifications = [], []
    monkeypatch.setattr(pb, "_acordar", wakeups.append)
    monkeypatch.setattr(pb.state_facts, "notify", lambda *args: notifications.append(args))
    body = pb.StateBody(sessao="s1", token=pb.mint("s1"), estado="idle", session_id="outro",
                        context={"used": 76_604, "window": 1_000_000})
    with pytest.raises(HTTPException):
        asyncio.run(pb.state(body, None))
    assert pb.estado_recente("s1") == ("idle", None)
    assert wakeups == ["s1"]
    assert notifications == [("s1", pb.state_facts.FORCE)]
    assert not transcript.with_suffix(".context.json").exists()


def test_context_write_failure_keeps_state_update_and_existing_cache(tmp_path, monkeypatch):
    from app import api, tmux, claude_context
    from app.registry import SessionRegistry
    transcript = tmp_path / f"{UUID}.jsonl"
    transcript.write_text("", encoding="utf-8")
    monkeypatch.setattr(tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(api.registry, "resolve_tracked", lambda *args: (str(transcript), True))
    cache = {"s1": (0, str(transcript), None, None)}
    monkeypatch.setattr(SessionRegistry, "_context_cache", cache)

    def failed_write(*_args):
        raise OSError("falha de escrita simulada")

    monkeypatch.setattr(claude_context, "publish", failed_write)
    body = pb.StateBody(sessao="s1", token=pb.mint("s1"), estado="idle", session_id=UUID,
                        context={"used": 76_604, "window": 1_000_000})
    assert asyncio.run(pb.state(body, None)) == {"ok": True}
    assert pb.estado_recente("s1") == ("idle", None)
    assert "s1" in cache


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
        assert (await asyncio.wait_for(pull, 5)) == {"text": "oi", "modo": "user", "faixa": False}
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


# Faixa, painéis e confirmações de clique dos mods.

def _ponte(sessao, **extra):
    return {"sessao": sessao, "token": pb.mint(sessao), **extra}


def _cliente():
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    app = FastAPI()
    app.include_router(pb.plugin_router)
    return TestClient(app, client=("127.0.0.1", 5000))


def test_faixa_guarda_paineis_e_largura():
    c = _cliente()
    try:
        r = c.post("/api/plugin/ui", json=_ponte("pane-a", above={"type": "Box"}, columns=87, panes=[
            {"id": "review-mr", "title": "Review !577", "placement": "dock", "columns": 72, "tree": {"type": "Box"}}]))
        assert r.status_code == 200
        versao, payload = pb.band("pane-a")
        assert versao > 0
        assert payload["above"] == {"type": "Box"}
        assert [p["id"] for p in payload["panes"]] == ["review-mr"]
        assert "columns" not in payload  # a largura é da prévia, não do app
        assert pb.band_columns("pane-a") == 87
        assert pb.transcript_columns("pane-a") == 87
    finally:
        pb.esquecer("pane-a")


def test_sem_painel_ancorado_a_previa_nao_corta():
    c = _cliente()
    try:
        c.post("/api/plugin/ui", json=_ponte("pane-b", above=None, columns=160, panes=[
            {"id": "x", "title": "x", "placement": "inline", "columns": 150, "tree": None}]))
        assert pb.transcript_columns("pane-b") is None
    finally:
        pb.esquecer("pane-b")


def test_mod_toast_stays_until_it_expires_and_carries_the_time_left(monkeypatch):
    c = _cliente()
    agora = [1000.0]
    monkeypatch.setattr(pb.time, "monotonic", lambda: agora[0])
    try:
        r = c.post("/api/plugin/toast", json=_ponte("aviso-a", text="Jenkins configurado.", timeoutMs=9000, plugin="demo"))
        assert r.status_code == 200
        agora[0] += 2
        ultimo, avisos = pb.toasts_after("aviso-a", 0)
        assert [(a["text"], a["plugin"], a["timeoutMs"]) for a in avisos] == [("Jenkins configurado.", "demo", 7000)]
        # Quem já viu este não o recebe de novo; quem conecta do zero recebe.
        assert pb.toasts_after("aviso-a", ultimo) == (ultimo, [])
        agora[0] += 8
        assert pb.toasts_after("aviso-a", 0) == (ultimo, [])
    finally:
        pb.esquecer("aviso-a")


def test_mod_toast_defaults_to_the_terminal_timeout_and_clips_long_text():
    c = _cliente()
    try:
        c.post("/api/plugin/toast", json=_ponte("aviso-b", text="x" * 5000))
        c.post("/api/plugin/toast", json=_ponte("aviso-b", text="   "))
        _, avisos = pb.toasts_after("aviso-b", 0)
        assert len(avisos) == 1
        assert len(avisos[0]["text"]) == pb.TOAST_MAX_CHARS
        assert avisos[0]["plugin"] == ""
        assert 0 < avisos[0]["timeoutMs"] <= pb.TOAST_DEFAULT_MS
    finally:
        pb.esquecer("aviso-b")


def test_mod_toast_requires_the_session_token():
    r = _cliente().post("/api/plugin/toast", json={"sessao": "aviso-c", "token": "errado", "text": "oi"})
    assert r.status_code == 403
    assert pb.toasts_after("aviso-c", 0) == (0, [])


@pytest.mark.asyncio
async def test_wait_toasts_wakes_when_a_toast_arrives():
    async def _chega():
        await asyncio.sleep(0.05)
        pb._store_toast("aviso-d", "oi", None, "mod")

    chegada = asyncio.create_task(_chega())
    try:
        ultimo, avisos = await asyncio.wait_for(pb.wait_toasts("aviso-d", 0, 5), timeout=2)
        assert ultimo > 0
        assert [a["text"] for a in avisos] == ["oi"]
    finally:
        chegada.cancel()
        pb.esquecer("aviso-d")


def test_sem_faixa_devolve_payload_vazio():
    assert pb.band("nunca-desenhou") == (0, {"above": None, "panes": []})


@pytest.mark.asyncio
async def test_esperar_faixa_acorda_quando_muda():
    try:
        vista = pb.band("pane-c")[0]
        espera = asyncio.create_task(pb.esperar_faixa("pane-c", vista, 5))
        await asyncio.sleep(0.05)
        assert not espera.done()
        # o mesmo caminho do POST /ui, chamado no loop do teste, onde o Event vive
        pb._guardar_faixa("pane-c", {"type": "Box"}, 80, [])
        assert await asyncio.wait_for(espera, 1) != vista
    finally:
        pb.esquecer("pane-c")


@pytest.mark.asyncio
async def test_confirmacao_de_clique_casa_site_e_chave_depois_do_clique():
    try:
        await pb.pressed(pb.PressBody(**_ponte("pane-d", requestId="above-prompt", element="velho")))
        desde = time.monotonic()
        assert not await pb.esperar_press("pane-d", "above-prompt", "velho", desde, 0.05)
        await pb.pressed(pb.PressBody(**_ponte("pane-d", requestId="review-mr", element="cp-1")))
        assert await pb.esperar_press("pane-d", "review-mr", "cp-1", desde, 0.5)
        assert not await pb.esperar_press("pane-d", "above-prompt", "cp-1", desde, 0.05)
        tentativa = pb.esperar_clique_do_app("pane-d", "review-mr", "cp-1", 2)
        await pb.copied(pb.CopiedBody(**_ponte("pane-d", attempt=tentativa, text="https://gitlab.exemplo/mr/1")))
        assert await pb.esperar_efeito("pane-d", tentativa, 0.5) == ("https://gitlab.exemplo/mr/1", None)
    finally:
        pb.esquecer("pane-d")


def test_confirmacao_com_token_errado_e_recusada():
    r = _cliente().post("/api/plugin/pressed", json={"sessao": "pane-e", "token": "x", "requestId": "a", "element": "b"})
    assert r.status_code == 403


def test_press_start_responde_sim_uma_vez_para_o_clique_esperado():
    c = _cliente()
    try:
        tentativa = pb.esperar_clique_do_app("pane-f", "above-prompt", "rv-1", 2)
        corpo = _ponte("pane-f", requestId="above-prompt", element="rv-1")
        assert c.post("/api/plugin/press-start", json=corpo).json() == {"fromApp": True, "attempt": tentativa}
        assert c.post("/api/plugin/press-start", json=corpo).json() == {"fromApp": False, "attempt": None}
        pb.esperar_clique_do_app("pane-f", "above-prompt", "rv-1", 2)
        outro = _ponte("pane-f", requestId="above-prompt", element="outro")
        assert c.post("/api/plugin/press-start", json=outro).json() == {"fromApp": False, "attempt": None}
    finally:
        pb.esquecer("pane-f")


def test_opened_so_aceita_http():
    c = _cliente()
    try:
        tentativa = pb.esperar_clique_do_app("pane-g", "above-prompt", "a", 2)
        assert c.post("/api/plugin/opened", json=_ponte("pane-g", attempt=tentativa, url="file:///etc/passwd")).status_code == 400
        assert c.post("/api/plugin/opened", json=_ponte("pane-g", attempt=tentativa, url="https://x.exemplo")).status_code == 200
    finally:
        pb.esquecer("pane-g")


def test_ancora_da_faixa_considera_o_rotulo_do_botao():
    # Faixa que começa por botão: o rótulo é a primeira linha desenhada, não o texto que vem depois.
    try:
        pb._guardar_faixa("ancora", {"type": "Box", "children": [
            {"type": "Button", "props": {"key": "a", "label": "[ abrir sonda ]"}},
            {"type": "Text", "children": [" Promedico"]}]}, 80, [])
        assert pb.band_anchor("ancora") == "[ abrir sonda ]"
    finally:
        pb.esquecer("ancora")


@pytest.mark.asyncio
async def test_efeito_so_vale_para_a_tentativa_aberta():
    # A cópia atrasada de um clique não pode cair no clique seguinte (outro aparelho, outro convidado).
    try:
        t1 = pb.esperar_clique_do_app("pane-h", "above-prompt", "a", 2)
        assert (await pb.press_start(pb.PressBody(**_ponte("pane-h", requestId="above-prompt", element="a"))))["attempt"] == t1
        pb.encerrar_clique_do_app("pane-h", t1)
        t2 = pb.esperar_clique_do_app("pane-h", "above-prompt", "b", 2)
        with pytest.raises(HTTPException) as e:
            await pb.copied(pb.CopiedBody(**_ponte("pane-h", attempt=t1, text="do clique 1")))
        assert e.value.status_code == 409
        await pb.copied(pb.CopiedBody(**_ponte("pane-h", attempt=t2, text="do clique 2")))
        assert await pb.esperar_efeito("pane-h", t2, 0.1) == ("do clique 2", None)
        assert await pb.esperar_efeito("pane-h", t1, 0.05) == (None, None)
    finally:
        pb.esquecer("pane-h")


@pytest.mark.asyncio
async def test_efeito_depois_da_resposta_volta_para_o_terminal():
    try:
        t = pb.esperar_clique_do_app("pane-i", "above-prompt", "a", 2)
        pb.encerrar_clique_do_app("pane-i", t)
        with pytest.raises(HTTPException) as e:
            await pb.opened(pb.OpenedBody(**_ponte("pane-i", attempt=t, url="https://x.exemplo")))
        assert e.value.status_code == 409
    finally:
        pb.esquecer("pane-i")


def test_pull_diz_se_o_backend_tem_a_faixa(monkeypatch):
    # Backend reiniciado começa sem a faixa: o `/pull` avisa, e o plugin reenvia sem esperar redesenho.
    monkeypatch.setattr(pb, "ESPERA_S", 0.01)
    try:
        assert asyncio.run(pb.pull(_pull(instance="a")))["faixa"] is False
        pb._guardar_faixa("s1", None, 80, [])
        assert asyncio.run(pb.pull(_pull(instance="a")))["faixa"] is True
    finally:
        pb.esquecer("s1")


def test_parada_solta_as_esperas_longas_na_hora(monkeypatch):
    # O uvicorn espera cada pedido aberto antes do lifespan: uma espera de 25 s passava do teto do
    # systemd e o backend saía por SIGKILL.
    monkeypatch.setattr(pb, "terminal_preso", lambda name: False)
    monkeypatch.setattr(pb, "_stopping", False)
    pb.app_entrou()

    async def cena():
        pull = asyncio.create_task(pb.pull(_pull(instance="a")))
        ask = asyncio.create_task(pb.ask(_corpo("ask:q1")))
        while "s1" not in pb._waiters or not pb._perguntas.get("s1", {}).get("fila"):
            await asyncio.sleep(0)
        inicio = time.monotonic()
        pb.stop_waits()
        assert (await pull)["text"] is None
        assert await ask == {"answers": None}
        assert time.monotonic() - inicio < 1
        # Pedido que chega já na parada também não espera.
        assert (await asyncio.wait_for(pb.pull(_pull(instance="a")), 1))["text"] is None

    try:
        asyncio.run(cena())
    finally:
        pb.esquecer("s1")


def test_sinal_de_parada_avisa_as_esperas(monkeypatch):
    import uvicorn
    from app import rust_server
    chamadas = []
    monkeypatch.setattr(pb, "stop_waits", lambda: chamadas.append("stop"))
    # A classe-mãe de verdade ligaria a parada global do sse-starlette para os testes seguintes.
    monkeypatch.setattr(uvicorn.Server, "handle_exit", lambda self, sig, frame: chamadas.append("uvicorn"))
    rust_server.Server(uvicorn.Config(lambda *a: None)).handle_exit(15, None)
    assert chamadas == ["stop", "uvicorn"]


def test_texto_que_chega_junto_com_a_parada_ainda_e_entregue(monkeypatch):
    monkeypatch.setattr(pb, "_stopping", False)

    async def cena():
        pull = asyncio.create_task(pb.pull(_pull(instance="a")))
        while "s1" not in pb._waiters:
            await asyncio.sleep(0)
        fila = pb._waiters["s1"]
        fila.put_nowait(pb._STOP)
        fila.put_nowait({"text": "chegou junto", "modo": "fill"})
        assert (await pull)["text"] == "chegou junto"
        pb.stop_waits()
        # Depois do sinal a entrega não vai para uma espera que vai fechar: o chamador usa o pane.
        assert pb._entregar("s1", "depois", "fill") is False

    try:
        asyncio.run(cena())
    finally:
        pb.esquecer("s1")


def test_pull_drops_its_wait_when_the_proxy_connection_dies(monkeypatch):
    # O Rust que repassava a espera morreu: sem soltar, o Rust novo publicava nela e a confirmação
    # nunca vinha (entrega incerta no terminal depois de uma queda).
    monkeypatch.setattr(pb, "ESPERA_S", 30)

    class Proxy:
        def __init__(self):
            self.gone = asyncio.Event()

        async def receive(self):
            await self.gone.wait()
            return {"type": "http.disconnect"}

    async def cena():
        proxy = Proxy()
        espera = asyncio.create_task(pb.pull(_pull(), proxy))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        proxy.gone.set()
        resposta = await asyncio.wait_for(espera, 2)
        assert resposta["text"] is None
        assert not pb.aguardando("s1"), "a espera morta não pode receber publicação"

    asyncio.run(cena())


def test_publication_racing_the_proxy_drop_is_not_written(monkeypatch):
    # Publicação que chegou junto com a queda: o plugin nunca a recebeu, então volta `not_written`
    # (o Rust digita pelo teclado), nunca `unknown` (que travaria a fila como entrega incerta).
    monkeypatch.setattr(pb, "ESPERA_S", 30)
    monkeypatch.setattr(pb, "PUBLICA_S", 5)
    monkeypatch.setattr(pb, "tracked_session_id", lambda name: UUID)

    class Proxy:
        def __init__(self):
            self.gone = asyncio.Event()

        async def receive(self):
            await self.gone.wait()
            return {"type": "http.disconnect"}

    async def cena():
        proxy = Proxy()
        espera = asyncio.create_task(pb.pull(_pull(modos=("fill", "receipt_v2")), proxy))
        await asyncio.wait_for(_ate(lambda: pb.aguardando("s1")), 5)
        fila = pb._waiters["s1"]
        original = fila.put_nowait
        def put_and_drop(item):
            original(item)
            proxy.gone.set()        # a conexão cai no mesmo instante da publicação
        fila.put_nowait = put_and_drop
        resultado = await asyncio.to_thread(pb.publish_terminal, "s1", UUID, 1,
            {"id":"pub-1", "mode":"fill", "text":"oi"}, lambda: None)
        await asyncio.wait_for(espera, 2)
        return resultado

    assert asyncio.run(cena()) == "not_written"


def test_user_publication_waits_for_ack_after_slow_prompt_hooks(monkeypatch):
    # O aviso do modo `user` sai depois dos hooks do UserPromptSubmit: com a máquina ocupada ele
    # passa do prazo do rascunho, e a entrega que chegou não pode voltar como incerta.
    monkeypatch.setattr(pb, "CONFIRMA_S", .05)

    async def cena():
        pb._loop = asyncio.get_running_loop()
        fila = pb._waiters["s1"] = asyncio.Queue()
        pb._donos["s1"] = ("i", {"fill", "user", "receipt_v2"}, time.monotonic())
        envio = asyncio.create_task(asyncio.to_thread(pb.publish_terminal, "s1", UUID, 1,
            {"id": "pub-1", "mode": "user", "text": "oi"}, lambda: None))
        await asyncio.wait_for(fila.get(), 1)
        await asyncio.sleep(.3)         # hooks lentos antes do `/submitted`
        assert pb._terminal_ack(pb.SubmittedBody(sessao="s1", token="t", ok=True,
            publication_id="pub-1", generation=1, session_id=UUID), "user")
        return await envio

    try:
        assert asyncio.run(cena()) == "accepted"
    finally:
        pb._publications.clear()


def test_publication_wait_stays_below_rust_policy_timeout():
    # Se o Rust desistir antes do Python, o aviso que chega no intervalo vira entrega incerta.
    import re
    actor = (Path(__file__).resolve().parents[2] / "crates/hangar-server/src/runtime/actor.rs").read_text()
    rust_s = int(re.search(r"PUBLISH_POLICY_TIMEOUT:Duration=Duration::from_secs\((\d+)\)", actor).group(1))
    assert pb.CONFIRMA_S < pb.PUBLICA_S < rust_s - 5
    # O `op` do Python espera a entrada inteira no Rust: fatos (15 s) mais a publicação.
    from app import rust_server
    assert rust_server.OP_TIMEOUT_S > rust_s + 15
