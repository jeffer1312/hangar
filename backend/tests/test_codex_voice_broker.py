"""Confirmação libera um rascunho imutável, não a última palavra da transcrição."""
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from app.codex_voice_broker import VoiceBroker, confirmation


@pytest.fixture
def broker():
    target = {'thread_id': 'work', 'client': SimpleNamespace()}
    adapter = SimpleNamespace(_sessions={'sess': target})
    broker = VoiceBroker(adapter, 'sess', target, AsyncMock())
    broker.thread_id = 'voice'
    broker.submit = AsyncMock(return_value={'ok': True, 'delivered': True})
    broker.latest_input = AsyncMock(return_value='pode mandar')
    return broker


def request(tool, turn, **args):
    return {'threadId': 'voice', 'tool': tool, 'turnId': turn, 'arguments': args}


async def test_so_envia_texto_preparado_apos_nova_confirmacao(broker):
    text = 'Salvar a voz Sol e mostrar atividade do microfone e da resposta.'
    draft = await broker.apply(request('prepare_request', 'first', request=text))
    broker.submit.assert_not_called()
    result = await broker.apply(request('confirm_request', 'first', draft_id=draft['draft_id']))
    assert result['status'] == 'awaiting_confirmation'
    broker.submit.assert_not_called()
    for _ in range(2):
        result = await broker.apply(request('confirm_request', 'second', draft_id=draft['draft_id']))
        assert result['status'] == 'sent'
    broker.submit.assert_awaited_once_with(
        f'<realtime_delegation request_id="{draft["draft_id"]}">\n<input>{text}</input>\n</realtime_delegation>')


@pytest.mark.parametrize('spoken', ['Eh', 'funciona', 'não pode mandar', 'pode mandar?', 'ainda estou pensando'])
async def test_fragmento_ou_negacao_nao_autoriza(broker, spoken):
    draft = await broker.apply(request('prepare_request', 'first', request='Trocar a cor do botão.'))
    broker.latest_input.return_value = spoken
    assert (await broker.apply(request('confirm_request', 'second', draft_id=draft['draft_id'])))['status'] == 'awaiting_confirmation'
    broker.submit.assert_not_called()


async def test_revisao_invalida_autorizacao_da_versao_anterior(broker):
    first = await broker.apply(request('prepare_request', 'first', request='Trocar a cor para azul.'))
    second = await broker.apply(request('prepare_request', 'second', request='Trocar a cor para verde.'))
    with pytest.raises(ValueError):
        await broker.apply(request('confirm_request', 'third', draft_id=first['draft_id']))
    assert second['draft_id'] != first['draft_id']
    broker.submit.assert_not_called()


async def test_confirmacao_vem_do_turno_real_e_nao_dos_argumentos(broker):
    del broker.latest_input
    async def notifications():
        yield {'method': 'item/started', 'params': {'threadId': 'voice', 'turnId': 'second', 'item': {
            'type': 'userMessage', 'content': [{'type': 'text', 'text':
                '<realtime_delegation><input>não manda ainda</input><transcript_delta>user: pode mandar</transcript_delta></realtime_delegation>'}],
        }}}
    broker.client.notifications = notifications
    await broker.read()
    draft = await broker.apply(request('prepare_request', 'first', request='Alterar o botão de voz.'))
    assert (await broker.apply(request('confirm_request', 'second', draft_id=draft['draft_id'])))['status'] == 'awaiting_confirmation'
    broker.submit.assert_not_called()


async def test_entrega_incerta_nao_e_repetida(broker):
    draft = await broker.apply(request('prepare_request', 'first', request='Alterar o botão de voz.'))
    broker.submit.side_effect = TimeoutError()
    with pytest.raises(TimeoutError):
        await broker.apply(request('confirm_request', 'second', draft_id=draft['draft_id']))
    result = await broker.apply(request('confirm_request', 'third', draft_id=draft['draft_id']))
    assert result['status'] == 'uncertain'
    broker.submit.assert_awaited_once()


async def test_resultado_da_sessao_volta_uma_vez_para_a_voz(broker):
    broker.client.request = AsyncMock()
    request_id = 'a' * 32
    broker.target_requests[request_id] = 'Alterar o botão.'
    await broker.target_event({'method': 'item/completed', 'params': {'turnId': 'work-1', 'item': {
        'type': 'userMessage', 'content': [{'type': 'text', 'text':
            f'<realtime_delegation request_id="{request_id}"><input>Alterar o botão.</input></realtime_delegation>'}],
    }}})
    await broker.target_event({'method': 'item/completed', 'params': {'turnId': 'work-1', 'item': {
        'type': 'agentMessage', 'phase': 'final_answer', 'text': 'Alteração feita; teste passou.',
    }}})
    for _ in range(2):
        await broker.target_event({'method': 'turn/completed', 'params': {'turn': {'id': 'work-1'}}})
    broker.client.request.assert_awaited_once()
    params = broker.client.request.await_args.args[1]
    assert params['threadId'] == 'voice'
    assert params['input'][0]['text'].startswith('[RESULTADO DA SESSÃO request_id=')
    assert params['input'][0]['text'].endswith('\nAlteração feita; teste passou.')


