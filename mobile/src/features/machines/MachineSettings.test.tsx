// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { createHmac } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ vault: new Map<string, string>(), fetch: vi.fn(), alert: vi.fn(), push: vi.fn(), retain: vi.fn(() => () => {}) }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: mocks.push }) }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: vi.fn() }) }));
vi.mock('expo-secure-store', () => ({
  getItemAsync: async (key: string) => mocks.vault.get(key) ?? null,
  setItemAsync: async (key: string, value: string) => { mocks.vault.set(key, value); },
}));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  Alert: { alert: mocks.alert },
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    React.createElement('input', { 'aria-label': p.accessibilityLabel, disabled: p.editable === false, value: p.value ?? '',
      onChange: (e: { target: { value: string } }) => p.onChangeText?.(e.target.value) }),
}));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ open, children }: { open?: boolean; children: ReactNode }) => open === false ? null : <div role="dialog">{children}</div> }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../config/Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../pair/PairAcceptSheet', () => ({ PairAcceptSheet: () => null }));
vi.mock('../../stores/sessions', async () => {
  const { create } = await import('zustand');
  return { useSessions: create(() => ({ rows: [], byServer: [], retain: mocks.retain })) };
});

import Maquinas from '../../../app/config/maquinas';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

const machine = { id: 'own-a', label: 'fixture-name', baseUrl: 'http://10.0.0.2:8765', token: 'secret-a' };
const config = { campos: { term_origins: { valor: 'http://old.test', origem: 'app', definido: true } }, somente_leitura: { lan_bind_ip: '0.0.0.0', port: 8765, public_url: 'https://public.test' } };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
function identityProof(url: string, identificador = 'server-a', token = machine.token) {
  const challenge = new URL(url).searchParams.get('desafio')!;
  return json({ identificador, prova: createHmac('sha256', token).update(`${challenge}|${identificador}`).digest('hex') });
}
let container: HTMLDivElement;
let root: Root;
async function render() { await act(async () => root.render(<Maquinas />)); }
async function click(label: string) {
  const button = [...container.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label || b.textContent === label);
  expect(button, label).toBeTruthy();
  await act(async () => button!.click());
}
async function type(label: string, value: string) {
  const input = container.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement;
  expect(input, label).toBeTruthy();
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}
const calls = (path: string) => mocks.fetch.mock.calls.filter(([url]) => String(url).includes(path));

beforeEach(async () => {
  mocks.fetch.mockReset(); mocks.alert.mockReset(); mocks.vault.clear(); mocks.retain.mockClear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [machine], activeId: machine.id }));
  await useServers.getState().load();
  configureApi({ getBaseUrl: () => 'http://unused.test', getToken: () => 'unused', origin: null,
    onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  mocks.fetch.mockImplementation(async (url: string) => {
    if (url.endsWith('/api/config')) return json(config);
    if (new URL(url).pathname === '/api/peers/prova') return identityProof(url);
    if (url.endsWith('/api/peers/identificador')) return json({ identificador: 'server-a', lan_url: '' });
    if (url.endsWith('/api/peers')) return json([]);
    if (url.endsWith('/api/alcance')) return json({ loopback: false, bind: '0.0.0.0', enderecos: [] });
    return json({ detail: 'unexpected_request' }, 404);
  });
  container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.useRealTimers(); });

