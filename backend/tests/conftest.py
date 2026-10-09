import os
import signal
import sys
import tempfile
from pathlib import Path

import pytest

# No import, antes de qualquer teste importar o `app.config`: o `settings` nasce na importação, e
# um `.env` de desenvolvimento com a branch de teste mudaria o alvo de toda a suíte do Atualizar.
os.environ["CP_UPDATE_BRANCH"] = ""


def _instalar_home_do_windows() -> None:
    """No Windows, `monkeypatch.setenv("HOME", tmp)` NAO isola nada — e a suite escreve no perfil
    REAL de quem roda.

    `ntpath.expanduser` (logo, `Path.home()` e todo `expanduser("~")`) le USERPROFILE e, na falta
    dele, HOMEDRIVE+HOMEPATH. HOME nao entra na conta em NENHUM ramo. Medido em 21/08/2026 nesta
    VM: os 14 `setenv("HOME", ...)` da suite (test_contas, test_commands, test_costs_sources,
    test_tmux, test_contas_api, test_conta_estado_api) resolviam pro C:\\Users\\<user> de verdade,
    e `test_contas` chegou a criar SEIS pastas `~/.claude-*` cheias de symlinks apontando pro
    ~/.claude real. Nao e so teste falhando: e a suite mexendo na casa de quem a roda.

    Em vez de reescrever os 14 pontos (e ter de lembrar do detalhe no 15o), o vinculo mora aqui:
    enquanto o teste roda, `setenv("HOME", v)` leva junto o trio que o Windows de fato consulta.
    No POSIX o fixture nao faz nada — HOME ja e a fonte, e o ramo fica byte-identico ao de hoje.

    Patch de CLASSE, feito UMA vez no import — nao um fixture autouse. A primeira versao disto era
    um autouse pedindo `monkeypatch` na assinatura, e isso instancia (e finaliza) o monkeypatch em
    outro ponto da ordem, em TODO teste da suite, inclusive nos que nunca ouviram falar daqui. O
    comentario do `models_cache_em_tmp`, mais abaixo, ja registrava essa armadilha com nome e
    sobrenome ("a versao autouse derrubava os mocks de test_tmux ... interacao de fixtures via
    test_codex_adapter") — e foi exatamente nela que eu pisei.

    Medido em 21/08/2026, o par `test_codex_adapter.py + test_tmux.py`: 7 falhas com um autouse
    VAZIO, 7 com `autouse(request)`, e 22 com `autouse(monkeypatch)`. No Linux a suite inteira ia
    de 0 pra 16 falhas, todas em test_tmux. Sem fixture nenhum nao ha ordem pra mudar.

    Cada chamada ao original registra o proprio undo NA INSTANCIA, entao o teardown de cada teste
    continua desfazendo tudo como sempre — o patch de classe nao guarda estado.
    """
    original = pytest.MonkeyPatch.setenv

    def setenv(self, name, value, prepend=None):
        original(self, name, value, prepend)
        if name == "HOME":
            # USERPROFILE e o 1o ramo do ntpath.expanduser; HOMEDRIVE+HOMEPATH e o 2o. Os dois
            # precisam ir: deixar o par velho de pe faria o fallback apontar pro perfil real caso
            # algum codigo (ou uma lib) limpe USERPROFILE.
            original(self, "USERPROFILE", value)
            drive, resto = os.path.splitdrive(value)
            original(self, "HOMEDRIVE", drive)
            original(self, "HOMEPATH", resto or "\\")

    pytest.MonkeyPatch.setenv = setenv


if os.name == "nt":
    _instalar_home_do_windows()


_SINAIS_DA_SESSAO = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}


@pytest.hookimpl(trylast=True)
def pytest_runtest_teardown(item, nextitem):
    # Nada que um teste deixa para trás pode alcançar o arquivo seguinte: com a suíte repartida
    # entre processos, quem pagava era o vizinho. Hook e não fixture autouse: fixture por teste muda
    # a ordem das outras (ver _instalar_home_do_windows).
    #
    # Um uvicorn que sai fora de ordem deixa o tratador de SIGTERM dele instalado; o sse-starlette
    # vê esse servidor encerrado, liga o should_exit global, e toda resposta SSE seguinte sai vazia.
    for sig, original in _SINAIS_DA_SESSAO.items():
        if signal.getsignal(sig) is not original:
            signal.signal(sig, original)
    sse = sys.modules.get("sse_starlette.sse")
    if sse is not None:
        sse.AppStatus.should_exit = False
    # A confirmação de entrega do app.api é um Timer de segundos: sobrando de um teste, dispara no
    # seguinte e fala com o tmux no meio dele.
    api = sys.modules.get("app.api")
    if api is None:
        return
    with api._confirm_lock:
        pending = [timer for timer, _ in api._confirm_pend.values()]
        api._confirm_pend.clear()
    for timer in pending:
        # Como no _agendar_confirmacao: há testes que trocam o Timer por um falso sem esses métodos.
        if callable(getattr(timer, "cancel", None)):
            timer.cancel()
        if callable(getattr(timer, "join", None)):
            timer.join(timeout=5)


