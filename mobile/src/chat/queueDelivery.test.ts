import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, type ChatEvent } from '@hangar/core';
import { answerWithDelivery, deliveryOptions, queueSendMode, readQueueJev, sendWithDelivery, timingFeedback } from './queueDelivery';
import * as m from '../paraglide/messages';

const server = { id: 'owner', label: 'fixture', baseUrl: 'https://owner.test', token: 'owner-token' };
const event = (id: string, kind: ChatEvent['kind'], text: string): ChatEvent => ({ id, kind, text });
const events = [event('u', 'user_msg', 'criar tela'), event('a', 'assistant_msg', 'Posso usar azul?'),
  event('queued-q', 'user_msg', 'próxima tarefa'), event('pending-p', 'user_msg', 'pendente'), event('peer', 'user_msg', '[de: par] recado')];
const select = (text = 'sim', changes: Partial<{ mode: 'queued' | 'direct' | 'jev' | 'conversation'; ready: boolean; state: string; provider: string; headless: boolean }> = {}) => {
  const { mode = 'jev', ready = true, state = 'working', provider = 'codex', headless = false } = changes;
  return queueSendMode(mode, ready, state, provider, headless, text, events, null);
};

it('a decisão usa o pedido real e a fala recente, ignorando a fila e recados', () => {
  expect(select()).toEqual({ kind: 'jev', task: 'Pedido atual:\ncriar tela\n\nFala recente do agente:\nPosso usar azul?', message: 'sim' });
});

it('a pergunta assíncrona substitui a fala recente', () => {
  expect(queueSendMode('jev', true, 'working', 'codex', false, 'sim', events,
    { is_async: true, questions: [{ header: 'fixture', question: 'Confirma excluir?', multiSelect: false, options: [] }] }))
    .toEqual({ kind: 'jev', task: 'Pedido atual:\ncriar tela\n\nFala recente do agente:\nConfirma excluir?', message: 'sim' });
});

it.each([{ mode: 'queued' as const }, { ready: false }, { state: 'idle' }, { state: 'awaiting_input' }, { provider: 'pi' }])(
  'conserva envio normal fora do turno suportado: %j', (changes) => expect(select('oi', changes)).toBeUndefined());
it('comandos não passam por nenhum modo automático', () => {
  expect(select('/clear')).toBeUndefined();
  expect(select('/clear', { mode: 'direct' })).toBeUndefined();
});
it.each([
  ['claude', false, true], ['claude', true, false], ['codex', false, false],
] as const)('Direto seleciona a rota de %s (sem terminal: %s)', (provider, headless, terminalClaude) => {
  expect(select('oi', { mode: 'direct', provider, headless })).toEqual({ kind: 'direct', terminalClaude });
});
it('limita por caracteres Unicode e conserva o fim da fala recente', () => {
  const value = queueSendMode('jev', true, 'working', 'claude', true, '🙂'.repeat(2000),
    [event('u', 'user_msg', '🙂'.repeat(500)), event('a', 'assistant_msg', 'x'.repeat(700) + 'fim')], null);
  expect(value).toEqual({ kind: 'jev', task: `Pedido atual:\n${'🙂'.repeat(350)}\n\nFala recente do agente:\n${'x'.repeat(597)}fim`, message: '🙂'.repeat(2000) });
  expect(select('🙂'.repeat(2001))).toEqual({ kind: 'jev', task: '', message: '🙂'.repeat(2000) });
});
it('sem contexto não inventa tarefa; sem fala recente limita o pedido', () => {
  expect(queueSendMode('jev', true, 'working', 'codex', false, 'sim', [], null)).toEqual({ kind: 'jev', task: '', message: 'sim' });
  expect(queueSendMode('jev', true, 'working', 'codex', false, 'sim', [event('u', 'user_msg', 'x'.repeat(1100))], null))
    .toEqual({ kind: 'jev', task: 'x'.repeat(1000), message: 'sim' });
});

it.each([
  ['direct', m.native_queue_jev_direct, false], ['queue', m.native_queue_jev_queued, false], ['delivered', m.native_queue_jev_delivered, false],
  ['uncertain', m.native_queue_jev_uncertain, true], ['no_context', m.native_queue_jev_no_context, true], ['timeout', m.native_queue_jev_timeout, true],
  ['work_changed', m.native_queue_jev_work_changed, true], ['refused', m.native_queue_jev_refused, true], ['future', m.native_queue_jev_unavailable, true],
] as const)('traduz o resultado %s sem afirmar entrega em aviso', (note, text, warning) => {
  expect(timingFeedback(note)).toEqual({ text: text(), warning });
});

