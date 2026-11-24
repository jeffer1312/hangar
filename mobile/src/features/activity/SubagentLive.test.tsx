// @vitest-environment happy-dom
import { act, createElement, useLayoutEffect, type ComponentProps, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, configureLocale, type SubagentRun } from '@hangar/core';
import { overwriteGetLocale } from '../../paraglide/runtime';
import * as m from '../../paraglide/messages';
import { SubagentLive } from './SubagentLive';
import * as subagentLive from './SubagentLive';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const servers = vi.hoisted(() => ({ servers: [
  { id: 'remote', label: 'remote', baseUrl: 'http://remote.test:8765', token: 'remote-token' },
  { id: 'active', label: 'active', baseUrl: 'http://active.test:8765', token: 'active-token' },
] }));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  ScrollView: ({ children, style }: { children?: ReactNode; style?: Record<string, unknown> }) => createElement('div', { style, 'data-native-scroll': true }, children),
  Text: ({ children, accessibilityRole, accessibilityLabel, accessibilityLiveRegion, onPress }: {
    children?: ReactNode; accessibilityRole?: string; accessibilityLabel?: string; accessibilityLiveRegion?: 'polite'; onPress?: () => void;
  }) => createElement('span', { role: accessibilityRole, 'aria-label': accessibilityLabel, 'aria-live': accessibilityLiveRegion, onClick: onPress }, children),
  TextInput: ({ value, onChangeText, accessibilityLabel, editable }: { value: string; onChangeText: (text: string) => void; accessibilityLabel?: string; editable?: boolean }) => createElement('textarea', {
    value, 'aria-label': accessibilityLabel, disabled: editable === false,
    onInput: (event: { currentTarget: HTMLTextAreaElement }) => onChangeText(event.currentTarget.value),
  }),
  Pressable: ({ children, onPress, accessibilityRole, accessibilityLabel, accessibilityState, disabled }: {
    children?: ReactNode; onPress?: () => void; accessibilityRole?: string; accessibilityLabel?: string; accessibilityState?: { expanded?: boolean }; disabled?: boolean;
  }) => createElement(accessibilityRole === 'button' ? 'button' : 'div', { onClick: onPress, role: accessibilityRole, 'aria-label': accessibilityLabel, 'aria-expanded': accessibilityState?.expanded, disabled }, children),
}));
vi.mock('react-native-keyboard-controller', () => ({
  KeyboardAvoidingView: ({ children, style, automaticOffset, behavior }: { children: ReactNode; style?: Record<string, unknown>; automaticOffset?: boolean; behavior?: string }) => createElement('div', { style, 'data-keyboard-offset': automaticOffset, 'data-keyboard-behavior': behavior }, children),
}));
vi.mock('../../stores/servers', () => ({ useServers: (select: (s: typeof servers) => unknown) => select(servers) }));
vi.mock('../../stores/aparencia', () => ({ useAparencia: (select: (s: unknown) => unknown) => select({ pensamentoTools: 'busca', ferramentas: 'tree', tarefas: false }) }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../../stores/chat', () => ({ chatStore: {} }));
vi.mock('expo-router', () => ({ useFocusEffect: () => {}, useRouter: () => ({ push: vi.fn() }) }));
// O aviso do Claude abre a folha do conserto, que importa a folha nativa.
vi.mock('../../ui/Sheet', () => ({ Sheet: () => null }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
// A lista leva a galeria e o salto da conversa, que avisam por toast; o sonner-native puxaria o react-native de verdade.
vi.mock('../../ui/Toast', () => ({ toast: { ok: () => {}, erro: () => {} } }));
// Ramos fora deste fluxo não montam; a lista, bolha e miniatura usadas abaixo são reais.
vi.mock('../../chat/AssistantBubble', () => ({ AssistantBubble: () => null, mkMarkdownStyle: () => ({}), mkMutedMarkdownStyle: () => ({}) }));
vi.mock('../../chat/BubbleActions', () => ({ BubbleActions: () => null }));
vi.mock('../../chat/OrqTimelineEntry', () => ({ OrqTimelineEntry: () => null }));
vi.mock('../../chat/PreviewBubble', () => ({ PreviewBubble: () => null }));
vi.mock('../../chat/tools/ToolDetailSheet', () => ({ ToolDetailSheet: () => null }));
vi.mock('../../chat/tools/HtmlPageCard', () => ({ HtmlPageCard: () => null }));
vi.mock('../../features/attachments/LightboxModal', () => ({ LightboxModal: () => null }));
vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('article', null, markdown) }));
vi.mock('expo-image', () => ({ Image: ({ source }: { source: { uri: string } }) => createElement('img', { src: source.uri }) }));
type ListConfig = { nestedScrollEnabled?: boolean; maintainScrollAtEnd?: boolean; initialScrollAtEnd?: boolean; maintainVisibleContentPosition?: boolean };
const legendList = vi.hoisted(() => ({ alignment: vi.fn(), config: vi.fn<(props: ListConfig) => void>() }));
vi.mock('@legendapp/list/react-native', () => ({
  LegendList: (props: ListConfig & {
    data: { id: string }[]; renderItem: (row: { item: unknown }) => ReactNode; ListFooterComponent?: ReactNode; onStartReached: () => void; alignItemsAtEnd?: boolean;
  }) => {
    const { data, renderItem, ListFooterComponent, onStartReached, alignItemsAtEnd } = props;
    legendList.config(props);
    legendList.alignment(alignItemsAtEnd);
    return createElement('div', null,
      createElement('button', { onClick: onStartReached, 'aria-label': 'load-older' }),
      ...data.map(item => createElement('div', { key: item.id }, renderItem({ item }))), ListFooterComponent);
  },
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const requests: { url: string; init: RequestInit | undefined; response: ReturnType<typeof deferred<Response>> }[] = [];
const roots: Root[] = [];
type Props = ComponentProps<typeof SubagentLive>;
const initial = { sessionName: 'sess', agentId: 'one', server: servers.servers[0] };

function detail(agentId: string, text: string, image = false): SubagentRun {
  return { agentId, agentType: 'Explore', prompt: 'prompt', startedAt: '2026-10-17T10:00:00Z',
    updatedAt: '2026-10-17T10:00:01Z', mtime: 1792231201, toolCalls: 0, tools: [], recent: [],
    lastText: '', finished: false, events: [{ id: `message-${agentId}`, kind: 'user_msg', text,
      ...(image ? { image_count: 1 } : {}) }] };
}
function CommitProbe({ children, inspect }: { children: ReactNode; inspect: () => void }) {
  useLayoutEffect(inspect);
  return children;
}
function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  const commits: string[] = [];
  const render = async (patch: Partial<Props> = {}) => {
    await act(async () => root.render(createElement(CommitProbe, {
      children: createElement(SubagentLive, { ...initial, ...patch }),
      inspect: () => { commits.push(container.textContent ?? ''); },
    })));
  };
  return { container, render, commits };
}
async function reply(index: number, body: SubagentRun | { detail: string | { code: string; msg: string; params?: Record<string, unknown> } } | { delivered: true }, status = 200) {
  await act(async () => requests[index].response.resolve(new Response(JSON.stringify(body), { status })));
}
async function poll() {
  await act(async () => { await vi.advanceTimersByTimeAsync(2500); });
}

beforeEach(() => {
  legendList.alignment.mockClear();
  legendList.config.mockClear();
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-17T10:00:30Z'));
  requests.length = 0;
  overwriteGetLocale(() => 'pt');
  configureLocale({ getLocale: () => 'pt' });
  configureApi({ getBaseUrl: () => 'http://active.test:8765', getToken: () => 'active-token',
    onUnauthorized: () => {}, origin: null, createEventSource: () => { throw new Error('unexpected SSE'); } });
  vi.stubGlobal('fetch', vi.fn((url: string, init?: RequestInit) => {
    const response = deferred<Response>();
    requests.push({ url: String(url), init, response });
    return response.promise;
  }));
});
afterEach(() => {
  act(() => { for (const root of roots.splice(0)) root.unmount(); });
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it('detalhe habilita pausa de grupo e nested scroll preservando polling e paginação', async () => {
  const { container, render } = mount();
  await render();
  const page = detail('one', 'conversa');
  page.events = [
    { id: 'thought-1', kind: 'thinking', text: 'first-thought' },
    { id: 'thought-2', kind: 'thinking', text: 'second-thought' },
    ...Array.from({ length: 198 }, (_, i) => ({ id: `notice-${i}`, kind: 'notice' as const, text: `notice-${i}` })),
  ];
  await reply(0, page);
  const config = () => legendList.config.mock.lastCall![0];
  expect(config().nestedScrollEnabled).toBe(true);
  expect(config().initialScrollAtEnd).toBe(true);
  expect(config().maintainVisibleContentPosition).toBe(true);
  expect(legendList.alignment.mock.lastCall![0]).toBe(false);
  const list = container.querySelector('[aria-label="load-older"]')!.parentElement!;
  await act(async () => list.querySelectorAll('button')[1].click());
  expect(container.textContent).toContain('first-thought');
  expect(config().maintainScrollAtEnd).toBe(false);
  await poll();
  await reply(1, JSON.parse(JSON.stringify(page)));
  expect(config().maintainScrollAtEnd).toBe(false);
  await act(async () => (container.querySelector('[aria-label="load-older"]') as HTMLElement).click());
  expect(requests[2].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=400');
  await reply(2, { ...page, events: [...page.events!, { id: 'new-notice', kind: 'notice', text: 'new-notice' }] });
  expect(config().maintainScrollAtEnd).toBe(false);
  expect(container.textContent).toContain('new-notice');
});

it('janela móvel preserva grupo aberto e Mostrar todas no polling concorrente ao toque', async () => {
  const { container, render } = mount();
  await render();
  const page = { ...detail('one', ''), toolCalls: 99, events: [
    { id: 'thought-prefix', kind: 'thinking' as const, text: 'prefixo carregado' },
    { id: 'thought-next', kind: 'thinking' as const, text: 'pensamento seguinte' },
    ...Array.from({ length: 99 }, (_, i) => [
      { id: `use-${i}`, kind: 'tool_use' as const, tool_name: 'Read', tool_use_id: `call-${i}`, tool_input: { file_path: `/src/arquivo-${i}.ts` } },
      { id: `result-${i}`, kind: 'tool_result' as const, tool_use_id: `call-${i}`, result: `conteúdo-${i}` },
    ]).flat(),
  ] };
  await reply(0, page);
  const group = () => container.querySelector('[aria-expanded]') as HTMLButtonElement;
  expect(group().getAttribute('aria-expanded')).toBe('false');
  await act(async () => group().click());
  expect(group().getAttribute('aria-expanded')).toBe('true');
  expect(container.textContent).toContain('prefixo carregado');
  expect(container.textContent).not.toContain('arquivo-80.ts');
  const showAll = [...container.querySelectorAll('button')].find(button => button.textContent === m.tool_mostrar_todas({ n: 101 }));
  expect(showAll).toBeDefined();
  await poll();
  expect(requests[1].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=200');
  await act(async () => {
    showAll!.click();
    requests[1].response.resolve(new Response(JSON.stringify({ ...page, events: [
      ...page.events.slice(1), { id: 'thought-new', kind: 'thinking', text: 'pensamento novo' },
    ] })));
  });
  expect(group().getAttribute('aria-expanded')).toBe('true');
  expect(container.textContent).toContain('arquivo-80.ts');
  expect(container.textContent).toContain('prefixo carregado');
  expect(container.textContent).toContain('pensamento novo');
  expect([...container.querySelectorAll('button')].some(button => button.textContent === m.tool_mostrar_todas({ n: 102 }))).toBe(false);
});

it('resposta nova atualiza texto resultado e metadados sem duplicar eventos compartilhados', async () => {
  const { container, render } = mount();
  await render();
  const page: SubagentRun = { ...detail('one', ''), canMessage: true, toolCalls: 1, events: [
    { id: 'prefix', kind: 'notice', text: 'histórico preservado' },
    { id: 'thought', kind: 'thinking', text: 'texto antigo' },
    { id: 'use', kind: 'tool_use', tool_name: 'Read', tool_use_id: 'call', tool_input: { file_path: '/src/arquivo.ts' } },
    { id: 'result', kind: 'tool_result', tool_use_id: 'call', result: 'resultado antigo' },
  ] };
  await reply(0, page);
  await act(async () => (container.querySelector('[aria-expanded]') as HTMLButtonElement).click());
  expect(container.textContent).toContain('texto antigo');
  expect(container.querySelector('textarea')).not.toBeNull();
  await poll();
  await reply(1, { ...page, state: 'finished', agentType: 'ResearchNew', toolCalls: 7,
    tools: [{ name: 'Read', count: 7 }], recent: [{ name: 'Read-recent-new', target: 'arquivo.ts' }], canMessage: false,
    events: [
      { ...page.events![1], text: 'texto atualizado' },
      page.events![2],
      { ...page.events![3], result: 'resultado atualizado', is_error: true },
    ],
  });
  expect(container.textContent?.split('texto atualizado')).toHaveLength(2);
  expect(container.textContent?.split('resultado atualizado')).toHaveLength(2);
  expect(container.textContent?.split('arquivo.ts')).toHaveLength(2);
  expect(container.textContent?.split('histórico preservado')).toHaveLength(2);
  expect(container.textContent).not.toContain('texto antigo');
  expect(container.textContent).not.toContain('resultado antigo');
  expect(container.querySelector('[aria-expanded]')?.getAttribute('aria-expanded')).toBe('true');
  expect(container.textContent).toContain(m.native_tools_failed_1());
  expect(container.textContent).toContain(m.atividade_sub_concluido());
  expect(container.textContent).toContain(m.atividade_chamadas({ n: 7 }));
  expect(container.textContent).toContain('ResearchNew');
  expect(container.textContent).toContain('Read-recent-new');
  expect(container.textContent).toContain('Read ×7');
  expect(container.querySelector('textarea')).toBeNull();
  await poll();
  expect(requests).toHaveLength(2);
});

it.each(['sem IDs comuns', 'vazia'])('cauda %s substitui conversa e estado anteriores', async (mode) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', ''), events: [
    { id: 'old-thought', kind: 'thinking', text: 'conversa antiga aberta' },
    { id: 'old-next', kind: 'thinking', text: 'outro pensamento antigo' },
  ] });
  await act(async () => (container.querySelector('[aria-expanded]') as HTMLButtonElement).click());
  expect(container.textContent).toContain('conversa antiga aberta');
  await poll();
  await reply(1, { ...detail('one', ''), events: mode === 'vazia' ? [] : [
    { id: 'new-thought', kind: 'thinking', text: 'conversa substituta' },
    { id: 'new-next', kind: 'thinking', text: 'outro pensamento novo' },
  ] });
  expect(container.textContent).not.toContain('conversa antiga aberta');
  expect(container.textContent).not.toContain('outro pensamento antigo');
  expect(container.querySelector('[role="alert"]')).toBeNull();
  if (mode === 'vazia') {
    expect(container.querySelector('[aria-label="load-older"]')).toBeNull();
    expect(container.textContent).toContain(m.atividade_pensando());
  } else {
    expect(container.querySelector('[aria-expanded]')?.getAttribute('aria-expanded')).toBe('false');
    expect(container.textContent).not.toContain('conversa substituta');
    await act(async () => (container.querySelector('[aria-expanded]') as HTMLButtonElement).click());
    expect(container.textContent).toContain('conversa substituta');
    expect(container.textContent).toContain('outro pensamento novo');
  }
});

it('detalhe do subagente alinha conversa curta no topo da LegendList', async () => {
  const { container, render } = mount();
  await render();
  await reply(0, detail('one', 'conversa curta'));
  expect(container.textContent).toContain('conversa curta');
  expect(legendList.alignment.mock.lastCall?.[0]).toBe(false);
});

it.each([
  ['agente', { agentId: 'two' }, 'http://remote.test:8765/api/sessions/sess/subagents/two?events=200'],
  ['servidor', { server: servers.servers[1] }, 'http://active.test:8765/api/sessions/sess/subagents/one?events=200'],
] as const)('troca de %s limpa conversa e descarta resultado antigo', async (_kind, patch, nextUrl) => {
  const { container, render } = mount();
  await render();
  await reply(0, detail('one', 'conversa anterior'));
  expect(container.textContent).toContain('conversa anterior');
  await poll();
  expect(requests).toHaveLength(2);
  await render(patch);
  expect(container.textContent).not.toContain('conversa anterior');
  expect(container.textContent).toContain(m.comum_carregando());
  expect(requests[2].url).toBe(nextUrl);
  await reply(2, detail('agentId' in patch ? patch.agentId : 'one', 'conversa nova'));
  await reply(1, detail('one', 'resultado atrasado'));
  expect(container.textContent).toContain('conversa nova');
  expect(container.textContent).not.toContain('resultado atrasado');
  await poll();
  expect(requests).toHaveLength(4);
  expect(requests[3].url).toBe(nextUrl);
});

it('troca de servidor não publica conversa antiga antes dos efeitos', async () => {
  const { container, render, commits } = mount();
  await render();
  await reply(0, detail('one', 'conversa do servidor anterior', true));
  expect(container.textContent).toContain('conversa do servidor anterior');
  commits.length = 0;
  await render({ server: servers.servers[1] });
  expect(commits.join('')).not.toContain('conversa do servidor anterior');
  expect(container.textContent).toContain(m.comum_carregando());
});

it.each(['sem detalhe', 'com conversa', 'sem eventos'])('falha %s mostra erro e permite tentar novamente', async (mode) => {
  const { container, render } = mount();
  await render();
  let index = 0;
  if (mode !== 'sem detalhe') {
    const loaded = detail('one', 'última conversa');
    if (mode === 'sem eventos') loaded.events = [];
    await reply(index++, loaded);
    await poll();
  }
  for (let failure = 0; failure < 3; failure++) {
    await reply(index++, { detail: 'backend indisponível' }, 503);
    if (failure < 2) await poll();
  }
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.atividade_erro_vivo());
  if (mode === 'com conversa') expect(container.textContent).toContain('última conversa');
  await poll();
  expect(requests).toHaveLength(index);
  const retry = [...container.querySelectorAll('button')].find(button => button.textContent === m.sync_retry());
  expect(retry).toBeDefined();
  await act(async () => retry!.click());
  expect(requests).toHaveLength(index + 1);
  expect(requests[index].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=200');
  await reply(index++, detail('one', 'conversa recuperada'));
  expect(container.textContent).toContain('conversa recuperada');
  expect(container.querySelector('[role="alert"]')).toBeNull();
  await poll();
  expect(requests).toHaveLength(index + 1);
});

it.each([
  [{ state: 'finished' }, true, m.atividade_sub_concluido],
  [{ state: 'failed' }, true, m.subagente_card_falhou],
  [{ finished: true }, true, m.atividade_sub_concluido],
  [{ failed: true }, true, m.subagente_card_falhou],
  [{ finished: false }, false, m.atividade_sub_concluido],
] as [Partial<SubagentRun>, boolean, () => string][])('detalhe conclusivo %j para atualização apesar do pai %s', async (patch, parent, label) => {
  const { container, render } = mount();
  await render({ paiRodando: parent });
  await reply(0, { ...detail('one', 'conversa final'), ...patch });
  expect(container.textContent).toContain(label());
  expect(container.textContent).not.toContain(m.atividade_rodando());
  await poll();
  expect(requests).toHaveLength(1);
});

it.each([false, true])('events ausente em concluído mostra falha e permite reler com conversa anterior %s', async (hadConversation) => {
  const { container, render } = mount();
  await render();
  let index = 0;
  if (hadConversation) {
    await reply(index++, detail('one', 'conversa anterior preservada'));
    await poll();
  }
  const failedRead = { ...detail('one', ''), state: 'finished' as const, toolCalls: 3 };
  delete failedRead.events;
  await reply(index++, failedRead);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.atividade_erro_transcript());
  if (hadConversation) expect(container.textContent).toContain('conversa anterior preservada');
  await poll();
  expect(requests).toHaveLength(index);
  const retry = [...container.querySelectorAll('button')].find(button => button.textContent === m.sync_retry());
  expect(retry).toBeDefined();
  await act(async () => retry!.click());
  expect(requests[index].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=200');
  await reply(index++, { ...detail('one', 'conversa concluída recuperada'), state: 'finished', toolCalls: 3 });
  expect(container.textContent).toContain('conversa concluída recuperada');
  expect(container.querySelector('[role="alert"]')).toBeNull();
  await poll();
  expect(requests).toHaveLength(index);
});

it.each(['running', 'finished'] as const)('events vazio legítimo em %s não inventa falha de transcript', async (state) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', ''), events: [], state, toolCalls: 3 });
  expect(container.textContent).not.toContain(m.atividade_erro_transcript());
  expect(container.querySelector('[role="alert"]')).toBeNull();
  expect([...container.querySelectorAll('button')].some(button => button.textContent === m.sync_retry())).toBe(false);
});

it('404 interrompe atualização na primeira falha e permite tentar novamente', async () => {
  const { container, render } = mount();
  await render();
  await reply(0, { detail: 'subagente não existe' }, 404);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.atividade_erro_vivo());
  await poll();
  expect(requests).toHaveLength(1);
  const retry = [...container.querySelectorAll('button')].find(button => button.textContent === m.sync_retry());
  await act(async () => retry!.click());
  expect(requests).toHaveLength(2);
});

it.each([
  [{ ilegivel: true }, m.atividade_sub_ilegivel],
  [{ ilegivel: true, state: 'running' }, m.atividade_sub_ilegivel],
  [{ problema: 'erro_subagente_estado_indisponivel' }, m.erro_subagente_estado_indisponivel],
  [{ updatedAt: '2026-10-17T09:40:00Z' }, () => m.atividade_sem_noticia({ t: '20 min' })],
] as [Partial<SubagentRun>, () => string][])('registro sem eventos %j não aparece pensando', async (patch, label) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', ''), events: [], ...patch });
  expect(container.textContent).toContain(label());
  expect(container.textContent).not.toContain(m.atividade_pensando());
});

