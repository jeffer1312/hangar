// @vitest-environment happy-dom
import { act, createElement, createRef } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { configureApi, type ChatEvent, type Server } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Entry = { key: string; uri: string; name: string; kind: string; origin?: string; eventId?: string; size?: number | null };
type ViewerProps = { entries: Entry[]; openKey: string | null; onOpenKey: (k: string) => void; onClose: () => void; onShowInChat?: (e: Entry) => void };
const h = vi.hoisted(() => ({ viewer: vi.fn(), toast: vi.fn() }));
const server: Server = { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 'tk' } as Server;

vi.mock('../features/attachments/gallery', () => ({
  GalleryViewer: (props: ViewerProps) => { h.viewer(props); return createElement('div', { 'data-open': props.openKey }); },
}));
vi.mock('../features/attachments/useAttachmentMeta', () => ({
  useGalleryMeta: () => ({ uploads: new Map([['a.png', { filename: 'a.png', size: 300, mtime: 1, expires_in_days: 2 }]]), cited: new Map() }),
}));
vi.mock('../stores/servers', () => ({ useServers: (select: (s: { servers: Server[] }) => unknown) => select({ servers: [server] }) }));
vi.mock('../paraglide/messages', () => ({ anexos_imagem_enviada: () => 'sent image', gallery_jump_not_found: () => 'not_found' }));
vi.mock('../ui/Toast', () => ({ toast: { erro: h.toast, ok: () => {} } }));

import { ConversationGallery, type ConversationGalleryHandle } from './ConversationGallery';

const events: ChatEvent[] = [
  { kind: 'user_msg', id: 'u1', text: 'look — 📎 imagem: /up/a.png', ts: 1 },
  { kind: 'assistant_msg', id: 'a1', text: 'done: /tmp/shot.png and https://cdn.test/clip.mp4', ts: 2 },
];

it('opens on any media of the conversation and "in chat" goes to the message', async () => {
  configureApi({ getBaseUrl: () => 'https://b.test', getToken: () => 'other', onUnauthorized: () => {},
    origin: null, createEventSource: () => { throw new Error('unexpected SSE'); } });
  const ref = createRef<ConversationGalleryHandle>();
  const onShowInChat = vi.fn(() => true);
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(ConversationGallery, { ref, events, serverId: 's1', sessionName: 'sess', onShowInChat })));
  expect(container.querySelector('[data-open]')).toBeNull();

  expect(ref.current!.open('nope#0')).toBe(false);
  let opened = false;
  await act(async () => { opened = ref.current!.open('a1#0'); });
  await act(async () => {});
  expect(opened).toBe(true);
  const props = h.viewer.mock.lastCall![0] as ViewerProps;
  expect(props.openKey).toBe('a1#0');
  expect(props.entries.map((e) => [e.key, e.kind, e.origin, e.eventId])).toEqual([
    ['u1#0', 'image', 'user', 'u1'],
    ['a1#0', 'image', 'agent', 'a1'],
    ['a1#1', 'video', 'agent', 'a1'],
  ]);
  expect(props.entries[0]).toMatchObject({ uri: 'https://a.test/api/sessions/sess/uploads/a.png', size: 300 });
  expect(props.entries[2]).toMatchObject({ uri: 'https://cdn.test/clip.mp4', headers: undefined });

  await act(async () => props.onShowInChat!(props.entries[0]));
  expect(onShowInChat).toHaveBeenCalledExactlyOnceWith('u1');
  expect(container.querySelector('[data-open]')).toBeNull();
  expect(h.toast).not.toHaveBeenCalled();
  act(() => root.unmount());
});

it('"in chat" for a message the list does not have keeps the viewer open and says why', async () => {
  const ref = createRef<ConversationGalleryHandle>();
  const onShowInChat = vi.fn(() => false);
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(ConversationGallery, { ref, events, serverId: 's1', sessionName: 'sess', onShowInChat })));
  await act(async () => { ref.current!.open('a1#0'); });
  await act(async () => {});
  const props = h.viewer.mock.lastCall![0] as ViewerProps;
  await act(async () => props.onShowInChat!(props.entries[1]));
  expect(onShowInChat).toHaveBeenCalledExactlyOnceWith('a1');
  expect(h.toast).toHaveBeenCalledExactlyOnceWith('not_found');
  expect(container.querySelector('[data-open]')?.getAttribute('data-open')).toBe('a1#0');
  act(() => root.unmount());
});
