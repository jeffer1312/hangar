import asyncio
import json

import pytest

from app.models import ChatEvent
from app.transcript import parse_line, TranscriptTailer


def _line(obj) -> str:
    return json.dumps(obj)


def test_user_text_message():
    evs = parse_line(_line({
        "type": "user", "uuid": "u1", "parentUuid": None,
        "message": {"role": "user", "content": "corrige o bug"},
    }))
    assert evs == [ChatEvent(kind="user_msg", id="u1", text="corrige o bug")]


def test_interrupcao_vira_aviso_e_nao_bolha_do_usuario():
    # O CLI grava a interrupção como entrada "user" sintética em inglês. Bolha ali dava a entender
    # que a pessoa tinha digitado aquilo; agora é aviso, com a frase por conta da interface.
    evs = parse_line(_line({
        "type": "user", "uuid": "u9", "parentUuid": None,
        "message": {"role": "user", "content": "[Request interrupted by user for tool use]"},
    }))
    assert evs == [ChatEvent(kind="notice", id="u9", text="interrupted")]


def test_resumo_do_compact_vira_aviso_e_nao_bolha_do_usuario():
    # O /compact grava o resumo inteiro como entrada "user". O terminal mostra só a marca; aqui ele
    # virava uma bolha de 20 KB em inglês, como se a pessoa tivesse escrito.
    evs = parse_line(_line({
        "type": "user", "uuid": "u12", "parentUuid": None, "isCompactSummary": True,
        "message": {"role": "user", "content": "This session is being continued from a previous…"},
    }))
    assert evs == [ChatEvent(kind="notice", id="u12", text="compacted")]


def test_interrupcao_em_blocos_tambem_vira_aviso():
    # Sem terminal o CLI grava o mesmo texto como LISTA de blocos (medido na 2.1.276) — só o ramo
    # da string não bastava, e o aviso voltava a aparecer como bolha.
    evs = parse_line(_line({
        "type": "user", "uuid": "u11", "parentUuid": None,
        "message": {"role": "user",
                    "content": [{"type": "text", "text": "[Request interrupted by user]"}]},
    }))
    assert evs == [ChatEvent(kind="notice", id="u11", text="interrupted")]


def test_mensagem_que_so_cita_a_interrupcao_continua_bolha():
    evs = parse_line(_line({
        "type": "user", "uuid": "u10", "parentUuid": None,
        "message": {"role": "user", "content": "por que apareceu [Request interrupted by user]?"},
    }))
    assert [e.kind for e in evs] == ["user_msg"]


def test_mensagem_orientada_no_claude_sem_terminal_vira_bolha():
    # Formato gravado pelo CLI quando um `user` chega no stdin com turno em voo.
    [ev] = parse_line(_line({
        "type": "attachment", "uuid": "att1", "parentUuid": "u1",
        "timestamp": "2026-09-14T11:47:33.802Z",
        "attachment": {"type": "queued_command", "commandMode": "prompt",
                       "prompt": [{"type": "text", "text": "MUDANÇA: pare os sleeps"}]},
        "rendered": [{"content": [{"type": "text", "text": "<system-reminder>..."}]}],
    }))
    assert ev.kind == "user_msg"
    assert ev.id == "att1"
    assert ev.text == "MUDANÇA: pare os sleeps"
    assert ev.ts is not None
    assert parse_line(_line({"type": "attachment", "uuid": "att2",
                             "attachment": {"type": "hook_success"}})) == []


def test_assistant_text_message():
    [ev] = parse_line(_line({
        "type": "assistant", "uuid": "a1", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [{"type": "text", "text": "vou olhar"}]},
    }))
    assert ev.kind == "assistant_msg"
    assert ev.text == "vou olhar"


def test_assistant_tool_use():
    [ev] = parse_line(_line({
        "type": "assistant", "uuid": "a2", "parentUuid": "u1",
        "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_9", "name": "Bash", "input": {"command": "ls"}},
        ]},
    }))
    assert ev.kind == "tool_use"
    assert ev.tool_name == "Bash"
    assert ev.tool_use_id == "toolu_9"
    assert ev.tool_input == {"command": "ls"}


def test_user_tool_result_is_not_a_bubble():
    [ev] = parse_line(_line({
        "type": "user", "uuid": "u2", "parentUuid": "a2",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_9", "content": "file.txt", "is_error": False},
        ]},
    }))
    assert ev.kind == "tool_result"
    assert ev.tool_use_id == "toolu_9"
    assert ev.result == "file.txt"
    assert ev.is_error is False


