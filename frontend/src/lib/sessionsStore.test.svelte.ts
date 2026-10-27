// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { sessionsStore } from './sessionsStore.svelte';
import { readGroupIdentity, configureDiag, groupGlobal, estaDesligado, registrarFalha, registrarSucesso, _limparEsfriamentoParaTestes } from '@hangar/core';

const streams = vi.hoisted(() => new Map<string, Map<string, (event: { data: string }) => void>>());
const objetos = vi.hoisted(() => new Map<string, { onerror?: () => void; close: unknown; addEventListener: unknown }>());
const connectionIds = vi.hoisted(() => new Map<string, string | undefined>());
const navListener = vi.hoisted(() => ({ start: vi.fn(), stop: vi.fn() }));
const pares = vi.hoisted(() => ({ setEphemeralServers: vi.fn(), listExternalPairs: vi.fn(), attachGuest: vi.fn() }));
vi.mock('./auth', () => ({
  setEphemeralServers: pares.setEphemeralServers,
  listServers: () => ['lan', 'vpn'].map(id => ({ id, label: id, baseUrl: `http://${id}`, token: 'test' })),
  onServersChanged: () => () => {},
  getActiveId: () => 'lan',
}));
vi.mock('./navPelaLista', () => ({ navPelaLista: vi.fn() }));
vi.mock('./navegadorPanel.svelte', () => ({
  podarNavMortos: vi.fn(),
  ouvirFechamentoNav: () => { navListener.start(); return navListener.stop; },
}));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  rotaDecidida: () => true,   // a decisão de rota (sonda de rede) é assunto de rota.test
  readGroupIdentity: vi.fn(async (server: { id: string }) => `backend-${server.id}`),
  listExternalPairs: pares.listExternalPairs,
  attachGuest: pares.attachGuest,
  openSessionsStream: (server: { id: string }, req?: string) => {
    connectionIds.set(server.id, req);
    const handlers = new Map();
    streams.set(server.id, handlers);
    const obj = { close: vi.fn(), addEventListener: (name: string, fn: unknown) => handlers.set(name, fn) };
    objetos.set(server.id, obj);
    return obj;
  },
}));
afterEach(() => { sessionsStore.release(); streams.clear(); objetos.clear(); connectionIds.clear(); vi.clearAllTimers(); vi.useRealTimers();
  configureDiag({ registrar: () => {}, novoReq: () => '' }); _limparEsfriamentoParaTestes(); });

it('falha real marca os outros servidores, nunca o ativo; erro do produtor não é queda de rede', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  objetos.get('lan')!.onerror!();
  objetos.get('vpn')!.onerror!();
  expect(estaDesligado('lan')).toBe(false);   // o ativo não espera: restart do backend não o deixa offline
  expect(estaDesligado('vpn')).toBe(true);
  sessionsStore.buscarAgora('vpn');
  streams.get('vpn')!.get('list_error')!({ data: '' });
  expect(estaDesligado('vpn')).toBe(false);   // a máquina respondeu: não é rede
});

it('falha do produtor com linhas na tela vai ao diário e não marca o servidor', () => {
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'list-error' });
  sessionsStore.retain();
  streams.get('vpn')!.get('sessions')!({ data: JSON.stringify([{ name: 'hangar', jsonl: '/h' }]) });
  streams.get('vpn')!.get('list_error')!({ data: '' });
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.falhou' }), 'http://vpn');
  expect(sessionsStore.byServer.find(s => s.server.id === 'vpn')?.error).toBeNull();
  expect(sessionsStore.rows.some(s => s.name === 'hangar')).toBe(true);
});