describe('Máquinas: cadastro do aparelho', () => {
  it('conserva todo o cadastro se a URL comprovada já pertence a outra entrada', async () => {
    const saved = { ...machine, lan: { id: 'server-a', url: machine.baseUrl } };
    const duplicate = { ...saved, id: 'own-b', baseUrl: 'https://replacement.test' };
    mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [saved, duplicate], activeId: saved.id }));
    await useServers.getState().load();
    const before = structuredClone(useServers.getState().servers);
    await render(); await click(m.mobile_machines_configure());
    await type(m.mobile_machines_label(), 'fixture-draft');
    await type(m.mobile_machines_endpoint(), duplicate.baseUrl);
    await click(m.mobile_machines_save());

    expect(useServers.getState().servers).toEqual(before);
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers).toEqual(before);
    expect(container.textContent).toContain(m.mobile_machines_duplicate_address());
    expect((container.querySelector(`input[aria-label="${m.mobile_machines_endpoint()}"]`) as HTMLInputElement).value).toBe(duplicate.baseUrl);
  });

  it('guarda a identidade comprovada durante a edição quando o cadastro antigo ainda não tinha uma', async () => {
    await render(); await click(m.mobile_machines_configure());
    await type(m.mobile_machines_endpoint(), 'https://replacement.test');
    await click(m.mobile_machines_save());
    expect(useServers.getState().servers[0].lan).toEqual({ url: '', id: 'server-a' });
    await act(async () => useServers.getState().load());
    expect(useServers.getState().servers[0].lan?.id).toBe('server-a');
  });

  it('abre a edição sem carregar configurações remotas até a pessoa abrir o diagnóstico', async () => {
    await render(); await click(m.mobile_machines_configure());
    expect(calls('/api/alcance')).toHaveLength(0);
    expect(calls('/api/config')).toHaveLength(0);
    expect(calls('/api/peers')).toHaveLength(0);

    await click(m.mobile_machines_more());
    await click(m.mobile_machines_technical());
    expect(calls('/api/alcance')).toHaveLength(1);
  });

  it('corrige a URL com identidade conhecida mesmo quando o endereço antigo não responde', async () => {
    const saved = { ...machine, lan: { id: 'server-a', url: machine.baseUrl } };
    mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [saved], activeId: saved.id }));
    await useServers.getState().load();
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url === `${machine.baseUrl}/api/peers/identificador`) throw new TypeError('fixture-old-address-offline');
      return original(url, init);
    });
    await render(); await click(m.mobile_machines_configure());
    await type(m.mobile_machines_endpoint(), 'https://replacement.test');
    await click(m.mobile_machines_save());

    expect(useServers.getState().servers[0].baseUrl).toBe('https://replacement.test');
    const destination = mocks.fetch.mock.calls.filter(([url]) => String(url).startsWith('https://replacement.test'));
    expect(new URL(destination[0][0]).pathname).toBe('/api/peers/prova');
    expect(new Headers(destination[0][1]?.headers).has('Authorization')).toBe(false);
    expect(destination.some(([url, init]) => url === 'https://replacement.test/api/config'
      && new Headers(init?.headers).get('Authorization') === `Bearer ${machine.token}`)).toBe(true);
  });

  it('edita o apelido e persiste sem recriar a máquina nem trocar a seleção', async () => {
    await render(); await click(m.mobile_machines_configure());
    await type(m.mobile_machines_label(), 'fixture-renamed');
    await click(m.mobile_machines_save());
    expect(useServers.getState().servers[0]).toEqual({ ...machine, label: 'fixture-renamed' });
    expect(useServers.getState().activeId).toBe(machine.id);
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers[0].label).toBe('fixture-renamed');
  });

  it('não envia token enquanto digita URL e preserva cadastro e rascunho se o teste falha', async () => {
    await render(); await click(m.mobile_machines_configure());
    await type(m.mobile_machines_label(), 'fixture-draft');
    await type(m.mobile_machines_endpoint(), 'https://new.test');
    expect(mocks.fetch.mock.calls.some(([url]) => String(url).startsWith('https://new.test'))).toBe(false);
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => url === 'https://new.test/api/config' ? json({ detail: 'fixture-refused' }, 403) : original(url, init));
    await click(m.mobile_machines_save());
    expect(useServers.getState().servers[0]).toEqual(machine);
    expect(container.textContent).toContain('fixture-refused');
    expect((container.querySelector(`input[aria-label="${m.mobile_machines_endpoint()}"]`) as HTMLInputElement).value).toBe('https://new.test');
    expect(calls('/api/config').at(-1)?.[0]).toBe('https://new.test/api/config');
  });

  it.each([
    { name: 'mesmo-identificador-hmac-falso', known: 'owner', target: 'owner', token: 'attacker', error: '' },
    { name: 'outra-identidade', known: 'owner', target: 'other', token: machine.token, error: '' },
    { name: 'identidade-atual-ausente', known: '', target: 'owner', token: machine.token, error: '' },
    { name: 'identidade-destino-ausente', known: 'owner', target: '', token: machine.token, error: '' },
    { name: 'leitura-atual-falhou', known: 'owner', target: 'owner', token: machine.token, error: 'current' },
    { name: 'prova-destino-falhou', known: 'owner', target: 'owner', token: machine.token, error: 'target' },
  ])('recusa trocar URL sem comprovar identidade, sem enviar token: $name', async ({ known, target, token, error }) => {
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/identificador')) {
        if (error === 'current') return json({ detail: 'fixture-id-unavailable' }, 503);
        return json({ identificador: known, lan_url: '' });
      }
      if (url.startsWith('https://replacement.test/api/peers/prova?')) {
        return error === 'target' ? json({ detail: 'fixture-proof-unavailable' }, 503) : identityProof(url, target, token);
      }
      return original(url, init);
    });
    await render(); await click(m.mobile_machines_configure());
    await type(m.mobile_machines_label(), 'fixture-draft');
    await type(m.mobile_machines_endpoint(), 'https://replacement.test');
    expect(mocks.fetch.mock.calls.some(([url]) => String(url).startsWith('https://replacement.test'))).toBe(false);
    await click(m.mobile_machines_save());
    const destination = mocks.fetch.mock.calls.filter(([url]) => String(url).startsWith('https://replacement.test'));
    expect(destination.every(([, init]) => !new Headers(init?.headers).has('Authorization'))).toBe(true);
    expect(destination.filter(([url]) => String(url).endsWith('/api/config'))).toHaveLength(0);
    expect(destination).toHaveLength(known && error !== 'current' ? 1 : 0);
    expect(useServers.getState().servers[0]).toEqual(machine);
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers[0]).toEqual(machine);
    expect((container.querySelector(`input[aria-label="${m.mobile_machines_endpoint()}"]`) as HTMLInputElement).value).toBe('https://replacement.test');
    expect((container.querySelector(`input[aria-label="${m.mobile_machines_label()}"]`) as HTMLInputElement).value).toBe('fixture-draft');
    if (error === 'current') expect(container.textContent).toContain('fixture-id-unavailable');
    else if (!known) expect(container.textContent).toContain(m.mobile_machines_known_identifier_required());
    else expect(container.textContent).toContain(m.mobile_machines_identity_unconfirmed());
  });

  it('só envia token após a prova e só salva URL depois de testar o acesso', async () => {
    await render(); await click(m.mobile_machines_configure());
    let resolveProof!: (response: Response) => void;
    let resolveConfig!: (response: Response) => void;
    const pendingProof = new Promise<Response>((done) => { resolveProof = done; });
    const pendingConfig = new Promise<Response>((done) => { resolveConfig = done; });
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.startsWith('https://same-machine.test/api/peers/prova?')) return pendingProof;
      if (url === 'https://same-machine.test/api/config') return pendingConfig;
      return original(url, init);
    });
    await type(m.mobile_machines_endpoint(), 'https://same-machine.test');
    await click(m.mobile_machines_save());
    const destination = () => mocks.fetch.mock.calls.filter(([url]) => String(url).startsWith('https://same-machine.test'));
    expect(destination()).toHaveLength(1);
    const [proofUrl, proofInit] = destination()[0];
    expect(new URL(proofUrl).pathname).toBe('/api/peers/prova');
    expect(new Headers(proofInit?.headers).has('Authorization')).toBe(false);
    expect(proofInit?.credentials).toBe('omit');
    expect(useServers.getState().servers[0]).toEqual(machine);
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers[0]).toEqual(machine);
    await act(async () => resolveProof(identityProof(proofUrl)));
    expect(destination()).toHaveLength(2);
    expect(destination()[1][0]).toBe('https://same-machine.test/api/config');
    expect(new Headers(destination()[1][1]?.headers).get('Authorization')).toBe(`Bearer ${machine.token}`);
    expect(useServers.getState().servers[0]).toEqual(machine);
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers[0]).toEqual(machine);
    await act(async () => resolveConfig(json(config)));
    expect(useServers.getState().servers[0]).toEqual({ ...machine, baseUrl: 'https://same-machine.test', lan: { url: '', id: 'server-a' } });
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers[0].baseUrl).toBe('https://same-machine.test');
    expect(destination().slice(1).every(([, init]) => new Headers(init?.headers).get('Authorization') === `Bearer ${machine.token}`)).toBe(true);
  });
});