def test_parallel_tool_results_all_emitted():
    # Tool calls PARALELAS gravam varios tool_result numa entrada user so; TODOS viram evento
    # (antes so o 1o -> o resultado dos demais nunca chegava na UI). Ids extras com sufixo
    # deterministico (o front deduplica e keia bubble por id).
    evs = parse_line(_line({
        "type": "user", "uuid": "u5",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": "um"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "dois"},
        ]},
    }))
    assert [(e.kind, e.id, e.tool_use_id, e.result) for e in evs] == [
        ("tool_result", "u5", "t1", "um"),
        ("tool_result", "u5:1", "t2", "dois"),
    ]


def test_assistant_text_and_tool_use_same_entry():
    # text + tool_use na MESMA entrada assistant: os dois viram evento, na ordem do content
    # (antes o tool_use vencia e o texto sumia do chat). O thinking COM texto entra junto, na
    # ordem, com kind proprio — e o que deixa "pensou -> buscou -> pensou" aparecer sozinho.
    evs = parse_line(_line({
        "type": "assistant", "uuid": "a9",
        "message": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "vou rodar"},
            {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}},
        ]},
    }))
    assert [(e.kind, e.id) for e in evs] == [
        ("thinking", "a9"), ("assistant_msg", "a9:1"), ("tool_use", "a9:2")]
    assert evs[0].text == "hmm"
    assert evs[1].text == "vou rodar"
    assert evs[2].tool_use_id == "toolu_1"


def test_thinking_cifrado_nao_vira_evento():
    # Sessao aberta SEM showThinkingSummaries: a API devolve o bloco cifrado — assinatura e
    # `thinking: ""`. Medido: 8746 de 8746 blocos de opus-5 assim antes de ligar a chave. Emitir
    # isso encheria a conversa de linhas que nao abrem nada.
    evs = parse_line(_line({
        "type": "assistant", "uuid": "a10",
        "message": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "", "signature": "CAISzAQKhwEIEBgC"},
            {"type": "text", "text": "pronto"},
        ]},
    }))
    assert [(e.kind, e.id) for e in evs] == [("assistant_msg", "a10")]


def test_command_meta_entries_are_skipped():
    # Claude Code logs slash-commands / local command I/O as synthetic "user" entries —
    # tooling meta, must not leak into the chat as bubbles.
    assert parse_line(_line({
        "type": "user", "uuid": "c1",
        "message": {"role": "user",
                    "content": "<command-name>/clear</command-name><command-message>clear</command-message>"},
    })) == []
    assert parse_line(_line({
        "type": "user", "uuid": "c2",
        "message": {"role": "user", "content": "<local-command-caveat>Caveat: ...</local-command-caveat>"},
    })) == []
    # a normal message that merely mentions such a tag mid-text is still a real message
    [ev] = parse_line(_line({
        "type": "user", "uuid": "c3",
        "message": {"role": "user", "content": "what does the <command-name> tag do?"},
    }))
    assert ev.kind == "user_msg"


def test_image_meta_entries_are_skipped():
    # Entradas user SINTETICAS cujo texto inteiro e "[Image...]" sao meta do harness (referencia de
    # imagem colada ou anotacao de leitura do modelo), nunca conversa real -> fora do chat.
    for text in (
        "[Image: source: /home/u/pic.png]",
        "[Image: original 1179x2556, displayed at 923x2000. Multiply coordinates by 1.28 to map to original image.]",
        "[Image]",
    ):
        assert parse_line(_line({
            "type": "user", "uuid": "i1",
            "message": {"role": "user", "content": text},
        })) == []
    # Mas uma msg real que MENCIONA a sintaxe nao pode ser engolida.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "i2",
        "message": {"role": "user", "content": "o que e [Image: foo] no log?"},
    }))
    assert ev.kind == "user_msg"


def test_system_reminder_only_message_is_skipped():
    # O harness injeta lembretes ("The user named this session…") como entrada "user" sintetica.
    # Quando a msg e SO o bloco <system-reminder>, e meta — nao pode virar bubble.
    assert parse_line(_line({
        "type": "user", "uuid": "r1",
        "message": {"role": "user",
                    "content": '<system-reminder>\nThe user named this session "corrigindo tmux".\n</system-reminder>'},
    })) == []


def test_system_reminder_stripped_from_real_message():
    # Lembrete ANEXADO a uma msg real: remove so o bloco, mantem o texto do usuario.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "r2",
        "message": {"role": "user",
                    "content": "roda o teste\n<system-reminder>tooling meta aqui</system-reminder>"},
    }))
    assert ev.kind == "user_msg" and ev.text == "roda o teste"


def test_pasted_content_envelope_is_unwrapped():
    # Texto colado no composer (todo recado do hangar-send chega assim): o CLI 2.1.278 embrulha em
    # <pasted_content id="…">, com o id repetido na tag de FECHAMENTO. Fica so o conteudo.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "p1",
        "message": {"role": "user",
                    "content": '\n\n<pasted_content id="2b26">\n[de: outra] recado\n</pasted_content id="2b26">\n'},
    }))
    assert ev.kind == "user_msg" and ev.text == "[de: outra] recado"