it('evento keyring marca só o servidor dele, sobrevive ao quadro de sessões e ignora dado torto', () => {
  sessionsStore.retain();
  const vpn = () => sessionsStore.byServer.find(s => s.server.id === 'vpn');
  const keyring = (data: string) => streams.get('vpn')!.get('keyring')?.({ data });
  keyring(JSON.stringify({ state: 'locked' }));
  streams.get('vpn')!.get('sessions')!({ data: '[]' });
  expect(vpn()?.keyringLocked).toBe(true);
  expect(sessionsStore.byServer.find(s => s.server.id === 'lan')?.keyringLocked).toBe(false);
  keyring('{torto');
  keyring(JSON.stringify({ state: 'outro' }));
  expect(vpn()?.keyringLocked).toBe(true);
  keyring(JSON.stringify({ state: 'unlocked' }));
  expect(vpn()?.keyringLocked).toBe(false);
});

it('falha do produtor sem linha nenhuma em todos os servidores continua marcando cada um', () => {
  sessionsStore.retain();
  for (const id of ['lan', 'vpn']) streams.get(id)!.get('list_error')!({ data: '' });
  // É o que acende o "Sem resposta de {servidores}" da lista vazia.
  expect(sessionsStore.byServer.every(s => s.error)).toBe(true);
});

it('resposta recebida fora da lista retoma o stream e atualiza as sessões antigas', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  const previous = objetos.get('vpn');
  const first = { name: 'hangar', jsonl: '/hangar.jsonl', state: 'idle' };
  streams.get('vpn')!.get('sessions')!({ data: JSON.stringify([first]) });
  previous!.onerror!();
  expect(estaDesligado('vpn')).toBe(true);
  expect(sessionsStore.sessionsForServer('vpn')).toEqual([first]);

  vi.advanceTimersByTime(1900);   // antes da primeira nova tentativa (2 s)
  expect(objetos.get('vpn')).toBe(previous);
  const lan = objetos.get('lan');
  registrarSucesso('vpn');
  const recovered = objetos.get('vpn');
  expect(recovered).not.toBe(previous);
  expect(estaDesligado('vpn')).toBe(false);
  streams.get('vpn')!.get('sessions')!({ data: JSON.stringify([first,
    { name: 'setup-vm', jsonl: '/setup-vm.jsonl', state: 'working' }]) });
  expect(sessionsStore.sessionsForServer('vpn').map(s => s.name)).toEqual(['hangar', 'setup-vm']);
  registrarSucesso('vpn');
  expect(objetos.get('vpn')).toBe(recovered);
  expect(objetos.get('lan')).toBe(lan);
});

it('servidor desligado antes da montagem aguarda o prazo; reconexão explícita tenta agora', () => {
  registrarFalha('vpn');
  sessionsStore.retain();
  expect(objetos.has('vpn')).toBe(false);
  expect(sessionsStore.byServer.find(s => s.server.id === 'vpn')?.error).toBe('offline');
  sessionsStore.refreshServers();
  expect(estaDesligado('vpn')).toBe(true);
  expect(objetos.has('vpn')).toBe(false);
  sessionsStore.reconnect();
  expect(estaDesligado('vpn')).toBe(false);
  expect(objetos.has('vpn')).toBe(true);
});

it('descobre a identidade apenas após quadro válido com pareamento remoto, uma vez por conexão', async () => {
  vi.mocked(readGroupIdentity).mockClear();
  registrarFalha('vpn');
  sessionsStore.retain();
  expect(readGroupIdentity).not.toHaveBeenCalled();
  const publish = streams.get('lan')!.get('sessions')!;
  publish({ data: '[]' });
  expect(readGroupIdentity).not.toHaveBeenCalled();
  const data = JSON.stringify([{ name: 'a', jsonl: '/a', pair_peers: ['backend-vpn::b'] }]);
  publish({ data });
  publish({ data });
  await vi.waitFor(() => expect(sessionsStore.identities.get('lan')).toBe('backend-lan'));
  expect(readGroupIdentity).toHaveBeenCalledTimes(1);
  expect(estaDesligado('vpn')).toBe(true);
});

