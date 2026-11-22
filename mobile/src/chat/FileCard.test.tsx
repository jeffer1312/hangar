// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { DownloadOptions } from '../features/attachments/mediaCache';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const deliver = vi.hoisted(() => vi.fn());
const share = vi.hoisted(() => vi.fn());
vi.mock('../features/attachments/mediaCache', () => ({ deliverFile: deliver, shareFile: share }));
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
  file_card_progress: ({ pct, size }: { pct: string; size: string }) => `progress ${pct}% of ${size}`,
  file_card_progress_unknown: ({ size }: { size: string }) => `progress ${size}`,
  file_card_cancel: ({ nome }: { nome: string }) => `cancel ${nome}`,
  attachment_share: ({ nome }: { nome: string }) => `share ${nome}`,
}));

import { FileCard } from './FileCard';

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  deliver.mockReset();
  share.mockReset();
  vi.useRealTimers();
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
const btn = (c: HTMLElement, label: string) => c.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
const press = (c: HTMLElement, label = 'open rel.docx') => act(async () => { btn(c, label)!.click(); });

// Download que o teste solta à mão, com o progresso que o componente recebeu.
function pending<T>() {
  let done!: (v: T) => void;
  let fail!: (e: unknown) => void;
  let opts!: DownloadOptions;
  const fn = vi.fn((_u: string, _h: unknown, _n: string, o: DownloadOptions) => {
    opts = o;
    return new Promise<T>((res, rej) => { done = res; fail = rej; });
  });
  return { fn, done: (v: T) => done(v), fail: (e: unknown) => fail(e), opts: () => opts };
}

describe('FileCard', () => {
  it('mostra nome e extensão e baixa pela ação do sistema', async () => {
    const p = pending<'opened'>();
    deliver.mockImplementation(p.fn);
    const c = await mount();
    expect(c.textContent).toContain('rel.docx');
    expect(c.textContent).toContain('DOCX');
    await press(c);
    expect(deliver).toHaveBeenCalledWith(expect.stringContaining('rel.docx'), { Authorization: 'Bearer t' }, 'rel.docx',
      expect.objectContaining({ signal: expect.any(AbortSignal) }));
    expect(c.textContent).toContain('downloading');
    // Durante o download o botão vira o anel de cancelar.
    expect(btn(c, 'open rel.docx')).toBeNull();
    expect(btn(c, 'cancel rel.docx')).not.toBeNull();
    // Android entregou ao app que lê o tipo: diz que abriu, não que baixou.
    await act(async () => p.done('opened'));
    expect(c.textContent).toContain('opened');
    expect(c.textContent).not.toContain('saved');
    expect(btn(c, 'open rel.docx')).not.toBeNull();
  });

  it('mostra a porcentagem e o tamanho; sem tamanho, só o que já chegou', async () => {
    const p = pending<'opened'>();
    deliver.mockImplementation(p.fn);
    const c = await mount();
    await press(c);
    await act(async () => p.opts().onProgress!({ written: 10 * 1024 * 1024, total: 24 * 1024 * 1024 }));
    expect(c.textContent).toContain('progress 41% of 24 MB');
    // Atualizações mais rápidas que um quadro são descartadas; a seguinte, um quadro depois, aparece.
    await act(async () => { await new Promise((r) => setTimeout(r, 120)); });
    await act(async () => p.opts().onProgress!({ written: 3 * 1024 * 1024, total: null }));
    expect(c.textContent).toContain('progress 3 MB');
    await act(async () => p.done('opened'));
  });

  it('tocar no anel cancela e volta ao começo sem erro', async () => {
    const p = pending<'opened'>();
    deliver.mockImplementation(p.fn);
    const c = await mount();
    await press(c);
    expect(p.opts().signal!.aborted).toBe(false);
    await press(c, 'cancel rel.docx');
    expect(p.opts().signal!.aborted).toBe(true);
    const abort = new Error('aborted');
    abort.name = 'AbortError';
    await act(async () => p.fail(abort));
    expect(c.textContent).not.toContain('failed');
    expect(c.textContent).not.toContain('downloading');
    expect(btn(c, 'open rel.docx')).not.toBeNull();
  });

  it('Android tem compartilhar ao lado, e ele vale mesmo quando nenhum app abre', async () => {
    deliver.mockResolvedValue('no-app');
    share.mockImplementation(async (_u: string, _h: unknown, _n: string, o: DownloadOptions) => {
      o.onProgress?.({ written: 2048, total: 2048 });
    });
    const c = await mount();
    await press(c);
    expect(c.textContent).toContain('no-app');
    expect(c.textContent).not.toContain('retry');
    await press(c, 'share rel.docx');
    expect(share).toHaveBeenCalledWith(expect.stringContaining('rel.docx'), { Authorization: 'Bearer t' }, 'rel.docx', expect.anything());
    // A folha não diz se a pessoa compartilhou ou fechou: nada de "baixado", e o aviso anterior sai.
    expect(c.textContent).not.toContain('downloaded');
    expect(c.textContent).not.toContain('saved');
    expect(c.textContent).not.toContain('no-app');
    expect(btn(c, 'open rel.docx')).not.toBeNull();
  });

  it('folha do sistema (shared) volta ao começo, sem dizer que baixou', async () => {
    deliver.mockImplementation(async (_u: string, _h: unknown, _n: string, o: DownloadOptions) => {
      o.onProgress?.({ written: 5 * 1024 * 1024, total: 5 * 1024 * 1024 });
      return 'shared';
    });
    const c = await mount();
    await press(c);
    expect(c.textContent).not.toContain('downloaded');
    expect(c.textContent).not.toContain('saved');
    expect(c.textContent).not.toContain('opened');
    expect(c.textContent).not.toContain('progress');
    expect(c.textContent).toContain('DOCX');
    expect(btn(c, 'open rel.docx')).not.toBeNull();
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
    await press(c, 'retry rel.docx');
    expect(c.textContent).toContain('failed');
    expect(deliver).toHaveBeenCalledTimes(2);
  });
});
