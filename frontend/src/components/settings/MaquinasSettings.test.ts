// @vitest-environment happy-dom
// Round 1/2 da 4b: logout idempotente — Sair e remover-último chamam onLogout UMA vez; enquanto a
// Promise anda, as portas de saída ficam bloqueadas; rejeição vira erro visível recuperável (o
// clear de credenciais é dono do App/lib/logout.ts, este componente não chama).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import MaquinasSettings from './MaquinasSettings.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import * as auth from '../../lib/auth';
import * as api from '../../lib/api';
import type { Server } from '../../lib/auth';

let mudouCb: (() => void) | null = null;
// importOriginal mantém REAIS os helpers de remoção (serverFingerprint/snapshotRemocao/
// removalStillMatches) — o componente precisa deles funcionando pra revisar a entidade; o resto
// (mutadores/leitores de localStorage) fica mockado.
vi.mock('../../lib/auth', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/auth')>();
  return {
    ...real,
    serverColor: () => '#fff',
    listServers: vi.fn(),
    getActiveId: vi.fn(),
    selectServer: vi.fn(),
    renameServer: vi.fn(),
    updateServer: vi.fn(() => true),
    removeServer: vi.fn(),
    onServersChanged: vi.fn((cb: () => void) => { mudouCb = cb; return () => {}; }),
  };
});
vi.mock('../../lib/sessionsStore.svelte', () => ({
  sessionsStore: { refreshServers: vi.fn(), reconnect: vi.fn() },
}));
vi.mock('../../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));
vi.mock('../../lib/peers', () => ({
  // Distingue o próprio servidor (null ou srv-a) de outro — Task 4: carregarIdsNavegador chama
  // getIdentificador por Server real, não só pelo apiTarget da aba.
  getIdentificador: vi.fn(async (alvo: Server | null) => ({ identificador: alvo && alvo.id !== 'srv-a' ? `id-${alvo.id}` : '' })),
  setIdentificador: vi.fn(async (v: string) => ({ identificador: v })),
  listarPeers: vi.fn(async () => []),
  gravarPeer: vi.fn(async (d: unknown) => [d]),
  removerPeer: vi.fn(async () => []),
  removerPeerDoisLados: vi.fn(async () => true),
  checkPeer: vi.fn(async () => ({ estado: 'ok' })),
}));
vi.mock('../../lib/alcance', () => ({
  alcanceDoServidor: vi.fn(() => new Promise(() => {})),
  pareamentoDoServidor: vi.fn(),
  fraseDeEstado: () => '',
}));

const authMock = vi.mocked(auth);
const apiMock = vi.mocked(api);
const peersMock = vi.mocked(await import('../../lib/peers'));
const SRV: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

let onLogoutResolve: (() => void) | null = null;
function onLogoutDeferred() {
  return new Promise<void>((res) => { onLogoutResolve = res; });
}

let onLogoutCalls: ReturnType<typeof vi.fn<() => Promise<void>>>;

function montar(over: { onLogout?: () => Promise<void>; servers?: Server[] } = {}) {
  authMock.listServers.mockReturnValue(over.servers ?? [SRV]);
  authMock.getActiveId.mockReturnValue(SRV.id);
  apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));   // fica pendente (irrelevante)
  onLogoutCalls = vi.fn<() => Promise<void>>(over.onLogout ?? onLogoutDeferred);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(MaquinasSettings, {
    target: el,
    props: { resolvedServer: SRV, apiTarget: null, onPickTarget: vi.fn(), onLogout: onLogoutCalls },
  });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); onLogoutResolve = null; mudouCb = null; });

