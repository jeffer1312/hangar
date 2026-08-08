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
    # (antes o tool_use vencia e o texto sumia do chat). thinking e ignorado.
    evs = parse_line(_line({
        "type": "assistant", "uuid": "a9",
        "message": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "vou rodar"},
            {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}},
        ]},
    }))
    assert [(e.kind, e.id) for e in evs] == [("assistant_msg", "a9"), ("tool_use", "a9:1")]
    assert evs[0].text == "vou rodar"
    assert evs[1].tool_use_id == "toolu_1"


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
    # app nao mostraria recado NENHUM. Tem que virar bubble no mesmo formato do cp-send ("[de: X]"),
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
    # Nome TMUX (o endereco do cp-send), nao o `origin.name` (que e o titulo da sessao).
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
