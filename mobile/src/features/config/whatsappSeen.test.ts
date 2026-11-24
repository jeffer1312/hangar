import { beforeEach, expect, it, vi } from 'vitest';

const { memory } = vi.hoisted(() => ({ memory: new Map<string, boolean>() }));
vi.mock('../../stores/prefs', () => ({ prefs: {
  getBoolean: (key: string) => memory.get(key),
  set: (key: string, value: boolean) => { memory.set(key, value); },
} }));

beforeEach(() => { memory.clear(); vi.resetModules(); });

it('o selo aparece até a primeira abertura e continua apagado depois de reabrir o app', async () => {
  const { useWhatsAppSeen } = await import('./whatsappSeen');
  expect(useWhatsAppSeen.getState().seen).toBe(false);
  useWhatsAppSeen.getState().markSeen();
  expect(useWhatsAppSeen.getState().seen).toBe(true);
  expect(memory.get('settings.whatsappSeen')).toBe(true);
  vi.resetModules();
  const reopened = await import('./whatsappSeen');
  expect(reopened.useWhatsAppSeen.getState().seen).toBe(true);
});
