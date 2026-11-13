// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Activity, ChatEvent, Server, SubagentRun, WorkflowSummary } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const data = vi.hoisted(() => ({
  activity: { tasks: [], agents: [], shells: [], total: 0, done: 0, inProgress: 0, runningAgents: 0, runningShells: 0 } as Activity,
  server: { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'token-a' } as Server,
  events: [] as ChatEvent[],
  workflows: [] as WorkflowSummary[],
  ferramentas: 'tree',
}));
vi.mock('../../stores/aparencia', () => ({ useAparencia: (select: (state: unknown) => unknown) => select({ ferramentas: data.ferramentas }) }));
vi.mock('./useActivity', () => ({ useActivity: () => data.activity }));
vi.mock('../../stores/chat', () => ({ chatStore: () => ({
  use: (select: (state: { events: ChatEvent[]; eventsRevision: number; stateEvent: null }) => unknown) =>
    select({ events: data.events, eventsRevision: 0, stateEvent: null }),
}) }));
vi.mock('../../stores/servers', () => ({ useServers: (select: (state: { servers: Server[] }) => unknown) => select({ servers: [data.server] }) }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getWorkflows: vi.fn(async () => data.workflows),
  getWorkflow: vi.fn().mockResolvedValue(null),
  getSubagents: vi.fn().mockResolvedValue([]),
  stopTask: vi.fn(async () => ({ pending: true })),
}));
const toastErro = vi.hoisted(() => vi.fn());
vi.mock('../../ui/Toast', () => ({ toast: { erro: toastErro } }));
vi.mock('expo-router', () => ({
  Stack: { Screen: () => null },
}));
vi.mock('expo-blur', () => ({ BlurView: () => null }));
vi.mock('expo-glass-effect', () => ({ GlassView: () => null, isLiquidGlassAvailable: () => false }));
vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: () => null }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: () => null }));
vi.mock('../../chat/AssistantBubble', () => ({ mkMarkdownStyle: () => ({}) }));
vi.mock('../../chat/MessageList', () => ({ MessageList: () => null }));
vi.mock('../../chat/LiveWork', () => ({ WorkingLine: () => null }));

import * as m from '../../paraglide/messages';
import { ActivityPanel } from './ActivityPanel';

let root: Root | undefined;
async function mount(onBackRequest?: (handler: () => boolean) => void) {
  const el = document.createElement('div');
  root = createRoot(el);
  await act(async () => root!.render(createElement(ActivityPanel, { serverId: 'a', name: 'sess', focused: true, onBackRequest })));
  return el;
}

beforeEach(() => {
  data.activity = { tasks: [], agents: [], shells: [], total: 0, done: 0, inProgress: 0, runningAgents: 0, runningShells: 0 };
  data.workflows = [];
  data.ferramentas = 'tree';
});
afterEach(async () => {
  if (root) await act(async () => root!.unmount());
  root = undefined;
  vi.useRealTimers();
});

it('pausa o polling ao perder foco explícito e retoma sem recarregar os workflows', async () => {
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] });
  const { getSubagents, getWorkflows } = await import('@hangar/core');
  vi.mocked(getSubagents).mockClear();
  vi.mocked(getWorkflows).mockClear();
  await mount();
  expect(getSubagents).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(getSubagents).toHaveBeenCalledTimes(2);
  await act(async () => root!.render(createElement(ActivityPanel, { serverId: 'a', name: 'sess', focused: false })));
  await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
  expect(getSubagents).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
  await act(async () => root!.render(createElement(ActivityPanel, { serverId: 'a', name: 'sess', focused: true })));
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(getSubagents).toHaveBeenCalledTimes(3);
  expect(getWorkflows).toHaveBeenCalledTimes(1);
});

it('mostra os dois shells em execução sem oferecer navegação de folha', async () => {
  data.activity = { ...data.activity, runningShells: 2, shells: [
    { id: 'build', command: 'npm run build', rotulo: 'npm run build', running: true },
    { id: 'dev', command: 'npm run dev', rotulo: 'npm run dev', running: true },
  ] };
  const el = await mount();
  expect(el.textContent).toContain('npm run build');
  expect(el.textContent).toContain('npm run dev');
  expect(el.textContent).toContain(m.atividade_shells_rodando({ n: 2 }));
  expect(el.textContent).not.toContain(m.atividade_vazio());
  expect(el.querySelector(`[aria-label="${m.native_close()}"]`)).toBeNull();
});

it('mostra o vazio existente quando não há atividade', async () => {
  const el = await mount();
  expect(el.textContent).toContain(m.atividade_vazio());
});

