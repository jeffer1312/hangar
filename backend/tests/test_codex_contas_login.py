"""Login nativo das contas Codex, sem usar o OAuth legado."""

from __future__ import annotations

import asyncio
import copy
import json
from pathlib import Path

import pytest

from app import codex_contas as accounts
from app.codex_contas_login import CodexContasLogin
from app.codex_importador import CodexNativo


class FakeNative:
    instances = []
    complete_before_response = False
    auth_by_home = {}
    start_delay = 0.0
    close_error = False
    complete_without_login_id = False
    # Quantas leituras de conta ainda saem vazias depois do `success` (a corrida do Codex real).
    leituras_vazias = 0

    def __init__(self, home, codex_home, binario="codex", *, account=None, **kwargs):
        self.home = Path(home)
        self.codex_home = Path(codex_home)
        self.account = account
        self.requests = []
        self.listeners = {}
        self.closed = False
        self.login_id = "native-login"
        self.__class__.instances.append(self)

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        await self.close()

    def subscribe(self, method):
        queue = asyncio.Queue()
        self.listeners.setdefault(method, set()).add(queue)
        return queue

    def unsubscribe(self, method, queue):
        self.listeners.get(method, set()).discard(queue)

    def _notify(self, method, params):
        message = {"jsonrpc": "2.0", "method": method, "params": copy.deepcopy(params)}
        for queue in tuple(self.listeners.get(method, ())):
            queue.put_nowait(message)

    async def request(self, method, params, timeout=None):
        self.requests.append((method, params, str(self.codex_home)))
        if method == "initialize":
            return {}
        if method == "account/login/start":
            if self.start_delay:
                await asyncio.sleep(self.start_delay)
            if self.complete_before_response:
                params = {"success": True, "error": None}
                if not self.complete_without_login_id:
                    params["loginId"] = self.login_id
                self._notify("account/login/completed", params)
            return {
                "type": "chatgptDeviceCode", "loginId": self.login_id,
                "verificationUrl": "https://auth.openai.com/device", "userCode": "ABCD-EFGH",
            }
        if method == "account/login/cancel":
            return {"status": "canceled"}
        if method == "account/read":
            if type(self).leituras_vazias > 0:
                type(self).leituras_vazias -= 1
                return {"account": None}
            return copy.deepcopy(self.auth_by_home.get(str(self.codex_home), {"account": None}))
        raise AssertionError(method)

    async def close(self):
        self.closed = True
        for queues in self.listeners.values():
            for queue in queues:
                queue.put_nowait(None)
        if self.close_error:
            raise RuntimeError("close failed")


@pytest.fixture
def contas(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(accounts, "_DEFAULT_HOME", tmp_path / ".codex")
    accounts.default_home().mkdir()
    work = accounts.create_account("work")
    (work.home / "config.toml").write_text('cli_auth_credentials_store = "file"\n', encoding="utf-8")
    FakeNative.instances = []
    FakeNative.complete_before_response = False
    FakeNative.start_delay = 0.0
    FakeNative.close_error = False
    FakeNative.complete_without_login_id = False
    FakeNative.leituras_vazias = 0
    FakeNative.auth_by_home = {
        str(accounts.default_home()): {"account": {"type": "chatgpt", "email": "a@x", "planType": "plus"}},
        str(work.home): {"account": {"type": "chatgpt", "email": "b@x", "planType": "pro"}},
    }
    return accounts.Account("default", accounts.default_home(), True), work


@pytest.fixture
def service(monkeypatch):
    monkeypatch.setattr("app.codex_contas_login.CodexNativo", FakeNative)
    return CodexContasLogin(native=FakeNative)


async def test_login_captura_evento_antecipado_e_usa_o_destino_da_conta(contas, service):
    default, work = contas
    FakeNative.complete_before_response = True

    first = await service.start_login(default)
    second = await service.start_login(work)
    await asyncio.sleep(0)

    assert first["verification_url"].startswith("https://")
    assert service.login_status(default)["status"] == "completed"
    assert service.login_status(work)["status"] == "completed"
    assert {item.codex_home for item in FakeNative.instances} == {default.home, work.home}
    assert all(item.closed for item in FakeNative.instances)


@pytest.mark.parametrize("forcar", [False, True])
async def test_preparo_atualiza_a_principal_antes_de_herdar_para_adicional(
        contas, monkeypatch, forcar):
    _, work = contas
    ordem = []

    async def atualizar_principal(forcar):
        ordem.append(("principal", forcar))

    async def herdar(account, force=False):
        ordem.append((account.id, force))
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", herdar)
    checker = CodexContasLogin(native=FakeNative, atualizar_principal=atualizar_principal)

    assert (await checker.prepare(work, forcar=forcar))["status"] == "running"
    result = await checker._preparations[checker._key(work)]
    assert result["status"] == "ready"
    assert ordem == [("principal", forcar), ("work", forcar)]


async def test_preparo_mostra_principal_enquanto_aguarda_sincronizacao(contas, monkeypatch):
    _, work = contas
    liberar_principal = asyncio.Event()
    iniciou_heranca = asyncio.Event()
    liberar_heranca = asyncio.Event()

    async def atualizar_principal(_forcar):
        await liberar_principal.wait()
        return {"estado": "ok"}

    async def herdar(_account):
        iniciou_heranca.set()
        await liberar_heranca.wait()
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", herdar)
    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.preparation_status",
                        lambda _: {"status": "running", "etapa": "plugins"})
    checker = CodexContasLogin(native=FakeNative, atualizar_principal=atualizar_principal)
    try:
        assert (await checker.prepare(work))["etapa"] == "principal"
        liberar_principal.set()
        await iniciou_heranca.wait()
        assert checker.preparation_status(work)["etapa"] == "plugins"
    finally:
        liberar_principal.set()
        liberar_heranca.set()
        await checker._preparations[checker._key(work)]
    assert checker.preparation_status(work)["status"] == "ready"


