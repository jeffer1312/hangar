// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const deliver = vi.hoisted(() => vi.fn());
vi.mock('../features/attachments/mediaCache', () => ({ deliverFile: deliver }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../paraglide/messages', () => ({
  file_card_open: () => 'open',
  file_card_download: () => 'download',
  file_card_downloading: () => 'downloading',
  file_card_saved: () => 'saved',
  file_card_opened: () => 'opened',
  file_card_empty: () => 'empty',
  file_card_missing: () => 'missing',
  file_card_failed: () => 'failed',
  file_card_retry: () => 'retry',
  file_card_no_app: () => 'no-app',
}));

import { FileCard } from './FileCard';

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  deliver.mockReset();
});

async function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(FileCard, {
    uri: 'https://srv/api/sessions/s/file?path=%2Ftmp%2Frel.docx', headers: { Authorization: 'Bearer t' }, name: 'rel.docx',
  })));
  return container;
}
const press = (c: HTMLElement) => act(async () => { c.querySelector('button')!.click(); });

describe('FileCard', () => {
  it('mostra nome e extensão e baixa pela ação do sistema', async () => {
    let done!: (v: 'opened') => void;
    deliver.mockReturnValue(new Promise((r) => { done = r; }));
    const c = await mount();
    expect(c.textContent).toContain('rel.docx');
    expect(c.textContent).toContain('DOCX');
    await press(c);
    expect(deliver).toHaveBeenCalledWith(expect.stringContaining('rel.docx'), { Authorization: 'Bearer t' }, 'rel.docx');
    expect(c.textContent).toContain('downloading');
    expect(c.querySelector('button')!.disabled).toBe(true);
    // Android entregou ao app que lê o tipo: diz que abriu, não que baixou.
    await act(async () => done('opened'));
    expect(c.textContent).toContain('opened');
    expect(c.textContent).not.toContain('saved');
    expect(c.querySelector('button')!.getAttribute('aria-label')).toBe('open rel.docx');
  });

  it('sem app para abrir avisa sem oferecer tentar de novo', async () => {
    deliver.mockResolvedValue('no-app');
    const c = await mount();
    await press(c);
    expect(c.textContent).toContain('no-app');
    expect(c.textContent).not.toContain('retry');
  });

  it.each(['cancelled', 'shared'])('folha do sistema (%s) não marca como baixado', async (r) => {
    deliver.mockResolvedValue(r);
    const c = await mount();
    await press(c);
    expect(c.textContent).not.toContain('opened');
    expect(c.textContent).not.toContain('downloading');
  });

  it('arquivo vazio avisa sem oferecer tentar de novo', async () => {
    deliver.mockResolvedValue('empty');
    const c = await mount();
    await press(c);
    expect(c.textContent).toContain('empty');
    expect(c.textContent).not.toContain('retry');
  });

  it('404 vira "não encontrado"; outra falha vira "não deu para baixar"', async () => {
    deliver.mockRejectedValueOnce(new Error('response has status: 404'));
    const c = await mount();
    await press(c);
    expect(c.textContent).toContain('missing');
    deliver.mockRejectedValueOnce(new Error('offline'));
    await press(c);
    expect(c.textContent).toContain('failed');
    expect(deliver).toHaveBeenCalledTimes(2);
  });
});
