"""Cota da conta do Codex no painel (app/cotas.py, fonte `codex`).

A leitura tenta primeiro `/wham/usage` do backend do ChatGPT (a rota que o próprio binário chama
com o token da conta) e cai no `account/rateLimits/read` de um app-server efêmero quando ela não
serve. Os testes antigos usam token que não é JWT, então caem direto no app-server, e a I/O
trocada neles continua sendo o `codex_appserver.perguntar`.
"""
import io
import json

import pytest

from app import codex_appserver, codex_contas, cotas

# Cópia da resposta real desta máquina em 30/08/2026 (campos que não usamos foram cortados).
# Detalhes que quebram parser ingênuo: o percentual já vem PRONTO (`usedPercent`, não used/cap), a
# janela se identifica pela DURAÇÃO em minutos, e `resetsAt` é epoch em SEGUNDOS — não em
# milissegundos como no CommandCode.
_RATE_LIMITS = {
    "rateLimits": {
        "limitId": "codex",
        "primary": {"usedPercent": 5, "windowDurationMins": 300, "resetsAt": 1788107727},
        "secondary": {"usedPercent": 1, "windowDurationMins": 10080, "resetsAt": 1788655220},
        "credits": {"hasCredits": False, "unlimited": False, "balance": "0"},
        "planType": "plus",
    },
}


@pytest.fixture(autouse=True)
def _cache_limpo():
    """A presença da credencial é cacheada pelo mtime (custo do tick do SSE). Sem zerar, um caso
    que muda o HOME herdaria a resposta do anterior e passaria por acidente."""
    cotas._cred_codex_cache = None
    cotas._codex_auth_cache = None
    yield
    cotas._cred_codex_cache = None
    cotas._codex_auth_cache = None


def _home(monkeypatch, alvo):
    """Quem resolve a pasta do Codex é o `codex_appserver`, não o `cotas` — patchar ali é o que diz
    a verdade sobre o caminho testado (`cotas.Path` é a mesma classe e funcionaria por acidente).
    O delenv anda junto: `CODEX_HOME` exportado na máquina de quem roda furaria o home falso."""
    monkeypatch.delenv("CODEX_HOME", raising=False)
    monkeypatch.setattr(codex_appserver.Path, "home", staticmethod(lambda: alvo))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", alvo / ".codex")
    # A conta padrão só aparece com o Codex instalado ou `~/.codex` existente; estes testes são
    # sobre credencial, não sobre instalação — e a máquina do CI não tem o binário.
    monkeypatch.setattr(codex_contas.shutil, "which", lambda nome: "/usr/bin/codex")


def _auth(home, tokens=True):
    d = home / ".codex"
    d.mkdir(parents=True, exist_ok=True)
    corpo = {"auth_mode": "chatgpt", "OPENAI_API_KEY": None}
    if tokens:
        corpo["tokens"] = {"access_token": "at-x", "refresh_token": "rt-x", "account_id": "acc-1"}
    (d / "auth.json").write_text(json.dumps(corpo), encoding="utf-8")
    return d


@pytest.fixture
def com_credencial(monkeypatch, tmp_path):
    """`_ler_codex` checa a credencial ANTES de perguntar: sem apontar o HOME pra um auth.json
    fabricado, estes testes liam o ~/.codex REAL da máquina — passavam onde há login do Codex e
    quebravam no CI (sem credencial, `sem_credencial` sai antes do mock de `perguntar`)."""
    _auth(tmp_path)
    _home(monkeypatch, tmp_path)


def test_le_as_duas_janelas(monkeypatch, com_credencial):
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw:_RATE_LIMITS)
    estado, janelas, motivo = cotas._ler_codex()
    assert (estado, motivo) == ("lida", None)
    assert [(j.rotulo, j.pct) for j in janelas] == [("5h", 5.0), ("7d", 1.0)]
    # Segundos, não milissegundos: dividir por 1000 aqui poria o reset em 1970.
    assert janelas[0].reset_ts == 1788107727