@pytest.fixture(scope="session", autouse=True)
def _sem_integracao_codex_real():
    # Lifespan e lançadores de teste não podem reconciliar o perfil real. Testes explícitos
    # instanciam o serviço em tmp_path e exercitam reconciliar sem depender deste gatilho.
    anterior = os.environ.get("CP_CODEX_SYNC_ENABLED")
    os.environ["CP_CODEX_SYNC_ENABLED"] = "0"
    try:
        yield
    finally:
        if anterior is None:
            os.environ.pop("CP_CODEX_SYNC_ENABLED", None)
        else:
            os.environ["CP_CODEX_SYNC_ENABLED"] = anterior


@pytest.fixture(scope="session", autouse=True)
def _sem_sessoes_sem_terminal_reais(tmp_path_factory):
    # O lifespan (`with TestClient(app)`) religa todo cano listado nos sidecars. Lendo os REAIS,
    # a suíte virava um segundo cliente dos canos das sessões vivas desta máquina e o backend de
    # verdade perdia a conexão. Session-scoped: os testes que trocam `_dir` pelo monkeypatch
    # voltam pra este diretório, nunca pro real.
    # A varredura de órfãos lê os processos REAIS em /proc: com os sidecars apontando pra pasta
    # vazia, todo cano vivo da máquina parecia órfão e levava SIGTERM — derrubava as sessões sem
    # terminal, inclusive a que roda a suíte.
    from app.adapters.claude_headless import adapter, sessions
    original = sessions._dir
    varredura = adapter.matar_orfaos
    pasta = tmp_path_factory.mktemp("claude-headless")
    sessions._dir = lambda: pasta
    adapter.matar_orfaos = lambda: 0
    try:
        yield
    finally:
        sessions._dir = original
        adapter.matar_orfaos = varredura


@pytest.fixture(scope="session", autouse=True)
def _isolated_runtime_storage(tmp_path_factory):
    # O novo cadastro do lifespan não pode adquirir a posse das sessões da máquina.
    from app.config import settings
    from app.adapters.codex import sessions
    original_projects, original_dir = settings.projects_dir, sessions._dir
    root = tmp_path_factory.mktemp("runtime-storage")
    settings.projects_dir = root / "projects"
    settings.projects_dir.mkdir()
    sessions._dir = lambda: root / "codex-sessions"
    try:
        yield
    finally:
        settings.projects_dir, sessions._dir = original_projects, original_dir


@pytest.fixture(scope="session", autouse=True)
def _sem_endereco_do_plugin_real():
    # O `_lifespan` grava ~/.hangar/plugin.json; sem isto o TestClient trocaria o arquivo da máquina
    # pela config do teste e as sessões vivas do plugin falariam com um backend que não existe.
    from app import plugin_bridge
    original = plugin_bridge.publish_address
    plugin_bridge.publish_address = lambda home=None: None
    try:
        yield
    finally:
        plugin_bridge.publish_address = original


@pytest.fixture(scope="session", autouse=True)
def _sem_espera_de_transcricao_real(tmp_path_factory):
    # A espera por cota mora em ~/.hangar: um teste que esgota um serviço falso não pode pôr em
    # espera o serviço real desta máquina.
    from app import transcribe
    original = transcribe._state_path
    path = tmp_path_factory.mktemp("transcription-wait") / "transcription-wait.json"
    transcribe._state_path = lambda: path
    try:
        yield
    finally:
        transcribe._state_path = original