it('pedido de identidades lê cada máquina conectada mesmo sem pareamento remoto', async () => {
  vi.mocked(readGroupIdentity).mockClear();
  sessionsStore.retain();
  streams.get('lan')!.get('sessions')!({ data: '[]' });
  streams.get('vpn')!.get('sessions')!({ data: '[]' });
  expect(readGroupIdentity).not.toHaveBeenCalled();
  sessionsStore.requestIdentities();
  await vi.waitFor(() => expect(sessionsStore.identities.get('vpn')).toBe('backend-vpn'));
  expect(sessionsStore.identities.get('lan')).toBe('backend-lan');
  sessionsStore.requestIdentities();
  expect(readGroupIdentity).toHaveBeenCalledTimes(2);
});

it('repete a identidade após falha transitória, com intervalo e sem reabrir o stream', async () => {
  vi.useFakeTimers();
  vi.mocked(readGroupIdentity).mockClear();
  vi.mocked(readGroupIdentity).mockRejectedValueOnce(new Error('timeout'));
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'identity-retry' });
  registrarFalha('vpn');   // só a lan conectada: a contagem é das leituras dela
  sessionsStore.retain();
  const original = objetos.get('lan');
  const publish = streams.get('lan')!.get('sessions')!;
  const data = JSON.stringify([{ name: 'a', jsonl: '/a', pair_peers: ['backend-vpn::b'] }]);
  publish({ data });
  await vi.waitFor(() => expect(registrar).toHaveBeenCalledWith(
    expect.objectContaining({ evento: 'lista.identificador_falhou' }), 'http://lan'));
  publish({ data });
  expect(readGroupIdentity).toHaveBeenCalledTimes(1);
  vi.setSystemTime(Date.now() + 30000);
  streams.get('lan')!.get('ping')!({ data: '' });
  await vi.waitFor(() => expect(sessionsStore.identities.get('lan')).toBe('backend-lan'));
  expect(readGroupIdentity).toHaveBeenCalledTimes(2);
  expect(objetos.get('lan')).toBe(original);
  expect(original!.close).not.toHaveBeenCalled();
});

it('grupo entre máquinas numa lista lê o identificador de todas as conectadas e cria o membro da máquina sem conexão', async () => {
  vi.mocked(readGroupIdentity).mockClear();
  sessionsStore.retain();
  // A (lan) é dona do grupo com D, que não está conectada aqui; B (vpn) não cita ninguém de fora.
  streams.get('vpn')!.get('sessions')!({ data: JSON.stringify([{ name: 'b', jsonl: '/b', state: 'idle' }]) });
  streams.get('lan')!.get('sessions')!({ data: JSON.stringify([{ name: 'a', jsonl: '/a', state: 'idle',
    pair_gid: 'g', pair_owner: 'backend-lan', pair_peers: ['backend-zzz::d'] }]) });
  await vi.waitFor(() => expect(sessionsStore.identities.get('vpn')).toBe('backend-vpn'));
  const grupo = groupGlobal(sessionsStore.rows, sessionsStore.identities).find((g) => g.key === 'fed:backend-lan/g');
  expect(grupo?.members).toContainEqual({ kind: 'missing', server_id: 'backend-zzz', name: 'd' });
});

it('identificador vazio é falha: aparece por máquina e é relido depois do intervalo; máquina fora do ar não fica em leitura', async () => {
  vi.useFakeTimers();
  vi.mocked(readGroupIdentity).mockClear();
  let falha: (e: Error) => void = () => {};
  vi.mocked(readGroupIdentity).mockImplementationOnce(() => new Promise((_, reject) => { falha = reject; }));
  registrarFalha('vpn');
  sessionsStore.retain();
  const data = JSON.stringify([{ name: 'a', jsonl: '/a', pair_peers: ['backend-vpn::b'] }]);
  streams.get('lan')!.get('sessions')!({ data });
  expect(sessionsStore.identityStatus.get('lan')).toBe('reading');
  expect(sessionsStore.identityStatus.has('vpn')).toBe(false);
  // A regra do vazio é do core (`readGroupIdentity`): aqui chega como a falha `vazio`.
  falha(Object.assign(new Error('machine_identity_empty'), { name: 'vazio' }));
  await vi.waitFor(() => expect(sessionsStore.identityStatus.get('lan')).toBe('failed'));
  expect(sessionsStore.identities.has('lan')).toBe(false);
  streams.get('lan')!.get('ping')!({ data: '' });
  expect(readGroupIdentity).toHaveBeenCalledTimes(1);
  vi.setSystemTime(Date.now() + 30000);
  streams.get('lan')!.get('ping')!({ data: '' });
  await vi.waitFor(() => expect(sessionsStore.identities.get('lan')).toBe('backend-lan'));
  expect(sessionsStore.identityStatus.has('lan')).toBe(false);
});

