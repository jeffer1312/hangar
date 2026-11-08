import pytest


from app import terminal_input as ti


@pytest.fixture
def teclado(monkeypatch):
    """Grava as teclas e serve os panes na ordem pedida."""
    teclas = []
    monkeypatch.setattr(ti, "send_keys",
                        lambda name, k, literal=False: teclas.append((k, literal)))
    monkeypatch.setattr(ti.time, "sleep", lambda s: None)
    return teclas


def _panes(monkeypatch, seq):
    it = iter(seq)
    ultimo = {}
    def cap(name):
        try:
            ultimo["v"] = next(it)
        except StopIteration:
            pass
        return ultimo.get("v", "")
    monkeypatch.setattr(ti, "_capture", cap)


def test_steer_com_marcador_manda_ctrl_s(teclado, monkeypatch):
    _panes(monkeypatch, ["↑ to edit · ctrl-s to steer immediately"])
    assert ti.steer_now("s") is True
    assert teclado == [("C-s", False)]


def test_steer_pane_ilegivel_degrada_pra_tecla(teclado, monkeypatch):
    # Pane ilegível NUNCA bloqueia: cai no comportamento de sempre (tecla, que é no-op se vazio).
    def cap(name):
        raise OSError("tmux fora")
    monkeypatch.setattr(ti, "_capture", cap)
    assert ti.steer_now("s") is True
    assert teclado == [("C-s", False)]

