"""Leitores de cota que ficam no Python (app/cotas.py) e a escolha de conta por folga.

A I/O de rede mora em `_get_json` e é trocada aqui; o resto é lógica pura em volta dela. Os
payloads são cópias do que as APIs reais devolveram em 18/08/2026 — inclusive o detalhe que quebra parser ingênuo: o Kimi manda
`limit`/`remaining` como STRING e não tem campo `used`.
"""
from app import cotas


# ------------------------------------------------------------------------- sugestão de conta


def _conta(id, *pcts, ativa=False, estado="lida", provedor="claude"):
    return cotas.CotaConta(id=id, label=id.split(":")[-1], provedor=provedor, ativa=ativa,
                           estado=estado,
                           janelas=[cotas.JanelaCota(rotulo=str(i), pct=p) for i, p in enumerate(pcts)])


def test_sugestao_e_a_conta_com_mais_folga_na_janela_que_aperta():
    """A conta 'b' tem 7d folgado mas o Fable a 90%: quem manda é a janela mais cheia."""
    s = cotas.sugerir_claude([_conta("claude:/a", 40, 60, 70),
                              _conta("claude:/b", 10, 20, 90),
                              _conta("claude:/c", 50, 75)])
    assert (s.path, s.folga) == ("/a", 30.0)


def test_sugestao_ignora_sem_leitura_e_outros_provedores():
    s = cotas.sugerir_claude([_conta("claude:/x", estado="expirada"),
                              _conta("kimi:k", 0, provedor="kimi"),
                              _conta("claude:/y", 99)])
    assert s.path == "/y"
    assert cotas.sugerir_claude([_conta("claude:/x", estado="expirada")]) is None


def test_sugestao_empate_fica_com_a_conta_padrao():
    s = cotas.sugerir_claude([_conta("claude:/outra", 30), _conta("claude:/padrao", 30, ativa=True)])
    assert (s.path, s.ativa) == ("/padrao", True)


_USAGE_KIMI = {
    "usage": {"limit": "100", "remaining": "80", "resetTime": "2026-08-24T17:59:46.782017Z"},
    "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                "detail": {"limit": "100", "remaining": "75",
                           "resetTime": "2026-08-18T17:59:46.782017Z"}}],
}


def test_kimi_usa_limit_menos_remaining(monkeypatch):
    """Não existe campo `used` nesta API: 100 de limite com 75 restando é 25% USADO."""
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (200, _USAGE_KIMI))
    estado, janelas, _ = cotas._ler_kimi("sk-kimi-x", "https://api.kimi.com/coding/v1")
    assert estado == "lida"
    assert [(j.rotulo, j.pct) for j in janelas] == [("5h", 25.0), ("7d", 20.0)]


def test_kimi_url_e_o_base_url_do_provider(monkeypatch):
    vistos = []
    monkeypatch.setattr(cotas, "_get_json",
                        lambda url, headers: (vistos.append(url), (200, _USAGE_KIMI))[1])
    cotas._ler_kimi("k", "https://api.kimi.com/coding/v1/")
    assert vistos == ["https://api.kimi.com/coding/v1/usages"]


def test_chave_que_nao_tem_a_rota_nao_vira_credencial_vencida(monkeypatch):
    """403 num provedor sem rota de cota (medido no OpenCode Zen) é "não informa", não "vencida" —
    o contrário mandaria a pessoa refazer um login que está inteiro."""
    for codigo in (401, 403, 404):
        monkeypatch.setattr(cotas, "_get_json", lambda url, headers, c=codigo: (c, None))
        estado, janelas, motivo = cotas._ler_kimi("sk-x", "https://opencode.ai/zen/go")
        assert (estado, janelas, motivo) == ("indisponivel", [], f"http-{codigo}")


# ------------------------------------------------------------------------------ CommandCode

