"""paste_text: caminho normal (tmux) e o plano B de quem nao tem paste-buffer (psmux/Windows)."""
import subprocess

from app import tmux


def _grava(monkeypatch, falha_paste: bool, trunca: bool = False):
    """Troca o RUN do tmux.py por um espiao. `falha_paste` simula o psmux VELHO, que devolve codigo
    != 0 no paste-buffer. `trunca` simula o que foi MEDIDO no psmux 3.3.7: rc=0 mentiroso, com o
    buffer cortado na primeira quebra de linha."""
    chamadas: list[list[str]] = []

    def fake(args, **kw):
        chamadas.append(args)
        rc = 1 if (falha_paste and "paste-buffer" in args) else 0
        # show-buffer do probe: `trunca` devolve so o que vem ANTES do \n, como o psmux faz.
        out = ("A\n" if trunca else "A\nB\n") if "show-buffer" in args else ""
        return subprocess.CompletedProcess(args, rc, stdout=out, stderr="")

    monkeypatch.setattr(tmux, "RUN", fake)
    monkeypatch.setattr(tmux, "_TRUNCA_BUFFER", None)   # probe roda por teste, sem cache vazado
    return chamadas


def test_linux_usa_paste_buffer_e_nao_cai_no_plano_b(monkeypatch):
    # A garantia de nao-regressao: onde o paste-buffer funciona, NADA muda — duas chamadas, e
    # nenhum send-keys. Se este teste comecar a ver send-keys, o Linux regrediu.
    chamadas = _grava(monkeypatch, falha_paste=False)
    tmux.paste_text("s", "uma\nduas\ntres")
    verbos = [c[1] for c in chamadas if c[1] not in ("show-buffer", "delete-buffer")][1:]
    assert verbos == ["set-buffer", "paste-buffer"]   # [1:] tira o set-buffer do probe


def test_sem_paste_buffer_manda_linha_a_linha_com_cj(monkeypatch):
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "uma\nduas\ntres")
    # depois do set-buffer + paste-buffer falho: linha, C-j, linha, C-j, linha
    depois = [c for c in chamadas if c[1] == "send-keys"]
    assert [c[-1] for c in depois] == ["uma", "C-j", "duas", "C-j", "tres"]
    # As linhas vao LITERAIS (-l --), senao um texto começando com '-' viraria flag do send-keys.
    assert all("-l" in c and "--" in c for c in depois if c[-1] != "C-j")


def test_nenhum_argumento_carrega_quebra_de_linha(monkeypatch):
    # O achado que motiva o plano B: com \n dentro do argumento o psmux engole tudo depois dele.
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "uma\nduas")
    for c in chamadas:
        if c[1] == "send-keys":
            assert "\n" not in c[-1]


def test_linha_vazia_no_meio_vira_so_um_cj(monkeypatch):
    # "a\n\nb" tem uma linha vazia: ela nao pode virar um send-keys -l com string vazia (o tmux
    # trataria como argumento faltando), so a quebra.
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "a\n\nb")
    assert [c[-1] for c in chamadas if c[1] == "send-keys"] == ["a", "C-j", "C-j", "b"]


def test_probe_detecta_multiplexador_que_trunca_e_vai_direto_pro_plano_b(monkeypatch):
    # O achado da sessao-irma no Windows: no psmux 3.3.7 o set-buffer devolve rc=0 e grava so ate a
    # primeira quebra ("ABC\nDEF\nGHI" vira "ABC"), e o paste-buffer TAMBEM devolve rc=0 entregando
    # nada. Confiar no rc mantinha o plano B — que funciona — desligado. Agora o probe pergunta ao
    # multiplexador o que ele faz e, se ele trunca, o paste-buffer nem e tentado.
    chamadas = _grava(monkeypatch, falha_paste=False, trunca=True)
    tmux.paste_text("s", "uma\nduas\ntres")
    assert not any(c[1] == "paste-buffer" for c in chamadas)   # nem tentou
    assert [c[-1] for c in chamadas if c[1] == "send-keys"] == ["uma", "C-j", "duas", "C-j", "tres"]


def test_probe_e_por_capacidade_e_fica_em_cache(monkeypatch):
    # Uma vez por processo: o comportamento do multiplexador nao muda no meio da vida do backend, e o
    # probe custa 3 chamadas. E e por CAPACIDADE, nao por nome de SO — um tmux que passe a truncar
    # (ou um psmux que conserte) e tratado certo sem tocar no codigo.
    chamadas = _grava(monkeypatch, falha_paste=False, trunca=True)
    tmux.paste_text("s", "a\nb")
    tmux.paste_text("s", "c\nd")
    assert len([c for c in chamadas if c[1] == "show-buffer"]) == 1