it('não reabre conexões quando a lista já foi desmontada', () => {
  sessionsStore.retain();
  const previous = objetos.get('vpn');
  previous!.onerror!();
  sessionsStore.release();
  registrarSucesso('vpn');
  expect(objetos.get('vpn')).toBe(previous);
});

it('retentar um remoto não cancela a recuperação já pendente do servidor ativo', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  const lan = objetos.get('lan');
  lan!.onerror!();
  objetos.get('vpn')!.onerror!();
  sessionsStore.buscarAgora('vpn');
  expect(objetos.get('lan')).toBe(lan);
  vi.advanceTimersByTime(30000);
  expect(objetos.get('lan')).not.toBe(lan);
});

it.each(['vpn', undefined])('buscarAgora(%s) reabre o produtor com erro sem interromper o servidor saudável', (id) => {
  sessionsStore.retain();
  const healthy = objetos.get('lan');
  const failed = objetos.get('vpn');
  streams.get('vpn')!.get('list_error')!({ data: '' });
  expect(estaDesligado('vpn')).toBe(false);
  expect(sessionsStore.byServer.find(s => s.server.id === 'vpn')?.error).toBeTruthy();

  sessionsStore.buscarAgora(id);
  expect(failed!.close).toHaveBeenCalledOnce();
  expect(objetos.get('vpn')).not.toBe(failed);
  expect(objetos.get('lan')).toBe(healthy);
  expect(healthy!.close).not.toHaveBeenCalled();
  failed!.onerror!();
  expect(estaDesligado('vpn')).toBe(false);
  streams.get('vpn')!.get('sessions')!({ data: JSON.stringify([{ name: 'recovered', jsonl: '/r' }]) });
  expect(sessionsStore.byServer.find(s => s.server.id === 'vpn')?.error).toBeNull();
  expect(sessionsStore.rows.some(s => s.name === 'recovered')).toBe(true);
});

it('em segundo plano a queda não marca offline e a nova tentativa não aumenta a espera', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  const visibilidade = vi.spyOn(document, 'visibilityState', 'get');
  visibilidade.mockReturnValue('hidden');
  document.dispatchEvent(new Event('visibilitychange'));
  for (let i = 0; i < 3; i++) {
    const antes = objetos.get('vpn');
    antes!.onerror!();
    expect(estaDesligado('vpn')).toBe(false);
    vi.advanceTimersByTime(29999);
    expect(objetos.get('vpn')).toBe(antes);
    vi.advanceTimersByTime(1);
    expect(objetos.get('vpn')).not.toBe(antes);
  }
  visibilidade.mockRestore();
});

it('voltar do segundo plano reconecta na hora; quem caiu com a tela à vista e nunca respondeu espera o prazo', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  objetos.get('vpn')!.onerror!();   // caiu com a tela à vista: conta
  const vpnCaido = objetos.get('vpn');
  const visibilidade = vi.spyOn(document, 'visibilityState', 'get');
  visibilidade.mockReturnValue('hidden');
  document.dispatchEvent(new Event('visibilitychange'));
  streams.get('lan')!.get('sessions')!({ data: '[]' });   // a lista chegou com o app já escondido
  const lanAntigo = objetos.get('lan');
  visibilidade.mockReturnValue('visible');
  lanAntigo!.onerror!();            // o iOS entrega o erro da suspensão antes do evento da volta
  expect(estaDesligado('lan')).toBe(false);
  document.dispatchEvent(new Event('visibilitychange'));
  expect(objetos.get('lan')).not.toBe(lanAntigo);
  expect(estaDesligado('vpn')).toBe(true);
  expect(objetos.get('vpn')).toBe(vpnCaido);
  visibilidade.mockRestore();
});

