// @vitest-environment happy-dom
// O trajeto usa Chat, Composer, MessageList e o stepper reais; só rede e painéis auxiliares são simulados.
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createRawSnippet, mount, tick, unmount } from 'svelte';
import Chat from './Chat.svelte';
import * as api from '@hangar/core';
import { guardarCaudaChat } from '../lib/queries';
import * as m from '../paraglide/messages';
import { overwriteGetLocale } from '../paraglide/runtime';
vi.mock('../lib/diag', () => ({ registrar: vi.fn(), novoReq: () => 'stream-teste' }));

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
  listAllServers: () => [{ id: 'codex-flow', label: 'Teste', baseUrl: 'http://teste', token: 'teste' }],
  findServer: (id: string) => [{ id: 'codex-flow', label: 'Teste', baseUrl: 'http://teste', token: 'teste' }].find((s) => s.id === id),
  listOwnServers: () => [{ id: 'codex-flow', label: 'Teste', baseUrl: 'http://teste', token: 'teste' }],
  getActiveId: () => 'codex-flow',
  getBaseUrl: () => 'http://teste',
  isActiveInvite: () => false,
}));

const sse = vi.hoisted(() => ({ handlers: new Map<string, (event: MessageEvent) => void>() }));
const harness = vi.hoisted(() => ({ provider: 'codex' as 'claude' | 'codex' | 'kimi' }));
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
  getSubagents: vi.fn().mockResolvedValue([]),
  getModelOptions: vi.fn().mockResolvedValue({ kind: 'anthropic', models: [] }),
  getCommands: vi.fn().mockResolvedValue([]),
  getCodexModels: vi.fn().mockResolvedValue({ models: [], current: { model: 'gpt-6-astra', effort: 'high', mode: 'default' } }),
  sendInput: vi.fn().mockResolvedValue({ ok: true, queued: true }),
  steerSession: vi.fn().mockResolvedValue({ ok: true, promoted: true }),
  broadcast: vi.fn().mockResolvedValue({}),
  implementCodexPlan: vi.fn().mockResolvedValue(undefined),
  answerQuestions: vi.fn().mockResolvedValue({ ok: true }),
  selectOption: vi.fn().mockResolvedValue(undefined),
  submitSelected: vi.fn().mockResolvedValue(undefined),
  skipQuestion: vi.fn().mockResolvedValue({ ok: true }),
  editQueuedMessage: vi.fn().mockResolvedValue(undefined),
  cancelQueuedMessage: vi.fn().mockResolvedValue(undefined),
  steerQueuedMessage: vi.fn().mockResolvedValue({ ok: true, steered: true }),
  reorderQueuedMessage: vi.fn().mockResolvedValue(undefined),
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
async function montarClaude(desktop = true) {
  const target = document.createElement('div'); document.body.appendChild(target);
  components.push(mount(Chat, { target, props: {
    sessionName: 'codex-flow', desktop, showContextPanel: false,
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
  expect(api.sendInput).toHaveBeenLastCalledWith('codex-flow', text, expect.objectContaining({ id: 'codex-flow' }));
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
  expect(api.steerSession).toHaveBeenLastCalledWith('codex-flow', undefined, expect.objectContaining({ id: 'codex-flow' }));
  await emit('message', { id: 'mensagem-real', kind: 'user_msg', text: 'Preserve a configuração atual', ts: '2026-09-07T12:00:01Z' });
  expect(bolhas('Preserve a configuração atual')).toHaveLength(1);
  expect(orientar()).toBeNull();
});

it('não sobrepõe consultas de subagentes mesmo se o estado mudar durante a resposta', async () => {
  harness.provider = 'claude';
  let responder!: (lista: Awaited<ReturnType<typeof api.getSubagents>>) => void;
  vi.mocked(api.getSubagents).mockImplementationOnce(() => new Promise(resolve => { responder = resolve; }));
  const intervalos = vi.spyOn(globalThis, 'setInterval');
  await montar();
  const poll = intervalos.mock.calls.find(([, ms]) => ms === 5000)?.[0];
  expect(typeof poll).toBe('function');
  (poll as () => void)();
  (poll as () => void)();
  await emit('state', { session: 'codex-flow', state: 'idle' });
  expect(api.getSubagents).toHaveBeenCalledTimes(1);
  responder([]);
  await flush();
});

it.each(['claude', 'codex'] as const)('preserva a descoberta de subagentes ao chegar histórico antigo de %s e a renova no reset', async provider => {
  harness.provider = provider;
  let finishHistory!: (value: Awaited<ReturnType<typeof api.getHistoryDesde>>) => void;
  vi.mocked(api.getHistoryDesde).mockReturnValueOnce(new Promise(resolve => { finishHistory = resolve; }));
  await montar();
  await emit('state', { session: 'codex-flow', state: 'idle' });
  const reads = vi.mocked(api.getSubagents).mock.calls.length;
  expect(reads).toBeGreaterThan(0);
  const tail = { id: 'tail', kind: 'assistant_msg', ts: 2, text: 'mensagem atual' } as const;
  await emit('message', tail);
  expect(api.getSubagents).toHaveBeenCalledTimes(reads);
  finishHistory({ eventos: [
    { id: 'older', kind: 'assistant_msg', ts: 1, text: 'mensagem antiga' }, tail,
  ], etag: 'history-v1' });
  await flush();
  expect(document.body.textContent).toContain('mensagem antiga');
  expect(api.getSubagents).toHaveBeenCalledTimes(reads);
  await emit('reset', {});
  expect(api.getSubagents).toHaveBeenCalledTimes(reads + 1);
});

it('abrir uma conversa Claude não consulta o seletor de modelos', async () => {
  harness.provider = 'claude';
  await montarClaude();
  expect(api.getModelOptions).not.toHaveBeenCalled();
});

it('não sobrepõe consultas de workflows enquanto a resposta está pendente', async () => {
  const intervalos = vi.spyOn(globalThis, 'setInterval');
  await montar();
  let responder!: (lista: Awaited<ReturnType<typeof api.getWorkflows>>) => void;
  vi.mocked(api.getWorkflows)
    .mockResolvedValueOnce([{ running: true }] as Awaited<ReturnType<typeof api.getWorkflows>>)
    .mockImplementationOnce(() => new Promise(resolve => { responder = resolve; }));
  await emit('message', { id: 'workflow', kind: 'tool_use', tool_use_id: 'workflow', tool_name: 'Workflow', tool_input: {} });
  const poll = intervalos.mock.calls.find(([, ms]) => ms === 4000)?.[0];
  expect(typeof poll).toBe('function');
  (poll as () => void)();
  (poll as () => void)();
  expect(api.getWorkflows).toHaveBeenCalledTimes(2);
  responder([]);
  await flush();
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

it.each([true, false])('aviso de demora aparece sem pedir resposta, desktop=%s', async desktop => {
  await montar(desktop);
  await emit('state', { session: 'codex-flow', state: 'working', codex_buffering: true });
  expect(document.querySelector('.codex-notice')?.textContent).toBe(m.chat_codex_buffering());
  expect(document.querySelector<HTMLTextAreaElement>('textarea')?.disabled).toBe(false);
  expect(api.answerQuestions).not.toHaveBeenCalled();
  await emit('state', { session: 'codex-flow', state: 'working', codex_buffering: false });
  expect(document.querySelector('.codex-notice')).toBeNull();
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
  ], 0, undefined, expect.objectContaining({ id: 'codex-flow' }));
  expect(document.body.textContent).toContain('Conexão interrompida');
  expect(document.body.textContent).toContain('João');
  document.querySelector<HTMLButtonElement>('.ask-card .primary-btn')!.click(); await flush();
  expect(api.answerQuestions).toHaveBeenCalledTimes(2);
  expect(document.querySelector('.ask-card')).toBeNull();
});

it.each([true, false])('ACK de A não fecha B com mesmo request_id e perguntas, desktop=%s', async desktop => {
  await montar(desktop);
  const first: api.AskQuestionPayload = { provider: 'codex', request_id: 0, interaction_id: 'form-a', questions: [{
    id: 'choice', header: 'Escolha', question: 'Qual?', multiSelect: false, options: [{ label: 'Primeira', description: '' }],
  }] };
  let finish!: (value: { ok: boolean }) => void;
  vi.mocked(api.answerQuestions).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
  await emit('ask_question', first);
  document.querySelector<HTMLButtonElement>('.option-btn')!.click(); await flush();
  document.querySelector<HTMLButtonElement>('.review-list + .primary-btn')!.click(); await flush();
  expect(api.answerQuestions).toHaveBeenLastCalledWith('codex-flow', expect.any(Array), 0, 'form-a', expect.objectContaining({ id: 'codex-flow' }));
  const second = { ...first, interaction_id: 'form-b' };
  await emit('ask_question', second);
  expect(document.querySelector('.review-list')).toBeNull();
  finish({ ok: true }); await flush();
  expect(document.body.textContent).toContain('Qual?');
  document.querySelector<HTMLButtonElement>('.option-btn')!.click(); await flush();
  document.querySelector<HTMLButtonElement>('.review-list + .primary-btn')!.click(); await flush();
  expect(api.answerQuestions).toHaveBeenLastCalledWith('codex-flow', expect.any(Array), 0, 'form-b', expect.objectContaining({ id: 'codex-flow' }));
});

it.each(['accepted', 'rejected', 'unknown'])('descarte PWA envia A exibida e preserva B no retorno %s', async outcome => {
  await montar(false);
  const first: api.AskQuestionPayload = { provider: 'codex', is_async: true,
    request_id: 'same-native-id', interaction_id: 'form-a', questions: [{
      id: 'choice', header: 'Escolha', question: 'Qual?', multiSelect: false,
      options: [{ label: 'Primeira', description: '' }],
    }] };
  let finish!: (value: { ok: boolean }) => void;
  let fail!: (error: Error) => void;
  vi.mocked(api.skipQuestion).mockReturnValueOnce(new Promise((resolve, reject) => { finish = resolve; fail = reject; }));
  await emit('ask_question', first);
  document.querySelector<HTMLButtonElement>('.option-btn')!.click(); await flush();
  document.querySelector<HTMLButtonElement>('.review-list ~ .ghost-btn')!.click(); await flush();
  expect(api.skipQuestion).toHaveBeenLastCalledWith('codex-flow', 'same-native-id', 'form-a', expect.objectContaining({ id: 'codex-flow' }));
  await emit('ask_question', { ...first, interaction_id: 'form-b' });
  if (outcome === 'rejected') fail(new Error('erro-antigo'));
  else finish({ ok: outcome === 'accepted' });
  await flush();
  expect(document.body.textContent).toContain('Qual?');
  expect(document.body.textContent).not.toContain('erro-antigo');
  document.querySelector<HTMLButtonElement>('.option-btn')!.click(); await flush();
  document.querySelector<HTMLButtonElement>('.review-list ~ .ghost-btn')!.click(); await flush();
  expect(api.skipQuestion).toHaveBeenLastCalledWith('codex-flow', 'same-native-id', 'form-b', expect.objectContaining({ id: 'codex-flow' }));
});

it('aprovação simultânea não empresta identidade ao formulário Claude', async () => {
  harness.provider = 'claude';
  await montarClaude();
  const state = { session: 'codex-flow', state: 'awaiting_input', question: 'Permitir?', options: ['Sim'], interaction_id: 'approval-a' };
  await emit('state', state);
  await emit('ask_question', { interaction_id: 'form-a', questions: [{
    header: 'Escolha', question: 'Qual?', multiSelect: false, options: [{ label: 'Primeira', description: '' }],
  }] });
  document.querySelector<HTMLButtonElement>('.ask-card .option-btn')!.click(); await flush();
  await emit('state', { ...state, state: 'working' });
  expect(document.querySelector('.ask-card .review-list')).not.toBeNull();
  await emit('state', state);
  document.querySelector<HTMLButtonElement>('.ask-card .primary-btn')!.click(); await flush();
  expect(api.answerQuestions).toHaveBeenLastCalledWith('codex-flow', expect.any(Array), undefined, 'form-a', expect.objectContaining({ id: 'codex-flow' }));
  document.querySelector<HTMLButtonElement>('.options-wrap .option-btn')!.click(); await flush();
  expect(api.selectOption).toHaveBeenLastCalledWith('codex-flow', 1, 'approval-a', expect.objectContaining({ id: 'codex-flow' }));
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
  expect(api.implementCodexPlan).toHaveBeenCalledExactlyOnceWith('codex-flow', expect.objectContaining({ id: 'codex-flow' }));
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
  expect(api.getSessionPlanPreview).toHaveBeenCalledWith('codex-flow', false, expect.objectContaining({ id: 'codex-flow' }));

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

it('Claude sem terminal mostra o plano enquanto o ExitPlanMode espera aprovação', async () => {
  harness.provider = 'claude';
  await montarClaude();
  await emit('state', { session: 'codex-flow', state: 'working', headless: true, claude_permission_mode: 'plan' });
  await emit('message', {
    id: 'a-exit:1', kind: 'tool_use', tool_name: 'ExitPlanMode', tool_use_id: 'toolu_plano',
    tool_input: { plan: '# Plano do teste\n\n- passo', planFilePath: 'C:\conta\plans\p.md' }, ts: 1,
  });
  expect(document.querySelector('.plan-preview')).toBeNull();

  await emit('state', {
    session: 'codex-flow', state: 'awaiting_input', headless: true, claude_permission_mode: 'plan',
    question: 'Aprovar o plano?', options: ['Aprovar plano', 'Continuar planejando'],
    claude_plan_pending: { plan: '# Plano do teste\n\n- passo', path: 'C:\conta\plans\p.md', tool_use_id: 'toolu_plano' },
  });
  expect(document.querySelector('.plan-name')?.textContent).toBe('Plano do teste');
  // Aprovar é o seletor da sessão; o card não oferece um segundo caminho.
  expect(document.querySelector('.plan-preview .plan-actions')).toBeNull();
  expect([...document.querySelectorAll('.option-btn .opt-text')].map((el) => el.textContent))
    .toEqual(expect.arrayContaining(['Aprovar plano', 'Continuar planejando']));

  document.querySelector<HTMLButtonElement>('.plan-open')!.click(); await flush();
  expect(document.body.textContent).toContain('C:\conta\plans\p.md');
  expect(document.querySelector('.prose')?.textContent).toContain('passo');
  expect(api.getSessionPlanPreview).not.toHaveBeenCalledWith('codex-flow');

  await emit('state', { session: 'codex-flow', state: 'working', headless: true, claude_permission_mode: 'default' });
  expect(document.querySelector('.plan-preview')).toBeNull();
});

it('Claude com terminal mostra o plano pendente e pede alteração com texto', async () => {
  harness.provider = 'claude';
  await montarClaude();
  await emit('message', {
    id: 'a-exit:2', kind: 'tool_use', tool_name: 'ExitPlanMode', tool_use_id: 'toolu_term',
    tool_input: { plan: '# Plano do terminal\n\n- passo' }, ts: 1,
  });
  const opcoes = ['Yes, and use auto mode', 'Yes, manually approve edits', 'Tell Claude what to change'];
  await emit('state', {
    session: 'codex-flow', state: 'awaiting_input', headless: false, claude_permission_mode: 'plan',
    question: 'Claude has written up a plan and is ready to execute. Would you like to proceed?',
    options: opcoes, interaction_id: 'plan-term',
    claude_plan_pending: { plan: '# Plano do terminal\n\n- passo', path: '/p/plano.md', tool_use_id: 'toolu_term' },
  });
  expect(document.querySelector('.plan-name')?.textContent).toBe('Plano do terminal');
  expect(document.querySelector('.plan-preview .plan-actions')).toBeNull();
  const botoes = [...document.querySelectorAll<HTMLButtonElement>('.option-btn:not(.option-btn--cancel)')];
  expect(botoes.map((b) => b.querySelector('.opt-text')?.textContent))
    .toEqual(['Aprovar com modo automático', 'Aprovar com revisão manual', 'Pedir alterações']);
  expect(botoes.map((b) => b.title)).toEqual(opcoes);

  botoes[1].click(); await flush();
  expect(api.selectOption).toHaveBeenLastCalledWith('codex-flow', 2, 'plan-term', expect.anything());

  // "O que mudar" abre o campo no cartão em vez de responder; vazio não envia.
  botoes[2].click(); await flush();
  expect(api.selectOption).toHaveBeenCalledTimes(1);
  const campo = document.querySelector<HTMLTextAreaElement>('.plan-feedback textarea')!;
  const enviar = document.querySelector<HTMLButtonElement>('.plan-feedback button[type="submit"]')!;
  expect(enviar.disabled).toBe(true);
  campo.value = '  troque o passo 2  '; campo.dispatchEvent(new Event('input', { bubbles: true })); await flush();
  expect(enviar.disabled).toBe(false);
  enviar.click(); await flush();
  expect(api.selectOption).toHaveBeenLastCalledWith('codex-flow', 3, 'plan-term', expect.anything(), 'troque o passo 2');
});

it('celular: com terminal, o plano pendente do estado abre acima das opções sem buscar no servidor', async () => {
  harness.provider = 'claude';
  vi.mocked(api.getHistory).mockResolvedValueOnce([]);
  vi.mocked(api.getSessionPlanPreview).mockResolvedValue(null);
  await montarClaude(false);
  await emit('message', { id: 'a-exit', kind: 'tool_use', tool_name: 'ExitPlanMode', tool_use_id: 'toolu_t', tool_input: {}, ts: 1 });
  vi.mocked(api.getSessionPlanPreview).mockClear();
  const pendente = { plan: '# Plano do terminal\n\n1. Ler', path: '/p/plano.md', tool_use_id: 'toolu_t' };
  await emit('state', { session: 'codex-flow', state: 'awaiting_input', headless: false, question: 'Would you like to proceed?',
    options: ['Yes', 'No'], claude_plan_pending: pendente });
  await flush();
  expect(document.querySelector('.plan-inline h3')?.textContent).toBe('Plano do terminal');
  expect(document.querySelector('.plan-inline .prose li')?.textContent).toBe('Ler');
  expect(api.getSessionPlanPreview).not.toHaveBeenCalledWith('codex-flow', true, expect.anything());

  await emit('state', { session: 'codex-flow', state: 'working', headless: false });
  await flush();
  expect(document.querySelector('.plan-inline')).toBeNull();
});

it('celular: outra pergunta depois de um plano gravado não vira aprovação do plano', async () => {
  harness.provider = 'claude';
  vi.mocked(api.getHistory).mockResolvedValueOnce([]);
  vi.mocked(api.getSessionPlanPreview).mockResolvedValue({ name: 'plano', path: '/p/plano.md', anchor_id: 'a-plan' } as unknown as Awaited<ReturnType<typeof api.getSessionPlanPreview>>);
  await montarClaude(false);
  await emit('message', { id: 'a-plan', kind: 'assistant_msg', text: 'Rascunho do plano gravado.', ts: 1 });
  await emit('state', { session: 'codex-flow', state: 'working' });
  await emit('state', { session: 'codex-flow', state: 'awaiting_input', question: 'Rodar o comando?', options: ['Sim', 'Não'] });
  await flush();
  expect(document.querySelector('.plan-inline')).toBeNull();
  expect(document.querySelector('.plan-preview')).not.toBeNull();
});

it('celular: sem terminal, o plano pendente aparece aberto sem buscar no servidor', async () => {
  harness.provider = 'claude';
  await montarClaude(false);
  await emit('state', { session: 'codex-flow', state: 'working', headless: true, claude_permission_mode: 'plan' });
  await emit('message', { id: 'a-exit:1', kind: 'tool_use', tool_name: 'ExitPlanMode', tool_use_id: 'toolu_plano', tool_input: { plan: '# Plano do teste\n\n- passo' }, ts: 1 });
  vi.mocked(api.getSessionPlanPreview).mockClear();
  await emit('state', {
    session: 'codex-flow', state: 'awaiting_input', headless: true, claude_permission_mode: 'plan',
    question: 'Aprovar o plano?', options: ['Aprovar plano', 'Continuar planejando'],
    claude_plan_pending: { plan: '# Plano do teste\n\n- passo', path: 'C:/conta/plans/p.md', tool_use_id: 'toolu_plano' },
  });
  await flush();
  expect(document.querySelector('.plan-inline h3')?.textContent).toBe('Plano do teste');
  expect(document.querySelector('.plan-inline')?.textContent).toContain('C:/conta/plans/p.md');
  expect(api.getSessionPlanPreview).not.toHaveBeenCalledWith('codex-flow', true, expect.anything());
});

it('evento queue: painel lista, a bolha some da conversa e o chip sai', async () => {
  await montar(false);
  await emit('message', { kind: 'user_msg', id: 'queued-a', text: 'na fila', queued_delivered: false, queued_ts: 1 });
  await emit('queue', { entries: [{ id: 'a', text: 'na fila', ts: 1 }], actions: true });
  expect(document.querySelectorAll('.qp-row')).toHaveLength(1);
  expect(document.querySelector('.queued-row')).toBeNull();
  expect(document.querySelector('button.fila-chip')).toBeNull();
  await emit('queue', { entries: [], actions: true });
  await emit('message', { kind: 'user_msg', id: 'queued-a', text: 'na fila', queued_delivered: true, queued_ts: 1 });
  expect(document.querySelector('.qp-row')).toBeNull();
  expect(document.querySelector('.queued-row')).not.toBeNull();
});

it('painel da fila: editar atualiza e devolve o rascunho, Esc sai sem pedido, X tira a bolha', async () => {
  await montar(false);
  const textarea = document.querySelector<HTMLTextAreaElement>('textarea')!;
  const digitar = async (text: string) => {
    textarea.value = text;
    textarea.dispatchEvent(new Event('input', { bubbles: true }));
    await flush();
  };
  await digitar('rascunho meu');
  await emit('message', { kind: 'user_msg', id: 'queued-a', text: 'primeira', queued_delivered: false, queued_ts: 1 });
  await emit('queue', { entries: [{ id: 'a', text: 'primeira', ts: 1 }, { id: 'b', text: 'segunda', ts: 2 }], actions: true });
  const linha = (i: number) => document.querySelectorAll<HTMLElement>('.qp-row')[i];

  linha(0).querySelector<HTMLButtonElement>('.qp-edit')!.click(); await flush();
  expect(textarea.value).toBe('primeira');
  expect(document.querySelector('.send-btn--update')?.textContent).toBe(m.queue_panel_update());
  expect(linha(0).querySelector('.qp-cancel')).not.toBeNull();
  // Atualizar sem mudar o texto só sai da edição.
  document.querySelector<HTMLButtonElement>('.send-btn--update')!.click(); await flush();
  expect(api.editQueuedMessage).not.toHaveBeenCalled();
  expect(textarea.value).toBe('rascunho meu');

  linha(0).querySelector<HTMLButtonElement>('.qp-edit')!.click(); await flush();
  await digitar('primeira editada');
  document.querySelector<HTMLButtonElement>('.send-btn--update')!.click(); await flush();
  expect(api.editQueuedMessage).toHaveBeenLastCalledWith('codex-flow', 'a', 'primeira editada', expect.objectContaining({ id: 'codex-flow' }));
  expect(textarea.value).toBe('rascunho meu');
  expect(document.querySelector('.send-btn--update')).toBeNull();

  linha(1).querySelector<HTMLButtonElement>('.qp-edit')!.click(); await flush();
  expect(textarea.value).toBe('segunda');
  textarea.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await flush();
  expect(textarea.value).toBe('rascunho meu');
  expect(api.editQueuedMessage).toHaveBeenCalledTimes(1);

  linha(0).querySelector<HTMLButtonElement>('.qp-remove')!.click(); await flush();
  expect(api.cancelQueuedMessage).toHaveBeenLastCalledWith('codex-flow', 'a', expect.objectContaining({ id: 'codex-flow' }));
  await emit('queue', { entries: [{ id: 'b', text: 'segunda', ts: 2 }], actions: true });
  expect(document.querySelector('.queued-row')).toBeNull();
  expect(bolhas('primeira')).toHaveLength(0);

  linha(0).querySelector<HTMLButtonElement>('.qp-steer')!.click(); await flush();
  expect(api.steerQueuedMessage).toHaveBeenLastCalledWith('codex-flow', 'b', expect.objectContaining({ id: 'codex-flow' }));
});

it('painel da fila: a entrada em edição que sai da fila devolve o rascunho e avisa', async () => {
  await montar(false);
  await emit('queue', { entries: [{ id: 'a', text: 'primeira', ts: 1 }], actions: true });
  document.querySelector<HTMLButtonElement>('.qp-edit')!.click(); await flush();
  expect(document.querySelector<HTMLTextAreaElement>('textarea')!.value).toBe('primeira');
  await emit('queue', { entries: [], actions: true });
  // Rascunho vazio: a edição fica no campo como rascunho.
  expect(document.querySelector<HTMLTextAreaElement>('textarea')!.value).toBe('primeira');
  expect(document.querySelector('.send-btn--update')).toBeNull();
  expect(document.body.textContent).toContain(m.queue_entry_gone());
});

it('celular, Kimi: painel derivado das bolhas, sem botões, e o chip da fila continua', async () => {
  harness.provider = 'kimi';
  await montarClaude(false);
  await emit('state', { session: 'codex-flow', state: 'working' });
  await emit('message', { kind: 'user_msg', id: 'queued-k', text: 'para o kimi', queued_delivered: false, queued_ts: 1 });
  expect(document.querySelectorAll('.qp-row')).toHaveLength(1);
  expect(document.querySelector('.qp-row button:not(.qp-text)')).toBeNull();
  expect(document.querySelector('.queued-row')).toBeNull();
  expect(document.querySelector('button.fila-chip')).not.toBeNull();
});