it('tradução pública do problema mostra estado indisponível e preserva código desconhecido', () => {
  const problemText = Reflect.get(subagentLive, 'subagentProblemText');
  expect(problemText).toBeTypeOf('function');
  expect(problemText('erro_subagente_estado_indisponivel')).toBe(m.erro_subagente_estado_indisponivel());
  expect(problemText('unknown-code')).toBe('unknown-code');
});

it.each([
  [{ state: 'failed' }, true, 'failed'],
  [{ state: 'finished' }, true, 'finished'],
  [{ state: 'running', failed: true, finished: true, problema: 'offline' }, false, 'running'],
  [{ failed: true }, false, 'failed'],
  [{ ilegivel: true }, true, 'unknown'],
  [{ problema: 'offline' }, false, 'unknown'],
  [{ finished: true }, true, 'finished'],
  [{ finished: false }, false, 'finished'],
  [{ updatedAt: '2026-10-16T10:00:00Z' }, true, 'running'],
  [{ updatedAt: '2026-10-17T10:00:01Z' }, null, 'running'],
  [{ updatedAt: '2026-10-16T10:00:00Z' }, null, 'silent'],
  [{ updatedAt: '', mtime: 1 }, null, 'silent'],
] as [Partial<SubagentRun>, boolean | null, string][])('estado do subagente %j com pai %s resulta em %s', (patch, parent, expected) => {
  const status = Reflect.get(subagentLive, 'subagentStatus');
  expect(status).toBeTypeOf('function');
  expect(status({ ...detail('one', 'conversa'), ...patch }, parent, Date.parse('2026-10-17T10:10:00Z'))).toBe(expected);
});

