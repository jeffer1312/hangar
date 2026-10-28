// @vitest-environment happy-dom
import { act, createElement, useEffect } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { projectSubagentActivity, type Activity, type AgentRun, type ChatEvent, type CodexSubagents, type SubagentRun } from '@hangar/core';
import { AgentesRodando } from './LiveWork';
import { useAgentSubagents } from '../features/activity/useAgentSubagents';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const api = vi.hoisted(() => ({ getSubagents: vi.fn(), registrarDiag: vi.fn() }));
const route = vi.hoisted(() => ({ focused: true }));
const server = vi.hoisted(() => ({ id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'token-a' }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), ...api }));
vi.mock('expo-router', () => ({ useFocusEffect: (effect: () => void | (() => void)) => useEffect(() => route.focused ? effect() : undefined, [effect, route.focused]) }));
vi.mock('../stores/servers', () => ({ useServers: (select: (state: unknown) => unknown) => select({ servers: [server] }) }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));

const agent: AgentRun = { id: 'launch', kind: 'agent', description: 'Conferir tela', subagentType: 'Explore', model: 'opus', prompt: 'Conferir a tela', running: true };
const sub: SubagentRun = { agentId: 'sub-1', agentType: 'Explore', prompt: agent.prompt ?? null, startedAt: '', updatedAt: '', mtime: 1, toolCalls: 0, tools: [], recent: [], lastText: '', events: [], model: 'gpt-6.1-sol' };
const roots: Root[] = [];
const start = () => 1;
const open = () => {};
async function mount(agents = [agent], events: ChatEvent[] = []) {
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  const render = () => act(async () => root.render(createElement(AgentesRodando, {
    agentes: agents, inicio: start, onAbrir: open, serverId: 'a', sessionName: 'sess', events: [...events],
  })));
  await render();
  return { el, render };
}
beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  route.focused = true;
  api.getSubagents.mockResolvedValue([]);
});
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  vi.useRealTimers();
});

it('mostra o apelido pedido antes da descoberta, inclusive no leitor de tela', async () => {
  const { el } = await mount();
  expect(el.textContent).toContain('opus');
  expect(el.querySelector('button')?.getAttribute('aria-label')).toContain('opus');
});

it('troca o apelido pelo modelo real e conserva a linha quando a consulta não muda', async () => {
  api.getSubagents.mockResolvedValue([{ ...sub, prompt: '  CONFERIR\n A TELA  ' }]);
  const { el } = await mount();
  expect(el.textContent).toContain('gpt-6.1-sol');
  expect(el.textContent).not.toContain('opus');
  expect(el.querySelector('button')?.getAttribute('aria-label')).toContain('gpt-6.1-sol');
  const row = el.querySelector('button');
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(el.querySelector('button')).toBe(row);
  expect(api.getSubagents).toHaveBeenLastCalledWith('sess', server);
});

it.each(['structured', 'text', 'teammate'])('a identidade do resultado vence um prompt repetido: %s', async (source) => {
  api.getSubagents.mockResolvedValue([
    { ...sub, agentId: 'other', model: 'modelo-errado' },
    { ...sub, prompt: 'Outro prompt', model: 'gpt-6-astra' },
  ]);
  const result: ChatEvent = { id: 'result', kind: 'tool_result', tool_use_id: 'launch',
    ...(source === 'structured' ? { bg_agent_id: 'sub-1' } : source === 'teammate'
      ? { bg_agent_id: 'teammate:reviewer', result: 'agent_id: sub-1' }
      : { result: 'Async agent launched. agentId: sub-1' }) };
  const { el } = await mount([agent], [result]);
  expect(el.textContent).toContain('gpt-6-astra');
  expect(el.textContent).not.toContain('modelo-errado');
});

it('não afirma o apelido quando o subagente listado não tem modelo', async () => {
  api.getSubagents.mockResolvedValue([{ ...sub, model: undefined, ilegivel: true }]);
  const { el } = await mount();
  expect(el.textContent).not.toContain('opus');
  expect(el.textContent).not.toContain('gpt-6.1-sol');
});