def test_le_redefinicoes_guardadas_com_expiracao(monkeypatch, com_credencial):
    resposta = {
        **_RATE_LIMITS,
        "rateLimitResetCredits": {
            "availableCount": 2,
            "credits": [
                {"id": "reset-1", "grantedAt": 1789000000, "expiresAt": 1789600000,
                 "resetType": "codexRateLimits", "status": "available",
                 "title": "Reset", "description": "Restaura os limites"},
                {"id": "reset-2", "grantedAt": 1789000100, "expiresAt": None,
                 "resetType": "codexRateLimits", "status": "available",
                 "title": None, "description": None},
            ],
        },
    }
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw: resposta)

    estado, _janelas, motivo, redefinicoes = cotas._ler_codex_detalhada()

    assert (estado, motivo) == ("lida", None)
    assert redefinicoes.available_count == 2
    assert redefinicoes.credits[0].model_dump() == {
        "id": "reset-1", "expires_at": 1789600000, "title": "Reset",
        "description": "Restaura os limites", "status": "available",
    }


def test_pergunta_o_metodo_de_cota(monkeypatch, com_credencial):
    vistos = []
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda m, **kw:(vistos.append(m), _RATE_LIMITS)[1])
    cotas._ler_codex()
    assert vistos == ["account/rateLimits/read"]


def test_janela_ausente_some_em_vez_de_zerar(monkeypatch, com_credencial):
    """Conta sem a janela semanal não pode desenhar 0% — 0% é uma afirmação, e falsa."""
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw:{
        "rateLimits": {"primary": _RATE_LIMITS["rateLimits"]["primary"], "secondary": None}})
    estado, janelas, _ = cotas._ler_codex()
    assert (estado, [j.rotulo for j in janelas]) == ("lida", ["5h"])


def test_resposta_sem_janela_nenhuma_nao_e_lida(monkeypatch, com_credencial):
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw:{"rateLimits": {}})
    assert cotas._ler_codex() == ("indisponivel", [], "formato-desconhecido")


def test_binario_ausente_tem_motivo_proprio(monkeypatch, com_credencial):
    """"não achei o codex" não é "o codex falhou" — e nenhum dos dois pode derrubar a lista das
    outras contas."""
    def some(m, **kw):
        raise codex_appserver.CodexAusente("nao achei o executavel `codex`")
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", some)
    assert cotas._ler_codex() == ("indisponivel", [], "codex-ausente")


@pytest.mark.parametrize("erro", [RuntimeError("nao respondeu"), OSError("boom")])
def test_falha_de_leitura_nao_levanta(monkeypatch, erro, com_credencial):
    def quebra(m, **kw):
        raise erro
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", quebra)
    estado, janelas, motivo = cotas._ler_codex()
    assert (estado, janelas) == ("indisponivel", [])
    assert motivo == "sem-resposta"


def test_credencial_nova_no_disco_e_notada(monkeypatch, tmp_path):
    """O cache é pelo mtime, e a leitura roda por sessão a cada varredura: fazer login no Codex
    não pode exigir reiniciar o backend pra a linha aparecer."""
    _home(monkeypatch, tmp_path)
    monkeypatch.setattr(cotas, "_codex_auth_cache",
                        lambda home: {"method": "none", "status": "disconnected"})
    cotas._cred_codex_cache = None
    assert cotas.id_conta_codex() is None
    _auth(tmp_path)
    assert cotas.id_conta_codex() is not None


def test_o_teto_de_tempo_e_o_das_outras_fontes(monkeypatch, com_credencial):
    """`_atualizar` espera TODAS as leituras juntas: uma fonte com teto maior que as outras vira o
    tempo de resposta do `/api/cotas` inteiro. O padrão do módulo (30s) é do catálogo, que é tela
    aberta por gente."""
    vistos = []
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda m, timeout: (vistos.append(timeout), _RATE_LIMITS)[1])
    cotas._ler_codex()
    assert vistos == [cotas._HTTP_TIMEOUT]


def test_sem_credencial_no_disco_nem_pergunta(monkeypatch, tmp_path):
    """Cobre a corrida (logout entre montar a fonte e ler): perguntar custa um processo de ~1,2s, e
    sem `auth.json` com tokens a linha diz "não há credencial" em vez de "falhou"."""
    _auth(tmp_path, tokens=False)
    _home(monkeypatch, tmp_path)
    monkeypatch.setattr(cotas, "_codex_auth_cache",
                        lambda home: {"method": "none", "status": "disconnected"})
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda m, **kw:pytest.fail("nao podia perguntar sem credencial"))
    assert cotas._ler_codex() == ("sem_credencial", [], None)


