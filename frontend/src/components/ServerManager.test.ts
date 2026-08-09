// @vitest-environment happy-dom
// Gate do botão Remover (round 1 da 4b): o ramo mobile (sem onSwitchActive) e o ramo com picker só
// mostram × quando sobra mais de 1 servidor OU podeRemoverUltimo=true.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ServerManager from './ServerManager.svelte';
import type { Server } from '../lib/auth';

vi.mock('../lib/auth', () => ({
  serverColor: () => '#fff',
  parseServerPairing: vi.fn(),
}));
vi.mock('../lib/vaultPush.svelte', () => ({
  vaultPush: { estado: 'idle', detalhe: '', clear: vi.fn() },
}));

const UNICO: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

function montar(props: Partial<Record<string, unknown>> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServerManager, {
    target: el,
    props: {
      servers: [UNICO],
      onRename: vi.fn(),
      onUpdateToken: () => true,
      onRemove: vi.fn(),
      onAdd: vi.fn(),
      ...props,
    },
  });
  return { el, comp: comp as never };
}

describe('ServerManager — botão Remover', () => {
  it('ramo mobile (sem onSwitchActive) com 1 servidor: × escondido', () => {
    const t = montar();
    expect(t.el.querySelector('.sm-srv-del')).toBeNull();
    unmount(t.comp);
  });

  it('ramo mobile com podeRemoverUltimo: × visível', () => {
    const t = montar({ podeRemoverUltimo: true });
    expect(t.el.querySelector('.sm-srv-del')).not.toBeNull();
    unmount(t.comp);
  });

  it('ramo com picker (onSwitchActive) com 1 servidor: × escondido', () => {
    const t = montar({ onSwitchActive: vi.fn() });
    expect(t.el.querySelector('.sm-srv-pick')).not.toBeNull();
    expect(t.el.querySelector('.sm-srv-del')).toBeNull();
    unmount(t.comp);
  });

  it('ramo com picker + podeRemoverUltimo: × visível', () => {
    const t = montar({ onSwitchActive: vi.fn(), podeRemoverUltimo: true });
    expect(t.el.querySelector('.sm-srv-del')).not.toBeNull();
    unmount(t.comp);
  });
});