describe('MaquinasSettings — logout idempotente', () => {
  it('Sair + remover-último durante a Promise chamam onLogout UMA vez', async () => {
    const t = montar();
    // 1) Sair -> confirmação -> logout começa (Promise pendente). O ModalDialog vive num portal
    // pro <body>, então o diálogo se busca no document, não dentro de t.el.
    t.el.querySelector<HTMLButtonElement>('.ss-danger')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    // 2) Durante a Promise: desmarcar "Acompanhar" (a linha do próprio servidor) fica bloqueado —
    // diálogo nem abre (mesmo guard de logoutInFlight que abrirRemocao já tinha).
    const del = t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-a"] .mq-acompanhar');
    expect(del).not.toBeNull();
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
    // Durante a Promise: Sair principal disabled (clique é no-op) e desmarcar Acompanhar nem abre diálogo
    const sairDeNovo = t.el.querySelector<HTMLButtonElement>('.ss-danger')!;
    expect(sairDeNovo.disabled).toBe(true);
    sairDeNovo.click();
    t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-a"] .mq-acompanhar')!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();
    // Resolve: segue 1 chamada só
    onLogoutResolve!();
    await Promise.resolve(); await Promise.resolve();
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('remoção revalida fingerprint: servidor mudou no sync → aviso, sem remover', async () => {
    const t = montar();
    t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-a"] .mq-acompanhar')!.click();
    await tick();
    // o sync alterou o servidor entre o diálogo e o clique (onServersChanged sobe a versão local)
    authMock.listServers.mockReturnValue([{ ...SRV, token: 'novo-token' }]);
    mudouCb?.();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(authMock.removeServer).not.toHaveBeenCalled();
    const aviso = t.el.querySelector<HTMLElement>('.ss-aviso');
    expect(aviso?.innerText).toContain(m.config_servidores_aviso_lista_mudou());
    expect(aviso?.getAttribute('role')).toBe('status');
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
    expect(aviso?.innerText).toContain(m.config_servidores_sair_erro());
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

describe('MaquinasSettings — remoção com fingerprint + revision (round 4)', () => {
  async function confirmarRemocao(t: { el: HTMLElement }) {
    t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-a"] .mq-acompanhar')!.click();
    await tick();
    // servers.length === 1 aqui: o diálogo acrescenta o aviso de que é a única saída (Sair).
    expect(document.querySelector('.confirm-card')!.textContent).toContain(m.config_servidores_voltar());
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
  }

  it('entidade inalterada: removeServer é chamado UMA vez com o id certo', async () => {
    const t = montar();
    authMock.getActiveId.mockReturnValue('outro-id');   // remover SRV não é remover o ativo -> sem reload
    await confirmarRemocao(t);
    expect(authMock.removeServer).toHaveBeenCalledTimes(1);
    expect(authMock.removeServer).toHaveBeenCalledWith(SRV.id);
    unmount(t.comp);
  });

  it('servidor ausente entre diálogo e clique: não remove, aviso role=status', async () => {
    const t = montar();
    t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-a"] .mq-acompanhar')!.click();
    await tick();
    authMock.listServers.mockReturnValue([]);   // apagado noutro aparelho ANTES do clique
    await tick();                                // revision inalterada (sem mudouCb)
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(authMock.removeServer).not.toHaveBeenCalled();
    const aviso = t.el.querySelector<HTMLElement>('.ss-aviso');
    expect(aviso?.innerText).toContain(m.config_servidores_aviso_ja_removido());
    expect(aviso?.getAttribute('role')).toBe('status');
    unmount(t.comp);
  });

  it('último servidor removido em Settings chama onLogout UMA vez', async () => {
    // removeServer de verdade esvazia a lista -> o controller vê "zerou" e dispara o logout global.
    authMock.removeServer.mockImplementation(() => {
      authMock.listServers.mockReturnValue([]);
    });
    const t = montar();
    await confirmarRemocao(t);
    expect(authMock.removeServer).toHaveBeenCalledTimes(1);
    expect(onLogoutCalls).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('cancelar a confirmação devolve o foco à caixa Acompanhar (restauração segura)', async () => {
    const t = montar();
    authMock.getActiveId.mockReturnValue('outro-id');   // remover não é remover o ativo -> sem reload
    const del = t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-a"] .mq-acompanhar')!;
    del.focus();   // happy-dom não move foco no click() — o gatilho precisa de focus explícito
    del.click();
    await tick(); await tick();
    // o foco saiu do gatilho (foi pra dentro do diálogo)
    expect(document.activeElement).not.toBe(del);
    const cancel = document.querySelector<HTMLElement>('.confirm-card')!
      .querySelector<HTMLButtonElement>('.c-btn:not(.c-danger)')!;
    cancel.click();
    await tick();
    // ConfirmDialog desmontou (pendingRemoval=null) e o ModalDialog restaurou o foco pro gatilho
    expect(document.activeElement).toBe(del);
    unmount(t.comp);
  });
});

// ── Seções da Task 5: identificador desta máquina + máquinas que este servidor alcança ───
describe('MaquinasSettings — identificador e peers (Task 5)', () => {
  async function esperarCarga() {
    // A Task 8 encadeia gravação + checagens + listagem no registrar: microtasks em
    // quantidade variável. settled() espera TODAS as pendentes + re-renders (Svelte 5).
    const { settled } = await import('svelte');
    await settled();
    await Promise.resolve(); await Promise.resolve(); await tick();
  }

  it('estado 1 do mock: sem identificador, aviso visível e lista sem como registrar', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.listarPeers.mockResolvedValue([]);
    const t = montar();
    await esperarCarga();
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(null);  // alvo null = servidor ativo
    // Esta máquina: rótulo + legenda + aviso, campo com borda de aviso e placeholder
    expect(t.el.textContent).toContain(m.peers_esta_maquina());
    expect(t.el.textContent).toContain(m.peers_legenda_identificador());
    const aviso = t.el.querySelector('.id-aviso');
    expect(aviso?.textContent).toContain(m.peers_aviso_nao_definido());
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo');
    expect(campo?.classList.contains('vazio')).toBe(true);
    expect(campo?.value).toBe('');
    expect(campo?.getAttribute('placeholder')).toContain('casa');
    // lista unificada: só a linha desta máquina (etiqueta, sem "servidores se falam" pra registrar)
    expect(t.el.querySelectorAll('.mq-linha').length).toBe(1);
    expect(t.el.querySelector('.mq-linha[data-chave="srv:srv-a"] .mq-falar')).toBeNull();
    expect(t.el.querySelector('.pr-btn.primaria')).toBeNull();
    unmount(t.comp);
  });

  it('estado 2 do mock: identificador definido, peer listado com endereço e Acompanhar/Falar', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: '••••reto' },
    ]);
    const t = montar();
    await esperarCarga();
    expect(t.el.textContent).toContain(m.peers_identificador_definido({ nome: 'casa' }));
    expect(t.el.querySelector('.id-aviso')).toBeNull();          // definido: sem aviso
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo');
    expect(campo?.classList.contains('vazio')).toBe(false);
    expect(campo?.value).toBe('casa');
    // notebook não casa com nenhum servidor conhecido do navegador: linha só-do-servidor
    const linha = t.el.querySelector<HTMLElement>('.mq-linha[data-chave="peer:notebook"]')!;
    expect(linha).not.toBeNull();
    expect(linha.querySelector('.mq-url')?.textContent).toContain('192.168.0.77');
    expect(linha.querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(true);
    unmount(t.comp);
  });

  it('Enter no campo grava o identificador; inválido mostra a dica e não grava', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.setIdentificador.mockResolvedValue({ identificador: 'casa' });
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledWith(null, 'casa');
    expect(t.el.querySelector('.id-aviso')).toBeNull();          // virou definido
    // inválido (maiúscula): a dica É a regra, e nada vai pro backend
    campo.value = 'Casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(t.el.querySelector('.id-erro')?.textContent).toContain(m.peers_identificador_dica({ exemplos: 'casa, notebook' }));
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('marcar "servidores se falam" registra as duas pontas sem diálogo, e a lista atualiza', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValueOnce([]).mockResolvedValue([{ id: 'nb', base_url: 'http://b', token: '••' }]);
    peersMock.checkPeer.mockResolvedValue({ estado: 'ok' });
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperarCarga();
    const cb = el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-b"] .mq-falar')!;
    expect(cb.disabled).toBe(false);
    cb.click();
    await esperarCarga();
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(null, { id: 'nb', base_url: 'http://b', token: 'tb' });
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'http://b' }), expect.objectContaining({ id: 'casa' }));
    expect(el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-b"] .mq-falar')!.checked).toBe(true);
    unmount(comp);
  });

  it('desmarcar "servidores se falam" pede confirmação e remove nas pontas que dá', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([{ id: 'vps', base_url: 'https://vps', token: '••' }]);
    const { el, comp } = montar();
    await tick(); await tick(); await tick();
    el.querySelector<HTMLInputElement>('.mq-linha[data-chave="peer:vps"] .mq-falar')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_remover_peer_so_aqui());
    [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.peers_remover())!.click();
    await tick(); await tick();
    expect(peersMock.removerPeerDoisLados).toHaveBeenCalledWith(null, 'vps', null);
    unmount(comp);
  });

  it('fala com o servidor ESCOLHIDO na aba, não com o ativo', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([]);
    const outro = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' } as Server;
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props: {
      resolvedServer: outro, apiTarget: outro, onPickTarget: vi.fn(), onLogout: vi.fn() } });
    await esperarCarga();
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(outro);
    expect(peersMock.listarPeers).toHaveBeenCalledWith(outro);
    // Task 4: carregarIdsNavegador também busca o id de cada máquina do NAVEGADOR (listServers()
    // residual desta suíte ainda devolve [SRV] — nenhum montar() rodou pra trocar isso aqui).
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(SRV);
    unmount(comp as never);
  });

  it('servidor indisponível não carrega nada', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props: {
      resolvedServer: null, apiTarget: null, onPickTarget: vi.fn(), onLogout: vi.fn() } });
    await esperarCarga();
    expect(peersMock.getIdentificador).not.toHaveBeenCalled();
    expect(peersMock.listarPeers).not.toHaveBeenCalled();
    unmount(comp as never);
  });

  it('falha ao listar peers aparece com nome, e não some quando não é Error', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar();
    await esperarCarga();
    expect(t.el.querySelector('.id-erro')?.textContent).toContain(m.falha_conexao());
    unmount(t.comp);

    peersMock.listarPeers.mockRejectedValueOnce('caiu');   // rejeição que NÃO é Error
    const t2 = montar();
    await esperarCarga();
    expect(t2.el.querySelector('.id-erro')?.textContent).toContain(m.erro_desconhecido());
    unmount(t2.comp);
  });

  it('peer existente aparece mesmo sem identificador, e a tela não diz "nenhuma"', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.listarPeers.mockResolvedValue([
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: '••••reto' },
    ]);
    const t = montar();
    await esperarCarga();
    const linha = t.el.querySelector<HTMLElement>('.mq-linha[data-chave="peer:notebook"]')!;
    expect(linha).not.toBeNull();
    expect(t.el.textContent).not.toContain(m.maquinas_vazio());
    expect(linha.querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);   // sem identificador, sem falar
    unmount(t.comp);
  });

  it('identificador definido e lista vazia: mostra "nenhuma máquina" (backend não vê nada, servers vazio)', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([]);
    const t = montar({ servers: [] });
    await esperarCarga();
    expect(t.el.textContent).toContain(m.maquinas_vazio());
    unmount(t.comp);
  });

  it('enquanto a listagem não volta, a tela não afirma que não há nenhum', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockReturnValue(new Promise(() => {}));   // nunca resolve
    const t = montar({ servers: [] });
    await esperarCarga();
    expect(t.el.textContent).not.toContain(m.maquinas_vazio());
    expect(t.el.textContent).toContain(m.comum_carregando());
    unmount(t.comp);
  });

  it('Enter não dispara dois PUT: o blur causado pelo disabled é ignorado', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    let resolver!: (v: { identificador: string }) => void;
    peersMock.setIdentificador.mockReturnValueOnce(new Promise((r) => { resolver = r; }));
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    campo.dispatchEvent(new Event('blur'));      // o blur que o disabled provoca no navegador
    await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    resolver({ identificador: 'casa' });
    await tick(); await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('o campo não é desabilitado durante a gravação (o foco não sai dele)', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    let resolver!: (v: { identificador: string }) => void;
    peersMock.setIdentificador.mockReturnValueOnce(new Promise((r) => { resolver = r; }));
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(campo.disabled).toBe(false);     // disabled tiraria o foco do campo
    expect(campo.readOnly).toBe(true);      // e readonly impede a edição sem tirar
    resolver({ identificador: 'casa' });
    await tick(); await tick();
    expect(campo.readOnly).toBe(false);
    unmount(t.comp);
  });

  it('resposta atrasada do alvo anterior nao sobrescreve a tela do alvo atual', async () => {
    const A = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' } as Server;
    const B = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' } as Server;
    authMock.listServers.mockReturnValue([A, B]);
    authMock.getActiveId.mockReturnValue('srv-a');
    apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));
    apiMock.getPushSettingsForServer.mockReturnValue(new Promise(() => {}));
    const adiado = <T,>() => { let r!: (v: T) => void; const p = new Promise<T>((x) => (r = x)); return { p, r }; };
    const idA = adiado<{ identificador: string }>(), peersA = adiado<unknown[]>();
    const idB = adiado<{ identificador: string }>(), peersB = adiado<unknown[]>();
    peersMock.getIdentificador.mockImplementation((alvo: Server | null) =>
      (alvo?.id === 'srv-b' ? idB.p : idA.p) as never);
    peersMock.listarPeers.mockImplementation((alvo: Server | null) =>
      (alvo?.id === 'srv-b' ? peersB.p : peersA.p) as never);

    const props = criarProps({ resolvedServer: A as Server | null, apiTarget: A as Server | null,
                               onPickTarget: vi.fn(), onLogout: vi.fn() });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props });
    const passos = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); await tick(); await tick(); };
    await passos();

    props.resolvedServer = B; props.apiTarget = B;       // clique na linha do servidor B
    await passos();
    idB.r({ identificador: 'bbb' });
    peersB.r([{ id: 'peer-de-B', base_url: 'http://b-peer', token: '***' }]);
    await passos();

    idA.r({ identificador: 'aaa' });                      // A chega ATRASADO
    peersA.r([{ id: 'peer-de-A', base_url: 'http://a-peer', token: '***' }]);
    await passos();

    expect(el.querySelector<HTMLInputElement>('.id-campo')!.value).toBe('bbb');
    expect(el.textContent).toContain('peer-de-B');
    expect(el.textContent).not.toContain('peer-de-A');
    unmount(comp as never);
  });

  it('falha ao LER o identificador aparece com nome', async () => {
    peersMock.getIdentificador.mockRejectedValueOnce(new Error('Failed to fetch'));
    peersMock.listarPeers.mockResolvedValue([]);
    const t = montar();
    await esperarCarga();
    expect(t.el.querySelector('.id-erro[role="alert"]')?.textContent).toContain(m.falha_conexao());
    unmount(t.comp);
  });

  it('falha ao GRAVAR o identificador aparece com nome', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: '' });
    peersMock.setIdentificador.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar();
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa'; campo.dispatchEvent(new Event('input')); await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await esperarCarga();
    expect(t.el.querySelector('.id-erro[role="alert"]')?.textContent).toContain(m.falha_conexao());
    unmount(t.comp);
  });

  it('falha ao registrar um peer aparece com nome', async () => {
    // registrarPeerDoisLados nunca lança (cada chamada interna tem catch próprio) — o erro que
    // chega na tela vem da RELISTAGEM depois do registro, não do próprio registro.
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-b"] .mq-falar')!.click();
    await esperarCarga();
    expect(t.el.querySelector('.id-erro')?.textContent).toContain(m.falha_conexao());
    unmount(t.comp);
  });

  it('registrar volta a funcionar no alvo novo (a chamada anterior travada não bloqueia)', async () => {
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' } as Server;
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' } as Server;
    const C: Server = { id: 'srv-c', label: 'C', baseUrl: 'http://c', token: 'tc' } as Server;
    authMock.listServers.mockReturnValue([A, B, C]);
    authMock.getActiveId.mockReturnValue('srv-a');
    apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));
    apiMock.getPushSettingsForServer.mockReturnValue(new Promise(() => {}));
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({
      identificador: alvo?.id === 'srv-c' ? 'c' : alvo?.id === 'srv-b' ? 'b' : 'casa',
    }));
    peersMock.listarPeers.mockResolvedValue([]);
    peersMock.gravarPeer.mockReturnValue(new Promise(() => {}) as never);   // POST que pendura

    const props = criarProps({ resolvedServer: A as Server | null, apiTarget: A as Server | null,
                               onPickTarget: vi.fn(), onLogout: vi.fn() });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props });
    const passos = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); await tick(); await tick(); };
    await passos();

    // marca "servidores se falam" na máquina B, com A como alvo: o POST fica pendurado
    el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-b"] .mq-falar')!.click();
    await passos();

    props.resolvedServer = B; props.apiTarget = B;   // desiste e troca de alvo

    await passos();

    // marca "servidores se falam" na máquina C, no alvo NOVO — não fica preso à chamada anterior
    el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-c"] .mq-falar')!.click();
    await passos();

    expect(peersMock.gravarPeer.mock.calls.length).toBe(2);
    unmount(comp as never);
  });

  // Rodada 3 (parecer do revisor): selos de lado derivados do ESTADO — o sucesso não pode
  // mostrar ✗ (o glifo não é mais texto chumbado); a montagem checa a ida da lista; e o bloco
  // de correção de endereço ganha vida (testa no endereço digitado).
  async function marcarFalar(chave: string, t: { el: HTMLElement }) {
    t.el.querySelector<HTMLInputElement>(`.mq-linha[data-chave="${chave}"] .mq-falar`)!.click();
    await esperarCarga();
  }

  it('B2 — sucesso com os dois lados ok não mostra ✗ em nenhum selo; falha mostra só na volta', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://192.168.0.77:8765', token: 'segredo' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'notebook' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([]);
    peersMock.gravarPeer.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    // sucesso: ida ok + volta ok — o hint só existe quando há falha real, então sucesso é ZERO .pr-lado
    peersMock.checkPeer.mockImplementation(async () => ({ estado: 'ok' }) as never);
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    await marcarFalar('srv:srv-b', t);
    expect(t.el.querySelectorAll('.pr-lado')).toHaveLength(0);
    unmount(t.comp);

    // falha: a volta recusou → ✗ só no selo da volta, ✓ no da ida. Montagem com lista VAZIA
    // (nenhuma checagem pré-gesto consome os drives do check), e o gesto re-lista com o peer.
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'recusou', motivo: 'credencial' });
    const t2 = montar({ servers: [SRV, B] });
    await esperarCarga();
    await marcarFalar('srv:srv-b', t2);
    const l2 = [...t2.el.querySelectorAll<HTMLElement>('.pr-lado')];
    expect(l2).toHaveLength(2);
    expect(l2[0].textContent).toContain('✓');
    expect(l2[0].textContent).not.toContain('✗');
    expect(l2[1].textContent).toContain('✗');
    expect(l2[1].textContent).not.toContain('✓');
    unmount(t2.comp);
  });

  it('B3 — peers já na lista são checados na montagem: nada fica "Testando…" e checkPeer roda por peer, ida e volta', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nuvem' : 'casa' }));
    peersMock.listarPeers.mockImplementation(async (alvo: Server | null) => {
      if (alvo === B) return [{ id: 'casa', base_url: 'https://casa.ts.net', token: '••••' }];   // o que B guarda de nós
      return [
        { id: 'notebook', base_url: 'http://n:8765', token: '••••' },
        { id: 'nuvem', base_url: 'http://nv:8765', token: '••••' },
      ];
    });
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    // uma checagem de IDA por peer, com id e endereço DELE
    expect(peersMock.checkPeer).toHaveBeenCalledWith(null, 'http://n:8765', 'notebook');
    expect(peersMock.checkPeer).toHaveBeenCalledWith(null, 'http://nv:8765', 'nuvem');
    // volta de 'nuvem' (casado com o navegador B): mede pelo endereço que B guardou pra nós,
    // não pelo baseUrl do navegador (decisão 3 da spec: aqui é LAN, lá é Tailscale)
    expect(peersMock.checkPeer).toHaveBeenCalledWith(B, 'https://casa.ts.net', 'casa');
    expect(peersMock.checkPeer).toHaveBeenCalledTimes(3);
    // nenhuma linha presa em "Testando as duas pontas…"
    expect(t.el.textContent).not.toContain(m.peers_estado_testando());
    expect(t.el.querySelectorAll('.mq-linha')).toHaveLength(3);   // SRV, srv-b/nuvem, peer:notebook
    unmount(t.comp);
  });

  it('B4 — "Testar de novo" do bloco de correção registra e testa no ENDEREÇO DIGITADO', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://192.168.0.77:8765', token: 'segredo' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'notebook' : 'casa' }));
    peersMock.gravarPeer.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    // montagem com lista vazia (nada consome os drives do check); o gesto e a correção re-listam
    // com o peer — sem isto o card some junto com o bloco de correção.
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    // gesto com a volta recusando: o bloco de correção abre
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'recusou', motivo: 'credencial' });
    await marcarFalar('srv:srv-b', t);
    const campo = t.el.querySelector<HTMLInputElement>('.corrige-input');
    expect(campo).not.toBeNull();   // bloco aberto
    // o usuário digita o endereço CERTO e clica em "Testar de novo"
    campo!.value = 'http://novo:9999';
    campo!.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    t.el.querySelector<HTMLButtonElement>('.corrige .btn.primaria')!.click();
    // o gesto de correção roda contra o NOVO endereço, com o token do NAVEGADOR (hoje não rodava contra nada)
    await esperarCarga();
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(
      null,
      expect.objectContaining({ id: 'notebook', base_url: 'http://novo:9999', token: 'segredo' }),
    );
    // o bloco só fecha quando o par fecha (o r.ok do segundo giro, com o mock default ok)
    expect(t.el.querySelector('.corrige')).toBeNull();
    unmount(t.comp);
  });

  it('volta na montagem: sem registro de lá mostra o aviso específico', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockImplementation(async (alvo: Server | null) => {
      if (alvo === B) return [];   // B ainda não registrou esta máquina de volta
      return [{ id: 'nb', base_url: 'http://b', token: '••' }];
    });
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    const linha = t.el.querySelector<HTMLElement>('.mq-linha[data-chave="srv:srv-b"]')!;
    expect(linha.textContent).toContain(m.maquinas_volta_sem_registro());
    unmount(t.comp);
  });

  it('volta na montagem: volta falhou abre a correção com o endereço de lá', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockImplementation(async (alvo: Server | null) => {
      if (alvo === B) return [{ id: 'casa', base_url: 'https://casa.ts.net', token: '••' }];
      return [{ id: 'nb', base_url: 'http://b', token: '••' }];
    });
    peersMock.checkPeer.mockImplementation(async (alvo: Server | null) => (alvo === B ? { estado: 'falhou' } : { estado: 'ok' }));
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    const campo = t.el.querySelector<HTMLInputElement>('.mq-linha[data-chave="srv:srv-b"] .corrige-input');
    expect(campo).not.toBeNull();
    expect(campo!.value).toBe('https://casa.ts.net');
    unmount(t.comp);
  });
});

describe('MaquinasSettings — ordem dos blocos', () => {
  it('esta máquina vem antes da lista de máquinas', async () => {
    const { el, comp } = montar();
    await tick();
    const texto = el.textContent ?? '';
    const esta = texto.indexOf(m.peers_esta_maquina());
    const secao = texto.indexOf(m.maquinas_secao());
    expect(esta).toBeGreaterThanOrEqual(0);
    expect(esta).toBeLessThan(secao);
    expect(texto).toContain(m.maquinas_secao_legenda());
    unmount(comp);
  });

  it('sem servidor resolvido, só a lista de máquinas aparece', async () => {
    authMock.listServers.mockReturnValue([]);
    authMock.getActiveId.mockReturnValue(null);
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props: { resolvedServer: null, apiTarget: null, onPickTarget: vi.fn(), onLogout: vi.fn(async () => {}) } });
    await tick();
    expect(el.textContent).not.toContain(m.peers_esta_maquina());
    expect(el.textContent).toContain(m.maquinas_secao());
    unmount(comp as never);
  });
});