it.each([true, false])('tempo parte de startedAt com eventos %s e usa relógio local', async (withEvents) => {
  const { container, render } = mount();
  await render();
  const loaded = detail('one', 'mensagem recente');
  loaded.startedAt = '2026-10-17T09:55:00Z';
  loaded.events = withEvents ? [{ ...loaded.events![0], ts: Date.parse('2026-10-17T10:00:25Z') / 1000 }] : [];
  await reply(0, loaded);
  expect(container.textContent).toContain('5 min');
  expect(container.textContent?.split(m.native_working_line())).toHaveLength(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(30000); });
  expect(container.textContent).toContain('6 min');
});

it('paginação real de 200 para 400 preserva anteriores e atualização nova no polling de 2500ms', async () => {
  const { container, render } = mount();
  await render();
  expect(requests[0].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=200');
  const page = detail('one', 'recente');
  page.events = Array.from({ length: 200 }, (_, i) => ({ id: `n-${i + 200}`, kind: 'notice', text: `linha-${i + 200}` }));
  await reply(0, page);
  await act(async () => (container.querySelector('[aria-label="load-older"]') as HTMLElement).click());
  expect(requests[1].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=400');
  const older = { ...page, events: [
    ...Array.from({ length: 200 }, (_, i) => ({ id: `n-${i}`, kind: 'notice' as const, text: `histórico anterior-${i}` })),
    ...page.events,
  ] };
  await reply(1, older);
  expect(container.textContent).toContain('histórico anterior-0');
  await act(async () => { await vi.advanceTimersByTimeAsync(2499); });
  expect(requests).toHaveLength(2);
  await act(async () => { await vi.advanceTimersByTimeAsync(1); });
  expect(requests[2].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=400');
  await reply(2, { ...older, events: [
    ...older.events.slice(1).map(event => event.id === 'n-250' ? { ...event, text: 'conteúdo revisado' } : event),
    { id: 'n-400', kind: 'notice', text: 'evento recém-chegado' },
  ] });
  expect(container.textContent?.split('histórico anterior-0')).toHaveLength(2);
  expect(container.textContent?.split('conteúdo revisado')).toHaveLength(2);
  expect(container.textContent?.split('evento recém-chegado')).toHaveLength(2);
  expect(container.textContent).not.toContain('linha-250');
});

it('contrato nativo permite lista encolher em 320pt sem ferramentas empurrarem envio', async () => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa'), state: 'running', canMessage: true,
    tools: Array.from({ length: 20 }, (_, index) => ({ name: `tool-${index}`, count: 1 })), toolCalls: 20 });
  const list = container.querySelector('[aria-label="load-older"]')!;
  const chatBox = list.parentElement!.parentElement!.parentElement!;
  // Orçamento reduzido: metadados32 + indicador32 + envio44 + espaços36 + ferramentas96.
  const availableForList = 320 - 32 - 32 - 44 - 36 - 96;
  expect(Number.parseFloat(chatBox.style.minHeight)).toBeLessThanOrEqual(availableForList);
  expect(chatBox.style.flexShrink).toBe('1');
  const tools = container.querySelector('[data-native-scroll="true"]') as HTMLElement | null;
  expect(tools).not.toBeNull();
  expect(Number.parseFloat(tools!.style.maxHeight)).toBeLessThanOrEqual(96);
  expect(tools!.style.flexShrink).toBe('1');
  expect(tools!.contains(list)).toBe(false);
  const send = container.querySelector(`[aria-label="${m.native_send()}"]`)!;
  expect(tools!.contains(send)).toBe(false);
  const keyboard = container.querySelector('[data-keyboard-offset="true"]') as HTMLElement;
  expect(keyboard.dataset.keyboardBehavior).toBe('padding');
  expect(Number.parseFloat(keyboard.style.minHeight)).toBe(0);
});