def test_rewrite_filter_descarta_a_conversa_regravada_pelo_resume():
    # `claude --resume` regrava a conversa inteira no mesmo jsonl (uuid novo, timestamp original):
    # a copia fica fora; mensagem NOVA depois dela entra; retrocesso de milissegundos nao e copia.
    from app.transcript import RewriteFilter
    def u(uuid, ts, text):
        return {"type": "user", "uuid": uuid, "timestamp": ts,
                "message": {"role": "user", "content": text}}
    f = RewriteFilter()
    assert f.keep(u("a1", "2026-09-18T11:24:35.529Z", "oi"))
    assert f.keep(u("a2", "2026-09-19T13:28:59.412Z", "tchau"))
    assert f.keep(u("a3", "2026-09-19T13:28:59.300Z", "quase junto"))          # 112ms atras: legitimo
    assert not f.keep(u("b1", "2026-09-18T11:24:35.529Z", "oi"))               # copia do resume
    assert not f.keep(u("b2", "2026-09-19T13:28:59.412Z", "tchau"))            # dentro da janela, repetida
    assert f.keep(u("c1", "2026-09-19T13:30:00.000Z", "mensagem nova"))


def test_recado_nativo_do_backend_nao_dobra_o_prefixo():
    # O backend escreve o recado no socket JA com "[de: X] …" (ou "[grupo: X] …") no corpo; o
    # parser mostra o corpo como esta, em vez de "[de: X] [grupo: X] …".
    [ev] = parse_line(_line({
        "type": "user", "uuid": "n1", "isMeta": True,
        "origin": {"kind": "peer", "from": "uds:/run/user/1000/cc-socks/1.sock", "name": "x",
                   "body": "[grupo: x] aviso do grupo"},
        "message": {"role": "user", "content": '<cross-session-message from="uds:/x" from-name="x">\n[grupo: x] aviso do grupo\n</cross-session-message>'},
    }))
    assert ev.kind == "user_msg" and ev.text == "[grupo: x] aviso do grupo"


def test_ismeta_user_entry_is_skipped():
    # Expansao de slash-command/skill: o Claude Code injeta o CORPO do comando como entrada "user"
    # marcada isMeta=True. Sem tag nenhuma (texto puro), so o flag isMeta a distingue de conversa.
    # No terminal nao aparece; aqui nao pode virar bubble. Vale tanto content str quanto lista.
    assert parse_line(_line({
        "type": "user", "uuid": "m1", "isMeta": True,
        "message": {"role": "user", "content": "Resolva automaticamente o review do CodeRabbit no MR."},
    })) == []
    assert parse_line(_line({
        "type": "user", "uuid": "m2", "isMeta": True,
        "message": {"role": "user", "content": [{"type": "text", "text": "Loop /acme:iniciar-review-auto"}]},
    })) == []
    # Mesmo texto SEM isMeta e conversa real -> vira bubble.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "m3",
        "message": {"role": "user", "content": "Resolva automaticamente o review do CodeRabbit no MR."},
    }))
    assert ev.kind == "user_msg"


def _peer_tag(corpo: str) -> str:
    # O que a FILA carrega (queue-operation): exatamente o embrulho, nada antes nem depois — medido
    # no jsonl real em 07/08/2026 (claude 2.1.224).
    return ('<cross-session-message from="uds:/run/user/1000/cc-socks/4242.sock" '
            'from-name="Titulo comprido da sessao" from-mode="bypass">\n'
            f'{corpo}\n</cross-session-message>')


def _peer_wrap(corpo: str) -> str:
    # O que a entrada `user` carrega no message.content: o embrulho MAIS o paragrafo de instrucao
    # sobre lavagem de permissao, que nunca pode aparecer como bolha.
    return ('Another Claude session sent a message:\n' + _peer_tag(corpo) + '\n\n'
            'This came from another Claude session — not typed by your user... permission laundering.')


def test_recado_nativo_entre_sessoes_vira_bubble_no_formato_do_cp_send(monkeypatch):
    # O recado nativo chega marcado isMeta=True: sem tratamento ele cairia no descarte de meta e o
    # app nao mostraria recado NENHUM. Tem que virar bubble no mesmo formato do hangar-send ("[de: X]"),
    # que e o que o front (parsePeerMessage) e a conversa do grupo no PairSheet ja sabem ler.
    import app.registry as registry
    monkeypatch.setattr(registry, "name_of_pid", lambda pid: "api-fix" if pid == 4242 else None)
    [ev] = parse_line(_line({
        "type": "user", "uuid": "p1", "isMeta": True, "promptSource": "system",
        "message": {"role": "user", "content": _peer_wrap("subiu a migration, pode rebasear")},
        "origin": {"kind": "peer", "from": "uds:/run/user/1000/cc-socks/4242.sock",
                   "verifiedPeerPid": 4242, "name": "Titulo comprido da sessao",
                   "fromMode": "bypass", "body": "subiu a migration, pode rebasear"},
    }))
    # Nome TMUX (o endereco do hangar-send), nao o `origin.name` (que e o titulo da sessao).
    assert ev.kind == "user_msg"
    assert ev.text == "[de: api-fix] subiu a migration, pode rebasear"