it('reabrir o app tenta na hora quem respondeu antes, mesmo com prazo gravado', () => {
  vi.useFakeTimers();
  registrarSucesso('lan');
  registrarFalha('lan');   // a suspensão derrubou antes de o app ser fechado
  registrarFalha('vpn');   // nunca respondeu
  sessionsStore.retain();
  expect(estaDesligado('lan')).toBe(false);
  expect(objetos.has('lan')).toBe(true);
  expect(estaDesligado('vpn')).toBe(true);
  expect(objetos.has('vpn')).toBe(false);
});

it('retoma automaticamente após o prazo, mantendo offline até uma resposta válida', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  const previous = objetos.get('vpn');
  previous!.onerror!();
  vi.advanceTimersByTime(2000);
  expect(objetos.get('vpn')).not.toBe(previous);
  expect(estaDesligado('vpn')).toBe(true);
  streams.get('vpn')!.get('sessions')!({ data: JSON.stringify([{ name: 'voltou', jsonl: '/v' }]) });
  expect(estaDesligado('vpn')).toBe(false);
  expect(sessionsStore.rows.some(s => s.name === 'voltou')).toBe(true);
});

it('remontar respeita o restante do prazo e o timeout também agenda outra tentativa', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  objetos.get('vpn')!.onerror!();
  vi.advanceTimersByTime(1000);
  sessionsStore.release();
  sessionsStore.retain();
  const previous = objetos.get('vpn');
  vi.advanceTimersByTime(999);
  expect(objetos.get('vpn')).toBe(previous);
  vi.advanceTimersByTime(1);
  const silent = objetos.get('vpn');
  expect(silent).not.toBe(previous);
  vi.advanceTimersByTime(10000);
  expect(silent!.close).toHaveBeenCalled();
  vi.advanceTimersByTime(60000);
  expect(objetos.get('vpn')).not.toBe(silent);
});

it('Ctrl+R não grava offline quando o navegador cancela as conexões da página antiga', () => {
  sessionsStore.retain();
  const previous = objetos.get('vpn');
  streams.get('vpn')!.get('sessions')!({ data: '[]' });
  window.dispatchEvent(new Event('beforeunload'));
  previous!.onerror!();
  registrarFalha('vpn'); // fetch cancelado pela navegação também não é queda da máquina
  expect(estaDesligado('vpn')).toBe(false);
  window.dispatchEvent(new Event('pagehide'));
  window.dispatchEvent(new Event('pageshow'));
  expect(objetos.get('vpn')).not.toBe(previous);
  expect(estaDesligado('vpn')).toBe(false);
});

it('erro e timer da conexão descartada não derrubam a conexão nova', () => {
  vi.useFakeTimers();
  sessionsStore.retain();
  const previous = objetos.get('vpn');
  vi.advanceTimersByTime(5000);
  sessionsStore.reconnect();
  const current = objetos.get('vpn');
  streams.get('vpn')!.get('sessions')!({ data: '[]' });
  previous!.onerror!();
  vi.advanceTimersByTime(5000);
  expect(estaDesligado('vpn')).toBe(false);
  expect(current!.close).not.toHaveBeenCalled();
  expect(objetos.get('vpn')).toBe(current);
});

it('passa à abertura da lista o mesmo ID registrado em cada servidor', () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  let sequence = 0;
  configureDiag({ registrar, novoReq: () => `lista-${++sequence}` });
  sessionsStore.retain();
  expect(new Set(connectionIds.values()).size).toBe(2);
  for (const [server, req] of connectionIds) {
    expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.abrir', req }), `http://${server}`);
  }
});