it.each([true, false])('envio com eventos %s usa servidor da folha e aguarda entrega antes de limpar texto', async (withEvents) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa concluída'), events: withEvents ? detail('one', 'conversa concluída').events : [], state: 'finished', canMessage: true });
  const input = container.querySelector('textarea');
  expect(input).not.toBeNull();
  const send = container.querySelector(`[aria-label="${m.native_send()}"]`) as HTMLButtonElement;
  expect(send.disabled).toBe(true);
  await act(async () => {
    input!.value = '  mensagem para o agente  ';
    input!.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => { send.click(); send.click(); });
  expect(requests.filter(request => request.init?.method === 'POST')).toHaveLength(1);
  expect(requests[1].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one/message');
  expect(new Headers(requests[1].init?.headers).get('Authorization')).toBe('Bearer remote-token');
  expect(JSON.parse(String(requests[1].init?.body))).toEqual({ text: 'mensagem para o agente' });
  expect(input!.value).toBe('  mensagem para o agente  ');
  expect(send.disabled).toBe(true);
  await reply(1, { delivered: true });
  expect(input!.value).toBe('');
  expect(requests[2].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=200');
  await reply(2, { ...detail('one', 'conversa retomada'), state: 'running', canMessage: true });
  await poll();
  expect(requests).toHaveLength(4);
});

it.each([
  ['erro_subagente_mensagem_sem_plugin', m.erro_subagente_mensagem_sem_plugin],
  ['erro_subagente_mensagem_plugin_antigo', m.erro_subagente_mensagem_plugin_antigo],
  ['erro_subagente_mensagem_sem_terminal', m.erro_subagente_mensagem_sem_terminal],
  ['erro_subagente_mensagem_vinculo_desatualizado', m.erro_subagente_mensagem_vinculo_desatualizado],
  ['erro_subagente_estado_indisponivel', m.erro_subagente_estado_indisponivel],
  ['unknown-code', m.native_subagent_message_unavailable],
  [null, m.native_subagent_message_unavailable],
] as [string | null, () => string][])('mensagem bloqueada por %s mostra motivo traduzido sem compositor', async (code, label) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa'), canMessage: false, messageBlocked: code });
  expect(container.querySelector('textarea')).toBeNull();
  expect(container.textContent).toContain(label());
});

it('envio recusado preserva texto e mostra erro traduzido com motivo', async () => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa'), state: 'finished', canMessage: true });
  const input = container.querySelector('textarea')!;
  await act(async () => {
    input.value = 'mensagem recusada';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => (container.querySelector(`[aria-label="${m.native_send()}"]`) as HTMLElement).click());
  await reply(1, { detail: { code: 'erro_subagente_mensagem_recusada', msg: 'Claude Code refused the message', params: { reason: 'agent missing' } } }, 409);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.erro_subagente_mensagem_recusada({ reason: 'agent missing' }));
  expect(input.value).toBe('mensagem recusada');
  expect((container.querySelector(`[aria-label="${m.native_send()}"]`) as HTMLButtonElement).disabled).toBe(false);
  expect(requests).toHaveLength(2);
});