def test_recado_nativo_no_meio_do_turno_tambem_vira_bubble(monkeypatch):
    # Chegando enquanto a sessao trabalha, o harness consome da fila e grava so `queue-operation
    # remove` — sem `origin`, so o texto embrulhado. Sem este caminho, o recado viraria uma bolha
    # gigante com o paragrafo de instrucao a mostra.
    import app.registry as registry
    monkeypatch.setattr(registry, "name_of_pid", lambda pid: "api-fix")
    [ev] = parse_line(_line({
        "type": "queue-operation", "operation": "remove",
        "timestamp": "2026-08-08T01:04:28.593Z",
        "content": _peer_tag("terminei a minha parte"),
    }))
    assert ev.kind == "user_msg" and ev.text == "[de: api-fix] terminei a minha parte"


def test_recado_nativo_cai_no_titulo_quando_o_nome_nao_resolve(monkeypatch):
    # tmux fora do ar / sessao que nao e do tmux: recado com nome menos preciso e melhor que recado
    # sumido. NUNCA pode derrubar o parse.
    import app.registry as registry
    monkeypatch.setattr(registry, "name_of_pid", lambda pid: (_ for _ in ()).throw(OSError("tmux")))
    [ev] = parse_line(_line({
        "type": "user", "uuid": "p2", "isMeta": True,
        "message": {"role": "user", "content": _peer_wrap("oi")},
        "origin": {"kind": "peer", "from": "uds:/run/user/1000/cc-socks/4242.sock",
                   "verifiedPeerPid": 4242, "name": "Titulo comprido da sessao", "body": "oi"},
    }))
    assert ev.text == "[de: Titulo comprido da sessao] oi"


def test_texto_do_usuario_com_a_tag_dentro_nao_vira_recado(monkeypatch):
    # Colar este próprio código numa conversa (ou qualquer documentação do formato) NÃO pode fazer a
    # mensagem do usuário ser descartada e substituída pelo miolo das tags, com remetente inventado —
    # seria perda calada do que a pessoa escreveu e forja da atribuição que o PairSheet usa pra dizer
    # de quem é a fala. Achado da revisão. Detecção é por `origin` (entrada user) ou por conteúdo
    # EXATAMENTE igual ao embrulho (fila), nunca por "contém a tag".
    import app.registry as registry
    monkeypatch.setattr(registry, "name_of_pid", lambda pid: "api-fix")
    texto = ("olha o formato que eu descobri:\n" + _peer_tag("corpo de exemplo") +
             "\nviu? o origin é que vale.")
    [ev] = parse_line(_line({
        "type": "user", "uuid": "u9", "message": {"role": "user", "content": texto},
    }))
    assert ev.text == texto                       # inteiro, do usuário, sem prefixo de remetente
    # Mesma coisa na fila (mensagem digitada durante trabalho): continua sendo a fala DELE, inteira,
    # e não um recado com remetente forjado.
    fila = "veja: " + _peer_tag("corpo") + " fim"
    [ev] = parse_line(_line({
        "type": "queue-operation", "operation": "remove", "timestamp": "t", "content": fila,
    }))
    assert ev.text == fila and not ev.text.startswith("[de:")


@pytest.mark.parametrize("nome_torto", [123, ["x"], {"a": 1}, True, None, ""])
def test_recado_nativo_com_origin_torto_nao_derruba_o_parse(nome_torto):
    # `origin` e JSON CRU: nada garante o tipo dos campos. Com `origin.name` nao-string,
    # `(fallback or "").strip()` levantava AttributeError — e a excecao subia por parse_line ->
    # _read_from -> follow() ate o except do sse.py, parando o tail da sessao INTEIRA. Uma linha
    # malformada tirava o transcript todo do ar, o oposto do que o codigo promete. Achado da revisao.
    [ev] = parse_line(_line({
        "type": "user", "uuid": "p9", "isMeta": True,
        "message": {"role": "user", "content": "irrelevante"},
        "origin": {"kind": "peer", "body": "corpo", "name": nome_torto,
                   "verifiedPeerPid": "nem inteiro e"},
    }))
    assert ev.kind == "user_msg" and ev.text == "[de: sessão] corpo"


def test_attachment_returns_no_events():
    assert parse_line(_line({"type": "attachment", "uuid": "x"})) == []


