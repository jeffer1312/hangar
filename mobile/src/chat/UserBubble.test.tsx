// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it, vi } from 'vitest';
import * as m from '../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('react-native', async (original) => ({
  ...await original<object>(),
  Pressable: ({ onLongPress, children }: { onLongPress?: () => void; children: ReactNode }) =>
    createElement('button', { onClick: onLongPress }, children),
}));
vi.mock('../stores/servers', () => ({ useServers: (select: (state: unknown) => unknown) => select({ servers: [
  { id: 'route', baseUrl: 'http://route.test', token: 'fixture' },
] }) }));
vi.mock('./ImageThumb', () => ({ ImageThumb: ({ items }: { items: { uri: string; kind: string; thumbUri?: string }[] }) =>
  createElement('span', {
    'data-images': items.map((item) => item.uri).join(','),
    'data-kinds': items.map((item) => item.kind).join(','),
    'data-thumbs': items.map((item) => item.thumbUri ?? '').join(','),
  }) }));
vi.mock('./FileCard', () => ({ FileCard: ({ name, uri }: { name: string; uri: string }) =>
  createElement('a', { 'data-file': uri }, name) }));
vi.mock('./BubbleActions', () => ({ BubbleActions: ({ text }: { text: string }) =>
  createElement('output', null, text) }));
vi.mock('./AssistantBubble', () => ({ mkMarkdownStyle: () => ({ paragraph: {} }), mkMutedMarkdownStyle: () => ({ paragraph: {} }) }));
vi.mock('react-native-enriched-markdown', () => ({
  EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('pre', null, markdown),
}));

import { UserBubble } from './UserBubble';

async function render(text: string) {
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(UserBubble, { text, serverId: 'route', sessionName: 'sess' })));
  return { container, root };
}

describe('recados na bolha', () => {
  it.each([
    ['[de: agente] corpo', () => m.board_peer_de({ n: 'agente' }), () => m.peer_message_sender({ name: 'agente' })],
    ['[grupo: agente] corpo', () => m.board_peer_grupo({ n: 'agente' }), () => m.peer_message_group_sender({ name: 'agente' })],
    ['[de fora: maquina::agente] corpo', () => m.board_peer_fora({ n: 'maquina::agente' }), () => m.peer_message_external_sender({ name: 'maquina::agente' })],
    ['[painel: tarefa] corpo', () => m.board_peer_painel({ n: 'tarefa' }), () => m.peer_message_panel_sender({ name: 'tarefa' })],
  ] as const)('separa remetente e corpo: %s', async (raw, label, accessibleLabel) => {
    const { container, root } = await render(raw);
    expect(container.textContent).toBe(`${label()}corpo`);
    expect(container.querySelector('[aria-label]')?.getAttribute('aria-label')).toBe(accessibleLabel());
    act(() => container.querySelector('button')!.click());
    expect(container.querySelector('output')!.textContent).toBe('corpo');
    expect(raw).toContain('] corpo');
    act(() => root.unmount());
  });

  it.each(['[de: agente]', '[grupo: agente]   ', '[de fora: maquina::agente]', '[painel: tarefa]'])('recado sem corpo mostra aviso e copia o original: %s', async (raw) => {
    const { container, root } = await render(raw);
    expect(container.textContent).toContain(m.peer_message_empty());
    act(() => container.querySelector('button')!.click());
    expect(container.querySelector('output')!.textContent).toBe(raw);
    act(() => root.unmount());
  });

  it('conserva mensagem normal e prefixo inválido', async () => {
    const raw = '[grupo fora: agente] corpo';
    const { container, root } = await render(raw);
    expect(container.textContent).toBe(raw);
    act(() => root.unmount());
  });

  it('imagem e arquivo juntos: foto em miniatura, arquivo em card', async () => {
    const { container, root } = await render('veja — 📎 imagem: /up/1-a.png 📎 arquivo: /up/2-b.txt');
    expect(container.querySelector('[data-images]')!.getAttribute('data-images')).toMatch(/1-a\.png$/);
    expect(container.querySelector('[data-file]')!.getAttribute('data-file')).toMatch(/uploads\/2-b\.txt$/);
    expect(container.textContent).toBe('veja2-b.txt');
    act(() => root.unmount());
  });

  it('limpa o remetente da legenda sem perder a imagem do servidor da rota', async () => {
    const { container, root } = await render('[de: agente] corpo — 📎 imagem: /up/a.png');
    expect(container.textContent).toBe(`${m.board_peer_de({ n: 'agente' })}corpo`);
    expect(container.querySelector('[data-images]')!.getAttribute('data-images')).toContain('http://route.test/');
    expect(container.querySelector('[data-images]')!.getAttribute('data-images')).toContain('a.png');
    act(() => root.unmount());
  });

  it('vídeo anexado vira mídia que toca no visor, não card de arquivo; a foto pede miniatura reduzida', async () => {
    const { container, root } = await render('olha — 📎 arquivo: /up/1-clip.mov 📎 imagem: /up/2-quadro.jpg');
    const thumb = container.querySelector('[data-images]')!;
    expect(thumb.getAttribute('data-kinds')).toBe('video,image');
    expect(thumb.getAttribute('data-images')).toBe(
      'http://route.test/api/sessions/sess/uploads/1-clip.mov,http://route.test/api/sessions/sess/uploads/2-quadro.jpg');
    // 96 pt × escala 3 do mock.
    expect(thumb.getAttribute('data-thumbs')).toBe(',http://route.test/api/sessions/sess/uploads/2-quadro.jpg?w=288');
    expect(container.querySelector('[data-file]')).toBeNull();
    act(() => root.unmount());
  });
});