it('ouve fechamento externo do navegador enquanto a lista está viva', () => {
  const starts = navListener.start.mock.calls.length;
  const stops = navListener.stop.mock.calls.length;
  sessionsStore.retain();
  expect(navListener.start).toHaveBeenCalledTimes(starts + 1);
  sessionsStore.release();
  expect(navListener.stop).toHaveBeenCalledTimes(stops + 1);
});

it('preserva a sessão da VPN quando a LAN assume a duplicata na lista visual', () => {
  sessionsStore.retain();
  const session = { name: 'hangar-6', jsonl: '/same.jsonl', provider: 'codex', tracked: true, state: 'working' };
  const publish = (id: string) => streams.get(id)!.get('sessions')!({ data: JSON.stringify([session]) });
  publish('vpn');
  expect(sessionsStore.sessionsForServer('vpn')).toEqual([session]);
  publish('lan');
  expect(sessionsStore.rows.map(s => s.serverId)).toEqual(['lan']);
  expect(sessionsStore.sessionsForServer('vpn')).toEqual([session]);
});

it('registra primeiro quadro, parse inválido, silêncio e volta sem copiar quadros ou URLs', async () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'conexao' });
  sessionsStore.retain();
  const publicar = (data: string) => streams.get('lan')!.get('sessions')!({ data });
  publicar('[]');
  publicar('[]');
  expect(registrar.mock.calls.filter(([e, destino]) => e.evento === 'lista.conectou' && destino === 'http://lan')).toHaveLength(1);
  publicar('segredo conversa token');
  publicar('[]');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ codigo: 'json_invalido' }), 'http://lan');
  expect(estaDesligado('lan')).toBe(false);
  await vi.advanceTimersByTimeAsync(25000);
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ codigo: 'silencio', espera_ms: 25000 }), 'http://lan');
  // `lan` é o ativo: não ganha prazo, tenta de novo em 1 s.
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.retentativa', espera_ms: 1000 }), 'http://lan');
  await vi.advanceTimersByTimeAsync(1000);
  publicar('[]');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.voltou' }), 'http://lan');
  expect(JSON.stringify(registrar.mock.calls.map(([e]) => e))).not.toMatch(/segredo|conversa|token|http:/);
});