def test_blank_or_bad_line_returns_no_events():
    assert parse_line("") == []
    assert parse_line("{not json") == []


def test_real_fixture_lines_parse():
    from pathlib import Path
    p = Path(__file__).parent / "fixtures" / "jsonl_samples.jsonl"
    events = []
    for line in p.read_text().splitlines():
        events.extend(parse_line(line))  # must not raise
    assert any(ev.kind == "assistant_msg" for ev in events)


@pytest.mark.asyncio
async def test_tailer_yields_existing_then_new(tmp_path):
    f = tmp_path / "s.jsonl"
    f.write_text(json.dumps({"type": "user", "uuid": "u1",
                             "message": {"role": "user", "content": "hi"}}) + "\n")
    tailer = TranscriptTailer(f)
    got = []

    async def consume():
        async for ev in tailer.follow():
            got.append(ev)
            if len(got) == 2:
                return

    async def append():
        await asyncio.sleep(0.2)
        with f.open("a") as fh:
            fh.write(json.dumps({"type": "assistant", "uuid": "a1", "parentUuid": "u1",
                                 "message": {"role": "assistant",
                                             "content": [{"type": "text", "text": "yo"}]}}) + "\n")

    await asyncio.wait_for(asyncio.gather(consume(), append()), timeout=5)
    assert [e.id for e in got] == ["u1", "a1"]


# --- fim de agente ENFILEIRADO -------------------------------------------------------------
# Agente de background que termina com o assistente NO MEIO de um turno nao vira mensagem de user:
# o harness grava uma entrada `queue-operation`/enqueue, sem `message` e sem `uuid`. Ela morria no
# early-return de `message` e o painel de Atividade ficava com o agente "RODANDO AGORA" pra sempre.

_QUEUED = json.dumps({
    "type": "queue-operation",
    "operation": "enqueue",
    "sessionId": "s1",
    "content": "<task-notification>\n<task-id>a4e4f68c8a3c46749</task-id>\n"
               "<status>completed</status>\n</task-notification>",
})


def test_task_notification_enfileirada_vira_tool_result_sintetico():
    evs = parse_line(_QUEUED)
    assert len(evs) == 1
    assert evs[0].kind == "tool_result"
    assert evs[0].tool_use_id == "task:a4e4f68c8a3c46749"   # e o que o fold do painel casa


_HAND_BACK = ('<agent-message from="abd7854a2736546e7">\n[Subagent hand-back] The text below is the final '
              'report.\n  relatorio\n</agent-message>')


def _user(content) -> str:
    return json.dumps({"type": "user", "uuid": "u1", "message": {"role": "user", "content": content}})


def test_hand_back_de_subagente_fecha_o_agent_e_nao_vira_bolha():
    for content in (_HAND_BACK, [{"type": "text", "text": _HAND_BACK}]):
        evs = parse_line(_user(content))
        assert [(e.kind, e.tool_use_id) for e in evs] == [("tool_result", "task:abd7854a2736546e7")]
    enfileirado = parse_line(json.dumps({"type": "queue-operation", "operation": "enqueue", "timestamp": "t",
                                         "content": _HAND_BACK}))
    assert [e.tool_use_id for e in enfileirado] == ["task:abd7854a2736546e7"]


def test_agent_message_intermediario_fica_fora_do_chat():
    assert parse_line(_user('<agent-message from="x">\nprogresso\n</agent-message>')) == []


def test_texto_que_so_cita_agent_message_continua_do_usuario():
    evs = parse_line(_user('olha esse <agent-message from="x">oi</agent-message> no codigo'))
    assert [e.kind for e in evs] == ["user_msg"]


def test_dois_agent_message_colados_nao_viram_um_so():
    evs = parse_line(_user(_HAND_BACK + _HAND_BACK.replace("abd7854a2736546e7", "bbb")))
    assert not any(e.tool_use_id == "task:abd7854a2736546e7" for e in evs)


def test_queue_operation_enqueue_nao_renderiza():
    # enqueue NAO vira bubble: a msg sai no `remove` (consumo mid-turn), pra nao duplicar a que
    # eventualmente vira turno real via `dequeue`. Ver test_queued_removed_message_vira_user_bubble.
    ev = json.dumps({"type": "queue-operation", "operation": "enqueue",
                     "sessionId": "s1", "content": "roda os testes"})
    assert parse_line(ev) == []


def test_queue_operation_sem_content_nao_explode():
    assert parse_line(json.dumps({"type": "queue-operation", "operation": "enqueue"})) == []


# --- msg de usuario digitada DURANTE o turno do agente (mid-turn) ---------------------------
# Enfileirada (`enqueue`) e consumida dentro do turno (`remove`) -> nunca vira type='user', entao
# some do chat (aparecia so no terminal). Renderiza no `remove`; enqueue/dequeue nao.

