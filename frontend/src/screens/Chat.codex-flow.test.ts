// @vitest-environment happy-dom
// O trajeto usa Chat, Composer, MessageList e o stepper reais; só rede e painéis auxiliares são simulados.
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createRawSnippet, mount, tick, unmount } from 'svelte';
import Chat from './Chat.svelte';
import * as api from '@hangar/core';
import { guardarCaudaChat } from '../lib/queries';
import * as m from '../paraglide/messages';
import { overwriteGetLocale } from '../paraglide/runtime';

function vazio() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }
vi.mock('../components/NavBar.svelte', vazio);
vi.mock('../components/SessionSwitcherSheet.svelte', vazio);
vi.mock('../components/CreateSessionSheet.svelte', vazio);
vi.mock('../components/UsageSheet.svelte', vazio);
vi.mock('../components/Git.svelte', vazio);
vi.mock('../components/PreviewSheet.svelte', vazio);
vi.mock('../components/ActivitySheet.svelte', vazio);
vi.mock('../components/TerminalMirror.svelte', vazio);
vi.mock('../components/RunSheet.svelte', vazio);
vi.mock('../components/MoreSheet.svelte', vazio);
vi.mock('../components/AttachmentsSheet.svelte', vazio);
vi.mock('../components/CodexLimitsSheet.svelte', vazio);
vi.mock('../components/ForwardSheet.svelte', vazio);
vi.mock('../components/PairSheet.svelte', vazio);
vi.mock('../components/PairChatModal.svelte', vazio);
vi.mock('../components/LoopSheet.svelte', vazio);
vi.mock('../components/DesktopSessionContext.svelte', vazio);
vi.mock('../components/files/FileViewer.svelte', vazio);
vi.mock('../lib/aquecimento', () => ({
  aoAquecer: () => Promise.resolve(), segurarAquecimento: vi.fn(), soltarAquecimento: vi.fn(),
}));
vi.mock('../lib/ttsPlayer.svelte', () => ({ ttsPlayer: { active: false, loading: false } }));
vi.mock('../lib/auth', () => ({
  listServers: () => [{ id: 'codex-flow', label: 'Teste', baseUrl: 'http://teste', token: 'teste' }],
  getActiveId: () => 'codex-flow',
}));

const sse = vi.hoisted(() => ({ handlers: new Map<string, (event: MessageEvent) => void>() }));
const harness = vi.hoisted(() => ({ provider: 'codex' as 'claude' | 'codex' }));
const sessionsStoreCtl = vi.hoisted(() => ({ pairPeers: null as string[] | null }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof api>(),
  getHistory: vi.fn().mockResolvedValue([]),
  getHistoryDesde: vi.fn().mockResolvedValue({ eventos: [], etag: null }),
  getSessions: vi.fn(async () => [{ name: 'codex-flow', provider: harness.provider, tracked: true, state: 'working', cwd: '/teste' }]),
  getSessionPlanPreview: vi.fn().mockResolvedValue(null),
  getRunners: vi.fn().mockResolvedValue({ running: false }),
  getPlan: vi.fn().mockResolvedValue(null),
  getWorkflows: vi.fn().mockResolvedValue([]),
  getCommands: vi.fn().mockResolvedValue([]),
  getCodexModels: vi.fn().mockResolvedValue({ models: [], current: { model: 'gpt-6-astra', effort: 'high', mode: 'default' } }),
  sendInput: vi.fn().mockResolvedValue({ ok: true, queued: true }),
  steerSession: vi.fn().mockResolvedValue({ ok: true, promoted: true }),
  broadcast: vi.fn().mockResolvedValue({}),
  implementCodexPlan: vi.fn().mockResolvedValue(undefined),
  answerQuestions: vi.fn().mockResolvedValue({ ok: true }),
  openEventStream: vi.fn(() => ({
    onmessage: null, onerror: null, close: vi.fn(), readyState: 1,
    addEventListener: (type: string, handler: (event: MessageEvent) => void) => sse.handlers.set(type, handler),
  })),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    sessionsForServer() { return this.rows; },
    get rows() {
      return [{
        name: 'codex-flow', provider: harness.provider, tracked: true, state: 'working', cwd: '/teste',
        serverId: 'codex-flow', serverLabel: 'Teste', serverColor: '#fff', pair_peers: sessionsStoreCtl.pairPeers,
      }];
    },
    get byServer() { return []; },
  },
}));

