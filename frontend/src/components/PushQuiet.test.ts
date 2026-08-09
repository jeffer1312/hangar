// @vitest-environment happy-dom
// Probe Svelte runtime do PushQuiet (round 3): prova que o efeito do componente depende SÓ de
// open + chave do alvo — um objeto target NOVO com a mesma chave (recomputo do pai) NÃO dispara
// novo GET, e não há realimentação depois da resposta. O resto da máquina de corrida vive no
// controller (lib/quietHours.test.ts).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import PushQuietHarness from './PushQuietHarness.svelte';
import * as api from '../lib/api';
import type { PushTarget } from '../lib/quietHours';
import type { Server } from '../lib/auth';

vi.mock('../lib/api', () => ({
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));

const apiMock = vi.mocked(api);
type PushPayload = { muted: string[]; quiet_hours: { start: string; end: string } | null };
const gets: Array<{ resolve: (v: PushPayload) => void }> = [];

interface HarnessApi {
  setarAlvo: (t: PushTarget) => void;
  setarAberto: (v: boolean) => void;
}
function montar(target: PushTarget, open = true) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  // O harness nasce com alvo global/aberto; os $state exportados permitem trocar como um pai real.
  const comp = mount(PushQuietHarness, { target: el }) as unknown as HarnessApi;
  comp.setarAlvo(target);
  comp.setarAberto(open);
  return { el, comp };
}

beforeEach(() => {
  vi.clearAllMocks();
  gets.length = 0;
  apiMock.getPushSettings.mockImplementation(
    () => new Promise((resolve) => gets.push({ resolve })),
  );
  apiMock.getPushSettingsForServer.mockImplementation(
    () => new Promise((resolve) => gets.push({ resolve })),
  );
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('PushQuiet runtime (1 GET por abertura/alvo estável)', () => {
  it('objeto target novo com a mesma chave não dispara 2º GET; sem realimentação após resposta', async () => {
    const t = montar({ mode: 'global' });
    await tick();
    expect(apiMock.getPushSettings).toHaveBeenCalledTimes(1);

    // Recomputo do pai: `target` é re-criado (objeto novo, mesma chave). Sem untrack no sync(),
    // o efeito registraría dependência do objeto e recarregaria aqui.
    t.comp.setarAlvo({ mode: 'global' });
    await tick();
    expect(apiMock.getPushSettings).toHaveBeenCalledTimes(1);

    // Resposta pinta uma vez e não realimenta: nada novo depois do paint.
    gets[0].resolve({ muted: [], quiet_hours: { start: '10:00', end: '11:00' } });
    await tick();
    await tick();
    expect((t.el.querySelector('input[aria-label="Início do silêncio"]') as HTMLInputElement).value).toBe('10:00');
    expect(apiMock.getPushSettings).toHaveBeenCalledTimes(1);

    // Reabrir: 1 GET novo (o resultado pode ter mudado no servidor).
    t.comp.setarAberto(false);
    await tick();
    t.comp.setarAberto(true);
    await tick();
    expect(apiMock.getPushSettings).toHaveBeenCalledTimes(2);
    unmount(t.comp as never);
  });

  it('alvo server usa a API ForServer', async () => {
    const srv = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;
    const t = montar({ mode: 'server', server: srv });
    await tick();
    expect(apiMock.getPushSettingsForServer).toHaveBeenCalledWith(srv);
    unmount(t.comp as never);
  });
});
