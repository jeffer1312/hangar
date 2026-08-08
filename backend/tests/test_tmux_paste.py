"""paste_text: caminho normal (tmux) e o plano B de quem nao tem paste-buffer (psmux/Windows)."""
import subprocess

from app import tmux


def _grava(monkeypatch, falha_paste: bool, trunca: bool = False):
    """Troca o `_run` do tmux.py por um espiao. `falha_paste` simula o psmux VELHO, que devolve
    codigo != 0 no paste-buffer. `trunca` simula o que foi MEDIDO no psmux 3.3.7: rc=0 mentiroso, com
    o buffer cortado na primeira quebra de linha.

    Patch em `_run`, nao em `RUN` (Task 3): `load-buffer -` manda o texto pela STDIN via
    `subprocess.run` direto (para escapar do teto de 16344 bytes do comando, ver `_run`), um caminho
    que NAO passa por `RUN` — um monkeypatch em `RUN` deixaria esse load-buffer cair no tmux de
    verdade, sem ninguem notar (rc=0 silencioso na ausencia de servidor)."""
    chamadas: list[list[str]] = []

    def fake(args, input=None):
        chamadas.append(args)
        rc = 1 if (falha_paste and "paste-buffer" in args) else 0
        # show-buffer do probe: `trunca` devolve so o que vem ANTES do \n, como o psmux faz.
        out = ("A\n" if trunca else "A\nB\n") if "show-buffer" in args else ""
        return subprocess.CompletedProcess(args, rc, stdout=out, stderr="")

    monkeypatch.setattr(tmux, "_run", fake)
    monkeypatch.setattr(tmux, "_TRUNCA_BUFFER", None)   # probe roda por teste, sem cache vazado
    return chamadas


def test_linux_usa_paste_buffer_e_nao_cai_no_plano_b(monkeypatch):
    # A garantia de nao-regressao: onde o paste-buffer funciona, NADA muda — duas chamadas (fora a
    # resolucao de pane do agentpane, Task 1), e nenhum send-keys. Se este teste comecar a ver
    # send-keys, o Linux regrediu.
    chamadas = _grava(monkeypatch, falha_paste=False)
    tmux.paste_text("s", "uma\nduas\ntres")
    verbos = [c[1] for c in chamadas
              if c[1] not in ("show-buffer", "delete-buffer", "list-panes")][1:]
    assert verbos == ["load-buffer", "paste-buffer"]   # [1:] tira o set-buffer do probe


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


def test_falha_confirmada_no_meio_da_2a_linha_devolve_false_e_para(monkeypatch):
    # Achado CRITICO da review 02/08/2026: o retorno de _send_literal era descartado aqui e em
    # paste_text -- send_prompt so tinha a leitura da tela como prova, e com ela aceitando o COMECO
    # como evidencia (terminal_input._RESIDUO_INICIO) um texto que parasse no MEIO ainda parecia
    # "entregue". Agora a falha CONFIRMADA (rc != 0) tem que propagar: False, e PARAR (nao tentar a
    # 3a linha, que deixaria um buraco no meio do texto).
    chamadas = _grava(monkeypatch, falha_paste=True)   # cai no plano B (linha a linha)
    base_fake = tmux._run

    def fake_com_falha_na_segunda(args, input=None):
        if args[1] == "send-keys" and args[-1] == "duas":
            return subprocess.CompletedProcess(args, 1, stdout="", stderr="deu ruim")
        return base_fake(args, input=input)

    monkeypatch.setattr(tmux, "_run", fake_com_falha_na_segunda)
    ok = tmux.paste_text("s", "uma\nduas\ntres")
    assert ok is False


def test_probe_e_por_capacidade_e_fica_em_cache(monkeypatch):
    # Uma vez por processo: o comportamento do multiplexador nao muda no meio da vida do backend, e o
    # probe custa 3 chamadas. E e por CAPACIDADE, nao por nome de SO — um tmux que passe a truncar
    # (ou um psmux que conserte) e tratado certo sem tocar no codigo.
    chamadas = _grava(monkeypatch, falha_paste=False, trunca=True)
    tmux.paste_text("s", "a\nb")
    tmux.paste_text("s", "c\nd")
    assert len([c for c in chamadas if c[1] == "show-buffer"]) == 1
