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


@pytest.fixture(scope="session", autouse=True)
def _git_sem_manutencao_automatica():
    # Depois de um commit o git dispara `maintenance run --auto` em segundo plano, que cria e apaga
    # `.git/objects/maintenance.lock` enquanto o teste copia ou apaga o repositório: com a suíte em
    # paralelo, o copytree pegava o arquivo sumindo. Nenhum teste depende da manutenção.
    chaves = {"maintenance.auto": "false", "gc.auto": "0"}
    nomes = ["GIT_CONFIG_COUNT", *(f"GIT_CONFIG_{t}_{i}" for i in range(len(chaves)) for t in ("KEY", "VALUE"))]
    anterior = {nome: os.environ.get(nome) for nome in nomes}
    os.environ["GIT_CONFIG_COUNT"] = str(len(chaves))
    for i, (chave, valor) in enumerate(chaves.items()):
        os.environ[f"GIT_CONFIG_KEY_{i}"], os.environ[f"GIT_CONFIG_VALUE_{i}"] = chave, valor
    try:
        yield
    finally:
        for nome, valor in anterior.items():
            if valor is None:
                os.environ.pop(nome, None)
            else:
                os.environ[nome] = valor


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
    # Os sidecars REAIS são das sessões vivas desta máquina. Session-scoped: os testes que trocam
    # `_dir` pelo monkeypatch voltam pra este diretório, nunca pro real.
    from app.adapters.claude_headless import sessions
    original = sessions._dir
    pasta = tmp_path_factory.mktemp("claude-headless")
    sessions._dir = lambda: pasta
    try:
        yield
    finally:
        sessions._dir = original


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
def _sem_orquestracoes_reais(tmp_path_factory):
    # O Rust resolve o cofre pela casa do processo; um patch no módulo Python não o isolaria.
    pasta = tmp_path_factory.mktemp("rust-home")
    nomes = ("HOME", "USERPROFILE", "CARGO_HOME", "RUSTUP_HOME")
    anteriores = {nome: os.environ.get(nome) for nome in nomes}
    # A casa de dados muda; a instalação do compilador e seu cache continuam os do preparo.
    os.environ.setdefault("CARGO_HOME", str(Path.home() / ".cargo"))
    os.environ.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    for nome in ("HOME", "USERPROFILE"):
        os.environ[nome] = str(pasta)
    try:
        yield
    finally:
        for nome, valor in anteriores.items():
            if valor is None:
                os.environ.pop(nome, None)
            else:
                os.environ[nome] = valor


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


@pytest.fixture
def hangar_server_bin(monkeypatch, tmp_path):
    """O `hangar-server` desta cópia (`scripts/cargo-pinned build -p hangar-server`) para o que passa
    pelo `hangar-server conta`; nunca o `~/.hangar/bin` da casa."""
    binary = Path(__file__).resolve().parents[2] / "crates" / "target" / "debug" / (
        "hangar-server.exe" if os.name == "nt" else "hangar-server")
    assert binary.is_file(), f"compile antes: scripts/cargo-pinned build -p hangar-server ({binary})"
    monkeypatch.setenv("CP_RUST_SERVER_BIN", str(binary))
    # A conta e a lista fixa de quem roda a suíte não chegam ao subprocesso.
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    monkeypatch.delenv("CP_CLAUDE_CONFIG_DIRS", raising=False)
    # No Windows o diário do subprocesso sai do LOCALAPPDATA, que o HOME de teste não troca.
    monkeypatch.setenv("LOCALAPPDATA", str(tmp_path / "localappdata"))
    return binary


@pytest.fixture
def forwarded_guest():
    """Identidade já verificada na borda; a assinatura tem prova em test_guest_header."""
    from contextlib import contextmanager
    from app import guest_header

    @contextmanager
    def scope():
        with pytest.MonkeyPatch.context() as patch:
            patch.setattr(guest_header, "_verify", lambda request: guest_header.GuestView({"s": "share"}))
            yield
    return scope
