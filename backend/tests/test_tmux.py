import os
import shutil
import subprocess
import time
import uuid
from unittest.mock import MagicMock, patch

import pytest
from types import SimpleNamespace

from app import tmux

from tmux_teste import matar_servidor, matar_sessao, novo_socket


@pytest.fixture(autouse=True)
def _isola_o_cache_do_scope():
    """`tmux._scope_usavel` e cache de MODULO, e `_scope_probe` o grava com `global`.

    Varios casos deste arquivo trocam so o `RUN` e chamam algo que passa pelo `_scope_prefix()`.
    A sonda entao roda contra o MOCK, grava True no cache — e o `patch.object(tmux, "RUN", ...)`
    devolve o RUN de verdade no fim, mas NAO desfaz a gravacao. Do teste seguinte em diante, quem
    fala com o tmux de verdade herda um prefixo `systemd-run` decidido por uma mentira.

    Foi o que derrubava o `test_new_hidden_shell_config_dir_chega_no_ambiente_do_pane` quando o
    arquivo rodava inteiro (sozinho ele passava): o `new_hidden_shell` real ia embrulhado em
    systemd-run, falhava, e `alvo` vinha None. Na suite completa isso ficou escondido por acidente
    ate 21/08/2026, quando outra correcao tirou o andaime que zerava o cache sem querer.

    Sem `monkeypatch` de proposito: pedir fixture dentro de fixture autouse muda a ordem de
    setup/teardown de todo teste — e foi exatamente essa a armadilha do conftest (ver o comentario
    de la). Guardar e repor na mao nao mexe em ordem nenhuma.
    """
    anterior = tmux._scope_usavel
    yield
    tmux._scope_usavel = anterior


def test_list_sessions_parses_output():
    fake = MagicMock(stdout="cc\t/home/u/p\nweb\t/home/u/w\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake) as run:
        out = tmux.list_sessions()
    assert out == [
        {"name": "cc", "cwd": "/home/u/p"},
        {"name": "web", "cwd": "/home/u/w"},
    ]
    args = run.call_args[0][0]
    assert args[:2] == ["tmux", "list-sessions"]


def test_provider_de_nascimento_vai_no_terminal_e_volta_na_listagem(monkeypatch):
    monkeypatch.setattr(tmux, "_scope_prefix", lambda: [])
    fake = MagicMock(stdout="cx\t1\t123\t/tmp\t%9\t\tcodex\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake) as run:
        assert tmux.new_session("cx", "/tmp", "sleep 30", provider="codex")
        assert "CP_PROVIDER=codex" in run.call_args[0][0]
        assert tmux.list_panes_all()["cx"][0]["provider"] == "codex"
        assert "#{CP_PROVIDER}" in run.call_args[0][0][-1]


def test_list_sessions_empty_when_no_server():
    fake = MagicMock(stdout="", returncode=1, stderr="no server running")
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_sessions() == []


def test_timeout_do_tmux_nao_vira_lista_vazia():
    # Caso do Windows (25/08/2026): as sessoes sumiam todas do app com o servidor ainda verde.
    # `list-panes -a` estourando o timeout virava rc=1, que era lido como "zero sessoes" — e um
    # snapshot vazio VALIDO faz o SSE mandar `[]` pro app, apagando a lista. Agora levanta, e quem
    # ouve (o refresher da lista) segura o ultimo snapshot bom.
    with patch.object(tmux, "RUN", side_effect=subprocess.TimeoutExpired(["tmux"], 5)):
        for chamada in (tmux.list_panes_all, tmux.list_panes_active, tmux.list_sessions):
            with pytest.raises(tmux.MuxIndisponivel):
                chamada()


def test_timeout_do_tmux_nao_registra_o_argv(caplog):
    # O argv pode carregar o texto de um atalho (credencial): nem o log nem o stderr o repetem.
    args = ["tmux", "new-session", "-d", "--", "sh", "-c", "senha=SEGREDO123"]
    with patch.object(tmux, "RUN", side_effect=subprocess.TimeoutExpired(args, 5)):
        with caplog.at_level("DEBUG"):
            cp = tmux._run(args)
    assert cp.returncode == tmux.RC_INDISPONIVEL and "SEGREDO123" not in cp.stderr
    assert "SEGREDO123" not in caplog.text


def test_tmux_ausente_tambem_levanta():
    # FileNotFoundError (tmux fora do PATH) e a mesma classe de "nao sei", nao "nao ha sessao".
    with patch.object(tmux, "RUN", side_effect=FileNotFoundError("tmux")):
        with pytest.raises(tmux.MuxIndisponivel):
            tmux.list_panes_all()


def test_sem_servidor_rodando_continua_sendo_lista_vazia():
    # A contraparte: rc=1 vindo do PROPRIO tmux ("no server running") e resposta legitima de que
    # nao ha sessao nenhuma — levantar aqui trocaria a lista vazia correta por um erro na tela.
    fake = MagicMock(stdout="", returncode=1, stderr="no server running on /tmp/tmux-1000/default")
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_panes_all() == {}
        assert tmux.list_panes_active() == []
        assert tmux.list_sessions() == []


def test_falha_de_comando_que_nao_lista_segue_silenciosa():
    # So a listagem separa os dois casos. send_keys com o tmux travado continua devolvendo False,
    # sem levantar — levantar ali so trocaria um caminho de falha silencioso por outro.
    with patch.object(tmux, "RUN", side_effect=subprocess.TimeoutExpired(["tmux"], 5)):
        assert tmux.send_keys("cc", "oi") is False


def test_list_panes_active_parses_pane_id():
    # #{pane_id} e o 5o campo (Task 5 do adapter Pi: bilhete da extensao e por-pane). Uma linha com o
    # formato ANTIGO de 4 campos tem de ser rejeitada, senao um tmux desatualizado alimentaria pid/cwd
    # errados no lugar do pane_id.
    fake = MagicMock(stdout="cc\t1\t123\t/home/u/p\t%9\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_panes_active() == [
            {"name": "cc", "pid": 123, "cwd": "/home/u/p", "pane_id": "%9"},
        ]


def test_list_panes_active_rejects_the_old_four_field_format():
    fake = MagicMock(stdout="cc\t1\t123\t/home/u/p\n", returncode=0)
    with patch.object(tmux, "RUN", return_value=fake):
        assert tmux.list_panes_active() == []


def test_list_panes_all_tolerates_a_missing_hidden_field():
    # C1 da revisao final: `#{@cp_hidden}` e opcao de USUARIO no `-F`. Multiplexador que nao a
    # interpola (psmux) devolve 5 campos ou o literal cru — e daqui saem as tres views, o
    # list_with_state, o _pane_info e o _cwd_has_siblings. Perder a MARCA e chato (o shell
    # escondido vira card); perder a SESSAO seria o app vazio.
    stdout = ("cc\t1\t123\t/home/u/p\t%9\n"                  # 5 campos: sem a opcao
              "term-cc\t1\t124\t/home/u/p\t%10\t1\n"          # 6 campos: marcada
              "web\t1\t125\t/home/u/w\t%11\t#{@cp_hidden}\n")  # literal nao interpolado
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout=stdout, returncode=0)):
        out = tmux.list_panes_all()
    assert set(out) == {"cc", "term-cc", "web"}
    assert out["cc"][0]["hidden"] is False
    assert out["term-cc"][0]["hidden"] is True
    assert out["web"][0]["hidden"] is False
    assert out["cc"][0]["pane_id"] == "%9"       # os 5 primeiros campos seguem no lugar certo


