import { afterEach, expect, it, vi } from 'vitest';
import { registrarDiag, novoReqDiag } from '@hangar/core';

const estado = vi.hoisted(() => ({
  base: 'https://a.test',
  servidores: [{ baseUrl: 'https://a.test', token: 'token-a' }],
  appState: vi.fn(),
}));
vi.mock('react-native', () => ({ Platform: { OS: 'android' }, AppState: { addEventListener: estado.appState } }));
vi.mock('../stores/servers', () => ({ useServers: {
  getState: () => ({ servers: estado.servidores, active: () => ({ baseUrl: estado.base }) }),
  subscribe: vi.fn(),
} }));
import { iniciarDiag } from './diag';

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); });

it('instala sink uma vez, grava plataforma e eventos no destino capturado', async () => {
  vi.useFakeTimers();
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"gravadas":2}'));
  iniciarDiag(); iniciarDiag();
  expect(estado.appState).toHaveBeenCalledTimes(1);
  const req = novoReqDiag();
  expect(req).not.toBe('');
  registrarDiag({ evento: 'acao', req });
  estado.base = 'https://b.test';
  await vi.advanceTimersByTimeAsync(4000);
  expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/diag');
  const eventos = JSON.parse(fetchMock.mock.calls[0][1]!.body as string).eventos;
  expect(eventos).toEqual([
    expect.objectContaining({ evento: 'app.abriu', so: 'android' }),
    expect.objectContaining({ evento: 'acao', req }),
  ]);
  expect(JSON.stringify(eventos)).not.toMatch(/token-a|https:/);
});
