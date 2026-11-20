// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import { MessageList } from './MessageList';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('../paraglide/messages', async (original) => ({ ...await original<typeof import('../paraglide/messages')>(), msg_aria_mensagens: () => 'msg_aria_mensagens' }));
vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('article', null, markdown) }));
vi.mock('../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => createElement('section', null, children) }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock('../ui/Toast', () => ({ toast: { erro: vi.fn() } }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../stores/servers', () => ({ useServers: { getState: () => ({ servers: [] }) } }));
const aparencia = vi.hoisted(() => ({ tarefas: false }));
vi.mock('../stores/aparencia', () => ({
  useAparencia: (select: (s: unknown) => unknown) => select({ pensamentoTools: 'busca', ferramentas: 'tree', tarefas: aparencia.tarefas }),
}));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('./PreviewBubble', () => ({ PreviewBubble: () => null }));
vi.mock('./LiveWork', () => ({ AgentesRodando: () => null, PensamentoVivo: () => null, WorkingLine: () => null }));
vi.mock('./tools/ToolCard', () => ({ ToolCard: () => null }));
vi.mock('./tools/ToolDetailSheet', () => ({ ToolDetailSheet: () => null }));
vi.mock('./tools/HtmlPageCard', () => ({ HtmlPageCard: () => null }));

// Os itens reais são `memo`; os dublês também, para o teste medir o que a lista entrega a eles.
const renders = vi.hoisted(() => ({ log: [] as string[], virtual: false, data: [] as unknown[] }));
vi.mock('./UserBubble', async () => {
  const { memo: m } = await import('react');
  return { UserBubble: m(({ eventId }: { eventId?: string }) => { renders.log.push(`user:${eventId}`); return null; }) };
});
vi.mock('./AssistantBubble', async () => {
  const { memo: m } = await import('react');
  return { AssistantBubble: m(({ eventId }: { eventId?: string }) => { renders.log.push(`assistant:${eventId}`); return null; }), mkMarkdownStyle: () => ({}) };
});
vi.mock('./tools/ToolGroup', async () => {
  const { memo: m } = await import('react');
  return {
    ToolGroup: m(({ parts, resultOf }: { parts: ChatEvent[]; resultOf: (t: ChatEvent) => ChatEvent | null }) => {
      renders.log.push(`fold:${parts[0].id}:${parts.map((p) => resultOf(p)?.result ?? '-').join(',')}`);
      return null;
    }),
  };
});
vi.mock('./TaskList', async () => {
  const { memo: m } = await import('react');
  return { TaskList: m(({ tasks }: { tasks: unknown[] }) => { renders.log.push(`tasks:${tasks.length}`); return null; }) };
});
// Como o LegendList: cada célula só refaz quando o item dela ou o renderItem muda.
vi.mock('@legendapp/list/react-native', async () => {
  const { createElement: h, memo: m } = await import('react');
  const Cell = m(({ item, renderItem }: { item: unknown; renderItem: (row: { item: unknown }) => ReactNode }) => h('div', null, renderItem({ item })));
  return {
    LegendList: ({ data, renderItem, keyExtractor }: {
      data: unknown[]; renderItem: (row: { item: unknown }) => ReactNode; keyExtractor: (i: unknown) => string;
    }) => {
      renders.data = data;
      return h('div', null, ...(renders.virtual ? [] : data.map((item) => h(Cell, { key: keyExtractor(item), item, renderItem }))));
    },
  };
});

const roots: Root[] = [];
function mount(events: ChatEvent[]) {
  const root = createRoot(document.createElement('div'));
  roots.push(root);
  const render = (evs: ChatEvent[]) => act(() => root.render(createElement(MessageList, {
    events: evs, preview: '', olderFailed: '', onLoadOlder: () => {}, sessionName: 'sess', serverId: 'a',
    session: { provider: 'claude', headless: false },
  })));
  render(events);
  return render;
}

const user = (id: string): ChatEvent => ({ id, kind: 'user_msg', text: id });
const assistant = (id: string): ChatEvent => ({ id, kind: 'assistant_msg', text: id });
const tool = (id: string, name = 'Bash'): ChatEvent => ({ id, kind: 'tool_use', tool_name: name, tool_use_id: id, tool_input: {} });
const result = (id: string, text: string): ChatEvent => ({ id: `r-${id}`, kind: 'tool_result', tool_use_id: id, result: text });

beforeEach(() => { renders.log.length = 0; renders.virtual = false; aparencia.tarefas = false; });
afterEach(() => { act(() => { for (const root of roots.splice(0)) root.unmount(); }); });

const historico = [user('u1'), tool('t1'), result('t1', 'ok1'), tool('t2'), result('t2', 'ok2'), assistant('a1')];

it('mensagem nova não refaz os itens antigos da conversa', () => {
  const render = mount(historico);
  expect(renders.log).toEqual(['user:u1', 'fold:t1:ok1,ok2', 'assistant:a1']);

  renders.log.length = 0;
  render([...historico, user('u2')]);
  expect(renders.log).toEqual(['user:u2']);

  renders.log.length = 0;
  render([...historico, user('u2'), tool('t3')]);
  expect(renders.log).toEqual(['fold:t3:-']);
});

it('5000 eventos e 200 appends leem somente o delta sem alterar snapshots', () => {
  renders.virtual = true;
  let reads = 0;
  const counted = (ev: ChatEvent) => new Proxy(ev, { get(target, key, receiver) {
    if (typeof key === 'string') reads++;
    return Reflect.get(target, key, receiver);
  } });
  let events = Array.from({ length: 5000 }, (_, i) => counted(tool(`t${i}`)));
  const root = createRoot(document.createElement('div'));
  roots.push(root);
  const render = () => act(() => root.render(createElement(MessageList, {
    events, eventsRevision: 0, preview: '', olderFailed: '', onLoadOlder: () => {},
    agentesRodando: [], queuePanel: { entries: [], actions: true },
  } as Parameters<typeof MessageList>[0])));
  render();
  const before = renders.data as { type: string; parts: ChatEvent[] }[];
  reads = 0;
  for (let i = 0; i < 200; i++) {
    events = [...events, counted(tool(`new${i}`))];
    render();
  }
  const deltaReads = reads;
  expect((renders.data[0] as { parts: ChatEvent[] }).parts).toHaveLength(5200);
  expect(before[0].parts).toHaveLength(5000);
  expect(deltaReads).toBeLessThan(200 * 100);
});

it('resultado que chega refaz só o trecho dele', () => {
  const base = [...historico, user('u2'), tool('t3')];
  const render = mount(base);
  renders.log.length = 0;
  render([...base, result('t3', 'ok3')]);
  expect(renders.log).toEqual(['fold:t3:ok3']);
});

it('lista de tarefas continua atualizando sem refazer o resto', () => {
  aparencia.tarefas = true;
  const task = (id: string, subject: string): ChatEvent => ({ id, kind: 'tool_use', tool_name: 'TaskCreate', tool_use_id: id, tool_input: { subject } });
  const base = [user('u1'), task('k1', 'um'), result('k1', 'Task #1 created successfully: um'), assistant('a1')];
  const render = mount(base);
  expect(renders.log).toContain('tasks:1');
  renders.log.length = 0;
  render([...base, task('k2', 'dois'), result('k2', 'Task #2 created successfully: dois')]);
  expect(renders.log).toEqual(['tasks:2']);
});