def test_sem_auth_json_e_sem_identidade_cacheada_nao_abre_app_server(monkeypatch, tmp_path):
    _home(monkeypatch, tmp_path)
    monkeypatch.setattr(cotas, "_codex_auth_cache", None)
    monkeypatch.setattr(cotas.codex_appserver, "perguntar",
                        lambda method, **kwargs: pytest.fail("nao podia consultar cota sem credencial"))

    assert cotas._tem_credencial_codex() is False
    assert cotas._ler_codex() == ("sem_credencial", [], None)


def test_codex_home_manda_no_caminho(monkeypatch, tmp_path):
    """Quem move a pasta do Codex move a credencial junto — o mesmo `CODEX_HOME` que o lançador
    respeita."""
    outro = tmp_path / "alhures"
    _auth(outro)
    _home(monkeypatch, tmp_path / "vazio")
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", outro / ".codex")
    assert cotas.id_conta_codex() == f"codex:{(outro / '.codex').resolve()}"


def _app_server_falso(monkeypatch, linhas: list[str], stderr: str = ""):
    """Um `codex app-server` de mentira: devolve as linhas dadas no stdout e nunca roda nada."""
    class ProcFalso:
        def __init__(self):
            self.stdin = io.StringIO()
            self.stdout = iter(linhas)
            self.stderr = io.StringIO(stderr)
            self.morto = False

        def kill(self):
            self.morto = True

        def wait(self, timeout=None):
            return 0

    proc = ProcFalso()
    monkeypatch.setattr(codex_appserver, "_binario", lambda: "codex")
    monkeypatch.setattr(codex_appserver.subprocess, "Popen", lambda *a, **kw: proc)
    return proc


def test_erro_do_app_server_vira_o_motivo_real(monkeypatch):
    """Resposta de ERRO é resposta.

    O laço só casava `result`, então um `error` JSON-RPC legítimo não casava nada, a leitura seguia
    até o EOF e quem chamou ouvia "nao respondeu" — para um servidor que respondeu, dizendo o porquê.
    Como isto alimenta a cota e o catálogo, o motivo real sumia do log."""
    _app_server_falso(monkeypatch, [
        json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}}) + "\n",
        json.dumps({"jsonrpc": "2.0", "id": 2,
                    "error": {"code": -32601, "message": "sem credencial"}}) + "\n",
    ])
    with pytest.raises(RuntimeError, match="sem credencial"):
        codex_appserver.perguntar("account/rateLimits/read")


def test_resposta_boa_continua_passando(monkeypatch):
    """Contra-prova do teste acima: o ramo novo não pode roubar o caminho feliz."""
    _app_server_falso(monkeypatch, [
        json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}}) + "\n",
        json.dumps({"jsonrpc": "2.0", "method": "algumaNotificacao"}) + "\n",
        json.dumps({"jsonrpc": "2.0", "id": 2, "result": {"rateLimits": {"primary": {}}}}) + "\n",
    ])
    assert codex_appserver.perguntar("account/rateLimits/read") == {"rateLimits": {"primary": {}}}