let components: ReturnType<typeof mount>[] = [];
async function flush() { await tick(); await new Promise(resolve => setTimeout(resolve, 0)); await tick(); }
async function emit(type: string, data: unknown) {
  expect(sse.handlers.has(type)).toBe(true);
  sse.handlers.get(type)!({ data: JSON.stringify(data) } as MessageEvent);
  await flush();
}
function bolhas(text: string) {
  return [...document.querySelectorAll('.bubble-text')].filter(el => el.textContent === text);
}
function orientar() { return document.querySelector<HTMLButtonElement>('button.fila-chip'); }
async function montar(desktop = true) {
  const target = document.createElement('div'); document.body.appendChild(target);
  components.push(mount(Chat, { target, props: {
    sessionName: 'codex-flow', desktop, showContextPanel: false,
    onBack: vi.fn(), onNavigateToChat: vi.fn(),
  } }));
  await flush(); await flush();
  await emit('state', { session: 'codex-flow', state: 'working', codex_mode: 'default' });
}
async function montarClaude() {
  const target = document.createElement('div'); document.body.appendChild(target);
  components.push(mount(Chat, { target, props: {
    sessionName: 'codex-flow', desktop: true, showContextPanel: false,
    onBack: vi.fn(), onNavigateToChat: vi.fn(),
  } }));
  await flush(); await flush();
}
async function enfileirar(text: string) {
  const textarea = document.querySelector<HTMLTextAreaElement>('textarea')!;
  textarea.value = text;
  textarea.dispatchEvent(new Event('input', { bubbles: true }));
  await flush();
  document.querySelector<HTMLButtonElement>('button.send-btn')!.click();
  await flush();
  expect(api.sendInput).toHaveBeenLastCalledWith('codex-flow', text);
  expect(textarea.value).toBe('');
  expect(bolhas(text)).toHaveLength(1);
  await emit('message', { id: 'queued-fila', kind: 'user_msg', text, ts: '2026-09-07T12:00:00Z' });
  expect(bolhas(text)).toHaveLength(1);
  expect(orientar()).not.toBeNull();
}

beforeEach(() => {
  vi.clearAllMocks(); sse.handlers.clear();
  harness.provider = 'codex';
  sessionsStoreCtl.pairPeers = null;
  const storage = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
    removeItem: (key: string) => storage.delete(key), clear: () => storage.clear(),
  });
  overwriteGetLocale(() => 'pt');
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}', { status: 200 }));
});
afterEach(async () => {
  for (const component of components) await unmount(component);
  components = []; document.body.innerHTML = ''; vi.restoreAllMocks(); vi.unstubAllGlobals();
});

it('digita, enfileira, orienta e reconcilia a mensagem definitiva sem duplicação', async () => {
  await montar();
  await enfileirar('Preserve a configuração atual');
  orientar()!.click(); await flush();
  expect(api.steerSession).toHaveBeenLastCalledWith('codex-flow');
  await emit('message', { id: 'mensagem-real', kind: 'user_msg', text: 'Preserve a configuração atual', ts: '2026-09-07T12:00:01Z' });
  expect(bolhas('Preserve a configuração atual')).toHaveLength(1);
  expect(orientar()).toBeNull();
});

it('uma falha ao orientar preserva a bolha da fila e permite tentar novamente', async () => {
  await montar(); await enfileirar('Mantenha esta orientação');
  vi.mocked(api.steerSession).mockRejectedValueOnce(new Error('O turno terminou'));
  orientar()!.click(); await flush();
  expect(bolhas('Mantenha esta orientação')).toHaveLength(1);
  expect(orientar()).not.toBeNull();
  expect(document.body.textContent).toContain('O turno terminou');
});