it('consome o voltar só no detalhe e libera o handler ao desmontar', async () => {
  data.workflows = [{ runId: 'run-1', name: 'run-1', status: 'running', agentCount: 1, phaseCount: 0, totalTokens: 100, durationMs: 1000, startTime: 1, running: true }];
  let back = () => false;
  const el = await mount((handler) => { back = handler; });
  expect(back()).toBe(false);
  const workflow = el.querySelector<HTMLButtonElement>('[aria-label="run-1"]');
  expect(workflow).not.toBeNull();
  await act(async () => workflow!.click());
  expect(el.textContent).toContain(m.atividade_run_nao_encontrado());
  await act(async () => { expect(back()).toBe(true); });
  expect(el.querySelector('[aria-label="run-1"]')).not.toBeNull();
  expect(back()).toBe(false);
  await act(async () => root!.unmount());
  root = undefined;
  expect(back()).toBe(false);
});

it('Claude Desktop junta subagentes e shells em Tarefas em segundo plano e recolhe as finalizadas', async () => {
  data.ferramentas = 'claudeDesktop';
  data.activity = { ...data.activity, runningAgents: 2, runningShells: 1,
    agents: [
      { id: 'call-cpu', kind: 'agent', description: 'diag-cpu', running: true },
      { id: 'call-disk', kind: 'agent', description: 'diag-disk', running: true },
    ],
    shells: [{ id: 'call-sh', command: 'npm run dev', rotulo: 'npm run dev', running: true }] };
  const finished: SubagentRun = { agentId: 'done-1', agentType: null, prompt: 'diag-services', startedAt: '2026-11-13T08:00:00Z',
    updatedAt: '2026-11-13T08:05:00Z', endedAt: new Date().toISOString(), mtime: 0, toolCalls: 3, tools: [], recent: [], lastText: '', state: 'finished' };
  const { getSubagents } = await import('@hangar/core');
  vi.mocked(getSubagents).mockResolvedValue([finished]);
  try {
    const el = await mount();
    expect(el.textContent).toContain(m.atividade_desktop_titulo());
    for (const name of ['diag-cpu', 'diag-disk', 'npm run dev']) expect(el.textContent).toContain(name);
    expect(el.textContent).not.toContain(m.atividade_grupo_rodando({ n: 2 }));
    expect(el.textContent).not.toContain(m.atividade_shells());
    expect(el.textContent).not.toContain('diag-services');
    const more = el.querySelector<HTMLElement>(`[aria-label="${m.atividade_desktop_finalizadas({ n: 1 })}"]`);
    expect(more).not.toBeNull();
    await act(async () => more!.click());
    expect(el.textContent).toContain('diag-services');
  } finally {
    vi.mocked(getSubagents).mockResolvedValue([]);
  }
});

const stopButton = (el: HTMLElement) => el.querySelector<HTMLElement>(`[aria-label="${m.atividade_desktop_parar()}: npm run dev"]`);

it('■ para o shell do Claude, mostra parando e volta com aviso sem confirmação em 15 s', async () => {
  data.ferramentas = 'claudeDesktop';
  data.activity = { ...data.activity, runningShells: 1,
    shells: [{ id: 'call-sh', command: 'npm run dev', rotulo: 'npm run dev', running: true, taskId: 'bg1' }] };
  const { stopTask } = await import('@hangar/core');
  vi.mocked(stopTask).mockClear();
  toastErro.mockClear();
  const el = await mount();
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  expect(stopButton(el)).not.toBeNull();
  await act(async () => stopButton(el)!.click());
  expect(stopTask).toHaveBeenCalledWith('sess', 'bg1', data.server);
  expect(el.textContent).toContain(m.atividade_parando());
  expect(stopButton(el)).toBeNull();
  expect(el.querySelector('[aria-busy="true"]')?.textContent).toContain('npm run dev');
  expect(el.querySelector('[accessibilityliveregion="polite"]')?.textContent).toBe(m.atividade_parando());
  await act(async () => { await vi.advanceTimersByTimeAsync(14_000); });
  expect(el.textContent).toContain(m.atividade_parando());
  await act(async () => { await vi.advanceTimersByTimeAsync(1_000); });
  expect(el.textContent).not.toContain(m.atividade_parando());
  expect(stopButton(el)).not.toBeNull();
  expect(toastErro).toHaveBeenCalledWith(m.atividade_parar_sem_confirmacao());
});