# Cópia da resposta real de 21/08/2026, com os detalhes que quebram parser ingênuo: `used`/`cap`
# em USD (float), `resetAt` em epoch-MILISSEGUNDOS e 0 quando a janela nem abriu.
_USAGE_COMMANDCODE = {
    "credits": {"belowThreshold": False, "creditThreshold": 0,
                "monthlyCredits": 52.2129667955, "purchasedCredits": 0, "freeCredits": 0},
    "windowLimits": {"limited": True, "exceeded": None,
                     "fiveHour": {"used": 0, "cap": 14, "exceeded": False, "resetAt": 0},
                     "weekly": {"used": 17.7870332045, "cap": 35, "exceeded": False,
                                "resetAt": 1787564008022}},
}


def test_commandcode_janelas_em_usd_e_reset_em_ms(monkeypatch):
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (200, _USAGE_COMMANDCODE))
    estado, janelas, motivo = cotas._ler_commandcode("user_4f-x")
    assert (estado, motivo) == ("lida", None)
    assert [(j.rotulo, round(j.pct, 1)) for j in janelas] == [("5h", 0.0), ("7d", 50.8)]
    assert janelas[0].reset_ts is None                       # resetAt 0 = sem reset marcado
    assert janelas[1].reset_ts == 1787564008.022             # milissegundos -> segundos


def test_commandcode_manda_ua_de_navegador(monkeypatch):
    """Sem UA de navegador o Cloudflare da rota devolve 403 1010 — o header é parte do contrato."""
    vistos = []
    monkeypatch.setattr(cotas, "_get_json",
                        lambda url, headers: (vistos.append((url, headers)),
                                              (200, _USAGE_COMMANDCODE))[1])
    cotas._ler_commandcode("user_4f-x")
    url, headers = vistos[0]
    assert url == cotas._URL_COMMANDCODE
    assert headers["Authorization"] == "Bearer user_4f-x"
    assert headers["User-Agent"].startswith("Mozilla/5.0")


def test_base_do_motor_kimi_ganha_v1_so_na_cota():
    """O motor usa `/coding` (formato Anthropic); o `/usages` mora sob `/v1` — 404 sem ele."""
    assert cotas._base_usages_kimi("https://api.kimi.com/coding") == "https://api.kimi.com/coding/v1"
    assert cotas._base_usages_kimi("https://api.kimi.com/coding/v1") == "https://api.kimi.com/coding/v1"
    assert cotas._base_usages_kimi("https://outro.com/api") == "https://outro.com/api"


def test_rotulo_vem_da_duracao_do_provedor():
    assert cotas._rotulo_janela(300) == "5h"
    assert cotas._rotulo_janela("10080") == "7d"
    assert cotas._rotulo_janela(90) == "90min"
    assert cotas._rotulo_janela(None) == "janela"


# ------------------------------------------------------------------------------ leitor isolado


def test_leitor_que_levanta_nao_derruba_a_lista():
    def explode():
        raise RuntimeError("boom")

    estado, janelas, motivo = cotas._seguro(cotas._Fonte("kimi:x", "x", "kimi", explode))
    assert (estado, janelas, motivo) == ("indisponivel", [], "erro-leitor")


# --------------------------------------------------------------------- conta herdada acabando


def test_conta_herdada_acabando_vai_para_a_de_mais_folga():
    """Criadora a 95% numa janela: a sessão nova nasce na de mais folga, como o --conta auto."""
    contas = [_conta("claude:/mae", 40, 95), _conta("claude:/folga", 10, 20)]
    cfg, aviso = cotas.conta_com_cota("/mae", contas)
    assert cfg == "/folga" and "95%" in aviso


def test_conta_herdada_com_folga_fica():
    contas = [_conta("claude:/mae", 50, 30), _conta("claude:/folga", 10, 20)]
    assert cotas.conta_com_cota("/mae", contas) == ("/mae", None)


def test_conta_acabando_fica_se_nenhuma_outra_tem_mais_folga():
    contas = [_conta("claude:/mae", 96), _conta("claude:/outra", 97)]
    assert cotas.conta_com_cota("/mae", contas) == ("/mae", None)