it('confirmação retira o eco antigo sem depender do replay do transcript', async () => {
  await montar();
  await emit('message', { id: 'queued-antiga', kind: 'user_msg', text: 'Recado já entregue', queued_delivered: true });
  await emit('message', { id: 'queued-nova', kind: 'user_msg', text: 'Recado aguardando', queued_delivered: false });
  expect(bolhas('Recado já entregue')).toHaveLength(1);
  await emit('queue_confirmed', { id: 'queued-antiga', kind: 'user_msg', text: 'Recado já entregue', queued_confirmed: true });
  expect(bolhas('Recado já entregue')).toHaveLength(0);
  expect(bolhas('Recado aguardando')).toHaveLength(1);
  expect(orientar()).not.toBeNull();
  await emit('queue_confirmed', { id: 'queued-antiga', kind: 'user_msg', queued_confirmed: true });
  expect(bolhas('Recado aguardando')).toHaveLength(1);
});

it.each([true, false])('pergunta assíncrona aparece sem abrir terminal, desktop=%s', async desktop => {
  await montar(desktop);
  await emit('ask_question', { provider: 'codex', request_id: 'async:thread:call:0', is_async: true, questions: [{
    id: 'destino', header: 'Destino', question: 'Onde aplicar a alteração?', multiSelect: false,
    isOther: true, options: [{ label: 'Nesta máquina', description: 'Preservar o servidor' }],
  }] });
  expect(document.body.textContent).toContain('Onde aplicar a alteração?');
  expect([...document.querySelectorAll('button')].some(el => el.textContent?.includes('Nesta máquina'))).toBe(true);
  await emit('ask_question', null);
  expect(document.body.textContent).not.toContain('Onde aplicar a alteração?');
  expect(api.answerQuestions).not.toHaveBeenCalled();
});

it('envia respostas de várias perguntas pelo request_id e conserva o formulário quando a API falha', async () => {
  await montar();
  await emit('ask_question', { provider: 'codex', request_id: 0, questions: [
    { id: 'destino', header: 'Destino', question: 'Onde aplicar?', multiSelect: false,
      options: [{ label: 'Localmente', description: '' }] },
    { id: 'nome', header: 'Nome', question: 'Qual nome?', multiSelect: false, options: [] },
  ] });
  document.querySelector<HTMLButtonElement>('.option-btn')!.click(); await flush();
  const input = document.querySelector<HTMLInputElement>('.ask-card input')!;
  expect(input).not.toBeNull();
  input.value = 'João'; input.dispatchEvent(new Event('input', { bubbles: true })); await flush();
  document.querySelector<HTMLButtonElement>('.text-actions .primary-btn')!.click(); await flush();
  vi.mocked(api.answerQuestions).mockRejectedValueOnce(new Error('Conexão interrompida'));
  document.querySelector<HTMLButtonElement>('.ask-card .primary-btn')!.click(); await flush();
  expect(api.answerQuestions).toHaveBeenLastCalledWith('codex-flow', [
    expect.objectContaining({ question_id: 'destino', kind: 'option', indices: [0] }),
    expect.objectContaining({ question_id: 'nome', kind: 'text', value: 'João' }),
  ], 0);
  expect(document.body.textContent).toContain('Conexão interrompida');
  expect(document.body.textContent).toContain('João');
  document.querySelector<HTMLButtonElement>('.ask-card .primary-btn')!.click(); await flush();
  expect(api.answerQuestions).toHaveBeenCalledTimes(2);
  expect(document.querySelector('.ask-card')).toBeNull();
});

