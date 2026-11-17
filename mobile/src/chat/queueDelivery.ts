import {
  comTeto, splitContextRefs, errorDetail, parseImageMessage, parsePeerMessage,
  probeServerResponse, sendInputForServer, type AnswerItem, type AskQuestionPayload, type ChatEvent, type Server,
} from '@hangar/core';
import * as m from '../paraglide/messages';

export type DeliveryMode = 'queued' | 'direct' | 'jev' | 'conversation';
export type QueueSendMode = 'steer' | { kind: 'direct'; terminalClaude: boolean } | { kind: 'jev'; task: string; message: string }
  | { kind: 'conversation' };
export type DeliveryFeedback = { text: string; warning: boolean };
export type JevStatus = 'loading' | 'ready' | 'setup' | 'unsupported' | 'error';
export type DeliveryControls = { mode: DeliveryMode; status: JevStatus; help: string };

// Lido na hora do uso: quem só importa o módulo (telas e testes com mensagens simuladas) não depende destas chaves.
export const deliveryModeLabel = (mode: DeliveryMode) =>
  mode === 'queued' ? m.delivery_mode_queue() : mode === 'direct' ? m.delivery_mode_direct()
    : mode === 'jev' ? m.delivery_mode_jev() : m.delivery_mode_conversation();
export const deliveryModeDescription = (mode: DeliveryMode) =>
  mode === 'queued' ? m.delivery_mode_queue_desc() : mode === 'direct' ? m.delivery_mode_direct_desc()
    : mode === 'jev' ? m.delivery_mode_jev_desc() : m.mobile_delivery_mode_conversation_desc();
export type DeliveryOption = { mode: DeliveryMode; checked: boolean; disabled: boolean };
// Jev salvo e indisponível envia pela fila (queueSendMode), então é a Fila que aparece marcada.
// Servidor sem o Jev configurado não oferece a opção.
export function deliveryOptions(d: DeliveryControls): DeliveryOption[] {
  const ready = d.status === 'ready';
  const effective = d.mode === 'jev' && !ready ? 'queued' : d.mode;
  const modes = d.status === 'setup' ? (['queued', 'direct', 'conversation'] as const) : (['queued', 'direct', 'jev', 'conversation'] as const);
  return modes.map((mode) => ({ mode, checked: effective === mode, disabled: mode === 'jev' && !ready }));
}

const chars = (value: string, count: number) => Array.from(value).slice(0, count).join('');
const caption = (value: string) => {
  const body = parseImageMessage(value)?.caption ?? value;
  return splitContextRefs(body)?.typed ?? body;
};

export function queueSendMode(mode: DeliveryMode, ready: boolean, state: string, provider: string | null | undefined,
  headless: boolean, message: string, events: ChatEvent[], ask: AskQuestionPayload | null): QueueSendMode | undefined {
  if (state !== 'working' || (provider !== 'claude' && provider !== 'codex') || message.trimStart().startsWith('/')) return;
  if (mode === 'direct') return { kind: 'direct', terminalClaude: provider === 'claude' && !headless };
  if (mode === 'conversation') return { kind: 'conversation' };
  if (mode !== 'jev' || !ready) return;
  let latest = -1;
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i];
    if (event.kind === 'user_msg' && !event.id.startsWith('queued-') && !event.id.startsWith('pending-') && !parsePeerMessage(caption(event.text ?? ''))) {
      latest = i; break;
    }
  }
  const task = latest < 0 ? '' : caption(events[latest].text ?? '');
  const recent = ask?.is_async ? ask.questions.map((q) => q.question).join('\n')
    : events.slice(latest + 1).reverse().find((e) => e.kind === 'assistant_msg' && e.text?.trim())?.text ?? '';
  let context = !recent.trim() ? chars(task, 1000)
    : `Pedido atual:\n${chars(task, 350)}\n\nFala recente do agente:\n${Array.from(recent).slice(-600).join('')}`;
  if (Array.from(message).length > 2000) context = '';
  return { kind: 'jev', task: context, message: chars(message, 2000) };
}

