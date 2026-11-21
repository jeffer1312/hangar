// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Platform } from 'react-native';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('react-native', async (original) => {
  const rn = await original<typeof import('react-native')>();
  return { ...rn, Pressable: ({ accessibilityState, ...props }: ComponentProps<typeof rn.Pressable>) =>
    createElement(rn.Pressable, { ...props, 'aria-busy': accessibilityState?.busy }) };
});
const native = vi.hoisted(() => ({ download: vi.fn(), open: vi.fn(), move: vi.fn(), copy: vi.fn(), remove: vi.fn(), source: null as unknown, store: new Map<string, string>() }));
vi.mock('expo-file-system', () => {
  class File {
    // O download da tarefa chama `native.download`, que o teste controla.
    static createDownloadTask(uri: string, target: { uri: string }, options: { headers?: Record<string, string> }) {
      return {
        state: 'idle',
        downloadAsync: async () => { await native.download(uri, { uri: target.uri }, { headers: options.headers }); return new File(target.uri); },
        release: () => {},
      };
    }
    exists = true;
    size = 0;
    modificationTime = 0;
    uri: string;
    constructor(dir: { uri: string } | string, name?: string) { this.uri = name === undefined ? String(dir) : `${(dir as { uri: string }).uri}/${name}`; }
    get name() { return this.uri.split('/').pop()!; }
    get contentUri() { return this.uri.replace('file:///cache/', 'content://hangar/cache/'); }
    moveSync(target: { uri: string }) { native.move(this.uri, target.uri); this.uri = target.uri; }
    copySync(target: { uri: string }) { native.copy(this.uri, target.uri); }
    delete() { native.remove(this.uri); }
  }
  return {
    Paths: { cache: 'file:///cache' },
    Directory: class {
      uri: string;
      constructor(...parts: (string | { uri: string })[]) { this.uri = parts.map((p) => typeof p === 'string' ? p : p.uri).join('/'); }
      create() {}
      exists = false;
      list() { return []; }
    },
    File,
    DownloadTask: { fromSavable: () => { throw new Error('unexpected resume'); } },
  };
});
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (k: string) => native.store.get(k), set: (k: string, v: string) => { native.store.set(k, v); },
    remove: (k: string) => native.store.delete(k), getAllKeys: () => [...native.store.keys()], clearAll: () => native.store.clear(),
  }),
}));
vi.mock('expo-intent-launcher', () => ({ startActivityAsync: native.open }));
vi.mock('react-native-webview', () => ({ WebView: ({ source }: { source: unknown }) => {
  native.source = source;
  return createElement('div', { 'data-testid': 'webview' });
} }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: vi.fn() } }));
vi.mock('../../paraglide/messages', () => ({
  arq_pdf_sem_leitor: () => 'arq_pdf_sem_leitor',
  native_pdf_open: () => 'native_pdf_open',
  native_pdf_download_failed: ({ reason }: { reason: string }) => `native_pdf_download_failed:${reason}`,
  native_pdf_reader_failed: ({ reason }: { reason: string }) => `native_pdf_reader_failed:${reason}`,
  anexos_fechar_visualizacao: () => 'anexos_fechar_visualizacao',
  comum_carregando: () => 'comum_carregando',
}));

import { DocumentViewer } from './DocumentViewer';

const originalOS = Platform.OS;
beforeEach(() => {
  vi.clearAllMocks();
  native.download.mockReset().mockResolvedValue(undefined);
  native.open.mockReset().mockResolvedValue({ resultCode: 0 });
  native.source = null;
  native.store.clear();
  // Sem rede: a conferência de um byte falha na hora e o download segue sem o tamanho.
  vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('Network request failed')));
});
afterEach(() => { Platform.OS = originalOS; vi.unstubAllGlobals(); });

const doc = { uri: 'https://a.test/api/sessions/s/uploads/manual.pdf', headers: { Authorization: 'Bearer token-a' }, name: 'manual.pdf', kind: 'pdf' as const };

function render(name = doc.name) {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(DocumentViewer, { doc: { ...doc, name }, onClose: vi.fn() })));
  return { container, root };
}

