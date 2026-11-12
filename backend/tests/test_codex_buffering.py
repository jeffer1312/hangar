from app.adapters.codex.adapter import map_state


def notice(show=True, *, thread="thread", turn="turn"):
    return {"method": "model/safetyBuffering/updated", "params": {
        "threadId": thread, "turnId": turn, "model": "gpt-6-astra",
        "useCases": [], "reasons": [], "showBufferingUi": show, "fasterModel": None,
    }}


def test_mapper_aceita_so_o_sinal_booleano_nativo():
    assert map_state(notice()).buffering is True
    assert map_state(notice(False)).buffering is False
    assert map_state(notice("true")).buffering is None
