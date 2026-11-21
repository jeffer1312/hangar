// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Alert } from 'react-native';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const h = vi.hoisted(() => ({ usage: vi.fn(), clear: vi.fn(), ok: vi.fn(), erro: vi.fn() }));
vi.mock('./mediaCache', () => ({ cacheUsage: h.usage, clearMediaCache: h.clear, MEDIA_CACHE_LIMIT: 1024 * 1024 * 1024 }));
vi.mock('./videoCache', () => ({ VIDEO_CACHE_LIMIT: 512 * 1024 * 1024 }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: h.ok, erro: h.erro } }));
vi.mock('../config/ServerConfigParts', async () => {
  const React = await import('react');
  return {
    BlockHead: ({ title, help }: { title: string; help?: string }) => React.createElement('h3', null, `${title} ${help ?? ''}`),
    Box: ({ children }: { children: ReactNode }) => React.createElement('section', null, children),
    Muted: ({ children }: { children: ReactNode }) => React.createElement('p', null, children),
  };
});
vi.mock('../config/PageHeader', async () => {
  const React = await import('react');
  return {
    Pill: ({ label, onPress, disabled }: { label: string; onPress: () => void; disabled?: boolean }) =>
      React.createElement('button', { 'aria-label': label, onClick: onPress, disabled }, label),
  };
});
vi.mock('../../paraglide/messages', () => ({
  media_cache_title: () => 'device',
  media_cache_help: ({ limit }: { limit: string }) => `help ${limit}`,
  media_cache_of_limit: ({ limit }: { limit: string }) => `of ${limit}`,
  media_cache_images: () => 'images',
  media_cache_videos: () => 'videos',
  media_cache_audio: () => 'audio',
  media_cache_documents: () => 'documents',
  media_cache_playback: () => 'watched',
  media_cache_playback_help: ({ limit }: { limit: string }) => `up to ${limit}`,
  media_cache_empty: () => 'empty',
  media_cache_clear: () => 'clear',
  media_cache_clearing: () => 'clearing',
  media_cache_clear_title: () => 'clear?',
  media_cache_clear_body: () => 'body',
  media_cache_cleared: () => 'cleared',
  media_cache_clear_video_failed: () => 'video-left',
  media_cache_clear_failed: ({ reason }: { reason: string }) => `clear-failed ${reason}`,
  media_cache_read_failed: ({ reason }: { reason: string }) => `read-failed ${reason}`,
  native_server_retry: () => 'retry',
  comum_cancelar: () => 'cancel',
  comum_carregando: () => 'loading',
}));

import { MediaCacheSection } from './MediaCacheSection';

const MB = 1024 * 1024;
const usage = (byKind: Record<string, number>, video: number | null = 0) => ({
  byKind, files: 4, total: Object.values(byKind).reduce((a, b) => a + b, 0), video,
});
// Confirma o Alert apertando o botão destrutivo, como a pessoa faria.
type AlertButton = { text?: string; style?: string; onPress?: () => void };
let alertSpy: ReturnType<typeof vi.spyOn>;

const roots: Root[] = [];
beforeEach(() => {
  h.usage.mockReset(); h.clear.mockReset(); h.ok.mockReset(); h.erro.mockReset();
  alertSpy = vi.spyOn(Alert, 'alert').mockImplementation((_t: unknown, _b: unknown, buttons?: unknown) => {
    (buttons as AlertButton[]).find((b) => b.style === 'destructive')?.onPress?.();
  });
});
afterEach(() => { act(() => roots.splice(0).forEach((r) => r.unmount())); alertSpy.mockRestore(); });

async function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(MediaCacheSection)));
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  return container;
}

it('mostra o total, o espaço por tipo e o cache dos vídeos assistidos', async () => {
  h.usage.mockResolvedValue(usage({ image: 10 * MB, video: 200 * MB, audio: 3 * MB, document: 1 * MB }, 50 * MB));
  const c = await mount();
  expect(c.textContent).toContain('214 MB');
  expect(c.textContent).toContain('of 1 GB');
  expect(c.textContent).toContain('images10 MB');
  expect(c.textContent).toContain('videos200 MB');
  expect(c.textContent).toContain('watched');
  expect(c.textContent).toContain('50 MB');
  expect(c.querySelector<HTMLButtonElement>('[aria-label="clear"]')!.disabled).toBe(false);
});

it('vazio diz que não há nada e não oferece limpar', async () => {
  h.usage.mockResolvedValue(usage({ image: 0, video: 0, audio: 0, document: 0 }, 0));
  const c = await mount();
  expect(c.textContent).toContain('empty');
  expect(c.querySelector<HTMLButtonElement>('[aria-label="clear"]')!.disabled).toBe(true);
});

it('limpar confirma, apaga, avisa e relê', async () => {
  h.usage.mockResolvedValueOnce(usage({ image: 10 * MB, video: 0, audio: 0, document: 0 }))
    .mockResolvedValue(usage({ image: 0, video: 0, audio: 0, document: 0 }));
  h.clear.mockResolvedValue(true);
  const c = await mount();
  await act(async () => c.querySelector<HTMLButtonElement>('[aria-label="clear"]')!.click());
  // Uma volta para o apagar terminar, outra para a releitura (que espera as interações).
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  expect(alertSpy).toHaveBeenCalledTimes(1);
  expect(h.clear).toHaveBeenCalledTimes(1);
  expect(h.ok).toHaveBeenCalledWith('cleared');
  expect(c.textContent).toContain('empty');
});

it('cache de vídeo preso por player aberto é dito, não vira sucesso', async () => {
  h.usage.mockResolvedValue(usage({ image: 10 * MB, video: 0, audio: 0, document: 0 }));
  h.clear.mockResolvedValue(false);
  const c = await mount();
  await act(async () => c.querySelector<HTMLButtonElement>('[aria-label="clear"]')!.click());
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  expect(h.erro).toHaveBeenCalledWith('video-left');
  expect(h.ok).not.toHaveBeenCalled();
});

it('falha ao medir aparece com o motivo e deixa tentar de novo', async () => {
  h.usage.mockRejectedValueOnce(new Error('EACCES')).mockResolvedValue(usage({ image: 0, video: 0, audio: 0, document: 0 }));
  const c = await mount();
  expect(c.textContent).toContain('read-failed EACCES');
  await act(async () => c.querySelector<HTMLButtonElement>('[aria-label="retry"]')!.click());
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  expect(c.textContent).toContain('empty');
});