def test_queued_removed_message_vira_user_bubble():
    ev = json.dumps({"type": "queue-operation", "operation": "remove", "sessionId": "s1",
                     "timestamp": "2026-07-29T17:16:37", "content": "no caso pode abrir com haiku"})
    [got] = parse_line(ev)
    assert got.kind == "user_msg"
    assert got.text == "no caso pode abrir com haiku"
    assert got.id.startswith("queued:2026-07-29T17:16:37:")   # ts + hash do conteudo


def test_queued_removed_ids_diferem_no_mesmo_timestamp():
    # Duas msgs consumidas no MESMO instante precisam de ids DISTINTOS, senao o front (deduplica por
    # id) esconde uma. Observado real: "no caso..." e "so pra..." ambas removidas as 17:16:37.
    ts = "2026-07-29T17:16:37"
    [a] = parse_line(json.dumps({"type": "queue-operation", "operation": "remove",
                                 "timestamp": ts, "content": "no caso pode abrir"}))
    [b] = parse_line(json.dumps({"type": "queue-operation", "operation": "remove",
                                 "timestamp": ts, "content": "so pra conversa crescer"}))
    assert a.id != b.id and a.text != b.text


def test_queued_dequeue_nao_renderiza_para_nao_duplicar_turno_real():
    # dequeue = virou turno de verdade (tem seu type='user'); renderizar aqui duplicaria a bubble.
    ev = json.dumps({"type": "queue-operation", "operation": "dequeue", "sessionId": "s1",
                     "timestamp": "2026-07-29T16:57:13", "content": "qual o modelo de tmux"})
    assert parse_line(ev) == []


def test_queued_removed_meta_nao_vira_bubble():
    # Conteudo de tooling (comando/skill/system-reminder) consumido da fila continua fora do chat.
    ev = json.dumps({"type": "queue-operation", "operation": "remove", "sessionId": "s1",
                     "timestamp": "2026-07-29T17:00:00",
                     "content": "<command-name>/tui</command-name>"})
    assert parse_line(ev) == []


def test_dois_embrulhos_concatenados_nao_viram_uma_bolha_so():
    # Com dois embrulhos colados, o fullmatch casa mesmo assim (o corpo não-guloso engole o
    # fechamento do primeiro e a abertura do segundo) e sairia UMA bolha, com as tags cruas no meio,
    # atribuída só ao primeiro remetente. Não foi observado no jsonl real — mas o dia em que dois
    # recados forem coalescidos num campo só é o dia em que isso vira texto podre exibido.
    # A saída escolhida é a honesta, não a bonita: o texto aparece CRU, como mensagem sem remetente,
    # em vez de virar uma bolha atribuída a quem mandou só a primeira metade. Some, nunca.
    # Sem mock de name_of_pid de propósito: este caminho REJEITA antes de resolver nome nenhum, e um
    # mock aqui daria a impressão de que a resolução está coberta neste teste (ela está nos outros).
    bruto = _peer_tag("primeiro") + _peer_tag("segundo")
    [ev] = parse_line(_line({
        "type": "queue-operation", "operation": "remove", "timestamp": "t", "content": bruto,
    }))
    assert ev.text == bruto and not ev.text.startswith("[de:")


# --- Defeito (A): recado preso em type='system' (entrega bloqueada por hook) ---
# Formato medido em 18/08/2026 no transcript do desc2-arbitro: o hook de UserPromptSubmit falhou
# e o Claude Code registrou a tentativa como system, com o texto da entrega apos "Original prompt:".

_SYSTEM_BLOQUEADO = (
    'UserPromptSubmit operation blocked by hook:\n'
    '["/home/jefferson/wt-desc/t8/backend/.venv/bin/python" "/x/state_hook.py"]: '
    "can't open file '/x/state_hook.py': [Errno 2] No such file or directory\n"
    '\n'
    '\n'
    'Original prompt: [de: desc2-exec2] RODADA 3 ENTREGUE — texto do reporte aqui.'
)


def test_system_com_recado_bloqueado_vira_bubble():
    # O recado nunca chegou ao agente (preventContinuation=true), mas o texto existe no
    # transcript e o app precisa mostra-lo — E com a marca vermelha, que e a verdade. Sem o
    # desistiu=True a bolha pareceria uma entrega normal e o usuario acharia que chegou.
    [ev] = parse_line(_line({
        "type": "system", "subtype": "informational", "level": "warning",
        "uuid": "f1aaa5a4", "timestamp": "2026-08-18T00:36:42.691Z",
        "content": _SYSTEM_BLOQUEADO,
    }))
    assert ev.kind == "user_msg"
    assert ev.text.startswith("[de: desc2-exec2] RODADA 3 ENTREGUE")
    assert ev.id.startswith("held:")
    assert ev.desistiu is True      # marca "nao chegou" visivel: o agente nao recebeu
    assert ev.ts == 1787013402.691  # ts da 1a tentativa, nao da ultima