def test_send_keys_literal_uses_dashdash():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "echo hi", literal=True)
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "-l", "--", "echo hi"]


@pytest.mark.skipif(os.name != "posix", reason="cliente de controle somente de leitura do tmux POSIX")
@pytest.mark.parametrize("key,literal", [("Texto de teste", True), ("-c", True), ("Enter", False), ("C-u", False)])
def test_send_keys_recovers_from_an_inferred_readonly_client(key, literal):
    refused = subprocess.CompletedProcess([], 1, stdout="", stderr="client is read-only\n")
    accepted = subprocess.CompletedProcess([], 0, stdout="", stderr="")
    with patch.object(tmux, "_pane_target", return_value="=cc:"), \
         patch.object(tmux, "RUN", side_effect=[refused, accepted]) as run:
        assert tmux.send_keys("cc", key, literal=literal) is True
    original, retry = [call.args[0] for call in run.call_args_list]
    assert retry == [*original[:2], "-c", "", *original[2:]]


@pytest.mark.parametrize("stderr", ["can't find pane\n", "permission denied\n", "invalid option\n"])
def test_send_keys_does_not_repeat_other_failures(stderr):
    with patch.object(tmux, "_pane_target", return_value="=cc:"), \
         patch.object(tmux, "RUN", return_value=subprocess.CompletedProcess([], 1, "", stderr)) as run:
        assert tmux.send_keys("cc", "Texto de teste", literal=True) is False
    assert run.call_count == 1


@pytest.mark.skipif(os.name != "posix", reason="recuperação do cliente inferido do tmux POSIX")
def test_readonly_client_recovery_stops_after_one_retry():
    refused = subprocess.CompletedProcess([], 1, stdout="", stderr="client is read-only\n")
    with patch.object(tmux, "_pane_target", return_value="=cc:"), \
         patch.object(tmux, "RUN", return_value=refused) as run:
        assert tmux.send_keys("cc", "Texto de teste", literal=True) is False
    assert run.call_count == 2


def test_explicit_readonly_client_is_not_overridden():
    refused = subprocess.CompletedProcess([], 1, stdout="", stderr="client is read-only\n")
    with patch.object(tmux, "RUN", return_value=refused) as run:
        result = tmux._run(["tmux", "send-keys", "-c", "observer", "-t", "%1", "Enter"])
    assert result.returncode == 1 and run.call_count == 1


def test_windows_does_not_use_the_posix_readonly_client_retry():
    refused = subprocess.CompletedProcess([], 1, stdout="", stderr="client is read-only\n")
    with patch.object(tmux, "os", SimpleNamespace(name="nt")), \
         patch.object(tmux, "RUN", return_value=refused) as run:
        result = tmux._run(["tmux", "send-keys", "-t", "%1", "Enter"])
    assert result.returncode == 1 and run.call_count == 1


@pytest.mark.skipif(os.name != "posix" or not shutil.which("tmux"), reason="exige tmux POSIX real")
def test_send_keys_reaches_real_pane_with_readonly_observer(monkeypatch):
    socket = novo_socket()
    command = ["tmux", "-L", socket]
    env = {key: value for key, value in os.environ.items() if key not in {"TMUX", "TMUX_PANE"}}
    observer = None

    def run(args, **kwargs):
        return subprocess.run([args[0], "-L", socket, *args[1:]], env=env, **kwargs)

    try:
        subprocess.run([*command, "-f", "/dev/null", "new-session", "-d", "-s", "send-test", "cat"],
                       env=env, check=True, capture_output=True)
        usage = subprocess.run([*command, "list-commands"], env=env, text=True,
                               check=True, capture_output=True).stdout
        if not any(line.startswith("send-keys ") and "-c target-client" in line for line in usage.splitlines()):
            pytest.skip("este tmux não infere um cliente para send-keys")
        observer = subprocess.Popen([*command, "-C", "attach-session", "-t", "send-test",
                                     "-f", "read-only,ignore-size,no-output"], env=env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 3
        while True:
            clients = subprocess.run([*command, "list-clients", "-F", "#{client_flags}"], env=env,
                                     capture_output=True, text=True, check=True).stdout
            if "read-only" in clients:
                break
            assert observer.poll() is None, "o observador saiu antes da reprodução"
            assert time.monotonic() < deadline, "o observador não se anexou ao servidor de teste"
            time.sleep(0.02)
        baseline = run(["tmux", "send-keys", "-t", "=send-test:", "-l", "--", "BASELINE"],
                       capture_output=True, text=True)
        if baseline.returncode == 0:
            pytest.skip("este tmux não recusa envios por causa do observador somente de leitura")
        assert baseline.returncode == 1 and baseline.stderr.strip() == "client is read-only"
        monkeypatch.setattr(tmux, "RUN", run)
        monkeypatch.setattr(tmux, "_pane_target", lambda _: "=send-test:")
        assert tmux.send_keys("send-test", "ENTREGA_REAL", literal=True) is True
        assert tmux.send_keys("send-test", "Enter") is True
        deadline = time.monotonic() + 3
        while True:
            pane = run(["tmux", "capture-pane", "-p", "-t", "=send-test:"],
                       capture_output=True, text=True).stdout
            if pane.splitlines().count("ENTREGA_REAL") >= 2:
                break
            assert time.monotonic() < deadline, "o cat não recebeu a linha submetida"
            time.sleep(0.02)
        assert "BASELINE" not in pane
        assert "read-only" in run(["tmux", "list-clients", "-F", "#{client_flags}"],
                                  capture_output=True, text=True).stdout
    finally:
        matar_servidor(socket)
        if observer is not None:
            observer.communicate(timeout=3)


@pytest.mark.skipif(os.name == "nt", reason="Windows usa o formato de evento de teclado para Esc")
def test_send_keys_named_key():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "Escape")
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "Escape"]


@pytest.mark.parametrize("key", ["Escape", "Esc"])
def test_windows_escape_uses_press_and_release_records(monkeypatch, key):
    monkeypatch.setattr(tmux, "os", SimpleNamespace(name="nt"))
    monkeypatch.setattr(tmux, "_pane_target", lambda name: "=cc:")
    with patch.object(tmux, "_run", return_value=MagicMock(returncode=0)) as run:
        assert tmux.send_keys("cc", key)
    assert run.call_args.args[0] == [
        "tmux", "send-keys", "-t", "=cc:", "-l", "--",
        "\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_",
    ]
    with patch.object(tmux, "_run", return_value=MagicMock(returncode=1)):
        assert not tmux.send_keys("cc", key)


