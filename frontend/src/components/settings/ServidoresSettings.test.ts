// @vitest-environment happy-dom
// Round 1/2 da 4b: logout idempotente — Sair e remover-último chamam onLogout UMA vez; enquanto a
// Promise anda, as portas de saída ficam bloqueadas; rejeição vira erro visível recuperável (o
// clear de credenciais é dono do App/lib/logout.ts, este componente não chama).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ServidoresSettings from './ServidoresSettings.svelte';
import * as auth from '../../lib/auth';
import * as api from '../../lib/api';
import type { Server } from '../../lib/auth';

vi.mock('../../lib/auth', () => ({
  serverColor: () => '#fff',
  listServers: vi.fn(),
  getActiveId: vi.fn(),
  selectServer: vi.fn(),
  renameServer: vi.fn(),
  updateServer: vi.fn(() => true),
  removeServer: vi.fn(),
  addServer: vi.fn(),
  validarPareamento: vi.fn(),
  onServersChanged: vi.fn(() => () => {}),
}));
vi.mock('../../lib/sessionsStore.svelte', () => ({
  sessionsStore: { refreshServers: vi.fn(), reconnect: vi.fn() },
}));
vi.mock('../../lib/api', () => ({
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));

const authMock = vi.mocked(auth);
const apiMock = vi.mocked(api);
const SRV: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

let onLogoutResolve: (() => void) | null = null;
function onLogoutDeferred() {
  return new Promise<void>((res) => { onLogoutResolve = res; });
}

let onLogoutCalls: ReturnType<typeof vi.fn<() => Promise<void>>>;

function montar(over: { onLogout?: () => Promise<void> } = {}) {
  authMock.listServers.mockReturnValue([SRV]);
  authMock.getActiveId.mockReturnValue(SRV.id);
  apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));   // fica pendente (irrelevante)
  onLogoutCalls = vi.fn<() => Promise<void>>(over.onLogout ?? onLogoutDeferred);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServidoresSettings, {
    target: el,
    props: { resolvedServer: SRV, apiTarget: null, onPickTarget: vi.fn(), onLogout: onLogoutCalls },
  });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); onLogoutResolve = null; });

describe('ServidoresSettings — logout idempotente', () => {
  it('Sair + remover-último durante a Promise chamam onLogout UMA vez', async () => {
    const t = montar();
    // 1) Sair -> confirmação -> logout começa (Promise pendente). O ModalDialog vive num portal
    // pro <body>, então o diálogo se busca no document, não dentro de t.el.
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    // 2) Durante a Promise: remover (×) fica bloqueado — diálogo nem abre
    const del = t.el.querySelector<HTMLButtonElement>('.sm-srv-del');
    expect(del).not.toBeNull();   // podeRemoverUltimo: visível
    del!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();   // guarda segurou
    // 3) Resolve: segue 1 chamada só
    onLogoutResolve!();
    await Promise.resolve(); await Promise.resolve();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('segundo disparo durante a Promise é bloqueado (Sair disabled e remover sem diálogo)', async () => {
    const t = montar();
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    // Durante a Promise: Sair principal disabled (clique é no-op) e remover nem abre diálogo
    const sairDeNovo = t.el.querySelector<HTMLButtonElement>('.ss-danger')!;
    expect(sairDeNovo.disabled).toBe(true);
    sairDeNovo.click();
    t.el.querySelector<HTMLButtonElement>('.sm-srv-del')!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();
    // Resolve: segue 1 chamada só
    onLogoutResolve!();
    await Promise.resolve(); await Promise.resolve();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('rejeição do onLogout vira erro visível e libera nova tentativa (sem unhandled)', async () => {
    const t = montar({ onLogout: () => Promise.reject(new Error('x')) });
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await Promise.resolve();   // o catch do logout roda em microtask; só então a mensagem renderiza
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    const aviso = t.el.querySelector<HTMLElement>('.ss-aviso');
    expect(aviso?.innerText).toContain('Não foi possível sair');
    expect(aviso?.getAttribute('role')).toBe('status');
    // Guard resetado: nova tentativa funciona
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await Promise.resolve();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });
});