describe('par externo', () => {
  beforeEach(() => { pares.listExternalPairs.mockReset(); pares.attachGuest.mockReset(); pares.setEphemeralServers.mockReset(); });
  const par = { alias: 'casa', owner: 'Ana', session: 'api', address: 'https://ana.ts.net:8443', level: 'view', pending: false };
  const linha = (externos: unknown[]) => JSON.stringify([{ name: 'hangar', state: 'idle', pair_externals: externos }]);

  it('par novo na lista relê os pares do servidor e monta a entrada temporária; o par sumir avisa', async () => {
    pares.listExternalPairs.mockResolvedValue([{ local_session: 'hangar', alias: 'casa', owner: 'Ana', session: 'api',
      address: 'https://ana.ts.net:8443', token: 'tp' }]);
    sessionsStore.retain();
    streams.get('lan')!.get('sessions')!({ data: linha([par]) });
    await vi.waitFor(() => expect(pares.setEphemeralServers).toHaveBeenCalled());
    expect(pares.listExternalPairs).toHaveBeenCalledWith(expect.objectContaining({ id: 'lan' }));
    expect(pares.listExternalPairs).toHaveBeenCalledTimes(1);
    expect(pares.setEphemeralServers.mock.lastCall![0]).toEqual([expect.objectContaining({
      baseUrl: 'https://ana.ts.net:8443', token: 'tp', invite: true, ephemeral: true })]);
    // Mesma lista de novo: nada a reler.
    streams.get('lan')!.get('sessions')!({ data: linha([par]) });
    expect(pares.listExternalPairs).toHaveBeenCalledTimes(1);
    expect(sessionsStore.pairLeft).toBeNull();
    pares.listExternalPairs.mockResolvedValue([]);
    streams.get('lan')!.get('sessions')!({ data: linha([]) });
    await vi.waitFor(() => expect(sessionsStore.pairLeft?.owner).toBe('Ana'));
    await vi.waitFor(() => expect(pares.setEphemeralServers.mock.lastCall![0]).toEqual([]));
    // A lista remonta ao voltar do chat: o mesmo aviso aparece uma vez só.
    expect(sessionsStore.takePairLeft()).toBe('Ana');
    expect(sessionsStore.takePairLeft()).toBeNull();
  });

  const doServidor = (token = 'tp', session = 'api') => ({ local_session: 'hangar', alias: 'casa', owner: 'Ana', session,
    address: 'https://ana.ts.net:8443', token });

  it('quem desfez o par aqui não recebe o aviso de que o outro saiu', async () => {
    pares.listExternalPairs.mockResolvedValue([doServidor()]);
    sessionsStore.retain();
    streams.get('lan')!.get('sessions')!({ data: linha([par]) });
    await vi.waitFor(() => expect(pares.setEphemeralServers).toHaveBeenCalled());   // a leitura terminou
    sessionsStore.markLeavingPair('casa', 'api');
    pares.listExternalPairs.mockResolvedValue([]);
    streams.get('lan')!.get('sessions')!({ data: linha([]) });
    await new Promise((r) => setTimeout(r, 0));
    expect(sessionsStore.pairLeft).toBeNull();
  });

  it('saída que falhou desfaz a marca: o outro sair depois avisa', async () => {
    pares.listExternalPairs.mockResolvedValue([doServidor()]);
    sessionsStore.retain();
    streams.get('lan')!.get('sessions')!({ data: linha([par]) });
    await vi.waitFor(() => expect(pares.setEphemeralServers).toHaveBeenCalled());   // a leitura terminou
    sessionsStore.markLeavingPair('casa', 'api')();
    pares.listExternalPairs.mockResolvedValue([]);
    streams.get('lan')!.get('sessions')!({ data: linha([]) });
    await vi.waitFor(() => expect(sessionsStore.pairLeft?.owner).toBe('Ana'));
  });

  it('linha do par sumir sem o par acabar (sessão fechada, lista com problema) não avisa', async () => {
    pares.listExternalPairs.mockResolvedValue([doServidor()]);
    sessionsStore.retain();
    streams.get('lan')!.get('sessions')!({ data: linha([par]) });
    await vi.waitFor(() => expect(pares.setEphemeralServers).toHaveBeenCalled());   // a leitura terminou
    streams.get('lan')!.get('sessions')!({ data: linha([]) });
    await new Promise((r) => setTimeout(r, 0));
    expect(sessionsStore.pairLeft).toBeNull();
  });

  it('par que acabou libera o attach do token: o mesmo par voltando liga de novo', async () => {
    pares.attachGuest.mockResolvedValue(undefined);
    const dois = [doServidor('t1'), doServidor('t2', 'docs')];
    const linhaDois = linha([par, { ...par, session: 'docs' }]);
    pares.listExternalPairs.mockResolvedValue(dois);
    sessionsStore.retain();
    streams.get('lan')!.get('sessions')!({ data: linhaDois });
    await vi.waitFor(() => expect(pares.attachGuest).toHaveBeenCalledTimes(1));
    pares.listExternalPairs.mockResolvedValue([doServidor('t1')]);
    streams.get('lan')!.get('sessions')!({ data: linha([par]) });
    await vi.waitFor(() => expect(pares.setEphemeralServers).toHaveBeenCalledTimes(2));
    pares.listExternalPairs.mockResolvedValue(dois);
    streams.get('lan')!.get('sessions')!({ data: linhaDois });
    await vi.waitFor(() => expect(pares.attachGuest).toHaveBeenCalledTimes(2));
  });
});