async def test_falha_da_principal_permanece_visivel_na_conta_adicional(contas, monkeypatch):
    _, work = contas

    async def atualizar_principal(_forcar):
        return {"estado": "erro"}

    async def herdar(_account):
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", herdar)
    checker = CodexContasLogin(native=FakeNative, atualizar_principal=atualizar_principal)
    await checker.prepare(work)
    result = await checker._preparations[checker._key(work)]

    assert result["status"] == "partial"
    assert result["issues"] == [{
        "code": "codex_account_source_sync_incomplete", "params": {"status": "erro"},
    }]
    assert checker.preparation_status(work) == result


async def test_pedido_manual_em_preparo_automatico_dispara_rodada_forcada(contas, monkeypatch):
    _, work = contas
    iniciou = asyncio.Event()
    liberar = asyncio.Event()
    chamadas = []

    async def atualizar_principal(forcar):
        chamadas.append(("principal", forcar))
        if not forcar:
            iniciou.set()
            await liberar.wait()
        return {"estado": "ok"}

    async def herdar(_account, force=False):
        chamadas.append(("adicional", force))
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", herdar)
    checker = CodexContasLogin(native=FakeNative, atualizar_principal=atualizar_principal)
    await checker.prepare(work)
    await iniciou.wait()
    await checker.prepare(work, forcar=True)
    liberar.set()

    result = await checker._preparations[checker._key(work)]
    assert result["status"] == "ready"
    assert chamadas == [
        ("principal", False), ("adicional", False),
        ("principal", True), ("adicional", True),
    ]


async def test_login_aceita_confirmacao_sem_login_id_permitida_pelo_schema(
        contas, service, monkeypatch):
    _, work = contas
    FakeNative.complete_before_response = True
    FakeNative.complete_without_login_id = True
    monkeypatch.setattr("app.codex_contas_login._LOGIN_TIMEOUT", 0.01)

    await service.start_login(work)
    await asyncio.sleep(0.03)

    assert service.login_status(work)["status"] == "completed"
    assert service.login_status(work).get("error") is None


async def test_read_auth_traduz_o_tipo_sem_expor_tokens(contas, service):
    default, _ = contas
    config = default.home / "config.toml"
    config.write_text('cli_auth_credentials_store = "keyring"\n', encoding="utf-8")

    result = await service.read_auth(default)

    assert result == {"method": "oauth", "status": "connected", "email": "a@x", "plan": "plus"}
    assert config.read_text(encoding="utf-8") == 'cli_auth_credentials_store = "keyring"\n'