it('falha de envio permanece visível quando atualização bloqueia mensagem', async () => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa'), state: 'running', canMessage: true });
  const input = container.querySelector('textarea')!;
  await act(async () => {
    input.value = 'rascunho preservado';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => (container.querySelector(`[aria-label="${m.native_send()}"]`) as HTMLElement).click());
  await poll();
  await reply(2, { ...detail('one', 'conversa'), state: 'running', canMessage: false, messageBlocked: 'erro_subagente_mensagem_sem_plugin' });
  await reply(1, { detail: 'erro de envio' }, 503);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe('erro de envio');
  expect(container.textContent).toContain(m.erro_subagente_mensagem_sem_plugin());
  await poll();
  await reply(3, { ...detail('one', 'conversa'), state: 'running', canMessage: true });
  expect(container.querySelector('textarea')?.value).toBe('rascunho preservado');
});

it.each([
  ['erro_subagente_mensagem_sem_resposta', 504, 'the plugin did not confirm the delivery', m.erro_subagente_mensagem_sem_resposta],
  ['erro_subagente_inexistente', 409, 'subagent not found in this conversation', m.erro_subagente_inexistente],
] as [string, number, string, () => string][])('envio falha por %s preserva texto e traduz erro', async (code, status, msg, label) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa'), state: 'finished', canMessage: true });
  const input = container.querySelector('textarea')!;
  await act(async () => {
    input.value = 'mensagem sem confirmação';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => (container.querySelector(`[aria-label="${m.native_send()}"]`) as HTMLElement).click());
  await reply(1, { detail: { code, msg } }, status);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(label());
  expect(input.value).toBe('mensagem sem confirmação');
});