it('aprovar o plano envia uma única vez à sessão atual mesmo com mandar pros dois ligado', async () => {
  sessionsStoreCtl.pairPeers = ['par'];
  let resolvePlan!: () => void;
  const plan = {
    promise: new Promise<void>(resolve => { resolvePlan = resolve; }),
    resolve: () => resolvePlan(),
  };
  vi.mocked(api.implementCodexPlan).mockReturnValueOnce(plan.promise);
  await montar();
  const both = document.querySelector<HTMLButtonElement>('button.both-chip')!;
  expect(both).not.toBeNull();
  both.click(); await flush();
  expect(both.getAttribute('aria-pressed')).toBe('true');
  await emit('message', {
    id: 'plano-completo', kind: 'assistant_msg', ts: '2026-09-07T12:00:00Z',
    text: '<proposed_plan>\n## Plano\nPreservar a configuração e validar a alteração.\n</proposed_plan>',
  });
  await emit('state', { session: 'codex-flow', state: 'idle', codex_mode: 'plan' });
  const resposta = document.querySelector('.assistant-msg:not(.preview)');
  expect(resposta?.nextElementSibling?.classList.contains('plan-preview')).toBe(true);
  const implement = document.querySelector<HTMLButtonElement>('.plan-actions .primary-btn')!;
  expect(implement.textContent).toContain(m.chat_plan_implementar());
  expect(implement.disabled).toBe(false);
  // Dois cliques no mesmo quadro precisam compartilhar o envio ainda em curso.
  implement.click(); implement.click(); await flush();
  expect(api.implementCodexPlan).toHaveBeenCalledExactlyOnceWith('codex-flow');
  expect(api.sendInput).not.toHaveBeenCalled();
  plan.resolve();
  await flush();
  expect(api.sendInput).not.toHaveBeenCalled();
  expect(api.broadcast).not.toHaveBeenCalled();
  expect(both.getAttribute('aria-pressed')).toBe('true');
});

it('histórico que chega depois do SSE mantém mensagens antigas antes das atuais', async () => {
  let resolverHistorico!: (value: Awaited<ReturnType<typeof api.getHistoryDesde>>) => void;
  vi.mocked(api.getHistoryDesde).mockReturnValueOnce(new Promise((resolve) => {
    resolverHistorico = resolve;
  }));
  await montar();
  await emit('message', {
    id: 'atual', kind: 'assistant_msg', ts: '2026-09-11T12:01:00Z', text: 'mensagem atual',
  });
  resolverHistorico({ eventos: [
    { id: 'antiga', kind: 'assistant_msg', ts: 1, text: 'mensagem antiga' },
    { id: 'atual', kind: 'assistant_msg', ts: 2, text: 'mensagem atual' },
  ], etag: 'v1' });
  await flush();
  expect([...document.querySelectorAll('.assistant-msg:not(.preview)')]
    .map((el) => el.textContent?.includes('mensagem antiga') ? 'antiga' : 'atual'))
    .toEqual(['antiga', 'atual']);
});

it('jsonl tardio descarta o cache antigo sem apagar o SSE que chegou durante o REST', async () => {
  let resolverSessoes!: (value: Awaited<ReturnType<typeof api.getSessions>>) => void;
  let resolverHistorico!: (value: Awaited<ReturnType<typeof api.getHistoryDesde>>) => void;
  vi.mocked(api.getSessions).mockReturnValueOnce(new Promise((resolve) => {
    resolverSessoes = resolve;
  }));
  vi.mocked(api.getHistoryDesde).mockReturnValueOnce(new Promise((resolve) => {
    resolverHistorico = resolve;
  }));
  guardarCaudaChat('codex-flow', '/tmp/jsonl-tardio.jsonl', {
    eventos: [{ id: 'cache-antigo', kind: 'assistant_msg', ts: 1, text: 'cache antigo' }],
    etag: 'cache-v1',
  });

  const target = document.createElement('div');
  document.body.appendChild(target);
  components.push(mount(Chat, { target, props: {
    sessionName: 'codex-flow', desktop: false, showContextPanel: false,
    onBack: vi.fn(), onNavigateToChat: vi.fn(),
  } }));
  await flush();
  resolverSessoes([{
    name: 'codex-flow', provider: 'codex', tracked: true, state: 'working', cwd: '/teste',
    jsonl: '/tmp/jsonl-tardio.jsonl',
  }]);
  await vi.waitFor(() => expect(document.body.textContent).toContain('cache antigo'));
  await emit('message', {
    id: 'sse-atual', kind: 'assistant_msg', ts: 2, text: 'mensagem SSE atual',
  });
  resolverHistorico({
    eventos: [{ id: 'historico-novo', kind: 'assistant_msg', ts: 3, text: 'histórico novo' }],
    etag: 'history-v2',
  });
  await flush();

  const textos = [...document.querySelectorAll('.assistant-msg:not(.preview)')]
    .map((el) => el.textContent?.trim());
  expect(textos.some((texto) => texto?.includes('cache antigo'))).toBe(false);
  expect(textos.map((texto) => texto?.includes('histórico novo') ? 'novo' : 'atual'))
    .toEqual(['novo', 'atual']);
});

