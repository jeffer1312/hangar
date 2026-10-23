// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { create } from 'zustand';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { composeWithContextRefs, configureApi, type MotivoFim, type Server } from '@hangar/core';
import type { DictationDraft } from '../stores/drafts';
import { useDeliveryMode } from './useQueueDelivery';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const h = vi.hoisted(() => ({
  disk: new Map<string, string>(),
  chat: null as any,
  listeners: [] as ((state: string) => void)[],
  server: { id: 'srv', label: 'server', baseUrl: 'https://example.test', token: 'token' } as Server,
  start: vi.fn(),
  onFinish: null as null | ((file: File, reason: MotivoFim, uri: string) => Promise<void>),
  transcribe: vi.fn(),
  navigation: { addListener: () => () => {}, isFocused: () => true },
}));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({
  getString: (key: string) => h.disk.get(key), set: (key: string, value: string) => h.disk.set(key, value),
  remove: (key: string) => h.disk.delete(key),
}) }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return { ...actual, AppState: { currentState: 'active', addEventListener: (_event: string, callback: (state: string) => void) => {
    h.listeners.push(callback);
    return { remove: () => { h.listeners = h.listeners.filter((current) => current !== callback); } };
  } } };
});
vi.mock('expo-router', () => ({ useNavigation: () => h.navigation, useRouter: () => ({ push: vi.fn() }) }));
vi.mock('../stores/chat', async (original) => ({
  ...await original<typeof import('../stores/chat')>(), chatStore: () => h.chat, filaCount: () => 0,
}));
vi.mock('../stores/sessions', () => {
  const value = { rows: [{ serverId: 'srv', name: 'sess', provider: 'claude', jsonl: '/sessions/current' }], byServerRecord: {} };
  return { useSessions: Object.assign((select: (state: typeof value) => unknown) => select(value), { getState: () => value }) };
});
vi.mock('../stores/servers', () => {
  const state = () => ({ servers: [h.server] });
  return { useServers: Object.assign((select: (value: ReturnType<typeof state>) => unknown) => select(state()), { getState: state }) };
});
vi.mock('../stores/newConversation', () => ({ readFirstInput: () => null, useNewConversation: { getState: () => ({ issues: {} }) },
  attachInsert: (_attach: unknown, path: string) => path, withAttach: (body: string, inserts: string) => `${inserts} ${body}` }));
vi.mock('../features/ditado/EstiloPill', () => ({ DictationStyleMenu: () => null, useDictationStyleLabel: () => 'raw' }));
vi.mock('../features/ditado/dictationOrganization', () => ({ useDictationOrganization: () => 'off' }));
vi.mock('../features/ditado/ditadoEstiloStore', () => ({ dictationStyleStore: () => ({ getState: () => ({ pronto: true, valor: 'cru' }) }) }));
vi.mock('../features/ditado/useDitado', () => ({ useDitado: ({ onFim }: { onFim: typeof h.onFinish }) => {
  h.onFinish = onFim;
  return { gravando: false, rms: 0, iniciar: h.start, parar: vi.fn() };
} }));
vi.mock('../features/ditado/dictationRun', () => ({
  DictationError: class extends Error {},
  runDictation: (...args: unknown[]) => h.transcribe(...args),
  uploadName: (path: string) => path.split('/').pop(),
  dictationAudio: (_server: Server, _name: string, audio: { uri: string; name: string } | null, path?: string) =>
    audio ?? (path ? { uri: `https://example.test/${path}`, name: path.split('/').pop() } : null),
}));
vi.mock('./draftAttachments', () => ({ retainDraftAttachment: async (attachment: unknown) => attachment, removeDraftAttachment: vi.fn() }));
vi.mock('./ComposerField', () => ({
  ComposerBox: ({ children }: { children: ReactNode }) => createElement('div', null, children),
  ComposerInput: ({ value }: { value: string }) => createElement('textarea', { value, readOnly: true }),
  ComposerActions: ({ mic, send }: { mic: { onPress: () => void; disabled: boolean }; send: { onPress: () => void; disabled: boolean } }) =>
    createElement('div', null,
      createElement('button', { 'aria-label': 'record', disabled: mic.disabled, onClick: mic.onPress }),
      createElement('button', { 'aria-label': 'send-fixture', disabled: send.disabled, onClick: send.onPress })),
}));
vi.mock('./AudioChip', () => ({ AudioChip: ({ name, onRemove }: { name: string; onRemove?: () => void }) =>
  createElement('div', { 'data-audio': name }, name, onRemove ? createElement('button', { 'aria-label': `remove-${name}`, onClick: onRemove }) : null) }));
