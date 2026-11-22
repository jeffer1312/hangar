// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, expect, it, vi } from 'vitest';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type StatusListener = (e: { status: string; error?: { message: string } }) => void;
const v = vi.hoisted(() => ({
  created: [] as {
    source: { uri: string; headers?: Record<string, string> }; release: ReturnType<typeof vi.fn>; finish: (t: unknown[]) => void;
    generateThumbnailsAsync: ReturnType<typeof vi.fn>; emit: (e: { status: string }) => void;
  }[],
  platform: { OS: 'android' as 'android' | 'ios' },
}));
vi.mock('react-native', () => ({ Platform: v.platform }));
vi.mock('expo-video', () => ({
  createVideoPlayer: (source: { uri: string; headers?: Record<string, string> }) => {
    let finish!: (t: unknown[]) => void;
    const done = new Promise<unknown[]>((r) => { finish = r; });
    const listeners = new Set<StatusListener>();
    const p = {
      source, release: vi.fn(), finish, status: 'loading', generateThumbnailsAsync: vi.fn(() => done),
      addListener: (_event: 'statusChange', l: StatusListener) => { listeners.add(l); return { remove: () => listeners.delete(l) }; },
      emit: (e: { status: string }) => listeners.forEach((l) => l(e)),
    };
    v.created.push(p);
    return p;
  },
}));

import { useVideoThumb } from './videoThumb';

const seen: Record<string, unknown> = {};
function Probe({ uri }: { uri: string }) {
  seen[uri] = useVideoThumb(uri, { Authorization: 'Bearer t' });
  return null;
}

const roots: Root[] = [];
afterEach(() => { act(() => roots.splice(0).forEach((r) => r.unmount())); });
async function mount(uri: string) {
  const root = createRoot(document.createElement('div'));
  roots.push(root);
  await act(async () => root.render(createElement(Probe, { uri })));
  return root;
}
// A fila anda em promessas encadeadas: espera elas assentarem.
const settle = () => act(async () => { await new Promise((r) => setTimeout(r, 0)); });

it('gera um quadro por vez, com o token, libera o player e reaproveita o quadro', async () => {
  await mount('https://srv/a.mp4');
  await mount('https://srv/b.mp4');
  // A segunda espera a primeira terminar.
  expect(v.created).toHaveLength(1);
  expect(v.created[0].source).toEqual({ uri: 'https://srv/a.mp4', headers: { Authorization: 'Bearer t' } });
  const frame = { width: 480, height: 270 };
  v.created[0].finish([frame]);
  await settle();
  expect(seen['https://srv/a.mp4']).toBe(frame);
  expect(v.created[0].release).toHaveBeenCalledTimes(1);
  expect(v.created).toHaveLength(2);
  v.created[1].finish([]);
  await settle();
  expect(seen['https://srv/b.mp4']).toBeNull();
  // Montar de novo o mesmo vídeo não abre outro player.
  await mount('https://srv/a.mp4');
  expect(seen['https://srv/a.mp4']).toBe(frame);
  expect(v.created).toHaveLength(2);
});

it('vídeo que saiu da tela antes da vez dele não gasta rede', async () => {
  const before = v.created.length;
  await mount('https://srv/c.mp4');
  const leaving = await mount('https://srv/d.mp4');
  roots.splice(roots.indexOf(leaving), 1);
  act(() => leaving.unmount());
  v.created[before].finish([]);
  await settle();
  expect(v.created).toHaveLength(before + 1);
});

it('vídeo sem quadro (prazo, rede) tenta de novo na próxima montagem; o quadro que veio fica guardado', async () => {
  const before = v.created.length;
  const first = await mount('https://srv/e.mp4');
  v.created[before].finish([]);
  await settle();
  expect(seen['https://srv/e.mp4']).toBeNull();
  roots.splice(roots.indexOf(first), 1);
  act(() => first.unmount());
  await mount('https://srv/e.mp4');
  expect(v.created).toHaveLength(before + 2);
  const frame = { width: 480, height: 270 };
  v.created[before + 1].finish([frame]);
  await settle();
  expect(seen['https://srv/e.mp4']).toBe(frame);
  await mount('https://srv/e.mp4');
  expect(v.created).toHaveLength(before + 2);
});

it('no iOS espera a fonte ficar pronta antes de pedir o quadro', async () => {
  v.platform.OS = 'ios';
  try {
    const before = v.created.length;
    await mount('https://srv/ios.mp4');
    const p = v.created[before];
    await settle();
    expect(p.generateThumbnailsAsync).not.toHaveBeenCalled();
    p.emit({ status: 'readyToPlay' });
    await settle();
    expect(p.generateThumbnailsAsync).toHaveBeenCalledTimes(1);
    const frame = { width: 480, height: 270 };
    p.finish([frame]);
    await settle();
    expect(seen['https://srv/ios.mp4']).toBe(frame);
  } finally {
    v.platform.OS = 'android';
  }
});

it('trocar de vídeo não mostra o quadro do anterior enquanto o novo não chega', async () => {
  let shown: unknown = 'unset';
  function Swap({ uri }: { uri: string }) {
    shown = useVideoThumb(uri);
    return null;
  }
  const before = v.created.length;
  const root = createRoot(document.createElement('div'));
  roots.push(root);
  await act(async () => root.render(createElement(Swap, { uri: 'https://srv/swap-a.mp4' })));
  const frameA = { width: 1, height: 1 };
  v.created[before].finish([frameA]);
  await settle();
  expect(shown).toBe(frameA);
  await act(async () => root.render(createElement(Swap, { uri: 'https://srv/swap-b.mp4' })));
  expect(shown).toBeNull();
  const frameB = { width: 2, height: 2 };
  v.created[before + 1].finish([frameB]);
  await settle();
  expect(shown).toBe(frameB);
  // Voltar ao primeiro reaproveita o quadro dele, sem outro player.
  await act(async () => root.render(createElement(Swap, { uri: 'https://srv/swap-a.mp4' })));
  expect(shown).toBe(frameA);
  expect(v.created).toHaveLength(before + 2);
});

it('uma rajada acima do teto é podada quando as gerações concluem; as que esperam ficam', async () => {
  const before = v.created.length;
  const uris = Array.from({ length: 45 }, (_, i) => `https://srv/burst-${i}.mp4`);
  for (const uri of uris) await mount(uri);
  // Todas esperando: nenhuma sai antes de concluir.
  for (let i = 0; i < uris.length; i++) {
    v.created[before + i].finish([{ width: i, height: i }]);
    await settle();
  }
  expect(v.created).toHaveLength(before + uris.length);
  // A mais recente ficou guardada; a mais antiga saiu na poda e pede outro player.
  await mount(uris[uris.length - 1]);
  expect(v.created).toHaveLength(before + uris.length);
  await mount(uris[0]);
  expect(v.created).toHaveLength(before + uris.length + 1);
  v.created[before + uris.length].finish([]);
  await settle();
});