@pytest.mark.skipif(os.name != "posix", reason="no Windows o Enter vai como tecla nomeada, nao como `-l -- \r` (tmux.py ramifica)")
def test_send_keys_enter_vira_cr_cru():
    # Enter NUNCA como nome de tecla: com extended-keys on no tmux (Shift+Enter do Pi), o "Enter"
    # nomeado sai no protocolo estendido e o composer do Claude Code engole o submit (regressão
    # 31/07). CR cru é o encoding legado que toda TUI aceita.
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)) as run:
        tmux.send_keys("cc", "Enter")
    assert run.call_args[0][0] == ["tmux", "send-keys", "-t", "=cc:", "-l", "--", "\r"]


def test_capture_pane_returns_stdout():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="screen", returncode=0)) as run:
        assert tmux.capture_pane("cc") == "screen"
    assert run.call_args[0][0][:2] == ["tmux", "capture-pane"]


def test_sessao_existe_distingue_timeout_de_morta():
    # Timeout do tmux e "nao sei", nao "morreu": o watcher do Codex derrubou o app-server de
    # todas as sessoes vivas num has-session que estourou o teto com a maquina carregada.
    def travado(args, **_kw):
        raise subprocess.TimeoutExpired(args, 5)
    with patch.object(tmux, "RUN", travado):
        assert tmux.sessao_existe("x") is None
        assert tmux.has_session("x") is False
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=1)):
        assert tmux.sessao_existe("x") is False
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0)):
        assert tmux.sessao_existe("x") is True


@pytest.mark.skipif(shutil.which("tmux") is None, reason="tmux nao instalado no ambiente")
def test_has_session_is_exact_against_real_tmux():
    # SEMANTICA REAL do tmux (nao mock): sem o `=`, o `-t` resolve exact -> fnmatch -> PREFIX match,
    # entao `has_session("X")` respondia VIVO por causa da IRMA "X-2" — e o /input dava "entregue"
    # digitando num pane que nao existe. Os outros testes stubam has_session, por isso nenhum via.
    # Socket proprio (-L) e sessao de nome aleatorio: nao encosta no tmux/sessoes do usuario.
    sock = novo_socket()
    base = f"pocket-{uuid.uuid4().hex[:6]}"

    def tmux_on_sock(args, **_kw):
        # injeta o `-L <socket>` logo apos o "tmux" -> o has_session real roda contra ESTE servidor
        return subprocess.run(["tmux", "-L", sock, *args[1:]], capture_output=True, text=True)

    subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", f"{base}-2", "sleep 60"],
                   capture_output=True, text=True)
    try:
        with patch.object(tmux, "RUN", tmux_on_sock):
            assert tmux.has_session(f"{base}-2") is True   # exata e viva
            assert tmux.has_session(base) is False         # NUNCA existiu (so a irma "-2") -> prefix mentia
            assert tmux.has_session(base[:-3]) is False    # prefixo puro
    finally:
        # "Matar a unica sessao ja encerra este servidor sozinho" era o que este comentario dizia,
        # e vale no tmux — nao no psmux, onde sobra um `-s __warm__` por socket, pra sempre. O
        # kill-server continua proibido solto: o `matar_servidor` exige o `-L` e confere a limpeza.
        matar_sessao(f"{base}-2", sock)
        matar_servidor(sock)


def test_pane_target_uses_exact_session_form():
    # Nome NUMERICO (0/1/2) nao pode virar indice de window -> `=NAME:` forca match exato de sessao.
    assert tmux._pane_target("0") == "=0:"
    assert tmux._pane_target("cc") == "=cc:"


def test_capture_pane_targets_exact_session():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="", returncode=0)) as run:
        tmux.capture_pane("0")
    assert "=0:" in run.call_args[0][0]


def test_pane_pid_targets_exact_session():
    with patch.object(tmux, "RUN", return_value=MagicMock(stdout="540144\n", returncode=0)) as run:
        assert tmux.pane_pid("0") == 540144
    assert "=0:" in run.call_args[0][0]


class _CP:
    returncode = 0
    stdout = ""
    stderr = ""


def test_new_session_forwards_explicit_config_dir():
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x", config_dir="/home/u/.claude-clean")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-clean" in captured["args"]