export function timingFeedback(note: string): DeliveryFeedback {
  const values: Record<string, () => string> = {
    direct: m.native_queue_jev_direct, queue: m.native_queue_jev_queued, delivered: m.native_queue_jev_delivered,
    uncertain: m.native_queue_jev_uncertain, no_context: m.native_queue_jev_no_context, timeout: m.native_queue_jev_timeout,
    work_changed: m.native_queue_jev_work_changed, refused: m.native_queue_jev_refused,
  };
  return { text: (Object.hasOwn(values, note) ? values[note] : m.native_queue_jev_unavailable)(), warning: !['direct', 'queue', 'delivered'].includes(note) };
}

async function request(server: Server, path: string, init: RequestInit): Promise<unknown> {
  const response = await probeServerResponse(server, path, init);
  if (!response.ok) throw Object.assign(new Error(`${response.status}: ${await errorDetail(response)}`), { status: response.status });
  try { return await response.json(); }
  catch (error) {
    if (error instanceof SyntaxError) throw new Error(m.mobile_machines_invalid_response(), { cause: error });
    throw error;
  }
}

export async function readQueueJev(server: Server, signal: AbortSignal): Promise<boolean> {
  const value = await request(server, '/api/jev/delivery', { signal: comTeto(signal, 10_000) });
  if (!value || typeof value !== 'object' || typeof (value as { available?: unknown }).available !== 'boolean') {
    throw new Error(m.mobile_machines_invalid_response());
  }
  return (value as { available: boolean }).available;
}

export async function sendWithDelivery(server: Server, name: string, text: string, mode: Exclude<QueueSendMode, 'steer'>): Promise<DeliveryFeedback | null> {
  if (mode.kind === 'conversation') {
    // A bolha "Em espera" na conversa já descreve o envio.
    await sendInputForServer(server, name, text, 'conversation');
    return null;
  }
  const steer = mode.kind === 'direct' && !mode.terminalClaude;
  const body = mode.kind === 'jev' ? { text, jev_task: mode.task, jev_message: mode.message }
    : steer ? { text } : { text, steer: true };
  // Sem timeout: cancelar a espera não desfaz um envio que o servidor já recebeu.
  const result = await request(server, `/api/sessions/${encodeURIComponent(name)}/${steer ? 'steer' : 'input'}`,
    { method: 'POST', signal: undefined, body: JSON.stringify(body) });
  if (!result || typeof result !== 'object' || (result as { ok?: unknown }).ok !== true) throw new Error(m.native_action_uncertain());
  if (mode.kind === 'direct') {
    const reply = result as { steered?: unknown; delivered?: unknown };
    if (steer || reply.steered === true) return { text: m.native_queue_panel_steered(), warning: false };
    // Sem orientação e sem entrega, a mensagem ficou esperando a vez: quem escolheu Direto precisa saber.
    return reply.delivered === true ? null : { text: m.mobile_queue_direct_queued(), warning: true };
  }
  const note = (result as { timing_note?: unknown }).timing_note;
  return typeof note === 'string' ? timingFeedback(note) : { text: m.native_action_uncertain(), warning: true };
}

export async function answerWithDelivery(server: Server, name: string, answers: AnswerItem[], displayed: AskQuestionPayload): Promise<{ ok: boolean; fallback?: boolean; feedback: DeliveryFeedback | null }> {
  const result = await request(server, `/api/sessions/${encodeURIComponent(name)}/answer`, {
    method: 'POST', signal: undefined,
    body: JSON.stringify({ answers, request_id: displayed.request_id, interaction_id: displayed.interaction_id, steer: true }),
  });
  if (!result || typeof result !== 'object' || typeof (result as { ok?: unknown }).ok !== 'boolean') throw new Error(m.native_action_uncertain());
  const reply = result as { ok: boolean; fallback?: unknown; queued?: unknown; timing_note?: unknown };
  const feedback = typeof reply.timing_note === 'string' ? timingFeedback(reply.timing_note)
    : reply.queued === true ? { text: m.mobile_queue_direct_queued(), warning: true }
      : typeof reply.fallback !== 'boolean' ? { text: m.native_action_uncertain(), warning: true } : null;
  return { ok: reply.ok, fallback: reply.fallback === true, feedback };
}
