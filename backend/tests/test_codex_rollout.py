"""Testes do parser do rollout JSONL do Codex CLI -> ChatEvent (mesmo shape do Claude)."""
import json
from pathlib import Path

import pytest

from app.adapters.codex.rollout import parse_rollout_line, parse_rollout_obj


def test_session_meta_ignored():
    obj = {"timestamp": "t", "type": "session_meta", "payload": {"id": "u", "cwd": "."}}
    assert parse_rollout_obj(obj) == []


def test_developer_message_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "developer",
                       "content": [{"type": "input_text", "text": "<permissions instructions>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_message():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "oi"}]}}
    evs = parse_rollout_obj(obj)
    assert len(evs) == 1 and evs[0].kind == "user_msg" and "oi" in evs[0].text


def test_user_environment_context_wrapper_ignored():
    # Fix 1: o Codex injeta um response_item/message/role:"user" no inicio de toda thread cujo
    # unico bloco input_text e um wrapper de contexto interno -- nao e chat do usuario, nao pode
    # virar a 1a bolha da sessao.
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "<environment_context>\ncwd: /x\n</environment_context>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_generic_instructions_wrapper_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "<user_instructions>seja util</user_instructions>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_recommended_plugins_wrapper_ignored():
    # O host anexa este bloco como role:user antes da mensagem real. Era o vazamento visto no app:
    # como nao comeca por environment_context, a bolha enorme (incluindo permissoes no fim) aparecia.
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "<recommended_plugins>\n...\n</recommended_plugins>"
                                            "<environment_context>...</environment_context>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_permissions_instructions_with_space_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text",
                                    "text": "<permissions instructions>\n...\n"
                                            "</permissions instructions>"}]}}
    assert parse_rollout_obj(obj) == []


def test_user_agents_md_injection_ignored():
    # Quando o cwd tem AGENTS.md, o Codex injeta o conteudo dele como role:"user" gigante que COMECA
    # com "# AGENTS.md instructions for <path>" (com o environment_context concatenado no fim) -- nao
    # e chat do usuario. Sem este filtro, vazava como a 1a bolha da sessao (bug visto ao vivo).
    text = ("# AGENTS.md instructions for /home/jefferson/pessoal/advocacia\n\n<INSTRUCTIONS>\n"
            "# AGENTS.md\n...regras...\n</INSTRUCTIONS>\n<environment_context>\n<cwd>/x</cwd>\n"
            "</environment_context>")
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": text}]}}
    assert parse_rollout_obj(obj) == []


def test_user_message_mentioning_agents_md_kept():
    # Conservador: uma mensagem REAL do usuario que apenas MENCIONA AGENTS.md nao pode ser descartada.
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "user",
                       "content": [{"type": "input_text", "text": "edita o AGENTS.md pra adicionar X"}]}}
    evs = parse_rollout_obj(obj)
    assert len(evs) == 1 and evs[0].kind == "user_msg"


def test_assistant_message_output_text():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "message", "role": "assistant",
                       "content": [{"type": "output_text", "text": "ok"}], "phase": "final_answer"}}
    evs = parse_rollout_obj(obj)
    assert len(evs) == 1 and evs[0].kind == "assistant_msg" and evs[0].text == "ok"


def test_reasoning_encrypted_ignored():
    obj = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "reasoning", "summary": [], "encrypted_content": "gAAAA..."}}
    assert parse_rollout_obj(obj) == []  # opaco no rollout; v1 ignora


def test_tool_call_and_result():
    call = {"timestamp": "t", "type": "response_item",
            "payload": {"type": "function_call", "name": "shell", "arguments": "{}", "call_id": "c1"}}
    out = {"timestamp": "t", "type": "response_item",
           "payload": {"type": "function_call_output", "call_id": "c1", "output": "done"}}
    assert parse_rollout_obj(call)[0].kind == "tool_use"
    assert parse_rollout_obj(out)[0].kind == "tool_result"