beforeEach(() => {
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', onUnauthorized: vi.fn(), origin: null, createEventSource: vi.fn() });
  vi.stubGlobal('fetch', vi.fn());
});
afterEach(() => vi.unstubAllGlobals());
it.each([true, false])('lê disponibilidade %s do servidor da conversa', async (available) => {
  vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ available })));
  await expect(readQueueJev(server, new AbortController().signal)).resolves.toBe(available);
  expect(vi.mocked(fetch).mock.calls[0][0]).toBe('https://owner.test/api/jev/delivery');
});
it.each([{}, { available: 'true' }])('não habilita o Jev com resposta fora do contrato: %j', async (value) => {
  vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify(value)));
  await expect(readQueueJev(server, new AbortController().signal)).rejects.toThrow();
});
it('conserva o 404 para explicar servidor sem suporte', async () => {
  vi.mocked(fetch).mockResolvedValue(new Response('{"detail":"Not Found"}', { status: 404 }));
  await expect(readQueueJev(server, new AbortController().signal)).rejects.toMatchObject({ status: 404 });
});
it('Direto no Codex usa steer sem prazo local ou outro envio', async () => {
  vi.mocked(fetch).mockResolvedValue(new Response('{"ok":true}'));
  await expect(sendWithDelivery(server, 'sess', 'oi', { kind: 'direct', terminalClaude: false }))
    .resolves.toEqual({ text: m.native_queue_panel_steered(), warning: false });
  const [url, init] = vi.mocked(fetch).mock.calls[0];
  expect(url).toBe('https://owner.test/api/sessions/sess/steer');
  expect(JSON.parse(init!.body as string)).toEqual({ text: 'oi' });
  expect(init!.signal).toBeUndefined();
  expect(fetch).toHaveBeenCalledTimes(1);
});
it.each([
  [{ steered: true, delivered: true }, { text: m.native_queue_panel_steered(), warning: false }],
  [{ steered: false, delivered: false }, { text: m.mobile_queue_direct_queued(), warning: true }],
  [{ steered: false, delivered: true }, null],
])('Direto no Claude com terminal diz se a mensagem ficou na fila: %j', async (reply, feedback) => {
  vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ ok: true, native: false, ...reply })));
  await expect(sendWithDelivery(server, 'sess', 'oi', { kind: 'direct', terminalClaude: true })).resolves.toEqual(feedback);
});
const ask = { is_async: true, provider: 'codex' as const, request_id: 'r1', interaction_id: 'i1', questions: [] };
it.each([
  [{ ok: true, fallback: false }, null],
  [{ ok: true, fallback: false, queued: true }, { text: m.mobile_queue_direct_queued(), warning: true }],
  [{ ok: true, fallback: false, timing_note: 'queue' }, { text: m.native_queue_jev_queued(), warning: false }],
  [{ ok: true }, { text: m.native_action_uncertain(), warning: true }],
])('resposta orientada devolve o aviso da entrega: %j', async (reply, feedback) => {
  vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify(reply)));
  await expect(answerWithDelivery(server, 'sess', [], ask)).resolves.toMatchObject({ ok: true, feedback });
});

it('opções do modo: a salva vem marcada; Jev indisponível fica desativado e marca a Fila; sem Jev configurado some', () => {
  const pick = (mode: 'queued' | 'direct' | 'jev' | 'conversation', status: 'loading' | 'ready' | 'setup' | 'unsupported' | 'error') =>
    deliveryOptions({ mode, status, help: 'h' }).map((o) => `${o.mode}:${o.checked ? 'on' : 'off'}:${o.disabled ? 'dis' : 'ok'}`);
  expect(pick('direct', 'ready')).toEqual(['queued:off:ok', 'direct:on:ok', 'jev:off:ok', 'conversation:off:ok']);
  expect(pick('jev', 'ready')).toEqual(['queued:off:ok', 'direct:off:ok', 'jev:on:ok', 'conversation:off:ok']);
  expect(pick('jev', 'setup')).toEqual(['queued:on:ok', 'direct:off:ok', 'conversation:off:ok']);
  expect(pick('jev', 'error')).toEqual(['queued:on:ok', 'direct:off:ok', 'jev:off:dis', 'conversation:off:ok']);
  expect(pick('direct', 'loading')).toEqual(['queued:off:ok', 'direct:on:ok', 'jev:off:dis', 'conversation:off:ok']);
  expect(pick('conversation', 'setup')).toEqual(['queued:off:ok', 'direct:off:ok', 'conversation:on:ok']);
});

it('"Na conversa" vale só com a sessão trabalhando, em Claude e Codex, e nunca para comando', () => {
  const pick = (changes: Partial<{ state: string; provider: string; headless: boolean }>, text = 'oi') => {
    const { state = 'working', provider = 'claude', headless = true } = changes;
    return queueSendMode('conversation', false, state, provider, headless, text, events, null);
  };
  expect(pick({})).toEqual({ kind: 'conversation' });
  // Claude com terminal fica na Fila; Codex com e sem terminal vai pelo caminho suave.
  expect(pick({ headless: false })).toBeUndefined();
  expect(pick({ provider: 'codex', headless: false })).toEqual({ kind: 'conversation' });
  expect(pick({ provider: 'codex', headless: true })).toEqual({ kind: 'conversation' });
  expect(pick({ state: 'idle' })).toBeUndefined();
  expect(pick({ provider: 'kimi' })).toBeUndefined();
  expect(pick({}, '/clear')).toBeUndefined();
});

it('"Na conversa" manda delivery; o servidor antigo recusa com 422 e a mensagem vai pela fila comum', async () => {
  vi.mocked(fetch)
    .mockResolvedValueOnce(new Response(JSON.stringify({ detail: [{ type: 'extra_forbidden', loc: ['body', 'delivery'],
      msg: 'Extra inputs are not permitted', input: 'conversation' }] }), { status: 422 }))
    .mockResolvedValueOnce(new Response('{"ok":true,"delivered":false}'));
  await expect(sendWithDelivery(server, 'sess', 'oi', { kind: 'conversation' })).resolves.toBeNull();
  const calls = vi.mocked(fetch).mock.calls;
  expect(calls).toHaveLength(2);
  expect(calls[0][0]).toBe('https://owner.test/api/sessions/sess/input');
  expect(JSON.parse(calls[0][1]!.body as string)).toEqual({ text: 'oi', delivery: 'conversation' });
  expect(JSON.parse(calls[1][1]!.body as string)).toEqual({ text: 'oi' });
});

it('"Na conversa" aceita de primeira não repete o envio', async () => {
  vi.mocked(fetch).mockResolvedValue(new Response('{"ok":true,"delivered":false,"conversation":true}'));
  await expect(sendWithDelivery(server, 'sess', 'oi', { kind: 'conversation' })).resolves.toBeNull();
  expect(fetch).toHaveBeenCalledTimes(1);
});
