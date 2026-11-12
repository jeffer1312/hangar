"""tok/s: medida do stream (live_rate)."""
import time

from app.live_rate import LiveRate, first_response_report


def test_medida_vale_so_para_a_conversa_e_ate_o_transcript_passar_a_frente():
    r = LiveRate()
    r.close(100, 1.0, "a")
    r.close(10, 0.1, "a")                 # curta demais: ruído de relógio
    assert r.snapshot("a", time.time()) == {"tok_s_now": 100.0, "tok_s_recent": 100.0,
                                            "tok_s_exact": True}
    # Transcript com resposta bem mais nova: quem media parou (plugin fora, CLI sem ele).
    assert r.snapshot("a", time.time() + 60) == {}
    # /clear ou sessão nova com o mesmo nome: outra conversa não herda as medidas.
    assert r.snapshot("b", None) == {}
    r.close(50, 1.0, "b")
    assert r.snapshot("b", None)["tok_s_recent"] == 50.0
    # Ler não apaga: duas conexões olhando conversas diferentes não se atrapalham.
    assert r.snapshot("a", None)["tok_s_now"] == 100.0


def test_codex_first_response_without_rate_is_scoped_to_conversation():
    r = LiveRate()
    for seconds in (1.0, 3.0):
        report = first_response_report({"first_response": True, "seconds": seconds,
                                        "conversation": "codex-thread"})
        assert report is not None
        r.first_response(*report)
    assert r.snapshot("codex-thread", None) == {"ttft_ms": 2000}
    assert r.snapshot("other-thread", None) == {}
    assert first_response_report({"first_response": True, "seconds": float("nan"),
                                  "conversation": "codex-thread"}) is None