vi.mock('./CommandSheet', () => ({ CommandSheet: () => null, useSessionCommands: () => ({ commands: [], error: '', retry: vi.fn() }) }));
vi.mock('./SessionSettings', () => ({ SessionSettingsButton: () => null }));
vi.mock('./SideQuestionSheet', () => ({ SideQuestionSheet: () => null }));
vi.mock('./SlashSuggest', () => ({ SlashSuggest: () => null }));
vi.mock('./QueuePanel', () => ({ QueuePanel: () => null }));
vi.mock('../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../ui/attachmentPicker', () => ({ readClipboard: vi.fn() }));
vi.mock('../ui/AttachSheet', () => ({ AttachSheet: () => null }));
vi.mock('../ui/AttachmentPreview', () => ({ AttachmentPreview: () => null }));
vi.mock('../ui/ContextRefTray', () => ({ ContextRefTray: () => null }));
vi.mock('./CiteReplyModal', () => ({ CiteReplyModal: () => null }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));

import { AppState } from 'react-native';
import { finishDictation, readDictation, readDraft, writeDictation, writeDraft } from '../stores/drafts';
import { overwriteGetLocale } from '../paraglide/runtime';
import * as m from '../paraglide/messages';
import { Composer } from './Composer';
import { addContextRef, contextRefsSnapshot, useContextRefs } from '../stores/contextRefs';

const roots: Root[] = [];
const props = { serverId: 'srv', name: 'sess', queuePanel: { entries: [], revision: 0 } as any };
async function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(Composer, props)));
  return { container, root };
}
const changeState = (state: string) => {
  Object.assign(AppState, { currentState: state });
  h.listeners.forEach((listener) => listener(state));
};
const record = async (container: HTMLElement, name = 'voice.m4a') => {
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="record"]')!.click());
  await act(async () => h.onFinish!(new File(['audio'], name, { type: 'audio/m4a' }), 'botao', `file:///${name}`));
};

beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  useDeliveryMode.setState({ mode: 'queued' });
  h.disk.clear(); h.listeners = []; vi.clearAllMocks();
  useContextRefs.setState({ drafts: {} });
  Object.assign(AppState, { currentState: 'active' });
  h.chat = { use: create(() => ({ pending: [], events: [], draftUpdate: 0, queueEdit: null, queueEditExit: null })), dropQueueEdit: vi.fn() };
  h.transcribe.mockImplementation(async (_server: Server, serverId: string, name: string, voice: DictationDraft,
    _source: unknown, opts: { check: () => void; isActive: () => boolean }) => {
    opts.check();
    const path = `/uploads/${voice.audio?.name ?? 'voice.m4a'}`;
    const result = { text: 'Texto ditado', raw: 'Texto ditado', issue: '', serverPath: path, applied: 'cru' };
    return { completed: finishDictation(serverId, name, voice.id, result, opts.isActive()), text: result.text,
      raw: result.raw, aviso: '', path, applied: 'cru' };
  });
});
afterEach(async () => {
  act(() => roots.splice(0).forEach((root) => root.unmount()));
  (await vi.importActual<typeof import('../stores/chat')>('../stores/chat'))._resetChatsForTests();
  vi.unstubAllGlobals();
});