it('atualiza modelo, mostra erro sem apagar o conhecido e pausa fora de foco', async () => {
  const { el, render } = await mount();
  api.getSubagents.mockResolvedValue([sub]);
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(el.textContent).toContain('gpt-6.1-sol');
  api.getSubagents.mockRejectedValue(new Error('503'));
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(el.querySelector('[role="alert"]')).not.toBeNull();
  expect(el.textContent).toContain('gpt-6.1-sol');
  const count = api.getSubagents.mock.calls.length;
  route.focused = false;
  await render();
  await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
  expect(api.getSubagents).toHaveBeenCalledTimes(count);
  api.getSubagents.mockResolvedValue([{ ...sub, model: 'gpt-6-astra' }]);
  route.focused = true;
  await render();
  expect(el.textContent).toContain('gpt-6-astra');
  expect(el.querySelector('[role="alert"]')).toBeNull();
});

it('falha da consulta registra a causa original uma vez e mostra o aviso com tentar de novo', async () => {
  api.getSubagents.mockRejectedValue(new Error('HTTP 503 upstream'));
  const { el } = await mount();
  await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
  expect(api.getSubagents.mock.calls.length).toBeGreaterThan(1);
  expect(api.registrarDiag).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
    evento: 'activity.subagents_failed', nivel: 'erro', detalhe: 'HTTP 503 upstream' }));
  expect(el.querySelector('[role="alert"]')).not.toBeNull();
  expect([...el.querySelectorAll('button')].some(node => !node.textContent?.includes('Conferir tela'))).toBe(true);
});

it('permite tentar de novo após timeout e ignora a resposta atrasada anterior', async () => {
  const pending = Promise.withResolvers<SubagentRun[]>();
  api.getSubagents.mockReturnValue(pending.promise);
  const { el } = await mount();
  await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
  expect(el.querySelector('[role="alert"]')).not.toBeNull();
  const retry = [...el.querySelectorAll('button')].find(node => !node.textContent?.includes('Conferir tela'));
  expect(retry).toBeDefined();
  api.getSubagents.mockResolvedValue([{ ...sub, model: 'gpt-6-astra' }]);
  await act(async () => retry!.click());
  expect(el.textContent).toContain('gpt-6-astra');
  await act(async () => pending.resolve([sub]));
  expect(el.textContent).not.toContain('gpt-6.1-sol');
  expect(el.querySelector('[role="alert"]')).toBeNull();
});

it('Codex aguarda a descoberta, acompanha o state e não consulta a lista a cada cinco segundos', async () => {
  function Monitor({ states }: { states: CodexSubagents }) {
    const snapshot = useAgentSubagents('a', 'sess', true, 'conversation', true, states);
    return createElement('output', null, `${snapshot.loaded}:${snapshot.subs[0]?.state ?? ''}`);
  }
  const pending = Promise.withResolvers<SubagentRun[]>();
  api.getSubagents.mockReturnValue(pending.promise);
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  const state = { agent_path: '/root/review', state: 'running' as const, ended_at: null, can_message: false, message_blocked: null };
  const render = (states: CodexSubagents) => act(async () => root.render(createElement(Monitor, { states })));
  await render({ 'sub-1': state });
  expect(el.textContent).toBe('false:');
  await act(async () => pending.resolve([{ ...sub, state: 'running' }]));
  expect(el.textContent).toBe('true:running');
  await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
  expect(api.getSubagents).toHaveBeenCalledTimes(1);
  await render({ 'sub-1': { ...state, state: 'finished', ended_at: 1000 } });
  expect(el.textContent).toBe('true:finished');
  expect(api.getSubagents).toHaveBeenCalledTimes(1);
  api.getSubagents.mockResolvedValue([{ ...sub, state: 'running' }]);
  await render({ 'sub-1': { ...state, state: 'finished', ended_at: 1000 }, other: state });
  expect(api.getSubagents).toHaveBeenCalledTimes(2);
  expect(el.textContent).toBe('true:finished');
});

