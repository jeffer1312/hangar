// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';

const estado = vi.hoisted(() => ({ base: 'https://a.test', servidores: [] as { baseUrl: string; token: string }[] }));
vi.mock('./auth', () => ({
  getBaseUrl: () => estado.base,
  getToken: () => estado.servidores.find((s) => s.baseUrl === estado.base)?.token ?? null,
  listServers: () => estado.servidores,
  onServersChanged: () => () => {},
}));
import { registrar, molduras } from './diag';

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); });

it('guarda login anterior à inicialização e preserva destino após trocar o servidor', async () => {
  vi.useFakeTimers();
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"gravadas":1}'));
  registrar({ evento: 'login.falhou' });
  await vi.advanceTimersByTimeAsync(4000);
  expect(fetchMock).not.toHaveBeenCalled();
  estado.base = 'https://b.test';
  estado.servidores = [{ baseUrl: estado.base, token: 'b' }];
  await vi.advanceTimersByTimeAsync(10000);
  expect(fetchMock).not.toHaveBeenCalled();
  estado.servidores.push({ baseUrl: 'https://a.test', token: 'a-corrigido' });
  await vi.advanceTimersByTimeAsync(10000);
  expect(fetchMock).toHaveBeenCalledWith('https://a.test/api/diag', expect.objectContaining({
    headers: expect.objectContaining({ Authorization: 'Bearer a-corrigido' }),
  }));
  const corpo = fetchMock.mock.calls[0][1]?.body as string;
  expect(corpo).toContain('login.falhou');
  expect(corpo).not.toMatch(/a-corrigido|https:/);
});

it('stack conserva script e linha, sem endereço ou query de autenticação', () => {
  const erro = new Error('conteudo privado');
  erro.stack = 'Error: segredo\n at executar (https://privado.test/assets/main.js?token=secreto:12:3)';
  expect(molduras(erro)).toBe('at executar (main.js)');
});
