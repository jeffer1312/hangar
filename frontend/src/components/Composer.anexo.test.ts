// @vitest-environment happy-dom
import { beforeEach, expect, it, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Composer from './Composer.svelte';
import { configureApi, transcribeUploaded, uploadFile } from '@hangar/core';
import { dictations } from '../lib/dictationStore.svelte';

vi.mock('@hangar/core', async (orig) => ({
  ...(await orig<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn(async () => ({ current: 'plan', modes: ['plan'] })),
  getCommands: vi.fn(async () => []),
  getModelOptions: vi.fn(async () => []),
  uploadFile: vi.fn(),
  transcribeUploaded: vi.fn(async () => ({ path: '/up/p/s/antigo.webm', text: 'de novo', raw: 'de novo' })),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: { epoca: () => 0, retain: vi.fn(), release: vi.fn(), sessionsForServer: () => [] },
}));

const flush = async () => { await tick(); await new Promise((r) => setTimeout(r, 0)); await tick(); };

beforeEach(() => {
  dictations._resetForTests();
  localStorage.clear();
  vi.clearAllMocks();
  configureApi({ getBaseUrl: () => 'http://h', getToken: () => 't', onUnauthorized: () => {}, origin: null,
    createEventSource: () => { throw new Error('sem SSE no teste'); } });
});

it('envio com anexo que termina com a conversa fechada não volta ao reabrir', async () => {
  vi.mocked(uploadFile).mockResolvedValue({ path: '/up/s/notas.txt' } as never);
  let terminar!: () => void;
  const onSend = vi.fn(() => new Promise<void>((r) => { terminar = r; }));
  localStorage.setItem('cp-draft:::s', JSON.stringify({ text: 'veja', jsonl: null }));
  const props = { sessionName: 's', sessionState: 'working' as const, status: null, inputText: 'veja', sessionJsonl: null,
    onSend, onCommand: vi.fn(), onInterrupt: vi.fn(), onOpenGit: vi.fn(), onOpenPreview: vi.fn() };
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(Composer, { target, props });
  await flush();
  const ev = new Event('paste', { bubbles: true, cancelable: true });
  const f = new File(['oi'], 'notas.txt', { type: 'text/plain' });
  Object.defineProperty(ev, 'clipboardData', { value: { items: [{ kind: 'file', type: 'text/plain', getAsFile: () => f }] } });
  target.querySelector('textarea')!.dispatchEvent(ev);
  await flush();
  target.querySelector<HTMLButtonElement>('.send-btn')!.click();
  await flush();
  expect(onSend).toHaveBeenCalled();
  unmount(comp as never);
  terminar();
  await flush();
  expect(localStorage.getItem('cp-draft:::s')).toBeNull();
  const de_novo = mount(Composer, { target, props: { ...props, inputText: '' } });
  await flush();
  expect(target.querySelector('.tile-name')).toBeNull();
  unmount(de_novo as never);
});

it('anexo já subido e removido enquanto o envio espera outro fica fora da mensagem', async () => {
  let terminarB!: (r: { path: string }) => void;
  vi.mocked(uploadFile).mockImplementation(((_: string, f: File) => f.name === 'a.txt'
    ? Promise.resolve({ path: '/up/s/a.txt' })
    : new Promise((r) => { terminarB = r; })) as never);
  const onSend = vi.fn(async () => {});
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(Composer, { target, props: { sessionName: 's', sessionState: 'idle', status: null, inputText: 'veja',
    onSend, onCommand: vi.fn(), onInterrupt: vi.fn(), onOpenGit: vi.fn(), onOpenPreview: vi.fn() } });
  await flush();
  const ev = new Event('paste', { bubbles: true, cancelable: true });
  const arquivos = ['a.txt', 'b.txt'].map((n) => new File(['oi'], n, { type: 'text/plain' }));
  Object.defineProperty(ev, 'clipboardData', { value: { items: arquivos.map((f) => ({ kind: 'file', type: 'text/plain', getAsFile: () => f })) } });
  target.querySelector('textarea')!.dispatchEvent(ev);
  await flush();
  target.querySelector<HTMLButtonElement>('.send-btn')!.click();
  await flush();
  // B ainda sobe e o ✕ dele continua lá.
  expect(target.querySelectorAll('.tile-x')).toHaveLength(2);
  target.querySelector<HTMLButtonElement>('.tile-x')!.click();
  await flush();
  terminarB({ path: '/up/s/b.txt' });
  await flush();
  expect(onSend).toHaveBeenCalledTimes(1);
  const [msg] = onSend.mock.calls[0] as unknown as [string];
  expect(msg).toContain('/up/s/b.txt');
  expect(msg).not.toContain('/up/s/a.txt');
  unmount(comp as never);
});

it('áudio da galeria transcreve pelo nome, sem subir bytes, e a barra toca do servidor', async () => {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(Composer, {
    target,
    props: { sessionName: 's', sessionState: 'idle', status: null, inputText: '', sessionJsonl: 'j1',
      onSend: vi.fn(), onCommand: vi.fn(), onInterrupt: vi.fn(), onOpenGit: vi.fn(), onOpenPreview: vi.fn() },
  }) as unknown as { ditarAnexo: (a: string) => void };
  comp.ditarAnexo('antigo.webm');
  await flush();
  expect(transcribeUploaded).toHaveBeenCalledWith('s', 'antigo.webm', expect.objectContaining({ limpar: true }), undefined);
  expect(uploadFile).not.toHaveBeenCalled();
  expect(target.querySelector('textarea')!.value).toBe('de novo');
  expect(target.querySelector<HTMLAudioElement>('.ditado-audio')!.src).toContain('/uploads/antigo.webm');
  unmount(comp as never);
});