@pytest.fixture(scope="session", autouse=True)
def _sem_compartilhamento_real(tmp_path_factory):
    # O lifespan (`with TestClient(app)`) sobe a varredura de convites. Com os sidecars apontando
    # pra pasta vazia acima, ela veria toda sessão viva da máquina como morta, revogaria os
    # convites REAIS (~/.claude/shares.json) e desligaria o Funnel de verdade. Sem varredura e
    # com o registro em tmp, nada no boot de teste toca o registro nem o tailscale reais.
    from app import share_api, share_store
    original_loop = share_api.sweep_loop
    original_path = share_store._path_override

    async def _sem_varredura():
        return None

    share_api.sweep_loop = _sem_varredura
    share_store._path_override = tmp_path_factory.mktemp("share") / "shares.json"
    share_store._reset()
    try:
        yield
    finally:
        share_api.sweep_loop = original_loop
        share_store._path_override = original_path
        share_store._reset()


@pytest.fixture(scope="session", autouse=True)
def _sem_orquestracoes_reais(tmp_path_factory):
    # A lista de sessões lê ~/.hangar/orq: uma orquestração `auto` viva na máquina viraria linha
    # extra em todo teste que conta sessões.
    from app.adapters.orq import runs
    original = runs.root
    pasta = tmp_path_factory.mktemp("orq")
    runs.root = lambda: pasta
    try:
        yield
    finally:
        runs.root = original


@pytest.fixture(scope="session", autouse=True)
def _sem_transferencias_reais(tmp_path_factory):
    # A lista e o aquecimento não podem consultar operações privadas da máquina durante a suíte.
    from app import conversation_transfer
    original = conversation_transfer._base
    pasta = tmp_path_factory.mktemp("conversation-transfers")
    conversation_transfer._base = lambda: pasta
    try:
        yield
    finally:
        conversation_transfer._base = original


@pytest.fixture(scope="session", autouse=True)
def _sem_git_dir_no_ambiente_de_teste():
    # git_ops._run passa os.environ inteiro pro subprocess: dentro de um hook (pre-push, p.ex.) o
    # processo herda GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE do git que roda o hook, e qualquer teste
    # que faz `git init`/`commit` num tmp_path (test_git_ops._repo) escreve no `.git` REAL por trás
    # do cwd em vez do repo isolado — foi o que corrompeu o `.git` desta sessão. Session-scoped
    # porque a suíte inteira roda no mesmo processo; monkeypatch é function-scoped e não cobre isto.
    for k in [k for k in os.environ if k.startswith("GIT_")]:
        os.environ.pop(k, None)


@pytest.fixture(scope="session", autouse=True)
def _sem_plugin_de_function_hooks_da_maquina():
    # O interruptor `claude_function_hooks` é config da MÁQUINA (~/.claude/runtime-config.json) e a
    # capacidade vem de `claude --help`: ligado aqui, todo teste que confere o comando ou o env de
    # uma sessão Claude ganhava `--plugin-dir` e `HANGAR_PLUGIN_*`, e a suíte passava no CI e
    # falhava na máquina de quem usa o recurso. Session-scoped e sem monkeypatch pelo motivo do
    # topo do arquivo. Quem testa o portão ligado troca `plugin_bridge.ligado` no próprio teste.
    from app import plugin_bridge
    original = plugin_bridge.ligado
    plugin_bridge.ligado = lambda: False
    try:
        yield
    finally:
        plugin_bridge.ligado = original


@pytest.fixture(scope="session", autouse=True)
def _sem_padrao_do_jev_da_maquina():
    # Mesmo defeito do fixture acima, outro interruptor: `jev_padrao` é config da MÁQUINA, e com ele
    # ligado todo teste que confere os argumentos do `registry.create` ganhava um `jev=True` que a
    # asserção não espera. Passava no CI (sem config gravada) e falhava em quem usa o recurso.
    # `headless_default` (nasce true) entra junto: o create passaria `headless=True` aos mocks.
    # Só estas chaves são forçadas — o resto do `get` continua o de verdade, e quem testa o padrão ligado
    # troca o `get` inteiro no próprio teste.
    from app import runtime_config
    original = runtime_config.get
    runtime_config.get = lambda campo: False if campo in ("jev_padrao", "headless_default") else original(campo)
    try:
        yield
    finally:
        runtime_config.get = original


