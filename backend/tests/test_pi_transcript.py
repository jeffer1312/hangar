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


def test_sgr_escapes_are_stripped_from_assistant_text():
    # O Pi imprime "✻ Turn took Ns" colorido DENTRO do texto da resposta (medido: 100% dos turnos
    # com resposta final carregam \x1b[38;2;...m). A bolha do app renderiza o escape como texto
    # literal ("[38;2;136;136;136m✻ Turn took 2s").
    obj = _msg("assistant", [{"type": "text", "text":
                              "pong\n\n\x1b[38;2;136;136;136m✻ Turn took 2s\x1b[0m"}])
    ev = pt.parse_obj(obj)
    assert ev[0].text == "pong\n\n✻ Turn took 2s"
    assert "\x1b" not in ev[0].text


def test_sgr_escapes_are_stripped_from_tool_results():
    obj = {"type": "message", "id": "n5", "message": {
        "role": "toolResult", "toolCallId": "c", "toolName": "bash", "isError": False,
        "content": [{"type": "text", "text": "\x1b[1;32mok\x1b[0m"}]}}
    assert pt.parse_obj(obj)[0].result == "ok"


def test_fixture_carries_ansi_and_the_parser_cleans_it():
    # A fixture era escrita a mao SEM escape nenhum — e por isso o parser passou meses copiando
    # \x1b[..m verbatim sem nenhum teste reclamar. Agora a fixture tem escape e o guarda de formato
    # cobre a limpeza.
    import pathlib
    fx = pathlib.Path(__file__).parent / "fixtures" / "pi_session.jsonl"
    raw = fx.read_text(encoding="utf-8")
    assert "\\u001b[" in raw, "a fixture precisa conter escape ANSI cru, senao nao pina nada"
    for line in raw.splitlines():
        for e in pt.parse_line(line):
            assert "\x1b" not in (e.text or ""), e.id
            assert "\x1b" not in (e.result or ""), e.id


def test_committed_user_lines_reads_pi_user_messages(tmp_path, monkeypatch):
    # FINDING 1: o oraculo de confirmacao de entrega so entendia o shape do Claude (`type: user` no
    # TOPO da linha). Pro Pi ele devolvia set() vazio -> o reconcile concluia "a TUI engoliu" e o
    # drain REDIGITAVA o mesmo prompt (evidencia no disco: pi-e2e.jsonl com attempts: 2 e o par
    # user/assistant duplicado no transcript).
    from app import pqueue
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    texto = "reply with exactly the word: pong"
    f = tmp_path / "2026-01-01T00-00-00-000Z_sid.jsonl"
    f.write_text(json.dumps({"type": "message", "id": "n1", "message": {
        "role": "user", "timestamp": 100_000, "content": [{"type": "text", "text": texto}]}}) + "\n",
        encoding="utf-8")

    committed = pqueue.committed_user_lines(str(f), provider="pi")
    assert texto in committed

    q = pqueue.PromptQueue("pi-e2e")
    q.path.write_text(json.dumps({"id": "e1", "text": texto, "ts": 100.0, "delivered": True}) + "\n",
                      encoding="utf-8")
    assert q.reconcile_delivered(committed, min_ts=0.0, now=1000.0) == []   # nada de reentrega
    assert q.load()[0]["confirmed"] is True


def test_committed_user_lines_keeps_the_claude_shape(tmp_path):
    # O ramo novo nao pode mexer no provider mais usado do app.
    from app import pqueue
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "message": {"role": "user", "content": "oi"}}) + "\n",
                 encoding="utf-8")
    assert "oi" in pqueue.committed_user_lines(str(j))
    assert "oi" in pqueue.committed_user_lines(str(j), provider="claude")


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


# --- surrogate solto (o Pi trunca por unidade UTF-16 e parte emoji ao meio) -------------------
# Shape REAL medido no transcript do usuario (linha 77, um toolResult): o texto termina em
# "... \ud83d... [truncated]". Reproduzido aqui SINTETICO de proposito — o transcript de origem e
# conversa real e este repo e publico.