def test_system_recado_repetido_tem_id_deterministico():
    # O reenvio grava N entradas system com o MESMO texto; id deterministico pelo texto -> o front
    # deduplica por id e mostra UMA bolha, no lugar da 1a tentativa.
    a = parse_line(_line({"type": "system", "uuid": "u1", "timestamp": "2026-08-18T00:36:42Z",
                          "content": _SYSTEM_BLOQUEADO}))
    b = parse_line(_line({"type": "system", "uuid": "u2", "timestamp": "2026-08-18T00:36:51Z",
                          "content": _SYSTEM_BLOQUEADO}))
    assert a[0].id == b[0].id
    assert a[0].ts < b[0].ts          # ordena pelo momento da 1a tentativa


def test_system_com_embrulho_nativo_vira_bubble_normalizada(monkeypatch):
    # O texto da entrega pode ser um recado NATIVO (embrulho <cross-session-message>): entra no
    # mesmo formato do hangar-send, como o resto do app le.
    import app.registry as registry
    monkeypatch.setattr(registry, "name_of_pid", lambda pid: None)
    bruto = ("<cross-session-message from=\"uds:/run/user/1000/cc-socks/4242.sock\" "
             "from-name=\"desc2-cap\">\ncorpo do recado\n</cross-session-message>")
    [ev] = parse_line(_line({"type": "system", "uuid": "u3",
                             "timestamp": "2026-08-18T00:36:42Z",
                             "content": _SYSTEM_BLOQUEADO.replace(
                                 "[de: desc2-exec2] RODADA 3 ENTREGUE — texto do reporte aqui.",
                                 bruto)}))
    assert ev.kind == "user_msg"
    assert ev.text == "[de: desc2-cap] corpo do recado"


def test_system_fala_da_pessoa_bloqueada_vira_bubble_com_o_erro_do_hook():
    # Hook quebrado barra TODO prompt da conta. Sem a bolha a fala some da conversa e, sem
    # terminal, o erro do hook nao aparece em lugar nenhum.
    [ev] = parse_line(_line({"type": "system", "uuid": "u4", "timestamp": "2026-08-18T00:36:42Z",
                             "content": _SYSTEM_BLOQUEADO.replace(
                                 "[de: desc2-exec2] RODADA 3 ENTREGUE — texto do reporte aqui.",
                                 "Tá aí ainda?")}))
    assert (ev.kind, ev.text, ev.desistiu) == ("user_msg", "Tá aí ainda?", True)
    assert ev.id.startswith("held:")
    assert ev.hook_error.startswith('["/home/jefferson/wt-desc/t8/backend/.venv/bin/python"')
    assert ev.hook_error.endswith("No such file or directory")


def test_system_ruido_de_tooling_nao_vira_bubble():
    # system carrega ruido de verdade: /model, avisos do harness e o aviso de recado SEGURADO
    # (preview truncado, sem o texto). Nenhum vira conversa — o filtro e pelo FORMATO da entrega
    # bloqueada, nunca pelo tipo.
    assert parse_line(_line({"type": "system", "content": None})) == []
    assert parse_line(_line({"type": "system", "content": "Held peer message — from uds:/x.sock "
                          "[verified pid 1] (peer claims name: cap-c4); preview: «de: desc2-rev2 "
                          "Parecer...» — not delivered to Claude (1 held)."})) == []
    assert parse_line(_line({"type": "system", "content": "o usuario citou 'Original prompt: [de: x] y' "
                          "num aviso qualquer"})) == []


def test_system_com_cara_de_recado_sem_ancora_registra_aviso(caplog):
    # Falha CALADA e o modo de falha que fez o defeito (A) durar: se o harness mudar o texto da
    # ancora, o recado para de aparecer e ninguem fica sabendo. Nao vira bubble (o formato nao esta
    # provado), mas vira linha de log.
    conteudo = "Algum aviso novo do harness\n\nPrompt original: [de: outra-sessao] RODADA 3 ENTREGUE"
    with caplog.at_level("WARNING", logger="hangar.transcript"):
        assert parse_line(_line({"type": "system", "uuid": "u9",
                                 "timestamp": "2026-08-18T00:36:42Z",
                                 "content": conteudo})) == []
    assert any("ancora do harness" in r.getMessage() for r in caplog.records)


def test_system_ruido_comum_nao_registra_aviso(caplog):
    # Ruido de tooling (a maioria das entradas system) nao pode virar log: aviso que aparece
    # sempre e aviso que ninguem le.
    with caplog.at_level("WARNING", logger="hangar.transcript"):
        assert parse_line(_line({"type": "system", "uuid": "u10",
                                 "timestamp": "2026-08-18T00:36:42Z",
                                 "content": "Kept model as claude-opus-5"})) == []
    assert caplog.records == []