it('voltar do segundo plano preserva o SSE posterior ao histórico recebido', async () => {
  const a = { id: 'a', kind: 'assistant_msg', text: 'mensagem A' } as const;
  const b = { id: 'b', kind: 'assistant_msg', text: 'mensagem B' } as const;
  const c = { id: 'c', kind: 'assistant_msg', text: 'mensagem C' } as const;
  await montar();
  await emit('message', a);
  let resolveTail!: (events: api.ChatEvent[]) => void;
  vi.mocked(api.getHistory).mockReturnValueOnce(new Promise(resolve => { resolveTail = resolve; }));
  vi.mocked(api.getHistory).mockResolvedValueOnce([a, b, c]);
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  document.dispatchEvent(new Event('visibilitychange'));
  await flush();
  await emit('message', c);
  resolveTail([b]);
  await flush(); await flush();
  expect([...document.querySelectorAll('.assistant-msg:not(.preview)')]
    .map(el => el.textContent?.match(/mensagem [ABC]/)?.[0])).toEqual(['mensagem A', 'mensagem B', 'mensagem C']);
});

it.each([true, false])('REST atrasado não ressuscita eco confirmado, desktop=%s', async desktop => {
  let resolveHistory!: (value: Awaited<ReturnType<typeof api.getHistoryDesde>>) => void;
  vi.mocked(api.getHistoryDesde).mockReturnValueOnce(new Promise(resolve => { resolveHistory = resolve; }));
  await montar(desktop);
  const queued = { id: 'queued-antiga', kind: 'user_msg', text: 'pode continuar agr' } as const;
  await emit('message', queued);
  await emit('queue_confirmed', { ...queued, queued_confirmed: true });
  resolveHistory({ eventos: [queued, { id: 'real', kind: 'user_msg', text: queued.text }], etag: null });
  await flush();
  expect(bolhas(queued.text)).toHaveLength(1);
  expect(document.querySelector('.queued-row')).toBeNull();
});

it('falha do histórico não esconde mensagem que já chegou pelo SSE', async () => {
  let rejeitarHistorico!: (error: Error) => void;
  vi.mocked(api.getHistoryDesde).mockReturnValueOnce(new Promise((_resolve, reject) => {
    rejeitarHistorico = reject;
  }));
  await montar();
  await emit('message', {
    id: 'viva', kind: 'assistant_msg', ts: 1, text: 'mensagem viva',
  });
  rejeitarHistorico(new Error('histórico indisponível'));
  await flush();
  expect(document.body.textContent).toContain('mensagem viva');
  expect(document.querySelector('.chat-error')).toBeNull();
});

it('Claude redescobre no fim de cada turno e ancora o cartão no plano novo', async () => {
  harness.provider = 'claude';
  vi.mocked(api.getHistory).mockResolvedValueOnce([]);
  vi.mocked(api.getSessionPlanPreview)
    .mockResolvedValueOnce(null)
    .mockResolvedValueOnce({ name: 'primeiro', path: '/p/primeiro.md', anchor_id: 'a-plan-1' } as unknown as Awaited<ReturnType<typeof api.getSessionPlanPreview>>)
    .mockResolvedValueOnce({ name: 'segundo', path: '/p/segundo.md', anchor_id: 'a-plan-2' } as unknown as Awaited<ReturnType<typeof api.getSessionPlanPreview>>);
  await montarClaude();
  expect(api.getSessionPlanPreview).toHaveBeenCalledTimes(1);

  await emit('state', { session: 'codex-flow', state: 'working' });
  expect(api.getSessionPlanPreview).toHaveBeenCalledTimes(1);
  await emit('state', { session: 'codex-flow', state: 'awaiting_input' });
  expect(api.getSessionPlanPreview).toHaveBeenCalledTimes(2);
  await emit('state', { session: 'codex-flow', state: 'awaiting_input' });
  expect(api.getSessionPlanPreview).toHaveBeenCalledTimes(2);
  await emit('message', { id: 'a-old', kind: 'assistant_msg', text: 'Resposta anterior.', ts: 1 });
  await emit('message', { id: 'a-plan-1', kind: 'assistant_msg', text: 'Plano primeiro.', ts: 2 });
  expect(document.querySelector('.plan-name')?.textContent).toBe(m.chat_plan_proposto());
  const anterior = [...document.querySelectorAll<HTMLElement>('.assistant-msg')]
    .find((el) => el.textContent?.includes('Resposta anterior.'));
  expect(anterior?.nextElementSibling?.classList.contains('plan-preview')).toBe(false);

  await emit('state', { session: 'codex-flow', state: 'working' });
  await emit('state', { session: 'codex-flow', state: 'idle' });
  expect(api.getSessionPlanPreview).toHaveBeenCalledTimes(3);
  await emit('message', { id: 'a-plan-2', kind: 'assistant_msg', text: 'Plano segundo.', ts: 3 });
  const respostas = [...document.querySelectorAll<HTMLElement>('.assistant-msg')];
  const segunda = respostas.find((el) => el.textContent?.includes('Plano segundo.'));
  expect(segunda?.nextElementSibling?.classList.contains('plan-preview')).toBe(true);
  expect(document.body.textContent).not.toContain('Resposta anterior.Plano');
});