async def test_read_auth_falha_mantem_identidade_cacheada(contas, service, monkeypatch):
    default, _ = contas
    assert (await service.read_auth(default))["email"] == "a@x"

    class BrokenNative:
        def __init__(self, *args, **kwargs):
            pass

        async def __aenter__(self):
            raise RuntimeError("app-server indisponível")

        async def __aexit__(self, *args):
            return None

    cached = copy.deepcopy(service._auth_cache)
    monkeypatch.setattr(service, "native", BrokenNative)
    result = await service.read_auth(default, refresh=True)
    assert result == {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
    assert service._auth_cache == cached
    assert service.cached_auth(default)["email"] == "a@x"


async def test_refresh_substitui_identidade_sem_mudar_arquivo(contas, service):
    default, _ = contas
    assert (await service.read_auth(default))["email"] == "a@x"
    FakeNative.auth_by_home[str(default.home)] = {"account": None}
    expected = {"method": "none", "status": "disconnected", "email": None, "plan": None}
    assert await service.read_auth(default, refresh=True) == expected
    assert await service.read_auth(default) == expected


async def test_cancelamento_antigo_nao_cancela_tentativa_nova(contas, service):
    _, work = contas
    first = await service.start_login(work)
    await service.cancel_login(work, first["attempt_id"])
    second = await service.start_login(work)

    with pytest.raises(accounts.AccountError) as error:
        await service.cancel_login(work, first["attempt_id"])

    assert error.value.status == 409
    assert service.login_status(work)["attempt_id"] == second["attempt_id"]
    assert service.login_status(work)["status"] == "waiting"


async def test_reserva_de_criacao_bloqueia_login_e_libera_depois(contas, service):
    _, work = contas
    lease = service.reserve_creation(work)
    with pytest.raises(accounts.AccountError) as error:
        await service.start_login(work)
    assert error.value.status == 409
    lease.release()
    assert (await service.start_login(work))["status"] == "waiting"


def test_ambiente_da_secundaria_remove_identidade_externa_e_padrao_preserva(contas):
    default, work = contas
    base = {"OPENAI_API_KEY": "secret", "OPENAI_BASE_URL": "https://external.invalid",
            "TOOL_ENDPOINT": "keep", "PATH": "/bin"}

    secondary = accounts.environment(work, home=Path("/tmp/hangar-home"), base=base)
    primary = accounts.environment(default, home=Path("/tmp/hangar-home"), base=base)

    assert "OPENAI_API_KEY" not in secondary
    assert "OPENAI_BASE_URL" not in secondary
    assert secondary["TOOL_ENDPOINT"] == "keep"
    assert primary["OPENAI_API_KEY"] == "secret"
    assert secondary["CODEX_HOME"] == str(work.home)
    assert "CODEX_CONFIG_HOME" not in secondary
    assert "CODEX_SQLITE_HOME" not in secondary


async def test_login_recusa_secundaria_sem_configuracao_file(contas, service):
    _, work = contas
    (work.home / "config.toml").write_text('cli_auth_credentials_store = "keyring"\n', encoding="utf-8")

    with pytest.raises(accounts.AccountError) as error:
        await service.start_login(work)

    assert error.value.code == "codex_account_auth_storage_invalid"
    assert not FakeNative.instances


async def test_post_repetido_acompanha_a_mesma_tentativa(contas, service):
    _, work = contas

    first, second = await asyncio.gather(service.start_login(work), service.start_login(work))

    assert first["attempt_id"] == second["attempt_id"]
    assert len(FakeNative.instances) == 1


async def test_cancelamento_antes_do_login_id_ainda_cancela_o_native(contas, service):
    _, work = contas
    FakeNative.start_delay = 0.03
    start = asyncio.create_task(service.start_login(work))
    await asyncio.sleep(0)
    attempt_id = next(iter(service._attempts.values())).attempt_id

    cancelled = await service.cancel_login(work, attempt_id)
    await start

    assert cancelled["status"] == "cancelled"
    # Cancelar é pedido atendido, não falha: com o erro junto a tela dizia as duas coisas.
    assert "error" not in service.login_status(work)
    assert any(call[0] == "account/login/cancel" for call in FakeNative.instances[0].requests)
    assert FakeNative.instances[0].closed


async def test_timeout_fecha_o_processo_e_marca_falha(contas, service, monkeypatch):
    _, work = contas
    monkeypatch.setattr("app.codex_contas_login._LOGIN_TIMEOUT", 0.01)

    await service.start_login(work)
    await asyncio.sleep(0.03)

    assert service.login_status(work)["status"] == "failed"
    assert FakeNative.instances[0].closed


async def test_diario_login_timeout_nao_exporta_dados_oauth(contas, service, monkeypatch, tmp_path):
    from app import diag
    _, work = contas
    monkeypatch.setattr(diag, "_base", lambda: tmp_path / "logs")
    monkeypatch.setattr("app.codex_contas_login._LOGIN_TIMEOUT", 0.01)
    await service.start_login(work)
    await service._attempts[service._key(work)].task
    texto = diag.caminho_do_dia().read_text(encoding="utf-8")
    eventos = [json.loads(linha) for linha in texto.splitlines()]
    fim = next(e for e in eventos if e["evento"] == "conta.login.terminou")
    assert fim["codigo"] == "codex_account_login_timeout"
    assert fim["etapa"] == "aguardar_autorizacao"
    assert fim["operacao"] == service.login_status(work)["attempt_id"]
    for segredo in ("ABCD-EFGH", "native-login", "https://auth.openai.com", "b@x", str(work.home)):
        assert segredo not in texto


async def test_diario_preparo_background_registra_falha_sem_texto(contas, monkeypatch, tmp_path):
    from app import diag
    _, work = contas
    monkeypatch.setattr(diag, "_base", lambda: tmp_path / "logs")

    async def falhar(*args, **kwargs):
        raise PermissionError(13, "token=SEGREDO-CLI")

    monkeypatch.setattr("app.codex_contas_sync.prepare_account", falhar)
    service = CodexContasLogin(native=FakeNative, account_in_use=lambda account: False)
    await service.prepare(work)
    await service._preparations[service._key(work)]
    texto = diag.caminho_do_dia().read_text(encoding="utf-8")
    eventos = [json.loads(linha) for linha in texto.splitlines()]
    falha = next(e for e in eventos if e["evento"] == "conta.preparar.falhou")
    assert falha["etapa"] == "herdar_configuracao"
    assert falha["errno"] == 13
    assert service.preparation_status(work)["status"] == "error"
    assert "SEGREDO-CLI" not in texto


async def test_diario_auth_nao_repete_falha_do_cache(contas, monkeypatch, tmp_path):
    from app import diag
    _, work = contas
    monkeypatch.setattr(diag, "_base", lambda: tmp_path / "logs")

    class FalhaAuth(FakeNative):
        async def __aenter__(self):
            raise RuntimeError("token=SEGREDO-CLI")

    service = CodexContasLogin(native=FalhaAuth)
    assert (await service.read_auth(work))["status"] == "unavailable"
    primeiro = diag.caminho_do_dia().read_text(encoding="utf-8")
    assert (await service.read_auth(work))["status"] == "unavailable"
    assert diag.caminho_do_dia().read_text(encoding="utf-8") == primeiro
    assert "conta.auth.falhou" in primeiro
    assert "SEGREDO-CLI" not in primeiro


async def test_diario_codex_ausente_nao_e_falha(contas, monkeypatch, tmp_path):
    # Maquina sem Codex instalado: cada listagem de credenciais gravava `conta.auth.falhou` nivel
    # erro. Ausencia de CLI e estado, nao defeito — vira aviso nomeado.
    from app import diag
    from app.codex_importador import CodexAusente
    _, work = contas
    monkeypatch.setattr(diag, "_base", lambda: tmp_path / "logs")

    class SemCodex(FakeNative):
        async def __aenter__(self):
            raise CodexAusente("Codex CLI não encontrado")

    service = CodexContasLogin(native=SemCodex)
    auth = await service.read_auth(work)
    assert (auth["status"], auth["reason"]) == ("unavailable", "cli_missing")
    # A lembranca do fracasso guarda o MOTIVO: a tela diz "Codex nao instalado" tambem no cache.
    assert (await service.read_auth(work))["reason"] == "cli_missing"
    eventos = [json.loads(l) for l in diag.caminho_do_dia().read_text(encoding="utf-8").splitlines()]
    assert not [e for e in eventos if e["evento"] == "conta.auth.falhou"]
    aviso = next(e for e in eventos if e["evento"] == "conta.auth.indisponivel")
    assert aviso["codigo"] == "cli_ausente"


async def test_conta_viva_recusa_login_antes_do_process(contas, monkeypatch):
    _, work = contas
    monkeypatch.setattr("app.codex_contas_login.CodexNativo", FakeNative)
    live = CodexContasLogin(native=FakeNative, account_in_use=lambda account: True)

    with pytest.raises(accounts.AccountError) as error:
        await live.start_login(work)

    assert error.value.code == "codex_account_in_use"
    assert not FakeNative.instances


async def test_apagar_conta_recusa_viva_e_apaga_parada(contas, service, monkeypatch):
    _, work = contas
    monkeypatch.setattr("app.codex_contas_login.CodexNativo", FakeNative)
    live = CodexContasLogin(native=FakeNative, account_in_use=lambda account: True)
    with pytest.raises(accounts.AccountError) as error:
        await live.delete_account(work)
    assert error.value.code == "codex_account_in_use"
    assert work.home.exists()

    await service.delete_account(work)
    assert not work.home.exists()


async def test_conta_que_demora_a_aparecer_nao_vira_login_falhado(contas, service, monkeypatch):
    """O Codex avisa `success` antes de a credencial ficar legível: esperar é o conserto."""
    _, work = contas
    monkeypatch.setattr("app.codex_contas_login._AUTH_APOS_LOGIN_INTERVALO", 0.01)
    FakeNative.complete_before_response = True
    FakeNative.leituras_vazias = 3

    await service.start_login(work)
    await asyncio.gather(*(a.task for a in service._attempts.values() if a.task))

    estado = service.login_status(work)
    assert estado["status"] == "completed"
    assert "error" not in estado


async def test_conta_que_nunca_aparece_ainda_falha(contas, service, monkeypatch):
    _, work = contas
    monkeypatch.setattr("app.codex_contas_login._AUTH_APOS_LOGIN", 0.0)
    FakeNative.complete_before_response = True
    FakeNative.auth_by_home = {}

    await service.start_login(work)
    await asyncio.gather(*(a.task for a in service._attempts.values() if a.task))

    assert service.login_status(work)["error"]["code"] == "codex_account_login_failed"


async def test_criar_conta_nao_prepara_e_ja_aceita_login(contas, service, monkeypatch):
    default, _ = contas

    async def prepare_proibido(account):
        raise AssertionError("criar conta não pode herdar a padrão antes do login")

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", prepare_proibido)
    created = await service.create_account("nova")

    assert created["sync"]["status"] == "idle"
    assert created["auth"]["status"] == "disconnected"
    assert created["has_settings"] is False
    nova = accounts.resolve_account("nova")
    FakeNative.auth_by_home[str(nova.home)] = {"account": {"type": "chatgpt", "email": "c@x", "planType": "plus"}}
    FakeNative.complete_before_response = True
    await service.start_login(nova)
    await asyncio.gather(*(a.task for a in service._attempts.values() if a.task))
    assert service._attempts[service._key(nova)].status == "completed"


async def test_has_settings_so_na_padrao_com_conteudo(contas, service):
    default, work = contas
    assert service._has_settings(default) is False
    assert service._has_settings(work) is False
    (default.home / "config.toml").write_text("model = \"x\"\n", encoding="utf-8")
    assert service._has_settings(default) is True


async def test_preparacao_em_curso_reserva_a_conta(contas, service, monkeypatch):
    _, work = contas
    gate = asyncio.Event()

    async def blocked_prepare(account):
        await gate.wait()
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", blocked_prepare)
    await service.prepare(work)
    await asyncio.sleep(0)

    with pytest.raises(accounts.AccountError) as error:
        await service.start_login(work)

    assert error.value.code == "codex_account_preparing"
    gate.set()
    await asyncio.gather(*service._preparations.values())


async def test_payload_publico_preserva_home_credencial_e_status(contas, service):
    default, _ = contas

    snapshot = await service.account_snapshot(default)

    assert snapshot["is_default"] is True
    assert snapshot["home"] == str(default.home.resolve())
    assert snapshot["credential_id"] == f"codex:{default.home.resolve()}"
    assert snapshot["auth"]["method"] == "oauth"
    assert snapshot["auth"]["status"] == "connected"
    assert "default" not in snapshot


async def test_duas_criacoes_podem_usar_a_mesma_conta(contas, service):
    _, work = contas
    first = service.reserve_creation(work)
    first.mark_live("one")

    second = service.reserve_creation(work)

    second.release()
    first.release()


async def test_falha_no_close_ainda_libera_reserva(contas, service):
    _, work = contas
    FakeNative.close_error = True
    first = await service.start_login(work)
    await service.cancel_login(work, first["attempt_id"])

    lease = service.reserve_creation(work)
    lease.release()


def test_sidecar_morto_nao_prova_sessao_viva(contas, monkeypatch, tmp_path):
    default, _ = contas
    from app.adapters.codex import sessions
    from app import procinfo, tmux

    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "codex-sessions")
    sessions.save("dead", "thread", str(default.home / "sessions" / "rollout.jsonl"), "/tmp",
                  endpoint="ws://127.0.0.1:1", app_pid=999999)
    monkeypatch.setattr(tmux, "list_panes_of", lambda name: [])
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: False)
    checker = CodexContasLogin(native=FakeNative)

    assert checker._live(default) is False


