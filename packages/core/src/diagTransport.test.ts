import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { criarTransporteDiag } from './diagTransport';
const aceitar = async (_url: unknown, init?: RequestInit) => new Response(JSON.stringify({
  gravadas: JSON.parse(init?.body as string).eventos.length,
}));

beforeEach(() => vi.useFakeTimers());
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); });

it('retém sem token, após offline e HTTP falho; entrega só ao destino original com credencial atual', async () => {
  const tokens = new Map<string, string>();
  const fetchMock = vi.spyOn(globalThis, 'fetch')
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValueOnce(new Response('', { status: 401 }))
    .mockImplementation(aceitar);
  const diario = criarTransporteDiag((base) => tokens.get(base));
  diario.registrar('https://a.test', { evento: 'login.falhou' });
  tokens.set('https://b.test', 'segredo-b');
  await diario.enviar();
  expect(fetchMock).not.toHaveBeenCalled();
  tokens.set('https://a.test', 'velho');
  await diario.enviar();
  await diario.enviar();
  tokens.set('https://a.test', 'novo');
  await diario.enviar();
  await diario.enviar();
  expect(fetchMock).toHaveBeenCalledTimes(3);
  expect(fetchMock.mock.calls.every(([url]) => url === 'https://a.test/api/diag')).toBe(true);
  const ultimo = fetchMock.mock.calls[2][1]!;
  expect(ultimo.headers).toEqual(expect.objectContaining({ Authorization: 'Bearer novo' }));
  expect(JSON.parse(ultimo.body as string)).toEqual({ eventos: [{ evento: 'login.falhou' }] });
});

it('envio em voo não duplica nem apaga eventos que chegaram depois', async () => {
  let concluir!: (r: Response) => void;
  const fetchMock = vi.spyOn(globalThis, 'fetch')
    .mockImplementationOnce(() => new Promise((resolve) => { concluir = resolve; }))
    .mockImplementation(aceitar);
  const diario = criarTransporteDiag(() => 'token');
  diario.registrar('https://a.test', { evento: 'primeiro' });
  const primeiro = diario.enviar();
  expect(diario.enviar()).toBe(primeiro);
  diario.registrar('https://a.test', { evento: 'segundo' });
  concluir(new Response('{"gravadas":1}'));
  await primeiro;
  await diario.enviar();
  expect(fetchMock).toHaveBeenCalledTimes(2);
  expect(JSON.parse(fetchMock.mock.calls[1][1]!.body as string)).toEqual({ eventos: [{ evento: 'segundo' }] });
});

it('limita a retenção e retenta automaticamente depois da falha', async () => {
  const fetchMock = vi.spyOn(globalThis, 'fetch')
    .mockRejectedValueOnce(new Error('offline')).mockImplementation(aceitar);
  const diario = criarTransporteDiag(() => 'token');
  for (let i = 0; i < 240; i++) diario.registrar('https://a.test', { evento: 'teste', seq: i });
  await vi.advanceTimersByTimeAsync(4000);
  await vi.advanceTimersByTimeAsync(10000);
  expect(fetchMock).toHaveBeenCalledTimes(5);
  const lotes = fetchMock.mock.calls.slice(1).map(([, init]) => JSON.parse(init!.body as string).eventos);
  expect(lotes.every((lote) => lote.length <= 50)).toBe(true);
  const eventos = lotes.flat();
  expect(eventos).toHaveLength(200);
  expect(eventos[0].seq).toBe(40);
});

it('retém quando o servidor não confirma a gravação e evita keepalive em lote grande', async () => {
  const fetchMock = vi.spyOn(globalThis, 'fetch')
    .mockResolvedValueOnce(new Response('{"gravadas":0}')).mockImplementation(aceitar);
  const diario = criarTransporteDiag(() => 'token');
  diario.registrar('https://a.test', { evento: 'teste', detalhe: 'x'.repeat(17000) });
  await diario.enviar();
  await diario.enviar();
  expect(fetchMock).toHaveBeenCalledTimes(2);
  expect(fetchMock.mock.calls[0][1]?.keepalive).toBe(false);
});