def test_lone_surrogate_becomes_replacement_char_and_the_event_serialises():
    # ANTES: parse dava um str com surrogate solto e model_dump_json estourava
    # PydanticSerializationError/UnicodeEncodeError -> /history 500 e o pump do SSE morria.
    line = json.dumps(_msg("toolResult", [{"type": "text", "text": "cauda \ud83d... [truncated]"}],
                           toolCallId="c1"))
    ev = pt.parse_line(line)[0]
    assert ev.result == "cauda �... [truncated]"
    assert not any("\ud800" <= c <= "\udfff" for c in ev.result)
    assert json.loads(ev.model_dump_json())["result"] == "cauda �... [truncated]"


def test_lone_surrogate_in_assistant_text_also_serialises():
    line = json.dumps(_msg("assistant", [{"type": "text", "text": "meio \udc4d fim"}]))
    ev = pt.parse_line(line)[0]
    assert ev.text == "meio � fim"
    ev.model_dump_json().encode("utf-8")   # o passo que crashava


def test_clean_is_where_the_scrub_happens():
    # Rede do ChatEvent a parte: o parser do Pi normaliza o texto do Pi (ANSI + surrogate) na fonte.
    assert pt._clean("a\ud83db") == "a�b"
    assert pt._clean("\x1b[0m ok 😀") == " ok 😀"


def test_wellformed_emoji_survives_untouched():
    # Par bem-formado escrito em \u no JSON (json.loads junta num codepoint so), bandeira
    # (2 regional indicators) e familia com ZWJ: nada disso pode virar U+FFFD.
    texto = "\\ud83d\\ude00 \\ud83c\\udde7\\ud83c\\uddf7 \\ud83d\\udc68\\u200d\\ud83d\\udc69\\u200d\\ud83d\\udc67\\u200d\\ud83d\\udc66"
    line = '{"type":"message","id":"n1","message":{"role":"user","content":[{"type":"text","text":"%s"}]}}' % texto
    ev = pt.parse_line(line)[0]
    assert ev.text == "😀 🇧🇷 👨‍👩‍👧‍👦"
    assert json.loads(ev.model_dump_json())["text"] == ev.text


# ── contexto de hook colado na mensagem do usuario ────────────────────────────────────────────
# Caso REAL (numa sessao real, 2026-07-30): a extensao claude-hooks-adapter.ts roda os hooks do
# Claude dentro do Pi, e o Pi cola o texto devolvido no inicio da mensagem do usuario. A bolha do app
# mostrava "[skill-suggester] ..." como se o usuario tivesse digitado.
_CTX = ("[skill-suggester] Prompt casa com skills instaladas que o usuario costuma esquecer:\n"
        "- polir layout: refactoring-ui")
_PROMPT = "Ué o layout não parece ter seguido os padrões?"


def _hook_ctx(parent="n1", content=_CTX):
    return {"type": "custom_message", "customType": "claude-hook-context",
            "content": content, "display": True, "id": "fb15", "parentId": parent}


def test_stream_strips_the_hook_context_from_the_user_bubble():
    s = pt.Stream()
    # A mensagem NAO sai na hora: a irmã que prova o que é hook chega na linha seguinte.
    assert s.feed_events(_msg("user", [{"type": "text", "text": f"{_CTX}\n\n{_PROMPT}"}])) == []
    evs = s.feed_events(_hook_ctx())
    assert [e.kind for e in evs] == ["user_msg"]
    assert evs[0].text == _PROMPT
    assert s.flush_events() == []


def test_stream_drops_a_message_that_was_only_hook_context():
    # Hook injetou contexto num turno sem texto do usuario -> bolha vazia nao vira bolha nenhuma.
    s = pt.Stream()
    s.feed_events(_msg("user", [{"type": "text", "text": _CTX}]))
    assert s.feed_events(_hook_ctx()) == []