def test_cota_codex_le_roots_com_mesma_assinatura(monkeypatch, tmp_path):
    # A cota lê o root resolvido; no Windows o tmp_path cru tem outra caixa.
    tmp_path = tmp_path.resolve()
    monkeypatch.setattr(__import__("pathlib").Path, "home",
                        classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    work = codex_contas.create_account("work")
    _auth(tmp_path)
    work_auth = work.home / "auth.json"
    work_auth.write_text(json.dumps({"tokens": {"access_token": "at-work"}}), encoding="utf-8")
    same = 1_700_000_000
    for account in (codex_contas.default_home(), work.home):
        auth = account / "auth.json"
        auth.touch()
        import os
        os.utime(auth, (same, same))
    vistos = []

    def perguntar(method, **kwargs):
        vistos.append(str(kwargs["codex_home"]))
        pct = 11 if kwargs["codex_home"] == codex_contas.default_home() else 22
        return {"rateLimits": {"primary": {"usedPercent": pct, "windowDurationMins": 300}}}

    monkeypatch.setattr(cotas.codex_appserver, "perguntar", perguntar)
    cotas._cred_codex_cache = None
    for home in (codex_contas.default_home(), work.home):
        assert cotas._ler_codex(home)[0] == "lida"
    assert {f"{home}" for home in vistos} == {
        str(codex_contas.default_home()), str(work.home),
    }


# ------------------------------------------------------------------ rota HTTP (/wham/usage)

import base64
import time
import urllib.error

# Recorte real de `/wham/usage` e `/wham/rate-limit-reset-credits` (29/09/2026, codex-cli
# 0.159.0), com o que o `account/rateLimits/read` devolveu para a MESMA conta ao lado.
_USO = {
    "plan_type": "pro",
    "rate_limit": {"allowed": True, "limit_reached": False,
                   "primary_window": {"used_percent": 79, "limit_window_seconds": 604800,
                                      "reset_after_seconds": 324219, "reset_at": 1791046696},
                   "secondary_window": None},
    "credits": {"has_credits": False, "unlimited": False, "balance": "0"},
    "rate_limit_reset_credits": {"available_count": 1, "applicable_available_count": 0},
}
_LISTA = {"available_count": 1, "credits": [{
    "id": "RateLimitResetCredit_7e23", "reset_type": "codex_rate_limits", "status": "available",
    "granted_at": "2026-09-22T20:31:55.904160Z", "expires_at": "2026-10-22T20:31:55.904160Z",
    "title": "Full reset", "description": "Thanks for using Codex!"}]}
_APP_SERVER = {
    "rateLimits": {"primary": {"usedPercent": 79, "windowDurationMins": 10080,
                               "resetsAt": 1791046696}, "secondary": None},
    "rateLimitResetCredits": {"availableCount": 1, "credits": [{
        "id": "RateLimitResetCredit_7e23", "resetType": "codexRateLimits", "status": "available",
        "expiresAt": 1792701115, "title": "Full reset", "description": "Thanks for using Codex!"}]},
}


def _jwt(exp):
    corpo = base64.urlsafe_b64encode(json.dumps({"exp": exp}).encode()).decode().rstrip("=")
    return f"h.{corpo}.s"


def _auth_jwt(home, exp):
    d = home / ".codex"
    d.mkdir(parents=True, exist_ok=True)
    (d / "auth.json").write_text(json.dumps({"auth_mode": "chatgpt", "tokens": {
        "access_token": _jwt(exp), "refresh_token": "rt", "account_id": "acc-1"}}),
        encoding="utf-8")


class _Resp(io.BytesIO):
    status = 200


def _backend(monkeypatch, rotas):
    """`rotas`: caminho -> dict (200) ou int (erro HTTP). Guarda o que foi pedido."""
    pedidos = []

    def urlopen(req, timeout):
        pedidos.append(req)
        caminho = req.full_url.removeprefix("https://chatgpt.com/backend-api")
        r = rotas[caminho]
        if isinstance(r, int):
            raise urllib.error.HTTPError(req.full_url, r, "x", {}, None)
        return _Resp(json.dumps(r).encode())
    monkeypatch.setattr(codex_appserver._opener, "open", urlopen)
    return pedidos


@pytest.fixture
def com_jwt(monkeypatch, tmp_path):
    _auth_jwt(tmp_path, time.time() + 3600)
    _home(monkeypatch, tmp_path)
    return tmp_path


def _sem_app_server(monkeypatch):
    def perguntar(m, **kw):
        raise AssertionError("app-server não devia ser chamado")
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", perguntar)


def test_http_da_o_mesmo_resultado_que_o_app_server(monkeypatch, com_jwt):
    _sem_app_server(monkeypatch)
    pedidos = _backend(monkeypatch, {"/wham/usage": _USO, "/wham/rate-limit-reset-credits": _LISTA})
    pelo_http = cotas._ler_codex_detalhada()

    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw: _APP_SERVER)
    monkeypatch.setattr(cotas, "_rate_limits_http_codex", lambda raiz: None)
    assert pelo_http == cotas._ler_codex_detalhada()
    assert pelo_http[0] == "lida" and pelo_http[1][0].rotulo == "7d"
    assert pelo_http[3].credits[0].expires_at == 1792701115
    cab = {k.lower(): v for k, v in pedidos[0].header_items()}
    assert cab["chatgpt-account-id"] == "acc-1" and cab["authorization"].startswith("Bearer h.")
    assert cab["user-agent"] == "codex-cli"