describe('Máquinas: ajustes no servidor', () => {
  it('permite editar URL depois de salvar um novo identificador na mesma folha', async () => {
    await act(async () => useServers.getState().rememberLan(machine.id, { id: 'server-a', url: machine.baseUrl }));
    let identifier = 'server-a';
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/identificador') && init?.method === 'PUT') {
        identifier = 'new-id';
        return json({ identificador: identifier, avisos: [] });
      }
      if (new URL(url).pathname === '/api/peers/prova') return identityProof(url, identifier);
      return original(url, init);
    });
    await render(); await click(m.mobile_machines_configure());
    await click(m.mobile_machines_more()); await click(m.mobile_machines_technical());
    await type(m.mobile_machines_identifier(), 'new-id');
    await click(m.mobile_machines_save_identifier());
    expect(useServers.getState().servers[0].lan?.id).toBe('new-id');
    await type(m.mobile_machines_endpoint(), 'https://replacement.test');
    await click(m.mobile_machines_save());
    expect(useServers.getState().servers[0].baseUrl).toBe('https://replacement.test');
  });

  it('não habilita o identificador após uma leitura recusada', async () => {
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => url.endsWith('/identificador')
      ? json({ detail: 'fixture-id-refused' }, 403) : original(url, init));
    await render(); await click(m.mobile_machines_configure());
    await click(m.mobile_machines_more()); await click(m.mobile_machines_technical());
    const input = container.querySelector(`input[aria-label="${m.mobile_machines_identifier()}"]`) as HTMLInputElement;
    expect(input.disabled).toBe(true);
    expect(container.textContent).toContain('fixture-id-refused');
    expect(calls('/identificador').filter(([, init]) => init?.method === 'PUT')).toHaveLength(0);
  });

  it('grava identificador explicitamente, mostra os avisos e não altera apelido antigo', async () => {
    await act(async () => useServers.getState().update(machine.id, { label: '10' }));
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => init?.method === 'PUT' && url.endsWith('/identificador')
      ? json({ identificador: 'new-id', avisos: [{ id: 'peer-b', estado: 'falhou', motivo: 'fixture-peer-warning' }] }) : original(url, init));
    await render(); await click(m.mobile_machines_configure());
    await click(m.mobile_machines_more()); await click(m.mobile_machines_technical());
    expect(useServers.getState().servers[0].label).toBe('10');
    await type(m.mobile_machines_identifier(), 'new-id');
    expect(calls('/identificador').filter(([, init]) => init?.method === 'PUT')).toHaveLength(0);
    await click(m.mobile_machines_save_identifier());
    expect(JSON.parse(calls('/identificador').at(-1)![1].body)).toEqual({ identificador: 'new-id' });
    expect(container.textContent).toContain('fixture-peer-warning');
  });

  it('mantém o rascunho de origens no erro, mostra configuração só leitura e diagnóstico real', async () => {
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/api/config') && init?.method === 'POST') return json({ detail: 'fixture-origins-refused' }, 400);
      if (url.endsWith('/api/alcance')) return json({ loopback: true, bind: '127.0.0.1', enderecos: [
        { tipo: 'rede_local', url: 'http://192.168.0.2:8765', estado: 'falhou', tempo_ms: 23, motivo: 'fixture-firewall' },
      ] });
      return original(url, init);
    });
    await render(); await click(m.mobile_machines_configure());
    await click(m.mobile_machines_more()); await click(m.mobile_machines_technical());
    expect(container.querySelector(`input[aria-label="${m.mobile_machines_bind()}"]`)).toBeNull();
    expect(container.textContent).toContain('https://public.test');
    expect(container.textContent).toContain('falhou · 23 ms · fixture-firewall');
    await type(m.mobile_machines_term_origins(), 'https://draft.test');
    expect(calls('/api/config').filter(([, init]) => init?.method === 'POST')).toHaveLength(0);
    await click(m.mobile_machines_save_origins());
    expect(container.textContent).toContain('fixture-origins-refused');
    expect((container.querySelector(`input[aria-label="${m.mobile_machines_term_origins()}"]`) as HTMLInputElement).value).toBe('https://draft.test');
    expect(JSON.parse(calls('/api/config').at(-1)![1].body)).toEqual({ term_origins: 'https://draft.test' });
  });
});

