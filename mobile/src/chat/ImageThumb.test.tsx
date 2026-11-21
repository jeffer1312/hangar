// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// O `src` do <img> é o endereço que a miniatura está pedindo.
vi.mock('expo-image', () => ({
  Image: ({ source, onError }: { source: { uri: string }; onError?: () => void }) =>
    createElement('img', { src: source.uri, onError }),
}));
vi.mock('../features/attachments/LightboxModal', () => ({ LightboxModal: () => null }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../paraglide/messages', () => ({
  anexos_nao_carregou: ({ nome }: { nome: string }) => `falhou:${nome}`,
  anexos_ver: ({ n }: { n: string }) => `ver:${n}`,
  anexos_tocar: ({ nome }: { nome: string }) => `tocar:${nome}`,
}));

import { ImageThumb } from './ImageThumb';

const roots: Root[] = [];
afterEach(() => { act(() => roots.splice(0).forEach((root) => root.unmount())); });

async function render(item: { uri: string; thumbUri?: string }) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(ImageThumb, {
    items: [{ ...item, name: 'foto.png', kind: 'image' as const }], index: 0, style: {},
  })));
  return container;
}

const fail = async (container: HTMLElement) => {
  await act(async () => { container.querySelector('img')!.dispatchEvent(new Event('error')); });
};

it('miniatura reduzida que falha tenta o original uma vez antes de mostrar o erro', async () => {
  const container = await render({ uri: 'https://a.test/foto.png', thumbUri: 'https://a.test/foto.png?w=320' });
  expect(container.querySelector('img')!.getAttribute('src')).toBe('https://a.test/foto.png?w=320');
  await fail(container);
  expect(container.querySelector('img')!.getAttribute('src')).toBe('https://a.test/foto.png');
  expect(container.textContent).not.toContain('falhou:foto.png');
  await fail(container);
  expect(container.querySelector('img')).toBeNull();
  expect(container.textContent).toContain('falhou:foto.png');
});

it('sem miniatura reduzida, a falha do original já mostra o erro', async () => {
  const container = await render({ uri: 'https://a.test/foto.png' });
  await fail(container);
  expect(container.querySelector('img')).toBeNull();
  expect(container.textContent).toContain('falhou:foto.png');
});