@pytest.fixture(scope="session", autouse=True)
def _sem_servidor_de_teste_vazado():
    """No fim da suite, nenhum socket de teste pode ter processo vivo.

    Os quatro teardowns que criam socket proprio ja recolhem o servidor deles (`matar_servidor`).
    Este fixture existe porque o defeito que ele cobre e justamente o que NAO aparece: a limpeza
    esquecida num arquivo novo nao quebra teste nenhum — ela deixa um `tmux server -s __warm__ -L
    cp-test-<hash>` de pe, com um shell e um console presos, ate a maquina reiniciar. Foram 70
    deles nesta VM em 22/08/2026 (~12,7 GB) e a sessao que rodava a suite morreu por falta de
    memoria. Falha de teardown, aqui, e barata; achar isso pela maquina travando nao e.

    Confere so o que a suite ENTREGOU por `novo_socket` — nunca varre `cp-test-*` da maquina, que
    poderia acusar sobra de outra rodada (ou de outro worker do xdist) como se fosse desta.
    """
    yield
    import tmux_teste
    vazados = tmux_teste.sockets_vazados()
    assert not vazados, (
        "a suite terminou deixando servidor de multiplexador vivo em socket de teste: "
        f"{vazados}. Falta `matar_servidor(sock)` no teardown de quem criou o socket."
    )


@pytest.fixture(autouse=True)
def _reset_auth_backoff():
    # O backoff de token errado (app.auth) e um dict de MODULO: sem este reset, os varios testes que
    # mandam token errado de proposito somariam entre si e o proximo caso levaria 429 em vez do 401
    # que ele testa.
    from app import auth
    auth.reset_backoff()
    yield
    auth.reset_backoff()


@pytest.fixture(autouse=True)
def _reset_pi_inbox_ws_warn():
    # Aviso-uma-vez-ate-mudar da recusa de conexao WS (app.api._ws_origem_avisada/_ws_token_avisado)
    # e dict de MODULO: sem reset, o teste que prova "recusa loga" falharia se um teste anterior na
    # mesma sessao ja tivesse "gasto" o aviso daquele host (mesmo padrao do _reset_auth_backoff acima).
    from app import api
    api._ws_origem_avisada.clear()
    api._ws_token_avisado.clear()
    yield
    api._ws_origem_avisada.clear()
    api._ws_token_avisado.clear()


@pytest.fixture(autouse=True)
def _reset_agentpane_cache():
    # _cache do agentpane e dict de MODULO com TTL de 60s: sem reset, a resolucao de uma sessao de
    # teste vazaria pro teste seguinte que reusa o mesmo nome (padrao do _reset_list_snapshot).
    from app import agentpane
    agentpane.invalidate()
    yield
    agentpane.invalidate()


_DIARIO_DE_TESTE = Path(tempfile.mkdtemp(prefix="hangar-diag-")) / ".hangar-diag"
# O CLAUDE_CONFIG_DIR que a suíte HERDOU é o da conta de quem roda (sessão em `--conta` tem um):
# esse é o real, e vale tanto quanto `~/.claude`. Só um valor DIFERENTE, posto por um teste, isola.
_CONFIG_DIR_HERDADO = os.environ.get("CLAUDE_CONFIG_DIR")


@pytest.fixture(autouse=True)
def _diario_isolado():
    # O diário e os logs privados precisam ficar fora dos arquivos reais da máquina:
    # 361 `pergunta.fallback_texto` da sessão `s1` (test_api.py) numa semana de uso exportada,
    # indistinguíveis de picker preso de verdade. Sem `monkeypatch`/`tmp_path` na assinatura —
    # ver a armadilha de ordem de fixtures documentada em `_instalar_home_do_windows`.
    import logging
    from app import diag, log_paths
    original = log_paths.base
    old_sources = diag._pastas_legadas
    old_logs = diag._logs_legados
    loggers = [logging.getLogger(name) for name in ("hangar", "app", "uvicorn.error", "asyncio")]
    handlers = {logger: set(logger.handlers) for logger in loggers}
    levels = {logger: logger.level for logger in loggers}
    def _base_de_teste() -> Path:
        v = os.environ.get("CLAUDE_CONFIG_DIR")
        return Path(v) / "logs" if v and v != _CONFIG_DIR_HERDADO else _DIARIO_DE_TESTE
    log_paths.base = _base_de_teste
    diag._pastas_legadas = lambda: []
    diag._logs_legados = lambda: set()
    yield
    for logger in loggers:
        logger.setLevel(levels[logger])
        for handler in set(logger.handlers) - handlers[logger]:
            logger.removeHandler(handler)
            handler.close()
    log_paths.base = original
    diag._pastas_legadas = old_sources
    diag._logs_legados = old_logs


@pytest.fixture(autouse=True)
def _reset_sem_agente_avisadas():
    # _SEM_AGENTE_AVISADAS (Task 5.5) e set de CLASSE (SessionRegistry) com dedup de log por nome
    # que nunca expira: sem reset, um teste que aciona o aviso pra um nome (ex: SESS reusado entre
    # arquivos) envenena o proximo teste que espera o log de novo -- mesmo padrao do
    # _reset_agentpane_cache acima.
    from app.registry import SessionRegistry
    SessionRegistry._SEM_AGENTE_AVISADAS.clear()
    yield
    SessionRegistry._SEM_AGENTE_AVISADAS.clear()


