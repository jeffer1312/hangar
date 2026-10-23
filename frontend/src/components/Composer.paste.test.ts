// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Composer from './Composer.svelte';
import { uploadFile } from '@hangar/core';
import { dictations } from '../lib/dictationStore.svelte';

vi.mock('@hangar/core', async (orig) => ({
  ...(await orig<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn(async () => ({ current: 'plan', modes: ['plan'] })),
  getCommands: vi.fn(async () => []),
  getModelOptions: vi.fn(async () => []),
  uploadFile: vi.fn(() => new Promise(() => {})),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: { epoca: () => 0, retain: vi.fn(), release: vi.fn(), sessionsForServer: () => [] },
}));

const flush = async () => { await tick(); await new Promise((r) => setTimeout(r, 0)); await tick(); };

function colar(alvo: Element, items: { kind: string; type: string; getAsFile: () => File | null }[]) {
  const ev = new Event('paste', { bubbles: true, cancelable: true });
  Object.defineProperty(ev, 'clipboardData', { value: { items } });
  alvo.dispatchEvent(ev);
  return ev;
}

function montar() {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const comp = mount(Composer, {
    target,
    props: { sessionName: 's', sessionState: 'idle', status: null, inputText: '',
      onSend: vi.fn(), onCommand: vi.fn(), onInterrupt: vi.fn(), onOpenGit: vi.fn(), onOpenPreview: vi.fn() },
  });
  return { target, comp };
}

beforeEach(() => { dictations._resetForTests(); localStorage.clear(); vi.clearAllMocks(); });

describe('colar no composer', () => {
  it('arquivo que não é imagem vira anexo', async () => {
    const { target, comp } = montar();
    await flush();
    const f = new File(['oi'], 'notas.txt', { type: 'text/plain' });
    const ev = colar(target.querySelector('textarea')!, [{ kind: 'file', type: 'text/plain', getAsFile: () => f }]);
    await flush();
    expect(ev.defaultPrevented).toBe(true);
    expect(target.querySelector('.tile-name')?.textContent).toBe('notas.txt');
    // O anexo começa a subir ao entrar na lista, não só no envio.
    expect(uploadFile).toHaveBeenCalledWith('s', f, expect.any(Function), undefined);
    unmount(comp);
  });

  it('áudio colado vai pro ditado, sem limpeza', async () => {
    const { target, comp } = montar();
    await flush();
    const f = new File(['a'], 'nota.m4a', { type: 'audio/mp4' });
    colar(target.querySelector('textarea')!, [{ kind: 'file', type: 'audio/mp4', getAsFile: () => f }]);
    await flush();
    expect(uploadFile).toHaveBeenCalledWith('s', f, undefined, undefined, { audioOnly: true });
    expect(dictations.get('', 's')?.opts.ditado).toBe(false);
    unmount(comp);
  });

  it('texto continua colando como texto', async () => {
    const { target, comp } = montar();
    await flush();
    const ev = colar(target.querySelector('textarea')!, [{ kind: 'string', type: 'text/plain', getAsFile: () => null }]);
    expect(ev.defaultPrevented).toBe(false);
    unmount(comp);
  });
});