describe('Máquinas: conexão e saída', () => {
  it('mostra a causa de acesso recusado dentro do editor em vez de chamar de fora do ar', async () => {
    useSessions.setState({ byServer: [{ server: machine, sessions: [], loaded: true, error: 'fixture-access-refused' }] });
    await render(); await click(m.mobile_machines_configure());

    expect(container.querySelector('[role="dialog"]')?.textContent).toContain('fixture-access-refused');
  });

  it('usa loaded/error da lista compartilhada, inclusive com erro e lista anterior carregada', async () => {
    useSessions.setState({ byServer: [{ server: machine, sessions: [], loaded: true, error: 'offline' }] });
    await render();
    expect(container.querySelector(`[aria-label="${machine.label}, ${m.mobile_machines_offline()}"]`)).toBeTruthy();
    await act(async () => useSessions.setState({ byServer: [{ server: machine, sessions: [], loaded: true, error: null }] }));
    expect(container.querySelector(`[aria-label="${machine.label}, ${m.mobile_machines_connected()}"]`)).toBeTruthy();
    expect(mocks.retain).toHaveBeenCalledTimes(1);
  });

  it('sai só do aparelho, por ação nomeada e confirmada, sem encerrar sessões', async () => {
    await render(); await click(m.mobile_machines_configure()); await click(m.mobile_machines_more()); await click(m.mobile_machines_leave());
    expect(useServers.getState().servers).toHaveLength(1);
    expect(mocks.alert.mock.calls.at(-1)![1]).toBe(m.mobile_machines_leave_body({ name: machine.label }));
    const accept = mocks.alert.mock.calls.at(-1)![2].find((b: { style?: string }) => b.style === 'destructive').onPress;
    await act(async () => { accept(); accept(); });
    expect(useServers.getState().servers).toHaveLength(0);
    expect(mocks.fetch.mock.calls.filter(([, init]) => init?.method && init.method !== 'GET')).toHaveLength(0);
  });

  it.each([{ kind: 'close', label: m.sessao_fechar }, { kind: 'cancel', label: m.comum_cancelar }])('fechar a folha por $kind cancela a prova atrasada sem enviar token nem persistir', async ({ label: closeLabel }) => {
    await render(); await click(m.mobile_machines_configure());
    let resolve!: (response: Response) => void;
    const pending = new Promise<Response>((done) => { resolve = done; });
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => url.startsWith('https://late.test/api/peers/prova?')
      ? pending : original(url, init));
    await type(m.mobile_machines_endpoint(), 'https://late.test'); await click(m.mobile_machines_save());
    const destination = () => mocks.fetch.mock.calls.filter(([url]) => String(url).startsWith('https://late.test'));
    expect(destination()).toHaveLength(1);
    const [proofUrl, proofInit] = destination()[0];
    expect(new URL(proofUrl).pathname).toBe('/api/peers/prova');
    expect(proofInit?.signal?.aborted).toBe(false);
    await click(closeLabel());
    expect(proofInit?.signal?.aborted).toBe(true);
    await act(async () => resolve(identityProof(proofUrl)));
    expect(destination()).toHaveLength(1);
    expect(destination().every(([, init]) => !new Headers(init?.headers).has('Authorization'))).toBe(true);
    expect(useServers.getState().servers[0]).toEqual(machine);
    expect(JSON.parse(mocks.vault.get('cp_servers_v1')!).servers[0]).toEqual(machine);
  });
});
