"""Escolha de conta por folga e rótulo de janela (app/cotas.py). Os leitores de Kimi, CommandCode
e OpenCode moram no Rust (`contract_quotas_other.rs`)."""
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