def test_nome_reutilizado_por_claude_nao_prova_codex_vivo(contas, monkeypatch, tmp_path):
    default, _ = contas
    from app.adapters.codex import sessions
    from app import procinfo, registry, tmux

    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "codex-sessions")
    sessions.save("same", "thread", str(default.home / "sessions" / "rollout.jsonl"), "/tmp",
                  endpoint="ws://127.0.0.1:1", app_pid=999999)
    monkeypatch.setattr(tmux, "list_panes_of", lambda name: [{"pid": 42}])
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: False)
    monkeypatch.setattr(registry, "agente_do_pane", lambda pid, children: ("claude", pid))
    checker = CodexContasLogin(native=FakeNative)

    assert checker._live(default) is False


async def test_falha_na_inspecao_da_sessao_bloqueia_login(contas, monkeypatch, tmp_path):
    default, work = contas
    from app.adapters.codex import sessions
    from app import procinfo, tmux

    monkeypatch.setattr(sessions, "_dir", lambda: tmp_path / "codex-sessions")
    sessions.save("broken", "thread", str(work.home / "sessions" / "rollout.jsonl"), "/tmp",
                  endpoint="ws://127.0.0.1:1", app_pid=999999)
    monkeypatch.setattr(procinfo, "pid_vivo", lambda pid: False)

    def inspection_failure(name):
        raise RuntimeError("tmux unavailable")

    monkeypatch.setattr(tmux, "list_panes_of", inspection_failure)
    checker = CodexContasLogin(native=FakeNative)

    with pytest.raises(accounts.AccountError) as error:
        await checker.start_login(work)

    assert error.value.code == "codex_account_usage_unknown"