def test_new_session_falls_back_to_backend_config_dir(monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/home/u/.claude-work")
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "CLAUDE_CONFIG_DIR=/home/u/.claude-work" in captured["args"]


def _casa_home(monkeypatch, tmp_path):
    """Home falsa nos DOIS sistemas. `HOME` sozinho nao move o `Path.home()` no Windows (o
    `ntpath.expanduser` olha `USERPROFILE`, medido), e era por isso que o caso do config dir padrao
    vinha vermelho aqui desde sempre: ele comparava com a home de VERDADE da maquina."""
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("USERPROFILE", str(tmp_path))
    return tmp_path / ".claude"


def _args_de_new_session(monkeypatch, **kw):
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x", **kw)
    return captured["args"]


def _valores_de_config_dir(args):
    return [a for a in args if str(a).startswith("CLAUDE_CONFIG_DIR=")]


@pytest.mark.skipif(os.name != "posix", reason="o `exec` com o prefixo so existe no comando POSIX")
def test_new_session_conta_padrao_no_tmux_sai_sem_a_variavel(monkeypatch, tmp_path):
    """No TMUX, conta padrao = AUSENCIA da variavel, como no wrapper do terminal: sem `-e` (que
    faria o CLI ler `~/.claude/.claude.json` em vez de `~/.claude.json`) e com `env -u` no comando,
    que barra a variavel herdada do SERVIDOR tmux. Reproduzido a mao, o vazamento que o `env -u`
    impede:
        CLAUDE_CONFIG_DIR=/tmp/conta-a tmux -L t new-session -d -s um 'sleep 30'
        env -u CLAUDE_CONFIG_DIR tmux -L t new-session -d -s dois 'sh -c "echo $CLAUDE_CONFIG_DIR"'
        -> /tmp/conta-a
    """
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    _casa_home(monkeypatch, tmp_path)
    monkeypatch.setattr(tmux, "_pane_herda_env_do_chamador", lambda: False)
    args = _args_de_new_session(monkeypatch)
    assert _valores_de_config_dir(args) == []
    assert args[-1] == "exec env -u CLAUDE_CONFIG_DIR claude --session-id x"


@pytest.mark.skipif(os.name != "posix", reason="o `exec` com o prefixo so existe no comando POSIX")
def test_new_session_conta_escolhida_no_tmux_vai_por_e_sem_env_u(monkeypatch, tmp_path):
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    _casa_home(monkeypatch, tmp_path)
    monkeypatch.setattr(tmux, "_pane_herda_env_do_chamador", lambda: False)
    conta = str(tmp_path / ".claude-work")
    args = _args_de_new_session(monkeypatch, config_dir=conta)
    assert f"CLAUDE_CONFIG_DIR={conta}" in args
    assert args[-1] == "exec claude --session-id x"


def test_new_session_omite_config_dir_padrao_no_psmux(monkeypatch, tmp_path):
    """No psmux o pane herda o ambiente de QUEM CHAMA, entao o -e com o valor PADRAO nao defende de
    vazamento nenhum — e exportar a variavel e o proprio bug: pro Claude Code, config dir setado
    (mesmo apontando pro ~/.claude) significa ler o `.claude.json` de DENTRO dele, que nasce vazio.
    Sintoma: toda sessao criada pelo app no Windows caia na tela de boas-vindas, com a credencial
    intacta."""
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    _casa_home(monkeypatch, tmp_path)
    monkeypatch.setattr(tmux, "_pane_herda_env_do_chamador", lambda: True)
    assert _valores_de_config_dir(_args_de_new_session(monkeypatch)) == []


def test_new_session_manda_conta_escolhida_no_psmux(monkeypatch, tmp_path):
    """A guarda que fica de pe: conta ESCOLHIDA (valor != padrao) vai sempre, nos dois
    multiplexadores — e o unico jeito de a conta chegar no pane."""
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    _casa_home(monkeypatch, tmp_path)
    monkeypatch.setattr(tmux, "_pane_herda_env_do_chamador", lambda: True)
    conta = str(tmp_path / ".claude-work")
    assert f"CLAUDE_CONFIG_DIR={conta}" in _args_de_new_session(monkeypatch, config_dir=conta)


def test_new_session_manda_padrao_no_psmux_quando_o_backend_tem_a_variavel(monkeypatch, tmp_path):
    """Backend com CLAUDE_CONFIG_DIR proprio -> o -e vai mesmo valendo o padrao: o pane herda o
    ambiente do backend, entao OMITIR nao apaga a variavel de la — mandar explicito e o que garante
    que valeu o valor pedido, e nao o que o backend calhava de ter."""
    padrao = _casa_home(monkeypatch, tmp_path)
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / ".claude-outra"))
    monkeypatch.setattr(tmux, "_pane_herda_env_do_chamador", lambda: True)
    args = _args_de_new_session(monkeypatch, config_dir=str(padrao))
    assert f"CLAUDE_CONFIG_DIR={padrao}" in args


def test_e_config_dir_compara_pasta_e_nao_string(monkeypatch, tmp_path):
    """A comparacao com o padrao e por PASTA (normpath+normcase), nao por string crua: o mesmo
    ~/.claude escrito de outro jeito (`.` no meio, barra final) e o mesmo diretorio, e mandar o -e
    por causa da grafia traria o bug de volta."""
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    padrao = _casa_home(monkeypatch, tmp_path)
    monkeypatch.setattr(tmux, "_pane_herda_env_do_chamador", lambda: True)
    assert tmux._e_config_dir(os.path.join(str(tmp_path), ".", ".claude")) == []
    assert tmux._e_config_dir(str(padrao) + os.sep) == []
    assert tmux._e_config_dir(str(tmp_path / ".claude2")) != []


@pytest.mark.skipif(os.name != "posix", reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] sem olhar o env")
def test_scope_prefix_empty_without_runtime_dir(monkeypatch):
    # Sem XDG_RUNTIME_DIR (host nao-systemd) -> spawn direto, sem wrap.
    monkeypatch.delenv("XDG_RUNTIME_DIR", raising=False)
    assert tmux._scope_prefix() == []


@pytest.mark.skipif(os.name != "posix", reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] sempre")
def test_scope_prefix_wraps_when_systemd_available(monkeypatch):
    # Com runtime dir + systemd-run QUE FUNCIONA -> tmux nasce em scope proprio (fora do cgroup do
    # backend). O probe entrou depois deste teste: "systemd-run instalado" deixou de bastar, porque o
    # gerenciador do usuario pode recusar o scope e ai o wrap fazia toda criacao de sessao falhar.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: subprocess.CompletedProcess(args, 0))
    assert tmux._scope_prefix()[:3] == ["systemd-run", "--user", "--scope"]


@pytest.mark.skipif(os.name != "posix", reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] sempre")
def test_scope_lost_after_startup_creates_without_it(monkeypatch):
    # A sonda passou na subida; horas depois o gerenciador recusa o scope e toda criação falhava.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", True)
    calls = []

    def run(args, **k):
        calls.append(args)
        return subprocess.CompletedProcess(args, 1 if args[0] == "systemd-run" else 0, "", "")

    monkeypatch.setattr(tmux, "RUN", run)
    assert tmux.new_session("s", "/tmp", "claude --session-id x") is True
    assert calls[0][0] == "systemd-run" and calls[-1][:2] == ["tmux", "new-session"]
    assert tmux._scope_usavel is False


@pytest.mark.skipif(os.name != "posix", reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] sempre")
def test_scope_still_working_keeps_the_tmux_refusal(monkeypatch):
    # Nome repetido: o tmux recusa com o scope funcionando, e não há segunda tentativa sem ele.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", True)
    calls = []

    def run(args, **k):
        calls.append(args)
        return subprocess.CompletedProcess(args, 0 if args[-1] == "true" else 1, "", "duplicate session: s")

    monkeypatch.setattr(tmux, "RUN", run)
    assert tmux.new_session("s", "/tmp", "claude --session-id x") is False
    assert not any(c[:2] == ["tmux", "new-session"] for c in calls)
    assert sum(c[-1] == "true" for c in calls) == 1 and tmux._scope_usavel is True


@pytest.mark.skipif(os.name != "posix", reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] sempre")
def test_scope_timeout_is_not_retried(monkeypatch):
    # Sem resposta, o comando pode ter rodado dentro do escopo: repetir criaria a sessão duas vezes.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", True)
    calls = []

    def run(args, **k):
        calls.append(args)
        return subprocess.CompletedProcess(args, tmux.RC_INDISPONIVEL, "", "")

    monkeypatch.setattr(tmux, "RUN", run)
    assert tmux.new_session("s", "/tmp", "claude --session-id x") is False
    assert len(calls) == 1 and tmux._scope_usavel is True


def test_new_session_passes_wayland_display_from_env(monkeypatch):
    monkeypatch.setenv("WAYLAND_DISPLAY", "wayland-7")
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "WAYLAND_DISPLAY=wayland-7" in captured["args"]


def test_new_session_detects_wayland_socket(monkeypatch, tmp_path):
    # Backend como servico systemd nao tem WAYLAND_DISPLAY -> detecta o socket no runtime dir
    # (ignorando o .lock). Sem isto, wl-paste no pane falha e o paste de imagem no claude morre.
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    (tmp_path / "wayland-1").touch()
    (tmp_path / "wayland-1.lock").touch()
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert "WAYLAND_DISPLAY=wayland-1" in captured["args"]