it.each([
  ['agente', { agentId: 'two' }, false],
  ['servidor', { server: servers.servers[1] }, false],
  ['agente', { agentId: 'two' }, true],
  ['servidor', { server: servers.servers[1] }, true],
] as const)('troca de %s durante envio para %j descarta resposta antiga com falha %s', async (_kind, patch, failed) => {
  const { container, render } = mount();
  await render();
  await reply(0, { ...detail('one', 'conversa antiga'), state: 'finished', canMessage: true });
  const input = container.querySelector('textarea')!;
  await act(async () => {
    input.value = 'mensagem antiga';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => (container.querySelector(`[aria-label="${m.native_send()}"]`) as HTMLElement).click());
  await render(patch);
  await reply(2, { ...detail('agentId' in patch ? patch.agentId : 'one', 'conversa nova'), state: 'finished', canMessage: true });
  const nextInput = container.querySelector('textarea')!;
  await act(async () => {
    nextInput.value = 'rascunho novo';
    nextInput.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await reply(1, failed ? { detail: 'falha antiga' } : { delivered: true }, failed ? 503 : 200);
  expect(nextInput.value).toBe('rascunho novo');
  expect(container.textContent).toContain('conversa nova');
  expect(requests).toHaveLength(3);
  expect(container.querySelector('[role="alert"]')).toBeNull();
});

it('carrega conversa e imagem no servidor da folha em vez do ativo', async () => {
  const { container, render } = mount();
  await render();
  expect(requests[0].url).toBe('http://remote.test:8765/api/sessions/sess/subagents/one?events=200');
  expect(new Headers(requests[0].init?.headers).get('Authorization')).toBe('Bearer remote-token');
  await reply(0, detail('one', 'conversa remota', true));
  expect(container.textContent).toContain('conversa remota');
  expect(container.querySelector('img')?.getAttribute('src')).toBe(
    'http://remote.test:8765/api/sessions/sess/transcript-image/message-one/0',
  );
});