def test_stream_releases_the_message_untouched_without_a_sibling():
    # Sessao sem a extensao de hooks (o caso comum): a proxima linha e a resposta do assistente e a
    # mensagem sai inteira, na frente dela.
    s = pt.Stream()
    assert s.feed_events(_msg("user", [{"type": "text", "text": "oi"}])) == []
    evs = s.feed_events({"type": "message", "id": "n2",
                         "message": {"role": "assistant", "content": [{"type": "text", "text": "ola"}]}})
    assert [(e.kind, e.text) for e in evs] == [("user_msg", "oi"), ("assistant_msg", "ola")]


def test_stream_flush_releases_the_last_message_of_the_batch():
    # Usuario mandou e o Pi ainda nao escreveu mais nada: sem o flush do lote a bolha so apareceria
    # quando o turno terminasse (minutos).
    s = pt.Stream()
    assert s.feed_events(_msg("user", [{"type": "text", "text": "oi"}])) == []
    assert [e.text for e in s.flush_events()] == ["oi"]


def test_stream_ignores_a_hook_context_of_another_message():
    # parentId de outra mensagem (ou irmã atrasada): nao pode comer o prefixo de quem esta retido.
    s = pt.Stream()
    s.feed_events(_msg("user", [{"type": "text", "text": f"{_CTX}\n\n{_PROMPT}"}]))
    evs = s.feed_events(_hook_ctx(parent="OUTRA"))
    assert evs[0].text == f"{_CTX}\n\n{_PROMPT}"


def test_stream_keeps_text_that_only_looks_like_the_context():
    # Sem irmã, texto que COMECA igual ao de um hook antigo continua intacto — o corte e por par
    # (parentId + conteudo), nunca por padrao de texto.
    s = pt.Stream()
    s.feed_events(_msg("user", [{"type": "text", "text": f"{_CTX} e mais coisa"}]))
    assert s.flush_events()[0].text == f"{_CTX} e mais coisa"


def test_queue_entry_right_after_the_session_start_survives_in_pi_history(tmp_path, monkeypatch):
    # A 1a linha util do Pi e um user_msg, e ele fica RETIDO no parser (sai so na linha seguinte).
    # Se o relogio da linha for pulado junto, o "inicio da sessao" vira o ts da RESPOSTA — e toda
    # entrada de fila enfileirada nesse meio (2o prompt mandado enquanto o Pi trabalha) caia na poda
    # de "anterior ao inicio" e sumia do historico sem erro nenhum.
    from app import pqueue
    from app.pqueue import PromptQueue, merged_history
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")

    t0 = 1785160179.0
    f = tmp_path / "2026-01-01T00-00-00-000Z_sid.jsonl"
    f.write_text(
        # Shape do Pi: SEM `timestamp` ISO no topo — o relogio mora em `message.timestamp` (ms).
        # Com o ISO no topo o caso nem acontece, e o teste passava com o bug presente.
        json.dumps({"type": "message", "id": "n1",
                    "message": {"role": "user", "timestamp": int(t0 * 1000),
                                "content": [{"type": "text", "text": "primeiro"}]}}) + "\n"
        + json.dumps({"type": "message", "id": "n2",
                      "message": {"role": "assistant", "timestamp": int((t0 + 5) * 1000),
                                  "content": [{"type": "text", "text": "resposta"}]}}) + "\n",
        encoding="utf-8")

    PromptQueue("sessao-pi").append("segundo", ts=t0 + 1)   # 2o prompt, 1s apos o inicio

    evs = merged_history("sessao-pi", str(f), provider="pi")
    textos = [e.text for e in evs]
    assert "primeiro" in textos and "resposta" in textos
    assert "segundo" in textos, "entrada de fila do inicio da sessao sumiu do historico"