it('Codex tenta novamente após falha inicial e para as consultas ao recuperar', async () => {
  function Monitor() {
    const snapshot = useAgentSubagents('a', 'sess', true, 'conversation', true);
    return createElement('output', null, `${snapshot.loaded}:${snapshot.error ? 'error' : snapshot.subs[0]?.state ?? ''}`);
  }
  api.getSubagents.mockRejectedValueOnce(new Error('offline')).mockResolvedValue([{ ...sub, state: 'finished' }]);
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  await act(async () => root.render(createElement(Monitor)));
  expect(el.textContent).toBe('false:error');
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(el.textContent).toBe('true:finished');
  const calls = api.getSubagents.mock.calls.length;
  await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
  expect(api.getSubagents).toHaveBeenCalledTimes(calls);
});

it('acompanhamento se recupera do timeout com todos os cartões concluídos e ocultos', async () => {
  function Monitor() {
    const { subs } = useAgentSubagents('a', 'sess', true);
    return createElement('output', null, subs[0]?.state);
  }
  api.getSubagents.mockResolvedValue([{ ...sub, state: 'finished' }]);
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  await act(async () => root.render(createElement(Monitor)));
  expect(el.textContent).toBe('finished');
  expect(el.querySelector('button')).toBeNull();
  const pending = Promise.withResolvers<SubagentRun[]>();
  api.getSubagents.mockReturnValue(pending.promise);
  await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
  expect(el.textContent).toBe('finished');
  api.getSubagents.mockResolvedValue([{ ...sub, state: 'running' }]);
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(el.textContent).toBe('running');
  await act(async () => pending.resolve([{ ...sub, state: 'finished' }]));
  expect(el.textContent).toBe('running');
});

it('/clear invalida filho do mesmo caminho, inclusive resposta tardia, timeout e desativação', async () => {
  const raw: Activity = { tasks: [], agents: [{ ...agent, agentPath: '/root/review' }], shells: [],
    total: 0, done: 0, inProgress: 0, runningAgents: 1, runningShells: 0 };
  function Monitor({ conversation, enabled }: { conversation: string; enabled: boolean }) {
    const { subs } = useAgentSubagents('a', 'sess', enabled, conversation);
    return createElement('output', null, `${subs[0]?.agentId ?? 'empty'}:${projectSubagentActivity(raw, subs).runningAgents}`);
  }
  const old = { ...sub, agentId: 'old-child', agentPath: '/root/review', state: 'finished' as const };
  api.getSubagents.mockResolvedValue([old]);
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  const render = (conversation: string, enabled = true) => act(async () => root.render(createElement(Monitor, { conversation, enabled })));
  await render('before-clear');
  expect(el.textContent).toBe('old-child:0');
  const oldRequest = Promise.withResolvers<SubagentRun[]>();
  api.getSubagents.mockReturnValue(oldRequest.promise);
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  const newRequest = Promise.withResolvers<SubagentRun[]>();
  api.getSubagents.mockReturnValue(newRequest.promise);
  await render('after-clear');
  expect(el.textContent).toBe('empty:1');
  await act(async () => oldRequest.resolve([old]));
  expect(el.textContent).toBe('empty:1');
  await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
  expect(el.textContent).toBe('empty:1');
  const current = { ...old, agentId: 'new-child', state: 'running' as const };
  api.getSubagents.mockResolvedValue([current]);
  await act(async () => { await vi.advanceTimersByTimeAsync(5_000); });
  expect(el.textContent).toBe('new-child:1');
  await act(async () => newRequest.resolve([old]));
  expect(el.textContent).toBe('new-child:1');
  await render('after-clear', false);
  expect(el.textContent).toBe('empty:1');
  const reopened = Promise.withResolvers<SubagentRun[]>();
  api.getSubagents.mockReturnValue(reopened.promise);
  await render('after-clear');
  expect(el.textContent).toBe('empty:1');
  await act(async () => reopened.resolve([current]));
  expect(el.textContent).toBe('new-child:1');
});