def test_new_session_skips_wayland_without_socket(monkeypatch, tmp_path):
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert not any(str(a).startswith("WAYLAND_DISPLAY=") for a in captured["args"])


@pytest.mark.skipif(os.name != "posix", reason="o `exec` e so do POSIX: no psmux o comando roda direto no ConPTY, sem shell no meio")
def test_new_session_execs_command_so_claude_owns_tty(monkeypatch):
    # O comando vai prefixado com `exec`: o tmux roda via `fish -c`, e sem exec o fish ficaria como
    # dono do tty e o send-keys nao chegaria no claude. Com exec, o fish vira o claude (o `env` do
    # prefixo da conta padrao tambem so faz exec, sem ficar no meio).
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    captured = {}
    with patch.object(tmux, "RUN", lambda args, **k: (captured.update(args=args) or _CP())):
        tmux.new_session("s", "/tmp", "claude --session-id x")
    assert captured["args"][-1].startswith("exec ")
    assert captured["args"][-1].endswith("claude --session-id x")


# --- scrollback real vs tela alternada ------------------------------------------------------
# Claude Code roda em tela alternada, onde o tmux NAO acumula historico: pedir mais linhas nunca
# traz nada e a UI nao pode oferecer "carregar mais historico". A TUI do Codex sobe com
# --no-alt-screen, entao la o scrollback existe. O formato do display resolve os dois num comando so.

def test_pane_scrollback_zero_em_tela_alternada():
    from unittest.mock import patch
    from app import tmux as t
    with patch.object(t, "_run", return_value=SimpleNamespace(stdout="0\n", returncode=0)):
        assert t.pane_scrollback("sess") == 0


def test_pane_scrollback_devolve_historico_quando_existe():
    from unittest.mock import patch
    from app import tmux as t
    with patch.object(t, "_run", return_value=SimpleNamespace(stdout="1873\n", returncode=0)):
        assert t.pane_scrollback("sess") == 1873


def test_pane_scrollback_falha_de_tmux_e_zero_MAS_logada(caplog):
    # 0 = "sem historico" E tmux quebrado davam a MESMA resposta: a UI so escondia o botao e a falha
    # real (tmux ausente, travado no timeout, sessao zumbi) sumia. Segue devolvendo 0 pra UI nao
    # explodir, mas TEM que aparecer no log.
    from unittest.mock import patch
    from app import tmux as t
    with caplog.at_level("WARNING"), \
         patch.object(t, "_run",
                      return_value=SimpleNamespace(stdout="", stderr="no server running", returncode=1)):
        assert t.pane_scrollback("sess") == 0
    assert "no server running" in caplog.text


def test_capture_pane_falha_de_tmux_e_logada(caplog):
    # stdout vazio numa falha e indistinguivel de pane genuinamente vazio -> o /pane devolvia 200
    # com texto "" e ninguem sabia que o tmux falhou.
    from unittest.mock import patch
    from app import tmux as t
    with caplog.at_level("WARNING"), \
         patch.object(t, "_run",
                      return_value=SimpleNamespace(stdout="", stderr="session not found", returncode=1)):
        assert t.capture_pane("sess") == ""
    assert "session not found" in caplog.text


@pytest.mark.skipif(os.name != "posix",
                    reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] "
                           "sem sondar — o caso passaria por vacuidade, nao por merito")
def test_scope_prefix_sem_scope_quando_systemd_run_falha(monkeypatch):
    # systemd-run pode ESTAR instalado e ainda assim recusar criar scope transiente ("Failed to start
    # transient scope unit"). Sem este gate, TODA criacao de sessao morria com "falha ao criar sessao
    # no tmux" — o app parava de abrir sessao por causa de um detalhe de cgroup que e opcional.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: subprocess.CompletedProcess(args, 1))
    assert tmux._scope_prefix() == []


@pytest.mark.skipif(os.name != "posix",
                    reason="systemd-run so existe no POSIX; no Windows _scope_prefix devolve [] sempre")
def test_scope_prefix_usa_scope_quando_systemd_run_funciona(monkeypatch):
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: subprocess.CompletedProcess(args, 0))
    assert tmux._scope_prefix() == ["systemd-run", "--user", "--scope", "--collect", "-q", "--"]


@pytest.mark.skipif(os.name != "posix", reason="a sonda do scope so roda no POSIX; no Windows _scope_prefix sai antes de sondar")
def test_scope_probe_roda_uma_vez_so(monkeypatch):
    # O probe custa um fork; repetir a cada sessao seria desperdicio num estado que quase nunca muda.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    chamadas = []
    monkeypatch.setattr(tmux, "RUN",
                        lambda args, **k: (chamadas.append(args) or subprocess.CompletedProcess(args, 0)))
    tmux._scope_prefix()
    tmux._scope_prefix()
    tmux._scope_prefix()
    assert len(chamadas) == 1


def test_new_session_sem_systemd_run_quebrado_ainda_cria(monkeypatch):
    # O caminho completo: systemd-run recusando, new_session tem de montar o comando do tmux SEM o
    # prefixo em vez de falhar.
    monkeypatch.setenv("XDG_RUNTIME_DIR", "/run/user/1000")
    monkeypatch.setattr(tmux.shutil, "which", lambda _: "/usr/bin/systemd-run")
    monkeypatch.setattr(tmux, "_scope_usavel", None)
    vistos = []

    def _fake(args, **k):
        vistos.append(args)
        # o probe (…-- true) falha; o tmux de verdade passa
        codigo = 1 if args[-1] == "true" else 0
        return subprocess.CompletedProcess(args, codigo, stdout="", stderr="")

    monkeypatch.setattr(tmux, "RUN", _fake)
    assert tmux.new_session("s", "/tmp", "claude --session-id x") is True
    cmd = vistos[-1]
    assert cmd[0] == "tmux"
    assert "systemd-run" not in cmd


# --- fatiamento de literal grande no Windows (corte do inicio pela TUI no submit) -----------------
# No Windows a TUI do Claude Code come o COMECO de um send-keys -l acima de ~1120 chars (entra em
# modo paste e o Enter envia so a cauda). Foi o corte do prompt de pareamento (1220 -> so ~300
# finais, sem o "[de: hangar]"). O send_keys fatia em pedacos <= _WIN_CHUNK com pausa, SO no
# Windows. Estes testes travam as duas garantias: Windows fatia byte-exato; Linux fica intocado.

def _captura_run(monkeypatch):
    # Filtra o "list-panes" da resolucao de pane do agentpane (Task 1, _pane_target agora delega pra
    # ela): estes testes exercitam o fatiamento/placeholder do _send_literal, nao a resolucao de
    # alvo — essa tem cobertura propria em test_agentpane.py.
    chamadas = []

    def fake(args, **k):
        if args[1] != "list-panes":
            chamadas.append(list(args))
        return _CP()

    monkeypatch.setattr(tmux, "RUN", fake)
    return chamadas


