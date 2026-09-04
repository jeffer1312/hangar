import os
import tempfile
from pathlib import Path

import pytest


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
    # `diag._base()` lê CLAUDE_CONFIG_DIR/~/.claude e a suíte escrevia no diário REAL da máquina:
    # 361 `pergunta.fallback_texto` da sessão `s1` (test_api.py) numa semana de uso exportada,
    # indistinguíveis de picker preso de verdade. Sem `monkeypatch`/`tmp_path` na assinatura —
    # ver a armadilha de ordem de fixtures documentada em `_instalar_home_do_windows`.
    from app import diag
    original = diag._base
    # `test_diag.py` isola pelo próprio CLAUDE_CONFIG_DIR (um por teste): só o fallback `~/.claude`
    # é trocado, senão aquela suíte inteira passaria a compartilhar um diário só.
    def _base_de_teste() -> Path:
        v = os.environ.get("CLAUDE_CONFIG_DIR")
        return Path(v) / ".hangar-diag" if v and v != _CONFIG_DIR_HERDADO else _DIARIO_DE_TESTE
    diag._base = _base_de_teste
    yield
    diag._base = original


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
def _reset_list_snapshot():
    # Endpoints quentes (history/workflows) resolvem a sessao via snapshot com TTL de
    # registry.list() (api._list_snap). Os testes patcham app.api.registry.list POR teste (context
    # manager) -> sem este reset, o snapshot preenchido num teste vazaria pro seguinte dentro do
    # TTL de 1s (fakes de um teste respondendo no outro).
    from app import api
    api._list_snap["snap"] = None
    yield
    api._list_snap["snap"] = None


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