describe('DocumentViewer PDF', () => {
  it('Android oferece abrir a cópia autenticada no leitor do aparelho', async () => {
    Platform.OS = 'android';
    const { container, root } = render();
    try {
      expect(container.querySelector('button[aria-label="native_pdf_open"]')).not.toBeNull();
      expect(container.textContent).not.toContain('arq_pdf_sem_leitor');
      expect(container.querySelector('[data-testid="webview"]')).toBeNull();
      expect(native.download).not.toHaveBeenCalled();
      await act(async () => container.querySelector<HTMLButtonElement>('button[aria-label="native_pdf_open"]')!.click());
      expect(native.download).toHaveBeenCalledExactlyOnceWith(doc.uri, expect.objectContaining({ uri: expect.stringMatching(/\.part$/) }), { headers: doc.headers });
      expect(native.open).toHaveBeenCalledExactlyOnceWith('android.intent.action.VIEW', {
        data: expect.stringMatching(/^content:\/\/hangar\/cache\/pdf-reader\/[^/]+\/manual\.pdf$/), type: 'application/pdf', flags: 1,
      });
    } finally { act(() => root.unmount()); }
  });

  it.each(['download', 'reader'] as const)('falha de %s fica visível e permite tentar de novo', async (stage) => {
    Platform.OS = 'android';
    if (stage === 'download') native.download.mockRejectedValueOnce(new Error('network-down'));
    else native.open.mockRejectedValueOnce(new Error('no-reader'));
    const { container, root } = render();
    try {
      const button = container.querySelector<HTMLButtonElement>('button[aria-label="native_pdf_open"]')!;
      await act(async () => button.click());
      expect(container.querySelector('[role="alert"]')?.textContent).toBe(stage === 'download'
        ? 'native_pdf_download_failed:network-down' : 'native_pdf_reader_failed:no-reader');
      expect(button.disabled).toBe(false);
      if (stage === 'download') {
        expect(native.open).not.toHaveBeenCalled();
        expect(native.remove).toHaveBeenCalledWith(expect.stringMatching(/\.part$/));
      }
      await act(async () => button.click());
      expect(container.querySelector('[role="alert"]')).toBeNull();
      expect(native.open).toHaveBeenLastCalledWith('android.intent.action.VIEW', {
        data: expect.stringMatching(/^content:\/\//), type: 'application/pdf', flags: 1,
      });
    } finally { act(() => root.unmount()); }
  });

  it('dois toques não iniciam downloads concorrentes', async () => {
    let finish!: () => void;
    native.download.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const { container, root } = render();
    try {
      const button = container.querySelector<HTMLButtonElement>('button[aria-label="native_pdf_open"]')!;
      // O download começa depois da conferência de um byte: espera ela assentar.
      await act(async () => { button.click(); button.click(); });
      expect(native.download).toHaveBeenCalledTimes(1);
      expect(button.disabled).toBe(true);
      expect(native.open).not.toHaveBeenCalled();
      await act(async () => finish());
      expect(native.open).toHaveBeenCalledTimes(1);
      expect(button.disabled).toBe(false);
    } finally { act(() => root.unmount()); }
  });

  it.each([
    ['', 'documento.pdf'], ['.', 'documento.pdf'], ['..', 'documento.pdf'],
    ['../manual final.pdf', '.._manual_final.pdf'], ['relatório.pdf', 'relat_rio.pdf'],
  ])('nome %j vira um arquivo seguro %s, nunca uma pasta', async (name, expected) => {
    const { container, root } = render(name);
    try {
      await act(async () => container.querySelector<HTMLButtonElement>('button[aria-label="native_pdf_open"]')!.click());
      expect(native.open).toHaveBeenCalledTimes(1);
      expect(native.open.mock.calls[0][1].data.split('/').at(-1)).toBe(expected);
      // Copiada para o leitor: a guardada no cache continua para a próxima abertura.
      expect(native.copy.mock.calls.at(-1)?.[1].split('/').at(-1)).toBe(expected);
    } finally { act(() => root.unmount()); }
  });

  it.each(['close', 'leave'] as const)('%s durante o download não abre o leitor depois', async (action) => {
    let finish!: () => void;
    native.download.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const { container, root } = render();
    try {
      await act(async () => container.querySelector<HTMLButtonElement>('button[aria-label="native_pdf_open"]')!.click());
      expect(native.download).toHaveBeenCalledTimes(1);
      act(() => root.render(action === 'close' ? createElement(DocumentViewer, { doc: null, onClose: vi.fn() }) : null));
      await act(async () => finish());
      expect(native.open).not.toHaveBeenCalled();
    } finally { act(() => root.unmount()); }
  });

  it('ausência de leitor mostra a mensagem própria sem a exceção nativa', async () => {
    native.open.mockRejectedValueOnce(Object.assign(new Error(
      'android.content.ActivityNotFoundException: No Activity found to handle Intent { act=android.intent.action.VIEW typ=application/pdf }',
    ), { code: 'ERR_UNEXPECTED' }));
    const { container, root } = render();
    try {
      await act(async () => container.querySelector<HTMLButtonElement>('button[aria-label="native_pdf_open"]')!.click());
      expect(container.querySelector('[role="alert"]')?.textContent).toBe('arq_pdf_sem_leitor');
      expect(container.textContent).not.toContain('ActivityNotFoundException');
      expect(container.textContent).not.toContain('No Activity found');
    } finally { act(() => root.unmount()); }
  });

  it('iOS conserva o PDF no WebView com o token no header', () => {
    Platform.OS = 'ios';
    const { container, root } = render();
    try {
      expect(container.querySelector('[data-testid="webview"]')).not.toBeNull();
      expect(native.source).toEqual({ uri: doc.uri, headers: doc.headers });
      expect(container.querySelector('button[aria-label="native_pdf_open"]')).toBeNull();
      expect(native.download).not.toHaveBeenCalled();
      expect(native.open).not.toHaveBeenCalled();
    } finally { act(() => root.unmount()); }
  });
});