it('Claude não mostra cartão sem âncora ou fora da janela da conversa', async () => {
  harness.provider = 'claude';
  const history = Array.from({ length: 121 }, (_, i) => ({
    id: i === 0 ? 'a-plan-old' : `a-${i}`,
    kind: 'assistant_msg' as const,
    text: i === 0 ? 'Plano antigo.' : `Resposta ${i}.`,
    ts: i,
  }));
  vi.mocked(api.getHistory).mockResolvedValueOnce(history);
  vi.mocked(api.getSessionPlanPreview).mockResolvedValueOnce({
    name: 'plano', path: '/p/plano.md', anchor_id: 'a-plan-old',
  } as unknown as Awaited<ReturnType<typeof api.getSessionPlanPreview>>);

  await montarClaude();

  expect(document.querySelectorAll('.plan-preview')).toHaveLength(0);
  expect(api.getSessionPlanPreview).toHaveBeenCalledWith('codex-flow', false);

  vi.mocked(api.getSessionPlanPreview).mockResolvedValueOnce({
    name: 'plano', path: '/p/plano.md', anchor_id: null,
  } as unknown as Awaited<ReturnType<typeof api.getSessionPlanPreview>>);
  await emit('state', { session: 'codex-flow', state: 'working' });
  await emit('state', { session: 'codex-flow', state: 'idle' });

  expect(document.querySelectorAll('.plan-preview')).toHaveLength(0);
});

it('Claude não anuncia plano enquanto a descoberta está em andamento', async () => {
  harness.provider = 'claude';
  let resolve!: (value: null) => void;
  vi.mocked(api.getSessionPlanPreview).mockReturnValueOnce(new Promise(r => { resolve = r; }));
  await montarClaude();
  expect(document.querySelector('.plan-preview')).toBeNull();
  resolve(null); await flush();
  expect(document.querySelector('.plan-preview')).toBeNull();
});

it('Claude retira a proposta quando a resposta humana chega, sem esperar outro turno terminar', async () => {
  harness.provider = 'claude';
  vi.mocked(api.getHistory).mockResolvedValueOnce([
    { id: 'proposta', kind: 'assistant_msg', text: 'Plano para conferir.' },
  ]);
  vi.mocked(api.getSessionPlanPreview).mockResolvedValueOnce({
    name: 'plano', path: '/p/plano.md', anchor_id: 'proposta',
  } as unknown as Awaited<ReturnType<typeof api.getSessionPlanPreview>>);
  await montarClaude();
  await emit('message', { id: 'proposta', kind: 'assistant_msg', text: 'Plano para conferir.' });
  expect(document.querySelector('.plan-preview')).not.toBeNull();
  await emit('message', { id: 'aceite', kind: 'user_msg', text: 'Pode implementar.' });
  expect(document.querySelector('.plan-preview')).toBeNull();
});
