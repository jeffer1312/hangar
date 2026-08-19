"""Cota por conta lida na fonte do provedor (app/cotas.py).

A I/O de rede mora em `_get_json` e é trocada aqui; o resto é lógica pura em volta dela
(mesmo precedente do `conta_estado._auth_status`). Os payloads são cópias do que as APIs
reais devolveram em 18/08/2026 — inclusive o detalhe que quebra parser ingênuo: o Kimi manda
`limit`/`remaining` como STRING e não tem campo `used`.
"""
import json
import time
from pathlib import Path

from app import cotas


# ------------------------------------------------------------------------------------ Claude

_USAGE_CLAUDE = {
    "five_hour": {"utilization": 13.0, "resets_at": "2026-08-18T15:10:00.013519+00:00"},
    "seven_day": {"utilization": 22.0, "resets_at": "2026-08-22T21:00:00.013548+00:00"},
    "seven_day_opus": None,
}


def _cred(dir_conta: Path, *, token="sk-ant-oat01-x", expira_em=3600,
          refresh: str | None = None, refresh_em=30 * 24 * 3600) -> Path:
    dir_conta.mkdir(parents=True, exist_ok=True)
    oauth = {
        "accessToken": token,
        "expiresAt": int((time.time() + expira_em) * 1000),   # a API manda MILISSEGUNDOS
        "subscriptionType": "max",
    }
    # `refresh` separado do resto: credencial SEM refresh token é o caso "login de verdade", e é
    # o padrão aqui de propósito — quem quer testar a renovação diz isso explicitamente.
    if refresh is not None:
        oauth["refreshToken"] = refresh
        oauth["refreshTokenExpiresAt"] = int((time.time() + refresh_em) * 1000)
    (dir_conta / ".credentials.json").write_text(json.dumps({"claudeAiOauth": oauth}),
                                                 encoding="utf-8")
    return dir_conta


def test_claude_le_as_duas_janelas(tmp_path, monkeypatch):
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (200, _USAGE_CLAUDE))
    estado, janelas, motivo = cotas._ler_claude(_cred(tmp_path / "c"))
    assert (estado, motivo) == ("lida", None)
    assert [(j.rotulo, j.pct) for j in janelas] == [("5h", 13.0), ("7d", 22.0)]
    assert janelas[0].reset_ts is not None


def test_claude_manda_o_token_da_conta(tmp_path, monkeypatch):
    """Cada conta lê com a credencial DELA — é o bug que este módulo existe pra fechar."""
    vistos = []
    monkeypatch.setattr(cotas, "_get_json",
                        lambda url, headers: (vistos.append(headers["Authorization"]),
                                              (200, _USAGE_CLAUDE))[1])
    cotas._ler_claude(_cred(tmp_path / "a", token="tok-a"))
    cotas._ler_claude(_cred(tmp_path / "b", token="tok-b"))
    assert vistos == ["Bearer tok-a", "Bearer tok-b"]


def test_token_expirado_nao_gasta_requisicao(tmp_path, monkeypatch):
    def _nunca(url, headers):
        raise AssertionError("não pode bater na rede com token vencido")
    monkeypatch.setattr(cotas, "_get_json", _nunca)
    estado, janelas, motivo = cotas._ler_claude(_cred(tmp_path / "c", expira_em=-10))
    # Sem refresh token no arquivo, o vencido é login de verdade — e nem tenta renovar.
    assert (estado, janelas, motivo) == ("expirada", [], "login-necessario")


def test_conta_sem_credencial_nao_e_zero(tmp_path, monkeypatch):
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (200, _USAGE_CLAUDE))
    (tmp_path / "vazia").mkdir()
    estado, janelas, _ = cotas._ler_claude(tmp_path / "vazia")
    assert estado == "sem_credencial" and janelas == []


def test_401_e_expirada_e_nao_indisponivel(tmp_path, monkeypatch):
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (401, None))
    estado, _, motivo = cotas._ler_claude(_cred(tmp_path / "c"))
    assert (estado, motivo) == ("expirada", "login-necessario")