def test_send_literal_curto_uma_chamada_so(monkeypatch):
    # Dentro do teto -> UMA chamada, identico ao de sempre (mesmo no Windows).
    monkeypatch.setattr(tmux.os, "name", "nt")
    chamadas = _captura_run(monkeypatch)
    tmux.send_keys("cc", "x" * 500, literal=True)
    assert chamadas == [["tmux", "send-keys", "-t", "=cc:", "-l", "--", "x" * 500]]


def test_send_literal_windows_fatia_e_e_byte_exato(monkeypatch):
    # > teto no Windows -> fatia. A CONCATENACAO dos pedacos tem de ser byte-exata (nada perdido,
    # nada duplicado, ordem preservada) e o INICIO (o que sumia) vai no primeiro pedaco.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)   # sem pausa real no teste
    chamadas = _captura_run(monkeypatch)
    texto = "".join(f"{i:04d}|" for i in range(1, 400))   # 1995 chars, marcado por posicao
    tmux.send_keys("cc", texto, literal=True)
    assert len(chamadas) > 1                                            # de fato fatiou
    assert all(c[:6] == ["tmux", "send-keys", "-t", "=cc:", "-l", "--"] for c in chamadas)
    pedacos = [c[6] for c in chamadas]
    assert all(len(p) <= tmux._WIN_CHUNK for p in pedacos)             # nenhum pedaco estoura o teto
    assert "".join(pedacos) == texto                                   # byte-exato
    assert pedacos[0].startswith("0001|")                              # o comeco vai primeiro


# --- psmux nao honra o `--`: pedaco que COMECA com '-' e engolido em silencio (rc=0) -------------
# Medido no psmux 3.3.7: send-keys -l -- "-X" e "--X" NAO chegam; "xX" e " -X" chegam -> o teste e o
# PRIMEIRO caractere. Como o rc vem 0 e o stderr vazio, NENHUMA checagem de erro pega: o pedaco some
# e o Enter submete o resto como se fosse a mensagem inteira. Aconteceu entre duas sessoes reais:
# recado de 2332 chars chegou com 1820, faltando exatamente 512 alinhados no chunk 2, que comecava
# com '-' — a sessao destino leu um texto emendado e discutiu tres rodadas em cima dele.

def test_fatiar_win_nenhum_pedaco_comeca_com_hifen():
    # Fronteira caindo numa corrida de hifens: a fronteira anda PRA FRENTE e engole a corrida.
    for texto in ("A" * 512 + "-" * 5 + "B" * 200,
                  "A" * 510 + "-" * 60 + "B" * 200,
                  ("x" * 100 + "-" * 3) * 12):
        pedacos = tmux._fatiar_win(texto)
        assert not any(p.startswith("-") for p in pedacos)
        assert "".join(pedacos) == texto                      # byte-exato
        assert all(len(p) <= tmux._WIN_CHUNK_MAX for p in pedacos)


def test_fatiar_win_corrida_maior_que_a_folga_aceita_a_fronteira():
    # Caso degenerado: hifens demais pra caber no teto. Prefere estourar a garantia do '-' a estourar
    # o teto de colapso (o placeholder do _send_literal cobre o pedaco que sobrar comecando com '-').
    texto = "A" * 500 + "-" * 400 + "B" * 100
    pedacos = tmux._fatiar_win(texto)
    assert "".join(pedacos) == texto
    assert all(len(p) <= tmux._WIN_CHUNK_MAX for p in pedacos)


def test_send_literal_texto_que_comeca_com_hifen_usa_placeholder(monkeypatch):
    # Texto INTEIRO comecando com '-': nao ha fronteira pra mover (o 1o pedaco comeca no indice 0).
    # Manda 'x' na frente e apaga no FIM com Home+DC. Vale tambem pro texto CURTO: o bug e
    # pre-existente ao fatiamento, mensagem de uma chamada so tambem e engolida.
    monkeypatch.setattr(tmux.os, "name", "nt")
    chamadas = _captura_run(monkeypatch)
    tmux.send_keys("cc", "-flag curta", literal=True)
    literais = [c[6] for c in chamadas if c[4:6] == ["-l", "--"]]
    assert literais == ["x-flag curta"]                       # placeholder na frente
    # Home+DC sao os ULTIMOS ENVIOS: feitos antes, o cursor volta a 0 e o resto entra no INICIO.
    # (Depois deles vem so a captura de verificacao, que nao digita nada.)
    envios = [c for c in chamadas if c[1] == "send-keys"]
    assert [c[4] for c in envios[-2:]] == ["Home", "DC"]
    assert chamadas[-1][1] == "capture-pane"   # confere se o 'x' saiu (rc do send-keys mente)


def test_send_literal_sem_hifen_nao_usa_placeholder(monkeypatch):
    # Sem '-' no inicio, nada muda: nem 'x' na frente, nem Home/DC. Protege o caminho comum.
    monkeypatch.setattr(tmux.os, "name", "nt")
    chamadas = _captura_run(monkeypatch)
    tmux.send_keys("cc", "texto normal", literal=True)
    assert chamadas == [["tmux", "send-keys", "-t", "=cc:", "-l", "--", "texto normal"]]


def test_send_literal_posix_ignora_o_hifen(monkeypatch):
    # O bug e do psmux. No Linux o `--` funciona -> nada de placeholder, byte-identico a sempre.
    monkeypatch.setattr(tmux.os, "name", "posix")
    chamadas = _captura_run(monkeypatch)
    tmux.send_keys("cc", "--flag no linux", literal=True)
    assert chamadas == [["tmux", "send-keys", "-t", "=cc:", "-l", "--", "--flag no linux"]]


def test_send_literal_posix_nunca_fatia(monkeypatch):
    # Decisao do dono do repo: no Linux o bug nao existe -> texto grande vai numa chamada SO, sem
    # pausa. O ramo posix fica byte-identico a hoje, risco zero.
    monkeypatch.setattr(tmux.os, "name", "posix")
    chamadas = _captura_run(monkeypatch)
    texto = "z" * 5000
    tmux.send_keys("cc", texto, literal=True)
    assert chamadas == [["tmux", "send-keys", "-t", "=cc:", "-l", "--", texto]]


class _CPFalha:
    returncode = 1
    stdout = ""
    stderr = "psmux: session not found"