it.each(['', 'Confira este trecho'])('limpa o campo após enviar citação com comentário e texto %j', async (typed) => {
  const real = await vi.importActual<typeof import('../stores/chat')>('../stores/chat');
  real._resetChatsForTests();
  h.chat = real.chatStore('srv', 'sess');
  configureApi({ getBaseUrl: () => h.server.baseUrl, getToken: () => h.server.token, onUnauthorized: () => {}, origin: null,
    createEventSource: () => ({ addEventListener() {}, removeEventListener() {}, close() {},
      onerror: null, onopen: null, readyState: 1 }) });
  let acknowledge!: () => void;
  const input = vi.fn((_url: string, _options?: RequestInit) => new Promise<Response>((resolve) => { acknowledge = () => resolve(new Response('{}')); }));
  vi.stubGlobal('fetch', (url: string, options?: RequestInit) => String(url).endsWith('/input')
    ? input(url, options) : Promise.resolve(new Response('{"available":false}')));
  const quote = { kind: 'quote' as const, source: 'reply-1', before: '', after: '', text: 'Trecho citado', comment: 'Corrija a anotação' };
  addContextRef('srv::sess', quote);
  writeDraft('srv', 'sess', { version: 1, text: typed, revision: 7, transcript: '/sessions/current', attachments: [], submission: null });
  const { container } = await mount();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="send-fixture"]')!.click());
  expect(input).toHaveBeenCalledTimes(1);
  expect(JSON.parse(input.mock.calls[0]![1]!.body as string).text).toBe(composeWithContextRefs([quote], typed));
  // O campo esvazia no envio; a citação só sai da bandeja com a confirmação.
  expect(container.querySelector('textarea')!.value).toBe('');
  expect(contextRefsSnapshot('srv::sess')).toHaveLength(1);
  await act(async () => acknowledge());
  expect(container.querySelector('textarea')!.value).toBe('');
  expect(readDraft('srv', 'sess')?.text ?? '').toBe('');
  expect(contextRefsSnapshot('srv::sess')).toEqual([]);
});

it('Enviar respeita Mandar direto sem ignorar a revisão do rascunho', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"available":true}')));
  useDeliveryMode.setState({ mode: 'direct' });
  writeDraft('srv', 'sess', { version: 1, text: 'usar azul', revision: 7, transcript: '/sessions/current', attachments: [], submission: null });
  h.chat.use.setState({ stateEvent: { state: 'working' }, events: [] });
  h.chat.send = vi.fn();
  const { container } = await mount();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="send-fixture"]')!.click());
  expect(h.chat.send).toHaveBeenCalledWith('usar azul', expect.any(Number), { kind: 'direct', terminalClaude: true });
});

it('Jev salvo e sem configuração avisa no envio que foi pela fila', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"available":false}')));
  useDeliveryMode.setState({ mode: 'jev' });
  writeDraft('srv', 'sess', { version: 1, text: 'usar azul', revision: 7, transcript: '/sessions/current', attachments: [], submission: null });
  h.chat.use.setState({ stateEvent: { state: 'working' }, events: [] });
  h.chat.send = vi.fn(async () => {});
  h.chat.showDeliveryFeedback = vi.fn();
  const { container } = await mount();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="send-fixture"]')!.click());
  expect(h.chat.send).toHaveBeenCalledWith('usar azul', expect.any(Number));
  expect(h.chat.showDeliveryFeedback).toHaveBeenCalledWith({ text: m.mobile_queue_jev_fallback({ reason: m.native_queue_jev_setup() }), warning: true });
});

it('turno que acaba durante o upload do anexo não manda direto para uma sessão parada', async () => {
  vi.stubGlobal('fetch', vi.fn(async (url: string) => {
    if (url === 'file:///a.png') { act(() => h.chat.use.setState({ stateEvent: { state: 'idle' } })); return new Response('png'); }
    if (String(url).includes('/upload')) return new Response('{"path":"/up/a.png"}');
    return new Response('{"available":true}');
  }));
  useDeliveryMode.setState({ mode: 'direct' });
  writeDraft('srv', 'sess', { version: 1, text: 'usar azul', revision: 7, transcript: '/sessions/current', submission: null,
    attachments: [{ uri: 'file:///a.png', name: 'a.png', mime: 'image/png', kind: 'image' }] });
  h.chat.use.setState({ stateEvent: { state: 'working' }, events: [] });
  h.chat.send = vi.fn();
  const { container } = await mount();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="send-fixture"]')!.click());
  expect(h.chat.send).toHaveBeenCalledTimes(1);
  expect(h.chat.send.mock.calls[0]).toHaveLength(2);
});