def test_formato_novo_nao_vira_zero(tmp_path, monkeypatch):
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (200, {"outra_coisa": 1}))
    estado, janelas, motivo = cotas._ler_claude(_cred(tmp_path / "c"))
    assert (estado, janelas, motivo) == ("indisponivel", [], "formato-desconhecido")


# -------------------------------------------------------------------------------------- Kimi

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


def test_rotulo_vem_da_duracao_do_provedor():
    assert cotas._rotulo_janela(300) == "5h"
    assert cotas._rotulo_janela("10080") == "7d"
    assert cotas._rotulo_janela(90) == "90min"
    assert cotas._rotulo_janela(None) == "janela"


# ------------------------------------------------------------------------------------- cache


def _fonte(chave, leitura):
    return cotas._Fonte(chave, chave, "claude", lambda: leitura)


def test_queda_de_rede_nao_apaga_leitura_boa(monkeypatch):
    monkeypatch.setattr(cotas, "_cache", {})
    boa = [cotas.JanelaCota(rotulo="5h", pct=13.0)]
    cotas._atualizar([_fonte("claude:/x", ("lida", boa, None))])
    monkeypatch.setattr(cotas, "_TTL_S", -1)          # força reler
    cotas._atualizar([_fonte("claude:/x", ("indisponivel", [], "sem-resposta"))])
    guardada = cotas._cache["claude:/x"][1]
    assert guardada.estado == "lida" and guardada.janelas[0].pct == 13.0


def test_conta_deslogada_sobrescreve_o_numero_velho(monkeypatch):
    """O contrário do de cima: `expirada` é fato sobre a conta — deixar o número antigo ali
    faria conta deslogada parecer em uso."""
    monkeypatch.setattr(cotas, "_cache", {})
    cotas._atualizar([_fonte("claude:/x", ("lida", [cotas.JanelaCota(rotulo="5h", pct=13.0)], None))])
    monkeypatch.setattr(cotas, "_TTL_S", -1)
    cotas._atualizar([_fonte("claude:/x", ("expirada", [], "http-401"))])
    guardada = cotas._cache["claude:/x"][1]
    assert guardada.estado == "expirada" and guardada.janelas == []


def test_dentro_do_ttl_nao_relê(monkeypatch):
    monkeypatch.setattr(cotas, "_cache", {})
    chamadas = []

    def leitor():
        chamadas.append(1)
        return ("lida", [cotas.JanelaCota(rotulo="5h", pct=1.0)], None)

    f = cotas._Fonte("claude:/x", "x", "claude", leitor)
    cotas._atualizar([f])
    cotas._atualizar([f])
    assert len(chamadas) == 1


def test_leitor_que_levanta_nao_derruba_a_lista(monkeypatch):
    monkeypatch.setattr(cotas, "_cache", {})

    def explode():
        raise RuntimeError("boom")

    cotas._atualizar([cotas._Fonte("claude:/x", "x", "claude", explode)])
    assert cotas._cache["claude:/x"][1].estado == "indisponivel"


# ---------------------------------------------------------------- renovação delegada ao CLI


def test_vencido_com_refresh_e_conta_livre_renova_e_le(tmp_path, monkeypatch):
    """Token vencido + refresh vivo + ninguém usando a conta: renova pelo CLI e lê o número.

    É o caso do dia a dia — conta parada, sessão nenhuma aberta nela. Sem isto a faixa mandava
    "precisa entrar" para uma credencial que só precisava de um refresh.
    """
    dir_conta = _cred(tmp_path / "c", expira_em=-10, refresh="rt-1")
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: False)

    chamou = []

    def _renova(p):
        chamou.append(p)
        _cred(p, expira_em=3600, refresh="rt-2")   # o CLI grava o par NOVO (rotação)
        return True

    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli", _renova)
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (200, {
        "five_hour": {"utilization": 7, "resets_at": None},
        "seven_day": {"utilization": 12, "resets_at": None},
    }))
    estado, janelas, motivo = cotas._ler_claude(dir_conta)
    assert chamou == [dir_conta]
    assert (estado, motivo) == ("lida", None)
    assert [(j.rotulo, j.pct) for j in janelas] == [("5h", 7.0), ("7d", 12.0)]