def test_send_literal_para_no_primeiro_pedaco_que_falha(monkeypatch, caplog):
    # O laco ignorava o returncode de cada pedaco: se o 3o de 4 falhasse, ele seguia mandando o 4o e o
    # pane recebia 1+2+4 — texto do usuario com um buraco no meio, e o app dizendo "enviado". E a MESMA
    # classe do bug que o fatiamento existe pra evitar, auto-infligida. Agora para no 1o erro, devolve
    # False e LOGA qual pedaco morreu. Bool e nao excecao de proposito: os outros call sites de
    # send_keys(literal=True) (answer_questions, send_text, drain) nao tratariam uma excecao — viraria
    # 500 cru nas rotas /answer e /term-input, e o `except Exception` cego do drain a engoliria.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)
    chamadas = []

    def run_falhando_no_3(args, **k):
        # ignora o list-panes da resolucao de pane do agentpane (Task 1) -> a contagem continua
        # so sobre os pedacos do send-keys, que e o que este teste mede.
        if args[1] == "list-panes":
            return _CP()
        chamadas.append(list(args))
        return _CPFalha() if len(chamadas) == 3 else _CP()

    monkeypatch.setattr(tmux, "RUN", run_falhando_no_3)
    texto = "".join(f"{i:04d}|" for i in range(1, 400))   # 1995 chars -> 4 pedacos de 512
    with caplog.at_level("ERROR"):
        assert tmux.send_keys("cc", texto, literal=True) is False
    assert len(chamadas) == 3                     # PAROU: o 4o pedaco nunca foi mandado
    msg = " ".join(r.getMessage() for r in caplog.records)
    assert "3/4" in msg                           # diz QUAL pedaco falhou
    assert "1024 de 1995 chars" in msg            # e quanto ja esta no input do pane


def test_send_literal_ok_devolve_true(monkeypatch):
    # O par: fatiou tudo sem erro -> True, pra send_prompt seguir pro Enter.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: _CP())
    assert tmux.send_keys("cc", "z" * 2000, literal=True) is True


def test_send_literal_uma_chamada_que_falha_loga_e_devolve_false(monkeypatch, caplog):
    # Chamada UNICA (Linux, ou Windows dentro do teto): falhar significa que NADA entrou, nao ha meia
    # mensagem no input -> mantem o comportamento historico de degradar, mas registra. Sem o log,
    # `send_prompt` devolvendo "sent" e indistinguivel de entrega de verdade.
    monkeypatch.setattr(tmux.os, "name", "posix")
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: _CPFalha())
    with caplog.at_level("WARNING"):
        assert tmux.send_keys("cc", "oi", literal=True) is False   # nao levanta, devolve False
    assert any("send-keys -l falhou" in r.getMessage() for r in caplog.records)


def test_kill_session_devolve_se_a_sessao_saiu(monkeypatch):
    # O contrato e "a sessao SAIU?", nao "o comando deu 0" — o kill-session do psmux devolve 0 com a
    # sessao ainda de pe (medido), e no caso do prefix match o comando falha porque ela JA estava
    # morta, que e sucesso pra quem chama.
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: _CP())
    monkeypatch.setattr(tmux, "has_session", lambda n: False)
    assert tmux.kill_session("cc") is True          # saiu (ou nem existia): sucesso

    monkeypatch.setattr(tmux, "RUN", lambda args, **k: _CP())   # rc=0, mentindo
    monkeypatch.setattr(tmux, "has_session", lambda n: True)
    assert tmux.kill_session("cc") is False         # sobreviveu: NAO e sucesso


def test_fronteiras_nunca_deixam_pedaco_comecar_com_dash():
    # O psmux nao honra o `--`: argumento que comeca com "-" e engolido em SILENCIO com rc=0. Medido
    # na pratica: recado de 2332 chars chegou com 1820, faltando EXATAMENTE 512 (o chunk), com o
    # buraco em [512:1024] — o chunk 2 comecava com "- trunca no primeiro". Como o rc e 0, nenhuma
    # checagem nossa pega; a unica saida e nao produzir esse pedaco.
    texto = "x" * 511 + "-comeca com dash" + "y" * 600
    pedacos = tmux._fatiar_win(texto)
    assert "".join(pedacos) == texto                       # byte-exato: so a divisao muda
    assert not any(p.startswith("-") for p in pedacos[1:])  # nenhum pedaco comeca com dash


def test_fronteiras_parede_de_dash_devolve_o_corte_original():
    # Pior caso: nao ha fronteira boa por perto. O teto do recuo devolve o corte de sempre — pior caso
    # e o comportamento de hoje, nunca pior (e o pedaco perdido ao menos vira 1 so, nao um laco).
    texto = "a" * 500 + "-" * 200
    pedacos = tmux._fatiar_win(texto)
    assert "".join(pedacos) == texto


def test_fronteiras_avanca_quando_nao_ha_saida_pra_tras():
    # Regua markdown de 40 hifens em cima da fronteira: recuar nao acha saida (o recuo pararia dentro
    # da regua), entao avanca. Cabe porque o teto real e o colapso de paste (700 ok / 900 colapsa),
    # nao os 512 — medicao da sessao-irma no Windows.
    texto = "x" * 500 + "-" * 40 + "resto do texto aqui" * 20
    pedacos = tmux._fatiar_win(texto)
    assert "".join(pedacos) == texto
    assert not any(p.startswith("-") for p in pedacos[1:])


class _PsmuxFalso:
    """RUN que imita o psmux MEDIDO: argumento comecando com hifen e ENGOLIDO (rc=0, nada chega).

    O teste antigo so conferia a ORDEM das chamadas com um mock que aceitava tudo — nao reproduzia o
    proprio bug que a receita existe pra resolver, entao dava falso senso de cobertura. Achado no
    review: com o placeholder numa chamada SEPARADA, o payload continuava sendo engolido e o teste
    passava.
    """

    def __init__(self):
        self.entregue: list[str] = []

    def __call__(self, args, **k):
        if args[1] == "send-keys" and "-l" in args:
            texto = args[-1]
            if not texto.startswith("-"):        # psmux: comeca com hifen -> some, calado
                self.entregue.append(texto)
        return _CP()


def test_texto_comecando_com_hifen_chega_inteiro_no_psmux_falso(monkeypatch):
    # O que importa nao e a ordem das chamadas, e o TEXTO chegar. Com o psmux falso, a versao com o
    # placeholder em chamada separada entregava so o "x".
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)
    psmux = _PsmuxFalso()
    monkeypatch.setattr(tmux, "RUN", psmux)
    texto = "-v ativa o modo verbose e mais um tanto de texto pra ficar realista"
    tmux.send_keys("cc", texto, literal=True)
    # o placeholder vai colado: o que chega e "x" + texto, e o Home/DC apaga o "x" no pane
    assert "".join(psmux.entregue) == "x" + texto


def test_texto_longo_comecando_com_hifen_nao_perde_o_primeiro_pedaco(monkeypatch):
    # O cenario que o review descreveu como critico: texto longo comecando com hifen perdia ~692 chars
    # do INICIO e a checagem de cauda nao via, porque a cauda vinha de um pedaco que chegou.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)
    psmux = _PsmuxFalso()
    monkeypatch.setattr(tmux, "RUN", psmux)
    texto = "-" + "".join(f"{i:04d}|" for i in range(1, 400))     # 1996 chars, marcado por posicao
    tmux.send_keys("cc", texto, literal=True)
    assert "".join(psmux.entregue) == "x" + texto   # nada perdido, ordem preservada