def test_sem_redefinicao_a_lista_que_falha_nao_derruba(monkeypatch, com_jwt):
    _sem_app_server(monkeypatch)
    uso = {**_USO, "rate_limit_reset_credits": {"available_count": 0}}
    pedidos = _backend(monkeypatch, {"/wham/usage": uso, "/wham/rate-limit-reset-credits": 500})
    estado, _janelas, _motivo, redefinicoes = cotas._ler_codex_detalhada()
    assert estado == "lida" and redefinicoes.available_count == 0
    assert [p.full_url.rsplit("/", 1)[1] for p in pedidos] == ["usage"], "sem redefinição, sem lista"


@pytest.mark.parametrize("caso", ["vencido", "401", "403", "formato", "rede", "lista-falha"])
def test_http_que_nao_serve_cai_no_app_server(monkeypatch, tmp_path, caso):
    _auth_jwt(tmp_path, time.time() - 10 if caso == "vencido" else time.time() + 3600)
    _home(monkeypatch, tmp_path)
    rotas = {"/wham/usage": _USO, "/wham/rate-limit-reset-credits": _LISTA}
    if caso in ("401", "403"):
        rotas["/wham/usage"] = int(caso)
    elif caso == "formato":
        rotas["/wham/usage"] = {"rate_limit": {"primary_window": None, "secondary_window": None}}
    elif caso == "lista-falha":
        rotas["/wham/rate-limit-reset-credits"] = 500
    pedidos = _backend(monkeypatch, rotas)
    if caso == "rede":
        def sem_rede(req, timeout):
            raise urllib.error.URLError("offline")
        monkeypatch.setattr(codex_appserver._opener, "open", sem_rede)
    monkeypatch.setattr(cotas.codex_appserver, "perguntar", lambda m, **kw: _RATE_LIMITS)
    estado, janelas, motivo, _ = cotas._ler_codex_detalhada()
    assert (estado, motivo, [j.rotulo for j in janelas]) == ("lida", None, ["5h", "7d"])
    if caso == "vencido":
        assert pedidos == []   # token vencido nem sai: renovar é do CLI


def test_429_nao_cai_no_app_server_e_espera(monkeypatch, com_jwt):
    """O app-server bate no mesmo backend com o mesmo token: cair nele só renovaria o 429."""
    _sem_app_server(monkeypatch)
    _backend(monkeypatch, {"/wham/usage": 429, "/wham/rate-limit-reset-credits": _LISTA})
    assert cotas._ler_codex_detalhada() == ("indisponivel", [], "http-429", None)


class _Servidor:
    """Backend local de verdade: prova o comportamento do urllib, não o de um dublê."""

    def __init__(self, responder):
        import http.server
        import threading

        vistos = self.vistos = []

        class H(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                vistos.append((self.path, self.headers.get("Authorization")))
                responder(self)

            def log_message(self, *a):
                pass

        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.srv.server_address[1]}"
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def fechar(self):
        self.srv.shutdown()
        self.srv.server_close()


def _servidor(monkeypatch, responder):
    s = _Servidor(responder)
    monkeypatch.setattr(codex_appserver, "_BACKEND", s.url)
    return s


def test_redirect_nao_leva_o_token_e_cai_no_app_server(monkeypatch, com_jwt):
    def responder(h):
        if h.path == "/wham/usage":
            h.send_response(302)
            h.send_header("Location", "/outro-host")
            h.end_headers()
        else:
            h.send_response(200)
            h.end_headers()
            h.wfile.write(b"{}")
    s = _servidor(monkeypatch, responder)
    try:
        with pytest.raises(codex_appserver.CodexIndisponivel, match="redirecionado"):
            codex_appserver.backend_get("/wham/usage", codex_home=com_jwt / ".codex")
    finally:
        s.fechar()
    assert [p for p, _ in s.vistos] == ["/wham/usage"]


def test_resposta_cortada_vira_indisponivel(monkeypatch, com_jwt):
    def responder(h):
        h.send_response(200)
        h.send_header("Content-Length", "100")
        h.end_headers()
        h.wfile.write(b'{"rate')
    s = _servidor(monkeypatch, responder)
    try:
        with pytest.raises(codex_appserver.CodexIndisponivel, match="IncompleteRead"):
            codex_appserver.backend_get("/wham/usage", codex_home=com_jwt / ".codex")
    finally:
        s.fechar()


def test_janela_com_segundos_float(monkeypatch):
    assert cotas._janela_http_codex({"limit_window_seconds": 18000.0})["windowDurationMins"] == 300