@pytest.fixture(autouse=True)
def _reset_pane_frames():
    # Quadro compartilhado de estado/prévia é por nome: sem limpar, o dublê de um teste responderia
    # no seguinte que usa o mesmo nome dentro da idade máxima.
    from app import state
    state._frames.clear()
    state._frames_inflight.clear()
    yield
    state._frames.clear()
    state._frames_inflight.clear()


@pytest.fixture(autouse=True)
def _reset_list_snapshot():
    # Endpoints quentes (history/workflows) resolvem a sessao via snapshot com TTL de
    # registry.list() (api._list_snap). Os testes patcham app.api.registry.list POR teste (context
    # manager) -> sem este reset, o snapshot preenchido num teste vazaria pro seguinte dentro do
    # TTL de 1s (fakes de um teste respondendo no outro).
    from app import api, sse
    api._list_snap["snap"] = None
    sse._list_refresher.latest = None
    yield
    api._list_snap["snap"] = None
    sse._list_refresher.latest = None


@pytest.fixture(autouse=True)
def _reset_pair_ausencias():
    # Backstop: com o dict vazio no início do teste, nenhuma varredura dentro de UM teste chega
    # aos 5s — nenhum teste alcança o .hangar-pair real por esquecer o fixture por arquivo.
    from app.registry import SessionRegistry
    SessionRegistry._pair_ausencias.clear()
    yield
    SessionRegistry._pair_ausencias.clear()


# Recortes REAIS do material_colors.scss, medidos em 05/08/2026 trocando o papel de parede: um azul
# e um vermelho. Nao invente valores — a graca e provar que neutro frio vira neutro quente pelo
# mesmo caminho de codigo.
PALETA_AZUL = """$darkmode: True;
$transparent: False;
$primary_paletteKeyColor: #5A77AB;
$background: #111318;
$surface: #111318;
$surfaceContainerLow: #191C20;
$surfaceContainer: #1D2024;
$surfaceContainerHigh: #282A2F;
$onSurface: #E2E2E9;
$onSurfaceVariant: #C4C6D0;
$outline: #8E9099;
$outlineVariant: #44474E;
$primary: #AAC7FF;
$onPrimary: #0A305F;
"""

PALETA_VERMELHA = (PALETA_AZUL
                   .replace("#111318", "#1C110D").replace("#191C20", "#251915")
                   .replace("#1D2024", "#291D18").replace("#282A2F", "#342722")
                   .replace("#E2E2E9", "#F5DED6").replace("#C4C6D0", "#DFC0B5")
                   .replace("#8E9099", "#A78B81").replace("#44474E", "#58423A"))


@pytest.fixture
def paleta_azul():
    return PALETA_AZUL


@pytest.fixture
def paleta_vermelha():
    return PALETA_VERMELHA


@pytest.fixture
def models_cache_em_tmp(tmp_path, monkeypatch):
    # O cache de modelos tem espelho em DISCO dentro do config dir (.hangar-models.json):
    # sem o redirecionamento, os testes das rotas de model-options ESCREVEM no ~/.claude real de
    # quem roda a suite, e os testes seguintes leem esse cache no lugar do mock. NAO e autouse
    # global de proposito: a versao autouse derrubava os mocks de test_tmux quando rodava na
    # suite cheia (interacao de fixtures via test_codex_adapter); cada arquivo que exercita as
    # rotas liga o isolamento com um autouse local de uma linha (test_api.py e
    # test_model_options_api.py).
    from app import api

    def _path_de_teste(chave: str):
        # A chave e um CAMINHO (o config dir). Trocar so a barra bastava no POSIX; no Windows
        # sobram `\` e o `:` do drive, que sao invalidos em nome de arquivo — o teste morria com
        # WinError 123 antes de exercitar qualquer rota. `_sanitizar` tira tudo que nao serve pra
        # nome, e o proposito continua o mesmo: um arquivo por chave, dentro do tmp_path.
        seguro = "".join(c if c.isalnum() or c in "-._" else "_" for c in chave)
        return tmp_path / f"models-{seguro}.json"

    monkeypatch.setattr(api, "_models_cache_path", _path_de_teste)
    api._claude_models_cache.clear()
    yield
    api._claude_models_cache.clear()