def test_texto_comecando_com_hifen_usa_placeholder_e_apaga_no_fim(monkeypatch):
    # Sem fronteira pra mover: o pedaco 1 comeca onde comeca. Receita medida e validada no psmux pela
    # sessao-irma — placeholder, texto, e Home+DC como ULTIMO passo (antes disso o cursor voltaria pro
    # inicio e o resto entraria embaralhado). Cobre tambem o caso PRE-EXISTENTE ao fatiamento:
    # mensagem curta de uma chamada so comecando com hifen ja se perdia.
    monkeypatch.setattr(tmux.os, "name", "nt")
    monkeypatch.setattr(tmux.time, "sleep", lambda *_a, **_k: None)
    chamadas = []
    # filtra o list-panes da resolucao de pane do agentpane (Task 1) -> so sobra o que este teste
    # quer medir: a sequencia de teclas do proprio _send_literal.
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: (
        chamadas.append(list(args)) if args[1] != "list-panes" else None) or _CP())
    tmux.send_keys("cc", "-comeca com hifen, curto", literal=True)
    teclas = [c[-1] for c in chamadas]
    assert teclas[0].startswith("x")                     # placeholder COLADO no 1o pedaco
    # Home e DC DEPOIS do texto (a ordem e o ponto: feito antes, o cursor volta pro inicio e o resto
    # entra embaralhado). Nao exijo que sejam as ULTIMAS chamadas: a versao do par ainda faz uma
    # captura de verificacao depois, e isso e dela.
    assert teclas.index("Home") > 0 and teclas.index("DC") > teclas.index("Home")
    assert teclas[0] == "x-comeca com hifen, curto"


def test_texto_normal_nao_usa_placeholder(monkeypatch):
    monkeypatch.setattr(tmux.os, "name", "nt")
    chamadas = []
    # filtra o list-panes da resolucao de pane do agentpane (Task 1), mesma razao do teste acima.
    monkeypatch.setattr(tmux, "RUN", lambda args, **k: (
        chamadas.append(list(args)) if args[1] != "list-panes" else None) or _CP())
    tmux.send_keys("cc", "texto normal", literal=True)
    assert [c[-1] for c in chamadas] == ["texto normal"]   # uma chamada so, nada de Home/DC


def test_session_created_ok():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0, stdout="1753970000\n")) as run:
        assert tmux.session_created("cc") == 1753970000.0
    assert run.call_args[0][0] == ["tmux", "display-message", "-p", "-t", "=cc:", "#{session_created}"]


def test_session_created_falha_vira_zero():
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=1, stdout="")):
        assert tmux.session_created("cc") == 0.0


def test_session_created_nao_numerico_vira_zero():
    # psmux/variantes podem imprimir lixo: corte inventado seria pior que sem corte.
    with patch.object(tmux, "RUN", return_value=MagicMock(returncode=0, stdout="abc\n")):
        assert tmux.session_created("cc") == 0.0


# --- load-buffer pela stdin: tira o teto de 16344 bytes do comando (Task 3) -----------------------

def test_run_passa_stdin_quando_pedido():
    # patch.object(tmux, "RUN"), nao "app.tmux.subprocess.run": RUN e a UNICA costura de mock do
    # modulo, e o mock devolve bytes (stdout=b"ok") porque em producao, sem text=True, e sempre bytes
    # que capture_output devolve — o contrato que este teste prova e que _run decodifica pra str.
    with patch.object(tmux, "RUN") as sr:
        sr.return_value = SimpleNamespace(returncode=0, stdout=b"ok", stderr=b"")
        cp = tmux._run(["tmux", "load-buffer", "-b", "p", "-"], input=b"abc")
    assert sr.call_args.kwargs["input"] == b"abc"
    # `text=True` e `input=bytes` sao incompativeis: quem manda bytes tem que sair do modo texto.
    assert sr.call_args.kwargs.get("text") is not True
    assert cp.stdout == "ok"   # decodificado: o retorno continua str pros chamadores de hoje


def test_paste_text_usa_load_buffer_pela_stdin():
    # `set-buffer -- <texto>` paga o teto de 16344 bytes do comando (medido: rc=1 "command too
    # long"). `load-buffer -` manda o texto pela stdin e some com o teto.
    grande = "x" * 50_000
    chamadas = []

    def fake_run(args, input=None):
        chamadas.append((args, input))
        return SimpleNamespace(returncode=0, stdout="", stderr="")

    with patch("app.tmux._run", side_effect=fake_run), \
         patch("app.tmux.buffer_trunca_no_newline", return_value=False):
        assert tmux.paste_text("cc", grande) is True
    assert chamadas[0][0][:3] == ["tmux", "load-buffer", "-b"]
    assert chamadas[0][0][3].startswith("cp-prompt-")
    assert chamadas[0][0][4] == "-"
    assert chamadas[0][1] == grande.encode()
    assert "set-buffer" not in " ".join(chamadas[0][0])


def test_paste_text_falha_no_load_buffer_nao_cola_nada():
    # load-buffer falhando (disco cheio, tmux morto) nao pode seguir pro paste-buffer: colaria o
    # buffer ANTERIOR, que e texto de outra mensagem.
    def fake_run(args, input=None):
        rc = 1 if "load-buffer" in args else 0
        return SimpleNamespace(returncode=rc, stdout="", stderr="erro")

    with patch("app.tmux._run", side_effect=fake_run), \
         patch("app.tmux.buffer_trunca_no_newline", return_value=False), \
         patch("app.tmux._paste_linha_a_linha", return_value=True) as plinha:
        assert tmux.paste_text("cc", "linha 1\nlinha 2") is True
    assert plinha.called


def test_list_panes_all_reads_session_birth_in_same_batch():
    output = "one\t1\t12\t/tmp\t%1\t\tclaude\t100\n" + "two\t1\t13\t/tmp\t%2\t\tcodex\t200\n"
    with patch.object(tmux, "_run", return_value=MagicMock(stdout=output, returncode=0)) as run:
        rows = tmux.list_panes_all()
    assert rows["one"][0]["session_created"] == 100
    assert rows["two"][0]["session_created"] == 200
    assert run.call_count == 1
    assert "#{session_created}" in run.call_args.args[0][-1]


@pytest.mark.parametrize("birth", ["", "#{session_created}", "invalid", "0", "-1"])
def test_list_panes_all_unknown_birth_does_not_invent_life(birth):
    output = f"one\t1\t12\t/tmp\t%1\t\tclaude\t{birth}\n"
    with patch.object(tmux, "_run", return_value=MagicMock(stdout=output, returncode=0)):
        row = tmux.list_panes_all()["one"][0]
    assert row["pid"] == 12 and row["session_created"] is None
