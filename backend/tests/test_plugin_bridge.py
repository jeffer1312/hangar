"""Ponte do plugin de function hooks: a regra de quem fica com o pedido de permissão e a ida e volta
da resposta do app. O resto (envio por `fill`) depende de tmux e é conferido no uso real."""
import asyncio
import json
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
              pb._donos):
        d.clear()
    pb._apps_abertos = 0


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


def test_rota_atende_pelo_nome_atual_da_sessao_do_processo(monkeypatch):
    # Renomeada sem relançar, o plugin segue mandando o nome de nascimento: o `/pull` e as outras rotas
    # usam o nome de agora, achado pela chave do token (a do runtime ou a do lançamento, `plugin_key`).
    from types import SimpleNamespace
    from app import runtime_coordinator
    slot = lambda meta: SimpleNamespace(binding=SimpleNamespace(meta=meta))
    fake = SimpleNamespace(names={"novo": "terminal_ab", "outra": "k-outra"},
                           slots={"terminal_ab": slot({"plugin_key": "lanc"}), "k-outra": slot({})})
    monkeypatch.setattr(runtime_coordinator, "_current", fake)
    for token, esperado in ((pb.mint("velho", "lanc"), "novo"), (pb.mint("velho", "terminal_ab"), "novo"),
                            (pb.mint("velho", "k-outra"), "outra"), (pb.mint("velho", "sumida"), "velho"),
                            (pb.mint("velho"), "velho")):
        body = pb.ToastBody.model_construct(sessao="velho", token=token)
        pb._entra(body)
        assert body.sessao == esperado, token
    with pytest.raises(HTTPException):
        pb._entra(pb.ToastBody.model_construct(sessao="outro", token=pb.mint("velho", "lanc")))


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


def test_confirmacao_com_token_errado_e_recusada():
    r = _cliente().post("/api/plugin/pressed", json={"sessao": "pane-e", "token": "x", "requestId": "a", "element": "b"})
    assert r.status_code == 403


def test_press_start_nunca_e_clique_do_app_aqui():
    # O clique do app é do Rust; o press que chega ao Python começou no terminal.
    c = _cliente()
    try:
        corpo = _ponte("pane-f", requestId="above-prompt", element="rv-1")
        assert c.post("/api/plugin/press-start", json=corpo).json() == {"fromApp": False, "attempt": None}
    finally:
        pb.esquecer("pane-f")


def test_opened_so_aceita_http():
    c = _cliente()
    try:
        assert c.post("/api/plugin/opened", json=_ponte("pane-g", attempt="a", url="file:///etc/passwd")).status_code == 400
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


def test_um_efeito_de_mod_sempre_volta_para_o_terminal():
    # O clique do app é do Rust: a cópia e a abertura que chegam aqui acontecem no terminal (409).
    c = _cliente()
    try:
        assert c.post("/api/plugin/copied", json=_ponte("pane-i", attempt="a", text="x")).status_code == 409
        assert c.post("/api/plugin/opened", json=_ponte("pane-i", attempt="a", url="https://x.exemplo")).status_code == 409
    finally:
        pb.esquecer("pane-i")


def test_sinal_de_parada_avisa_as_esperas(monkeypatch):
    import uvicorn
    from app import rust_server
    chamadas = []
    monkeypatch.setattr(pb, "stop_waits", lambda: chamadas.append("stop"))
    # A classe-mãe de verdade ligaria a parada global do sse-starlette para os testes seguintes.
    monkeypatch.setattr(uvicorn.Server, "handle_exit", lambda self, sig, frame: chamadas.append("uvicorn"))
    rust_server.Server(uvicorn.Config(lambda *a: None)).handle_exit(15, None)
    assert chamadas == ["stop", "uvicorn"]


def test_publication_waits_fit_inside_the_rust_op():
    # A confirmação cabe na publicação, e o `op` do Python espera a entrada inteira no Rust: fatos
    # (15 s) mais a publicação.
    assert pb.CONFIRMA_S < pb.PUBLICA_S
    from app import rust_server
    assert rust_server.OP_TIMEOUT_S > pb.PUBLICA_S + 15