def _tool_result_line(tool_use_result, extra_results=()):
    content = [{"type": "tool_result", "tool_use_id": "toolu_1",
                "content": "The file /a.ts has been updated successfully."}, *extra_results]
    return _line({"type": "user", "uuid": "u1", "timestamp": "2026-10-04T19:11:00.000Z",
                  "message": {"role": "user", "content": content},
                  "toolUseResult": tool_use_result})


_HUNK = {"oldStart": 6, "oldLines": 3, "newStart": 6, "newLines": 3, "lines": [" a", "-b", "+B", " c"]}


def test_edit_result_carries_patch_hunks_without_the_original_file():
    [ev] = parse_line(_tool_result_line(
        {"filePath": "/a.ts", "originalFile": "x" * 5000, "structuredPatch": [_HUNK]}))
    assert ev.kind == "tool_result"
    assert ev.patch == [{"old_start": 6, "new_start": 6, "lines": [" a", "-b", "+B", " c"]}]
    assert "xxxxx" not in ev.model_dump_json()


def test_created_file_and_text_result_have_no_patch():
    [created] = parse_line(_tool_result_line({"type": "create", "structuredPatch": []}))
    [failed] = parse_line(_tool_result_line("Error: String to replace not found in file."))
    assert created.patch is None
    assert failed.patch is None


@pytest.mark.parametrize("hunk", [
    {**_HUNK, "oldStart": "6"},
    {**_HUNK, "newStart": True},
    {**_HUNK, "oldStart": 6.0},
    {**_HUNK, "lines": "ab"},
    {**_HUNK, "lines": [" a", 3]},
    {**_HUNK, "oldStart": -1},
    {**_HUNK, "newStart": -1},
    {**_HUNK, "oldStart": 2**32},
    {**_HUNK, "newStart": 2**64},
    "hunk",
])
def test_malformed_patch_is_dropped_and_the_result_still_parses(hunk):
    [ev] = parse_line(_tool_result_line({"structuredPatch": [hunk]}))
    assert ev.kind == "tool_result"
    assert ev.patch is None


def test_patch_accepts_the_largest_u32_position():
    [ev] = parse_line(_tool_result_line({"structuredPatch": [{**_HUNK, "oldStart": 2**32 - 1}]}))
    assert ev.patch[0]["old_start"] == 2**32 - 1


def test_patch_accepts_a_zero_start():
    [ev] = parse_line(_tool_result_line({"structuredPatch": [{**_HUNK, "oldStart": 0, "newStart": 0}]}))
    assert (ev.patch[0]["old_start"], ev.patch[0]["new_start"]) == (0, 0)


def test_background_agent_launch_carries_the_agent_id():
    [ev] = parse_line(_tool_result_line({"status": "async_launched", "agentId": "ag1", "isAsync": True}))
    [final] = parse_line(_tool_result_line({"agentId": "ag1"}))
    assert ev.bg_agent_id == "ag1"
    assert final.bg_agent_id is None


def test_patch_keeps_exactly_the_line_ceiling():
    full = {**_HUNK, "lines": ["+x"] * 2000}
    [ev] = parse_line(_tool_result_line({"structuredPatch": [full]}))
    assert len(ev.patch[0]["lines"]) == 2000


def test_patch_over_the_ceiling_across_two_hunks_is_dropped():
    first = {**_HUNK, "lines": ["+x"] * 1000}
    second = {**_HUNK, "lines": ["+y"] * 1001}
    [ev] = parse_line(_tool_result_line({"structuredPatch": [first, second]}))
    assert ev.patch is None


def test_error_result_carries_no_patch():
    content = [{"type": "tool_result", "tool_use_id": "toolu_1", "is_error": True, "content": "falhou"}]
    [ev] = parse_line(_line({"type": "user", "uuid": "u1", "timestamp": "2026-10-04T19:11:00.000Z",
                             "message": {"role": "user", "content": content},
                             "toolUseResult": {"structuredPatch": [_HUNK]}}))
    assert ev.is_error is True
    assert ev.patch is None


def test_patch_over_the_line_ceiling_is_dropped():
    big = {**_HUNK, "lines": ["+x"] * 2001}
    [ev] = parse_line(_tool_result_line({"structuredPatch": [big]}))
    assert ev.patch is None


def test_line_with_two_results_carries_no_patch():
    other = {"type": "tool_result", "tool_use_id": "toolu_2", "content": "ok"}
    events = parse_line(_tool_result_line({"structuredPatch": [_HUNK]}, extra_results=[other]))
    assert [e.patch for e in events] == [None, None]