def test_vencido_com_sessao_viva_nao_renova(tmp_path, monkeypatch):
    """Processo vivo naquela pasta: NÃO renova. O refresh da Anthropic rotaciona, e o par novo
    deixaria a sessão viva com um refresh morto na memória."""
    dir_conta = _cred(tmp_path / "c", expira_em=-10, refresh="rt-1")
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: True)
    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli",
                        lambda p: (_ for _ in ()).throw(AssertionError("não pode renovar")))
    monkeypatch.setattr(cotas, "_get_json",
                        lambda url, headers: (_ for _ in ()).throw(AssertionError("nem rede")))
    assert cotas._ler_claude(dir_conta) == ("expirada", [], "sessao-viva")


def test_varredura_de_processos_falha_nao_renova(tmp_path, monkeypatch):
    """Não deu pra olhar os processos = trata como em uso (fail-closed, regra do
    `renova_token.esta_em_uso`). O contrário seria renovar por cima de uma sessão que existe e que
    a varredura não conseguiu enxergar."""
    dir_conta = _cred(tmp_path / "c", expira_em=-10, refresh="rt-1")
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: True)   # varredura falhou -> fail-closed
    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli",
                        lambda p: (_ for _ in ()).throw(AssertionError("não pode renovar")))
    assert cotas._ler_claude(dir_conta) == ("expirada", [], "sessao-viva")


def test_conta_ativa_nunca_renova(tmp_path, monkeypatch):
    """A conta padrão (~/.claude) fica de fora: processo que a usa não define CLAUDE_CONFIG_DIR,
    então a varredura por ambiente não o enxerga — e ela se conserta sozinha no próximo turno."""
    dir_conta = _cred(tmp_path / "c", expira_em=-10, refresh="rt-1")
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: False)
    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli",
                        lambda p: (_ for _ in ()).throw(AssertionError("não pode renovar")))
    assert cotas._ler_claude(dir_conta, ativa=True) == ("expirada", [], "sessao-viva")


def test_renovacao_que_nao_grava_vira_motivo_proprio(tmp_path, monkeypatch):
    """CLI chamado e o arquivo não mudou: some com o número, mas dizendo que a tentativa houve —
    "precisa entrar" ali seria mentira, o refresh token continua no arquivo."""
    dir_conta = _cred(tmp_path / "c", expira_em=-10, refresh="rt-1")
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: False)
    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli", lambda p: False)
    assert cotas._ler_claude(dir_conta) == ("expirada", [], "renovacao-falhou")


def test_401_com_refresh_vivo_tenta_renovar_e_nao_mente(tmp_path, monkeypatch):
    """Provedor recusou um token que o relógio dizia bom: tenta renovar UMA vez e, se não der,
    devolve o motivo de verdade.

    O errado (e o que a primeira versão fazia) era responder "sessao-viva" só porque o refresh
    ainda não venceu — sem ter olhado processo nenhum. A tela mandava "abra uma sessão nela" numa
    conta revogada, e a leitura seguinte caía no mesmo 401 para sempre.
    """
    dir_conta = _cred(tmp_path / "c", refresh="rt-1")   # access VÁLIDO pelo relógio
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: False)
    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli", lambda p: False)
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (401, None))
    assert cotas._ler_claude(dir_conta) == ("expirada", [], "renovacao-falhou")


def test_401_depois_de_renovar_e_login_necessario(tmp_path, monkeypatch):
    """Renovou e o provedor recusou o par NOVO: aí é login de verdade, e a recursão para."""
    dir_conta = _cred(tmp_path / "c", refresh="rt-1")
    monkeypatch.setattr(cotas.renova_token, "esta_em_uso", lambda p: False)
    monkeypatch.setattr(cotas.renova_token, "renovar_por_cli",
                        lambda p: bool(_cred(p, refresh="rt-2")))
    monkeypatch.setattr(cotas, "_get_json", lambda url, headers: (401, None))
    assert cotas._ler_claude(dir_conta) == ("expirada", [], "login-necessario")