def test_real_fixture_parses():
    p = Path(__file__).parent / "fixtures/codex/rollout_sample.jsonl"
    evs = [e for ln in p.read_text(encoding="utf-8").splitlines() if ln.strip()
           for e in parse_rollout_line(ln)]
    kinds = {e.kind for e in evs}
    assert "user_msg" in kinds and "assistant_msg" in kinds


# -- Os dois pré-requisitos do estado ao vivo (ticket 02) ----------------------
# Ligar o marcador de estado do Codex faz o backend passar a rodar o gatilho de transição também
# nesse provider, e lá dentro moram dois caminhos que digitam no pane. Estes casos travam a
# regressão ANTES de ela existir.

def test_confirmacao_de_entrega_entende_o_rollout(tmp_path):
    # Sem o ramo do Codex o oráculo devolve set() vazio, e vazio ali significa "nada chegou" — que
    # é exatamente o que autoriza o reconcile a re-enfileirar e o drain a REDIGITAR a mensagem do
    # usuário na conversa (o incidente já registrado no Pi e no Kimi).
    from app import pqueue
    texto = "responda apenas: ok"
    f = tmp_path / "rollout-2026-08-30T10-00-00-019f5c00-5d7d-7dd2-b2cb-085ca6d76251.jsonl"
    f.write_text("\n".join([
        json.dumps({"timestamp": "2026-08-30T10:00:00.000Z", "type": "response_item", "payload": {
            "type": "message", "role": "user",
            "content": [{"type": "input_text",
                         "text": "<environment_context>x</environment_context>"}]}}),
        json.dumps({"timestamp": "2026-08-30T10:00:01.000Z", "type": "response_item", "payload": {
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": texto}]}}),
    ]) + "\n", encoding="utf-8")

    committed = pqueue.committed_user_lines(str(f), provider="codex")
    assert texto in committed
    assert "<environment_context>x</environment_context>" not in committed   # não é fala do usuário


def test_confirmacao_de_entrega_no_rollout_real():
    from app import pqueue
    p = Path(__file__).parent / "fixtures/codex/rollout_sample.jsonl"
    assert "responda apenas: ok" in pqueue.committed_user_lines(str(p), provider="codex")


# -- Ticket 06: o comando que o Codex rodou aparece na conversa ----------------
# `exec` e a ferramenta que ele mais usa, e ela chega como `custom_tool_call`/
# `custom_tool_call_output` — os dois tipos que o parser descartava. A fixture e COPIADA de
# rollouts reais desta maquina (ja houve regressao por um teste fabricar um campo que o agente
# nunca manda).

def _eventos_exec():
    p = Path(__file__).parent / "fixtures/codex/rollout_exec.jsonl"
    return [e for ln in p.read_text(encoding="utf-8").splitlines() if ln.strip()
            for e in parse_rollout_line(ln)]


def test_exec_vira_tool_use_e_tool_result():
    evs = _eventos_exec()
    assert [e.kind for e in evs] == ["tool_use", "tool_result"] * 4
    assert {e.tool_name for e in evs if e.kind == "tool_use"} == {"exec", "apply_patch"}
    # O par continua ligado pelo call_id — e o que o front usa pra casar chamada e resultado.
    assert evs[0].tool_use_id == evs[1].tool_use_id


def test_comando_e_extraido_do_codigo_com_e_sem_aspas_na_chave():
    """O `cmd` vem dentro de codigo JavaScript, e o Codex escreve a chave das duas formas:
    `{cmd:"..."}` e `{"cmd":"..."}`. As duas aparecem na fixture real."""
    usos = [e for e in _eventos_exec() if e.kind == "tool_use"]
    assert usos[0].tool_input["command"] == "sleep 30"
    assert usos[2].tool_input["command"] == "echo oi"


def test_codigo_cru_fica_guardado_sempre():
    """Contrato do ticket: o codigo cru fica no evento MESMO quando o comando saiu — o resumo usa o
    comando, e quem abre o card ainda ve o que foi executado de verdade."""
    usos = [e for e in _eventos_exec() if e.kind == "tool_use"]
    assert usos[0].tool_input["code"].startswith("const r = await tools.exec_command(")


def test_sem_cmd_no_codigo_sobra_o_codigo_e_nunca_um_campo_vazio():
    """`tools.write_stdin(...)` nao tem `cmd`. Falhando a extracao, o resumo mostra o codigo — um
    `command: ""` faria a linha aparecer vazia, que e pior que mostrar o codigo."""
    uso = [e for e in _eventos_exec() if e.kind == "tool_use"][1]
    assert "command" not in uso.tool_input
    assert "tools.write_stdin" in uso.tool_input["code"]


def test_saida_em_lista_de_blocos_vira_texto():
    """A saida do `exec` e uma LISTA de blocos, nao um escalar como no function_call_output."""
    res = [e for e in _eventos_exec() if e.kind == "tool_result"]
    assert "Script completed" in res[0].result
    assert "wall_time_seconds" in res[0].result   # o 2o bloco tambem entra, nao so o 1o


def test_saida_em_string_continua_valendo():
    """O mesmo tipo tambem aparece com `output` string na maquina — os dois formatos sao reais."""
    res = [e for e in _eventos_exec() if e.kind == "tool_result"]
    assert res[1].result.startswith("Script running with cell ID 2")


def test_apply_patch_resume_pelos_arquivos_e_nao_pelo_diff():
    """`apply_patch` e o `Edit` do Codex, e vem pelo MESMO tipo de entrada que o `exec`. Sem campo
    saliente, o front cai no generico e a linha de resumo vira o diff cortado — trocaria
    "invisivel" por "lixo truncado", que nao e melhora. `file_path` e a chave que ele ja sabe
    resumir."""
    uso = [e for e in _eventos_exec() if e.tool_name == "apply_patch"][0]
    assert uso.tool_input["file_path"] == [
        "/home/jefferson/Projetos/claude-cockpit/backend/tests/test_codex_registry.py"]
    assert uso.tool_input["code"].startswith("*** Begin Patch")
    assert "command" not in uso.tool_input


@pytest.mark.parametrize("codigo", [
    "const r = await tools.exec_command({cmd:'echo oi'});",       # aspas SIMPLES
    "const r = await tools.exec_command({cmd:`echo oi`});",       # template literal
])
def test_forma_de_aspas_que_a_regex_nao_le_cai_no_codigo(codigo):
    """Nenhuma das duas aparece nos rollouts desta maquina — nao da pra saber se o Codex as produz,
    e o ticket proibe fabricar fixture. O que este caso trava e a DEGRADACAO: seja qual for a forma,
    o card nunca fica com a linha em branco; sobra o codigo, que e o que foi executado."""
    obj = {"type": "response_item", "payload": {
        "type": "custom_tool_call", "name": "exec", "call_id": "c1", "input": codigo}}
    entrada = parse_rollout_obj(obj)[0].tool_input
    assert "command" not in entrada
    assert entrada["code"] == codigo


def test_bloco_de_outro_tipo_nao_entra_na_saida():
    """A saida usa o MESMO leitor de blocos das mensagens, com o mesmo filtro de tipo: um bloco de
    outro tipo entrando aqui e nao la seria uma diferenca que ninguem escreveu de proposito."""
    obj = {"type": "response_item", "payload": {
        "type": "custom_tool_call_output", "call_id": "c1",
        "output": [{"type": "input_text", "text": "vale"},
                   {"type": "output_audio", "text": "nao vale"}]}}
    assert parse_rollout_obj(obj)[0].result == "vale"


def test_function_call_output_com_lista_nao_vira_repr_de_python():
    """Irmao do mesmo defeito: este tipo TAMBEM chega com lista de blocos (ferramenta `wait`), e o
    `str()` de antes punha `[{'type': 'input_text', ...}]` na conversa em vez da saida."""
    obj = {"type": "response_item", "payload": {
        "type": "function_call_output", "call_id": "c1",
        "output": [{"type": "input_text", "text": "Script completed\n"}]}}
    assert parse_rollout_obj(obj)[0].result == "Script completed\n"
