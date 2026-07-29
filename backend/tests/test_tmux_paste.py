"""paste_text: caminho normal (tmux) e o plano B de quem nao tem paste-buffer (psmux/Windows)."""
import subprocess

from app import tmux


def _grava(monkeypatch, falha_paste: bool):
    """Troca o RUN do tmux.py por um espiao. `falha_paste` simula o psmux, que nao implementa
    paste-buffer e devolve codigo != 0."""
    chamadas: list[list[str]] = []

    def fake(args, **kw):
        chamadas.append(args)
        rc = 1 if (falha_paste and "paste-buffer" in args) else 0
        return subprocess.CompletedProcess(args, rc, stdout="", stderr="")

    monkeypatch.setattr(tmux, "RUN", fake)
    return chamadas


def test_linux_usa_paste_buffer_e_nao_cai_no_plano_b(monkeypatch):
    # A garantia de nao-regressao: onde o paste-buffer funciona, NADA muda — duas chamadas, e
    # nenhum send-keys. Se este teste comecar a ver send-keys, o Linux regrediu.
    chamadas = _grava(monkeypatch, falha_paste=False)
    tmux.paste_text("s", "uma\nduas\ntres")
    verbos = [c[1] for c in chamadas]
    assert verbos == ["set-buffer", "paste-buffer"]


def test_sem_paste_buffer_manda_linha_a_linha_com_cj(monkeypatch):
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "uma\nduas\ntres")
    # depois do set-buffer + paste-buffer falho: linha, C-j, linha, C-j, linha
    depois = chamadas[2:]
    assert [c[-1] for c in depois] == ["uma", "C-j", "duas", "C-j", "tres"]
    # As linhas vao LITERAIS (-l --), senao um texto começando com '-' viraria flag do send-keys.
    assert all("-l" in c and "--" in c for c in depois if c[-1] != "C-j")


def test_nenhum_argumento_carrega_quebra_de_linha(monkeypatch):
    # O achado que motiva o plano B: com \n dentro do argumento o psmux engole tudo depois dele.
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "uma\nduas")
    for c in chamadas[2:]:
        assert "\n" not in c[-1]


def test_linha_vazia_no_meio_vira_so_um_cj(monkeypatch):
    # "a\n\nb" tem uma linha vazia: ela nao pode virar um send-keys -l com string vazia (o tmux
    # trataria como argumento faltando), so a quebra.
    chamadas = _grava(monkeypatch, falha_paste=True)
    tmux.paste_text("s", "a\n\nb")
    assert [c[-1] for c in chamadas[2:]] == ["a", "C-j", "C-j", "b"]
