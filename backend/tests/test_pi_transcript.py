import json

from app.adapters.pi import transcript as pt


def _msg(role, content, **extra):
    return {"type": "message", "id": "n1", "parentId": "n0",
            "message": dict({"role": role, "content": content}, **extra)}


def test_user_text_becomes_user_msg():
    ev = pt.parse_obj(_msg("user", [{"type": "text", "text": "oi"}]))
    assert len(ev) == 1
    assert ev[0].kind == "user_msg"
    assert ev[0].text == "oi"
    assert ev[0].id == "n1"


def test_assistant_text_becomes_assistant_msg_with_cache_read():
    obj = _msg("assistant", [{"type": "text", "text": "ola", "textSignature": "x"}],
               model="gpt-5.5", usage={"input": 5, "output": 2, "cacheRead": 8704})
    ev = pt.parse_obj(obj)
    assert [e.kind for e in ev] == ["assistant_msg"]
    assert ev[0].text == "ola"
    assert ev[0].cache_read == 8704


def test_thinking_block_is_dropped():
    # Igual ao Claude: raciocinio nao vira bolha de chat. Sem isto o app mostraria o rascunho
    # interno do modelo como se fosse resposta.
    ev = pt.parse_obj(_msg("assistant", [{"type": "thinking", "thinking": "hmm",
                                          "thinkingSignature": "s"}]))
    assert ev == []


def test_tool_call_becomes_tool_use():
    ev = pt.parse_obj(_msg("assistant", [{"type": "toolCall", "id": "call_1",
                                          "name": "bash", "arguments": {"cmd": "ls"}}]))
    assert len(ev) == 1
    assert ev[0].kind == "tool_use"
    assert ev[0].tool_name == "bash"
    assert ev[0].tool_input == {"cmd": "ls"}
    assert ev[0].tool_use_id == "call_1"


def test_text_and_tool_call_in_one_message_yield_two_events():
    # Uma mensagem do Pi carrega VARIOS blocos. Devolver so o primeiro engoliria a tool call
    # silenciosamente — o mesmo bug que parse_obj do Claude documenta em transcript.py:94.
    ev = pt.parse_obj(_msg("assistant", [
        {"type": "text", "text": "vou listar"},
        {"type": "toolCall", "id": "call_2", "name": "bash", "arguments": {}},
    ]))
    assert [e.kind for e in ev] == ["assistant_msg", "tool_use"]
    assert ev[0].id != ev[1].id, "ids precisam ser unicos por bloco, senao o front deduplica"


def test_tool_result_links_by_tool_call_id():
    obj = {"type": "message", "id": "n2", "parentId": "n1",
           "message": {"role": "toolResult", "toolCallId": "call_1", "toolName": "bash",
                       "isError": False, "content": [{"type": "text", "text": "saida"}]}}
    ev = pt.parse_obj(obj)
    assert len(ev) == 1
    assert ev[0].kind == "tool_result"
    assert ev[0].tool_use_id == "call_1"
    assert ev[0].result == "saida"
    assert ev[0].is_error is False


def test_tool_result_error_flag_survives():
    # is_error some => o front pinta falha como sucesso. Regra "falha aparece, nao some".
    obj = {"type": "message", "id": "n3", "message": {
        "role": "toolResult", "toolCallId": "c", "toolName": "bash", "isError": True,
        "content": [{"type": "text", "text": "boom"}]}}
    assert pt.parse_obj(obj)[0].is_error is True


def test_tool_result_image_block_is_summarised_not_inlined():
    # `data` e base64 inteiro. Colar no `result` mandaria megabytes pelo SSE a cada evento.
    obj = {"type": "message", "id": "n4", "message": {
        "role": "toolResult", "toolCallId": "c", "toolName": "read", "isError": False,
        "content": [{"type": "image", "data": "AAAA", "mimeType": "image/png"}]}}
    ev = pt.parse_obj(obj)
    assert len(ev) == 1
    assert "image/png" in ev[0].result
    assert "AAAA" not in ev[0].result


def test_non_message_lines_produce_nothing():
    for t in ("session", "model_change", "thinking_level_change"):
        assert pt.parse_obj({"type": t, "id": "x"}) == []


def test_parse_line_survives_a_torn_write():
    # O tailer le enquanto o Pi escreve: linha pela metade e normal, nao pode levantar.
    assert pt.parse_line('{"type": "mess') == []
    assert pt.parse_line("") == []


def test_parses_the_format_guard_fixture_without_raising():
    # Guarda de formato: roda o parser contra um JSONL escrito a mao (conteudo inventado, mas
    # reproduz as formas medidas na maquina real -- os 4 `type`s de linha, os 3 `role`s de
    # mensagem, bloco de imagem, thinking+toolCall juntos etc). Nao e um transcript real: o
    # arquivo original vazava nomes internos do empregador do dev (repo publico).
    import pathlib
    fx = pathlib.Path(__file__).parent / "fixtures" / "pi_session.jsonl"
    events = []
    for line in fx.read_text(encoding="utf-8").splitlines():
        events.extend(pt.parse_line(line))
    kinds = {e.kind for e in events}
    assert kinds <= {"user_msg", "assistant_msg", "tool_use", "tool_result"}
    assert "tool_use" in kinds and "tool_result" in kinds and "user_msg" in kinds
    # todo tool_result aponta pra um tool_use presente no mesmo arquivo
    tool_use_ids = {e.tool_use_id for e in events if e.kind == "tool_use"}
    orphans = [e.tool_use_id for e in events
               if e.kind == "tool_result" and e.tool_use_id not in tool_use_ids]
    assert not orphans, f"tool_result sem tool_use correspondente: {orphans[:3]}"


def test_merged_history_uses_the_pi_parser(tmp_path):
    # Sem o dispatch, merged_history usa o parser do Claude e devolve [] pra toda linha do Pi:
    # o chat retomado abre vazio e so popula com o que chegar ao vivo.
    from app.pqueue import merged_history
    f = tmp_path / "2026-01-01T00-00-00-000Z_sid.jsonl"
    f.write_text(json.dumps({"type": "message", "id": "n1", "message": {
        "role": "user", "timestamp": 1785160179834,
        "content": [{"type": "text", "text": "oi"}]}}) + "\n", encoding="utf-8")

    ev = merged_history("sessao-inexistente", str(f), provider="pi")
    assert [e.kind for e in ev] == ["user_msg"]
    assert ev[0].text == "oi"