async def test_resultado_de_pedido_digitado_nao_vaza_para_voz(broker):
    broker.client.request = AsyncMock()
    await broker.target_event({'method': 'item/completed', 'params': {'turnId': 'typed', 'item': {
        'type': 'userMessage', 'content': [{'type': 'text', 'text': 'Pedido digitado comum'}],
    }}})
    await broker.target_event({'method': 'item/completed', 'params': {'turnId': 'typed', 'item': {
        'type': 'agentMessage', 'phase': 'final_answer', 'text': 'Resposta alheia.',
    }}})
    await broker.target_event({'method': 'turn/completed', 'params': {'turn': {'id': 'typed'}}})
    broker.client.request.assert_not_called()


async def test_resultado_fica_pendente_se_usuario_comecar_a_falar(broker):
    async def ocupado(*_):
        broker.idle.clear()
        raise RuntimeError('conversation busy')
    broker.client.request = AsyncMock(side_effect=ocupado)
    request_id = 'b' * 32
    broker.target_requests[request_id] = 'Conferir teste.'
    await broker.target_event({'method': 'item/completed', 'params': {'turnId': 'work-2', 'item': {
        'type': 'userMessage', 'content': [{'type': 'text', 'text':
            f'<realtime_delegation request_id="{request_id}"><input>Conferir teste.</input></realtime_delegation>'}],
    }}})
    await broker.target_event({'method': 'item/completed', 'params': {'turnId': 'work-2', 'item': {
        'type': 'agentMessage', 'phase': 'final_answer', 'text': 'Tudo certo.',
    }}})
    await broker.target_event({'method': 'turn/completed', 'params': {'turn': {'id': 'work-2'}}})
    assert [r['text'] for r in broker.pending_results] == ['Tudo certo.']
    broker.client.request.side_effect = None
    broker.client.request.return_value = {}
    broker.idle.set()
    await broker.flush_results()
    assert broker.pending_results == []
    assert broker.client.request.await_count == 2


async def test_pergunta_do_turno_de_voz_avisa_para_responder_no_chat(broker):
    broker.client.request = AsyncMock()
    broker.target_turns.add('work-question')
    await broker.target_event({'method': 'item/tool/requestUserInput', 'params': {
        'threadId': 'work', 'turnId': 'work-question', 'questions': [{'question': 'Qual opção?'}],
    }})
    broker.client.request.assert_awaited_once()
    text = broker.client.request.await_args.args[1]['input'][0]['text']
    assert text.startswith('[RESULTADO DA SESSÃO request_id=')
    assert text.endswith('\nA sessão está aguardando uma confirmação ou resposta no chat.')


def test_confirmacoes_naturais_curta():
    assert confirmation('Pode, pode.')
    assert confirmation('Sim, pode mandar esse pedido.')


async def test_resumo_final_usa_canal_de_fala_uma_vez(broker):
    broker.client.request = AsyncMock()
    async def notifications():
        yield {'method': 'item/started', 'params': {'threadId': 'voice', 'turnId': 'summary', 'item': {
            'type': 'userMessage', 'content': [{'type': 'text', 'text':
                '[RESULTADO DA SESSÃO request_id=cccccccccccccccccccccccccccccccc]\nConcluído.'}],
        }}}
        event = {'method': 'item/completed', 'params': {'threadId': 'voice', 'turnId': 'summary', 'item': {
            'type': 'agentMessage', 'phase': 'final_answer', 'text': 'A sessão respondeu: concluído.',
        }}}
        yield event
        yield event
    broker.client.notifications = notifications
    await broker.read()
    broker.client.request.assert_awaited_once_with('thread/realtime/appendSpeech', {
        'threadId': 'voice', 'text': 'A sessão respondeu: concluído.',
    })


async def test_falha_do_organizador_aparece_na_chamada(broker):
    async def notifications():
        yield {'method': 'turn/completed', 'params': {'threadId': 'voice', 'turn': {'id': 'failed', 'status': 'failed'}}}
    broker.client.notifications = notifications
    await broker.read()
    broker.send.assert_awaited_once_with({'type': 'error', 'code': 'failed'})