async def test_resposta_auth_antiga_nao_repopula_cache(contas, service):
    default, _ = contas
    gate = asyncio.Event()
    calls = 0

    async def old_read(native):
        await gate.wait()
        return {"method": "oauth", "status": "connected", "email": "old", "plan": "plus"}

    async def new_read(native):
        nonlocal calls
        calls += 1
        return {"method": "oauth", "status": "connected", "email": "new", "plan": "plus"}

    service._read_auth_native = old_read
    pending = asyncio.create_task(service.read_auth(default))
    await asyncio.sleep(0)
    service._invalidate_auth(service._key(default))
    gate.set()
    await pending
    service._read_auth_native = new_read

    result = await service.read_auth(default)

    assert result["email"] == "new"
    assert calls == 1


async def test_listagem_nao_espera_preparacao_longa(contas, service, monkeypatch):
    _, work = contas
    gate = asyncio.Event()

    async def blocked_prepare(account):
        await gate.wait()
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setattr("app.codex_contas_login.codex_contas_sync.prepare_account", blocked_prepare)
    await service.prepare(work)
    await asyncio.sleep(0)

    result = await asyncio.wait_for(service.accounts_snapshot(), 0.2)

    secondary = next(item for item in result if item["id"] == "work")
    assert secondary["sync"]["status"] == "running"
    assert secondary["auth"]["status"] == "unavailable"
    assert not any(item.codex_home == work.home for item in FakeNative.instances)
    gate.set()
    await asyncio.gather(*service._preparations.values())


async def test_dispatch_considera_method_antes_de_id():
    native = CodexNativo(Path("/tmp"), Path("/tmp"))
    native._pending[7] = asyncio.get_running_loop().create_future()
    listener = native.subscribe("account/login/completed")

    native._dispatch({"id": 7, "method": "account/login/completed", "params": {"success": True}})

    assert not native._pending[7].done()
    assert (await listener.get())["method"] == "account/login/completed"