describe('Composer — ditado', () => {
  it('parar depois de voltar ao app transcreve e não rearma autoenvio', async () => {
    const { container } = await mount();
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="record"]')!.click());
    act(() => { changeState('background'); changeState('active'); });
    await act(async () => h.onFinish!(new File(['audio'], 'voice.m4a', { type: 'audio/m4a' }), 'silencio', 'file:///voice.m4a'));
    expect(readDraft('srv', 'sess')?.text).toBe('Texto ditado');
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(1);
    expect(container.textContent).not.toContain('interrompida');
    expect(container.textContent).not.toContain('Enviando em');
  });

  it('mantém vários chips ao gravar novamente e remover preserva o texto, inclusive após remontar', async () => {
    const { container, root } = await mount();
    await record(container, 'first.m4a');
    await record(container, 'second.m4a');
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(2);
    const text = readDraft('srv', 'sess')?.text;
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="remove-first.m4a"]')!.click());
    expect(readDraft('srv', 'sess')?.text).toBe(text);
    await act(async () => root.render(null));
    await act(async () => root.render(createElement(Composer, props)));
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(1);
    expect(container.textContent).toContain('second.m4a');
  });

  it('exibe a interrupção uma vez, acima da linha de áudio e ações', async () => {
    const issue = 'Gravação ou transcrição interrompida. O áudio foi guardado; você pode transcrever de novo.';
    const { container } = await mount();
    h.transcribe.mockRejectedValueOnce(new Error(issue));
    await record(container);
    // Mesmo erro armazenado que acompanha o retorno do POST malsucedido.
    const draft = readDraft('srv', 'sess')!;
    writeDictation('srv', 'sess', { version: 1, id: 'failed', audio: { uri: 'file:///voice.m4a', name: 'voice.m4a', mime: 'audio/m4a', kind: 'file' },
      transcript: draft.transcript, draftRevision: draft.revision, before: '', motivo: 'botao', status: 'failed', text: '', raw: '', issue });
    act(() => h.chat.use.setState({ draftUpdate: 1 }));
    expect(container.textContent?.split(issue)).toHaveLength(2);
    const notice = [...container.querySelectorAll('span')].find((node) => node.textContent === issue)!;
    const card = notice.parentElement!;
    expect(notice.getAttribute('role')).toBe('alert');
    expect(card.style.flexDirection).not.toBe('row');
    expect(notice.style.alignSelf).toBe('stretch');
    expect(card.children[1].querySelector('[data-audio]')).not.toBeNull();
    expect(card.children[1].textContent).toContain('Transcrever de novo');
  });

  it('recuperar o rascunho de outra conversa conserva também seus chips', async () => {
    writeDraft('srv', 'sess', { version: 1, text: 'Texto anterior', revision: 2, transcript: '/sessions/old',
      recordings: [{ id: 'old', serverPath: '/uploads/old.m4a' }], attachments: [], submission: null });
    const { container } = await mount();
    const recover = [...container.querySelectorAll('button')].find((button) => button.textContent === 'Recuperar')!;
    await act(async () => recover.click());
    expect(readDraft('srv', 'sess')).toMatchObject({ text: 'Texto anterior',
      recordings: [{ id: 'old', serverPath: '/uploads/old.m4a' }] });
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(1);
  });

  it('o X retira também o chip persistido quando a limpeza do ditado aplicado falhou', async () => {
    writeDraft('srv', 'sess', { version: 1, text: 'Texto já transcrito', revision: 2, transcript: '/sessions/current',
      dictationId: 'applied', recordings: [{ id: 'applied', serverPath: '/uploads/voice.m4a' }], attachments: [], submission: null });
    writeDictation('srv', 'sess', { version: 1, id: 'applied',
      audio: { uri: 'file:///voice.m4a', name: 'voice.m4a', mime: 'audio/m4a', kind: 'file' },
      serverPath: '/uploads/voice.m4a', transcript: '/sessions/current', draftRevision: 1, before: '', motivo: 'botao',
      status: 'applied', text: 'Texto já transcrito', raw: 'Texto já transcrito', issue: 'cleanup-failed' });
    const { container, root } = await mount();
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(1);
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="remove-voice.m4a"]')!.click());
    expect(readDictation('srv', 'sess')).toBeNull();
    expect(readDraft('srv', 'sess')).toMatchObject({ text: 'Texto já transcrito', revision: 2, recordings: [] });
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(0);
    await act(async () => root.render(null));
    await act(async () => root.render(createElement(Composer, props)));
    expect(container.querySelectorAll('[data-audio]')).toHaveLength(0);
    expect(container.querySelector('textarea')?.value).toBe('Texto já transcrito');
  });
});