it('■ só encerra o parando pela ausência da tarefa depois que a lista carregou', async () => {
  data.ferramentas = 'claudeDesktop';
  data.activity = { ...data.activity, runningAgents: 1,
    agents: [{ id: 'call-cpu', kind: 'agent', description: 'diag-cpu', running: true, agentId: 'ag-cpu' }] };
  const { getSubagents } = await import('@hangar/core');
  let loadSubs: (subs: SubagentRun[]) => void = () => {};
  vi.mocked(getSubagents).mockImplementationOnce(() => new Promise((resolve) => { loadSubs = resolve; }));
  toastErro.mockClear();
  const el = await mount();
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  const button = () => el.querySelector<HTMLElement>(`[aria-label="${m.atividade_desktop_parar()}: diag-cpu"]`);
  await act(async () => button()!.click());
  expect(el.textContent).toContain(m.atividade_parando());
  const agents = data.activity.agents;
  const rerenderWith = async (list: typeof agents) => {
    data.activity = { ...data.activity, runningAgents: list.length, agents: list };
    await act(async () => root!.render(createElement(ActivityPanel, { serverId: 'a', name: 'sess', focused: true })));
  };
  // Lista ainda carregando: a tarefa some do conjunto, mas isso não prova a parada.
  await rerenderWith([]);
  await rerenderWith(agents);
  expect(el.textContent).toContain(m.atividade_parando());
  expect(button()).toBeNull();
  // Lista carregada e a tarefa fora dela: a parada se confirmou.
  await rerenderWith([]);
  await act(async () => { loadSubs([]); });
  await rerenderWith(agents);
  expect(el.textContent).not.toContain(m.atividade_parando());
  expect(button()).not.toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
  expect(toastErro).not.toHaveBeenCalled();
});

it('■ recusado pelo servidor volta a linha e mostra o texto do código', async () => {
  data.ferramentas = 'claudeDesktop';
  data.activity = { ...data.activity, runningShells: 1,
    shells: [{ id: 'call-sh', command: 'npm run dev', rotulo: 'npm run dev', running: true, taskId: 'bg1' }] };
  const { stopTask } = await import('@hangar/core');
  vi.mocked(stopTask).mockRejectedValueOnce(Object.assign(new Error(m.erro_parar_sem_plugin()), { status: 409, code: 'erro_parar_sem_plugin' }));
  toastErro.mockClear();
  const el = await mount();
  expect(stopButton(el)).not.toBeNull();
  await act(async () => stopButton(el)!.click());
  expect(toastErro).toHaveBeenCalledWith(m.erro_parar_sem_plugin());
  expect(el.textContent).not.toContain(m.atividade_parando());
  expect(stopButton(el)).not.toBeNull();
});

it('■ também nas seções da Árvore: shell, agente e subagente rodando, com o mesmo parando', async () => {
  data.activity = { ...data.activity, runningAgents: 1, runningShells: 1,
    agents: [{ id: 'call-cpu', kind: 'agent', description: 'diag-cpu', running: true, agentId: 'ag-cpu' }],
    shells: [{ id: 'call-sh', command: 'npm run dev', rotulo: 'npm run dev', running: true, taskId: 'bg1' }] };
  const running: SubagentRun = { agentId: 'ag-sub', agentType: null, prompt: 'diag-sub', startedAt: '2026-11-13T08:00:00Z',
    updatedAt: new Date().toISOString(), endedAt: null, mtime: 0, toolCalls: 1, tools: [], recent: [], lastText: '', state: 'running' };
  const { getSubagents, stopTask } = await import('@hangar/core');
  vi.mocked(getSubagents).mockResolvedValue([running]);
  vi.mocked(stopTask).mockClear();
  try {
    const el = await mount();
    const button = (title: string) => el.querySelector<HTMLElement>(`[aria-label="${m.atividade_desktop_parar()}: ${title}"]`);
    expect(button('diag-cpu')).not.toBeNull();
    expect(button('diag-sub')).not.toBeNull();
    expect(stopButton(el)).not.toBeNull();
    await act(async () => stopButton(el)!.click());
    expect(stopTask).toHaveBeenCalledWith('sess', 'bg1', data.server);
    expect(el.textContent).toContain(m.atividade_parando());
    expect(stopButton(el)).toBeNull();
    await act(async () => button('diag-cpu')!.click());
    expect(stopTask).toHaveBeenCalledWith('sess', 'ag-cpu', data.server);
  } finally {
    vi.mocked(getSubagents).mockResolvedValue([]);
  }
});

it('■ fica fora do convidado só de leitura e do Codex', async () => {
  data.activity = { ...data.activity, runningShells: 1,
    shells: [{ id: 'call-sh', command: 'npm run dev', rotulo: 'npm run dev', running: true, taskId: 'bg1' }] };
  const { useSessions } = await import('../../stores/sessions');
  const before = useSessions.getState().rows;
  for (const row of [{ provider: 'codex' }, { provider: 'claude', guest_kind: 'pair' }]) {
    useSessions.setState({ rows: [{ serverId: 'a', name: 'sess', ...row }] as never });
    try {
      const el = await mount();
      expect(el.textContent).toContain('npm run dev');
      expect(stopButton(el)).toBeNull();
      await act(async () => root!.unmount());
      root = undefined;
    } finally {
      useSessions.setState({ rows: before });
    }
  }
});

it('shell sem id de fundo não ganha ■', async () => {
  data.ferramentas = 'claudeDesktop';
  data.activity = { ...data.activity, runningShells: 1,
    shells: [{ id: 'call-sh', command: 'npm run dev', rotulo: 'npm run dev', running: true }] };
  const el = await mount();
  expect(el.textContent).toContain('npm run dev');
  expect(stopButton(el)).toBeNull();
});
