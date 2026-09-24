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
import * as api from '@hangar/core';
import type { Server } from '../../lib/auth';

let mudouCb: (() => void) | null = null;
// importOriginal mantém REAIS os helpers de remoção (serverFingerprint/snapshotRemocao/
// removalStillMatches) — o componente precisa deles funcionando pra revisar a entidade; o resto
// (mutadores/leitores de localStorage) fica mockado.
vi.mock('../../lib/auth', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/auth')>();
  // A tela lê a lista INTEIRA (desligadas também): as duas leituras são o mesmo mock, e os testes
  // configuram por `listServers`.
  const listServers = vi.fn();
  return {
    ...real,
    serverColor: () => '#fff',
    listServers,
    listAllServers: listServers,
    getActiveId: vi.fn(),
    selectServer: vi.fn(),
    renameServer: vi.fn(),
    updateServer: vi.fn(() => true),
    removeServer: vi.fn(),
    addServer: vi.fn(() => ({ id: 'srv-novo', existed: false })),
    setServerDisabled: vi.fn(() => true),
    onServersChanged: vi.fn((cb: () => void) => { mudouCb = cb; return () => {}; }),
  };
});
vi.mock('../../lib/sessionsStore.svelte', () => ({
  sessionsStore: { refreshServers: vi.fn(), reconnect: vi.fn() },
}));
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));
vi.mock('../../lib/peers', () => ({
  // Distingue o próprio servidor (null ou srv-a) de outro — carregarIdsNavegador chama
  // getIdentificador por Server real, não só pelo apiTarget da aba.
  getIdentificador: vi.fn(async (alvo: Server | null) => ({ identificador: alvo && alvo.id !== 'srv-a' ? `id-${alvo.id}` : '' })),
  setIdentificador: vi.fn(async (v: string) => ({ identificador: v })),
  listarPeers: vi.fn(async () => []),
  gravarPeer: vi.fn(async (d: unknown) => [d]),
  removerPeer: vi.fn(async () => []),
  removerPeerDoisLados: vi.fn(async () => true),
  checkPeer: vi.fn(async () => ({ estado: 'ok' })),
  tokenDoPeer: vi.fn(async () => ({ token: 'tok-do-servidor' })),
  setPeerEnabled: vi.fn(async () => []),
  descobrirMaquinas: vi.fn(async () => []),
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
    props: { resolvedServer: SRV, apiTarget: null, onLogout: onLogoutCalls },
  });
  return { el, comp: comp as never };
}

// Janela de um teste que falhou antes do unmount fica no <body> e seria achada pelo seguinte.
// localStorage também: ids lembrados e resultados salvos de um teste abririam o seguinte.
beforeEach(() => { vi.clearAllMocks(); onLogoutResolve = null; mudouCb = null; document.body.innerHTML = ''; localStorage.clear(); });

// Os detalhes vivem em portais no <body>: abrem pelo cartão ou pela linha e se buscam no document.
// O tick antes do clique deixa o $effect da montagem rodar: ele fecha as janelas do alvo anterior.
async function abrir(el: HTMLElement, chave: string) {
  await tick();
  el.querySelector<HTMLElement>(`.sv-linha[data-chave="${chave}"]`)!.click();
  await tick();
  return document.querySelector<HTMLElement>(`.mq-linha[data-chave="${chave}"]`)!;
}
async function abrirEste(el: HTMLElement) {
  await tick();
  el.querySelector<HTMLElement>('.sv-este')!.click();
  await tick();
  return document.querySelector<HTMLElement>('.sd-dialogo')!;
}

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
    // 2) Durante a Promise: remover o próprio servidor deste aparelho fica bloqueado —
    // diálogo nem abre (mesmo guard de logoutInFlight que abrirRemocao já tinha).
    const del = (await abrirEste(t.el)).querySelector<HTMLButtonElement>('.sv-remover-este');
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
    // Durante a Promise: Sair principal disabled (clique é no-op) e remover deste aparelho nem abre diálogo
    const sairDeNovo = t.el.querySelector<HTMLButtonElement>('.ss-danger')!;
    expect(sairDeNovo.disabled).toBe(true);
    sairDeNovo.click();
    (await abrirEste(t.el)).querySelector<HTMLButtonElement>('.sv-remover-este')!.click();
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
    (await abrirEste(t.el)).querySelector<HTMLButtonElement>('.sv-remover-este')!.click();
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
    (await abrirEste(t.el)).querySelector<HTMLButtonElement>('.sv-remover-este')!.click();
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
    (await abrirEste(t.el)).querySelector<HTMLButtonElement>('.sv-remover-este')!.click();
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

  it('cancelar a confirmação devolve o foco ao botão de remover (restauração segura)', async () => {
    const t = montar();
    authMock.getActiveId.mockReturnValue('outro-id');   // remover não é remover o ativo -> sem reload
    const del = (await abrirEste(t.el)).querySelector<HTMLButtonElement>('.sv-remover-este')!;
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
    // O cartão já avisa; o detalhe traz legenda, aviso e o campo com borda de aviso e placeholder
    expect(t.el.textContent).toContain(m.peers_esta_maquina());
    expect(t.el.textContent).toContain(m.servidores_sem_identificador_curto());
    const este = await abrirEste(t.el);
    expect(este.textContent).toContain(m.peers_legenda_identificador());
    const aviso = este.querySelector('.id-aviso');
    expect(aviso?.textContent).toContain(m.peers_aviso_nao_definido());
    const campo = este.querySelector<HTMLInputElement>('.id-campo');
    expect(campo?.classList.contains('vazio')).toBe(true);
    expect(campo?.value).toBe('');
    expect(campo?.getAttribute('placeholder')).toContain('casa');
    // o próprio servidor não se repete na lista de baixo
    expect(t.el.querySelectorAll('.sv-linha').length).toBe(0);
    unmount(t.comp);
  });

  it('estado 2 do mock: identificador definido, peer listado com endereço e Acompanhar/Falar', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: '••••reto' },
    ]);
    const t = montar();
    await esperarCarga();
    expect(t.el.querySelector('.sv-id')?.textContent).toBe('casa');
    expect(t.el.textContent).not.toContain(m.servidores_sem_identificador_curto());
    const este = await abrirEste(t.el);
    expect(este.textContent).toContain(m.peers_identificador_definido({ nome: 'casa' }));
    expect(este.querySelector('.id-aviso')).toBeNull();          // definido: sem aviso
    const campo = este.querySelector<HTMLInputElement>('.id-campo');
    expect(campo?.classList.contains('vazio')).toBe(false);
    expect(campo?.value).toBe('casa');
    este.querySelector<HTMLButtonElement>('.sv-fechar')!.click();
    await tick();
    // notebook não casa com nenhum servidor conhecido do navegador: linha só-do-servidor
    const linha = await abrir(t.el, 'peer:notebook');
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
    const este = await abrirEste(t.el);
    const campo = este.querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(peersMock.setIdentificador).toHaveBeenCalledWith(null, 'casa');
    expect(este.querySelector('.id-aviso')).toBeNull();          // virou definido
    // inválido (maiúscula): a dica É a regra, e nada vai pro backend
    campo.value = 'Casa';
    campo.dispatchEvent(new Event('input'));
    await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(este.querySelector('.id-erro')?.textContent).toContain(m.peers_identificador_dica({ exemplos: 'casa, notebook' }));
    expect(peersMock.setIdentificador).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('marcar "servidores se falam" registra as duas pontas sem diálogo, e a lista atualiza', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValueOnce([]).mockResolvedValue([{ id: 'nb', base_url: 'http://b', token: '••' }]);
    // um teste anterior (ordem embaralhada) pode deixar gravarPeer pendurado num mockReturnValue —
    // clearAllMocks só limpa chamadas, nunca a implementação (vitest não reseta mockImplementation).
    peersMock.gravarPeer.mockImplementation(async (_alvo, dado) => [dado] as never);
    peersMock.checkPeer.mockResolvedValue({ estado: 'ok' });
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperarCarga();
    const cb = (await abrir(el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-falar')!;
    expect(cb.disabled).toBe(false);
    cb.click();
    await esperarCarga();
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(null, { id: 'nb', base_url: 'http://b', token: 'tb' });
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'http://b' }), expect.objectContaining({ id: 'casa' }));
    expect((await abrir(el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(true);
    unmount(comp);
  });

  it('marcar "servidores se falam" duas vezes seguidas não dispara dois registros (trava de clique duplo)', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([]);
    peersMock.gravarPeer.mockReturnValue(new Promise(() => {}) as never);   // fica pendurado: gesto em voo
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperarCarga();
    const cb = (await abrir(el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-falar')!;
    cb.click();
    cb.click();   // segundo clique com o primeiro ainda em voo
    await tick();
    expect(peersMock.gravarPeer).toHaveBeenCalledTimes(1);
    expect(document.querySelector('.mq-linha[data-chave="srv:srv-b"] .sd-testado')!.textContent).toBe(m.acesso_testando());
    unmount(comp);
  });

  it('desmarcar "servidores se falam" pede confirmação e remove nas pontas que dá', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([{ id: 'vps', base_url: 'https://vps', token: '••' }]);
    const { el, comp } = montar();
    await tick(); await tick(); await tick();
    (await abrir(el, 'peer:vps')).querySelector<HTMLInputElement>('.mq-falar')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_remover_peer_so_aqui());
    [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.peers_remover())!.click();
    await tick(); await tick();
    expect(peersMock.removerPeerDoisLados).toHaveBeenCalledWith(null, 'vps', null);
    unmount(comp);
  });

  it('desmarcar "servidores se falam" numa máquina COM navegador avisa que os dois lados saem', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([{ id: 'nb', base_url: 'http://b', token: '••' }]);
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperarCarga();
    (await abrir(el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-falar')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_remover_peer_lados());
    [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.peers_remover())!.click();
    await tick(); await tick();
    expect(peersMock.removerPeerDoisLados).toHaveBeenCalledWith(null, 'nb', B);
    unmount(comp);
  });

  it('o ✕ de uma linha SEM peer não promete o servidor na confirmação', async () => {
    // `removerLinhaConfirmado` só chama `removerPeer` sob `if (linha.peer)`. Esta é a última tela
    // antes de apagar: prometer o registro do servidor numa linha que o servidor não conhece é a
    // etiqueta divergindo da ação.
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    // identificadores diferentes: o mesmo agruparia B com o servidor escolhido numa linha só
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([]);
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperarCarga();
    (await abrir(el, 'srv:srv-b')).querySelector<HTMLButtonElement>('.mq-remover')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_remover_linha_local());
    expect(document.body.textContent).not.toContain(m.maquinas_remover_linha());
    unmount(comp);
  });

  it('o ✕ de uma linha SÓ do servidor promete o registro daqui, não o aparelho nem o lado de lá', async () => {
    // Peer sem entrada em `cp_servers` → linha `peer:<id>`, `navegador: null`. Sem `remoto`,
    // `removerPeerDoisLados` sai no `if (!remoto) return null` e o outro servidor nem é chamado;
    // e `removeServer` não roda, porque não há entrada deste aparelho para apagar.
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([{ id: 'vps', base_url: 'https://vps', token: '••' }]);
    const { el, comp } = montar();
    await esperarCarga();
    (await abrir(el, 'peer:vps')).querySelector<HTMLButtonElement>('.mq-remover')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_remover_peer_so_aqui());
    expect(document.body.textContent).not.toContain(m.maquinas_remover_linha());
    expect(document.body.textContent).not.toContain(m.maquinas_remover_linha_local());
    unmount(comp);
  });

  it('o ✕ de uma linha COM peer continua dizendo que sai dos dois lados', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([{ id: 'nb', base_url: 'http://b', token: '••' }]);
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperarCarga();
    (await abrir(el, 'srv:srv-b')).querySelector<HTMLButtonElement>('.mq-remover')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_remover_linha());
    unmount(comp);
  });

  it('remover peer com o lado de lá falhando mostra o aviso específico', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([{ id: 'vps', base_url: 'https://vps', token: '••' }]);
    peersMock.removerPeerDoisLados.mockResolvedValueOnce(false);
    const { el, comp } = montar();
    await esperarCarga();
    (await abrir(el, 'peer:vps')).querySelector<HTMLInputElement>('.mq-falar')!.click();
    await tick();
    [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.peers_remover())!.click();
    await esperarCarga();
    expect(el.textContent).toContain(m.maquinas_remover_peer_lado_de_la_falhou());
    unmount(comp);
  });

  it('fala com o servidor ESCOLHIDO na aba, não com o ativo', async () => {
    // listServers explícito (não residual de outro teste): carregarIdsNavegador busca o id de
    // cada máquina do NAVEGADOR, e a asserção abaixo depende de SRV estar nessa lista.
    authMock.listServers.mockReturnValue([SRV]);
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([]);
    const outro = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' } as Server;
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props: {
      resolvedServer: outro, apiTarget: outro, onLogout: vi.fn() } });
    await esperarCarga();
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(outro);
    expect(peersMock.listarPeers).toHaveBeenCalledWith(outro);
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(SRV);
    unmount(comp as never);
  });

  it('servidor indisponível não carrega nada', async () => {
    authMock.listServers.mockReturnValue([]);   // explícito: sem isso, ordem embaralhada pode
    // deixar listServers() sem implementação nenhuma (undefined), e unirMaquinas quebra no .map.
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props: {
      resolvedServer: null, apiTarget: null, onLogout: vi.fn() } });
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
    const linha = (await abrir(t.el, 'peer:notebook'));
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
    const campo = (await abrirEste(t.el)).querySelector<HTMLInputElement>('.id-campo')!;
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
    const campo = (await abrirEste(t.el)).querySelector<HTMLInputElement>('.id-campo')!;
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
                               onLogout: vi.fn() });
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

    expect((await abrirEste(el)).querySelector<HTMLInputElement>('.id-campo')!.value).toBe('bbb');
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
    const campo = (await abrirEste(t.el)).querySelector<HTMLInputElement>('.id-campo')!;
    campo.value = 'casa'; campo.dispatchEvent(new Event('input')); await tick();
    campo.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await esperarCarga();
    expect(document.querySelector('.sd-dialogo .id-erro[role="alert"]')?.textContent).toContain(m.falha_conexao());
    unmount(t.comp);
  });

  it('falha ao registrar um peer aparece com nome', async () => {
    // registrarPeerDoisLados nunca lança (cada chamada interna tem catch próprio) — o erro que
    // chega na tela vem da RELISTAGEM depois do registro, não do próprio registro.
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo && alvo.id === 'srv-b' ? 'nb' : 'casa' }));
    // explícito, não o default do topo — um teste anterior (ordem embaralhada) pode ter deixado
    // gravarPeer pendurado num mockReturnValue, e clearAllMocks não desfaz isso.
    peersMock.gravarPeer.mockImplementation(async (_alvo, dado) => [dado] as never);
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    (await abrir(t.el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-falar')!.click();
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
                               onLogout: vi.fn() });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props });
    const passos = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); await tick(); await tick(); };
    await passos();

    // marca "servidores se falam" na máquina B, com A como alvo: o POST fica pendurado
    (await abrir(el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-falar')!.click();
    await passos();

    props.resolvedServer = B; props.apiTarget = B;   // desiste e troca de alvo

    await passos();

    // marca "servidores se falam" na máquina C, no alvo NOVO — não fica preso à chamada anterior
    (await abrir(el, 'srv:srv-c')).querySelector<HTMLInputElement>('.mq-falar')!.click();
    await passos();

    expect(peersMock.gravarPeer.mock.calls.length).toBe(2);
    unmount(comp as never);
  });

  // Rodada 3 (parecer do revisor): selos de lado derivados do ESTADO — o sucesso não pode
  // mostrar ✗ (o glifo não é mais texto chumbado); a montagem checa a ida da lista; e o bloco
  // de correção de endereço ganha vida (testa no endereço digitado).
  async function marcarFalar(chave: string, t: { el: HTMLElement }) {
    (await abrir(t.el, chave)).querySelector<HTMLInputElement>('.mq-falar')!.click();
    await esperarCarga();
  }

  const cartao = () => document.querySelector<HTMLElement>('.mq-linha[data-chave="srv:srv-b"] .sd-res[role="status"]');
  const botaoEm = (raiz: Element, texto: string) =>
    [...raiz.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === texto)!;
  // "Outro" na escolha do endereço: digita a URL e clica "Usar e testar".
  async function usarOutro(url: string) {
    const c = cartao()!;
    c.querySelectorAll<HTMLInputElement>('input[type="radio"]')[c.querySelectorAll('input[type="radio"]').length - 1].click();
    const inp = c.querySelector<HTMLInputElement>(`input[aria-label="${m.servidores_rec_outro()}"]`)!;
    inp.value = url; inp.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    botaoEm(c, m.servidores_rec_usar()).click();
  }

  it('B2 — sucesso com os dois lados ok mostra o cartão ok; volta falhando pede o endereço da volta', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://192.168.0.77:8765', token: 'segredo' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'notebook' : 'casa' }));
    // montagem sem peer; depois do gesto a listagem já traz o peer registrado
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    peersMock.gravarPeer.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    // sucesso: ida ok + volta ok — o hint só existe quando há falha real, então sucesso é ZERO .pr-lado
    peersMock.checkPeer.mockImplementation(async () => ({ estado: 'ok' }) as never);
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    await marcarFalar('srv:srv-b', t);
    expect(cartao()!.classList.contains('ok')).toBe(true);
    expect(cartao()!.textContent).toContain(m.servidores_rec_ok());
    unmount(t.comp);

    // falha real (não é recusa de token): ida ok, volta falhou → escolher o endereço da volta. Montagem com lista
    // VAZIA (nenhuma checagem pré-gesto consome os drives do check), e o gesto re-lista com o peer.
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'falhou' });
    const t2 = montar({ servers: [SRV, B] });
    await esperarCarga();
    await marcarFalar('srv:srv-b', t2);
    expect(cartao()!.textContent).toContain(m.servidores_rec_so_um_lado({ este: 'A', nome: 'Notebook' }));
    expect(cartao()!.textContent).not.toContain(m.servidores_rec_ok());
    expect(cartao()!.querySelectorAll('input[type="radio"]').length).toBeGreaterThan(0);
    unmount(t2.comp);
  });

  it('B2b — volta recusada por token (credencial) ganha cartão próprio, não o de endereço', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://192.168.0.77:8765', token: 'segredo' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'notebook' : 'casa' }));
    peersMock.gravarPeer.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    peersMock.listarPeers.mockResolvedValueOnce([]);
    peersMock.listarPeers.mockImplementation(async () => [
      { id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' },
    ] as never);
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'recusou', motivo: 'credencial' });
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    await marcarFalar('srv:srv-b', t);
    expect(cartao()!.textContent).toContain(m.servidores_rec_token_recusado({ nome: 'Notebook' }));
    expect(cartao()!.textContent).not.toContain(m.servidores_rec_so_um_lado({ este: 'A', nome: 'Notebook' }));
    unmount(t.comp);
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
    expect(t.el.querySelectorAll('.sv-linha')).toHaveLength(2);   // srv-b/nuvem, peer:notebook (SRV é o cartão)
    unmount(t.comp);
  });

  it('B4 — "Usar e testar" grava o ENDEREÇO ESCOLHIDO como o DESTA máquina, no peer', async () => {
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
    // gesto com a volta falhando: a escolha do endereço da volta aparece
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'ok' });
    peersMock.checkPeer.mockResolvedValueOnce({ estado: 'falhou' });
    await marcarFalar('srv:srv-b', t);
    expect(cartao()!.textContent).toContain(m.servidores_rec_so_um_lado({ este: 'A', nome: 'Notebook' }));
    // o usuário escolhe "Outro", digita o endereço CERTO e clica em "Usar e testar"
    peersMock.checkPeer.mockResolvedValue({ estado: 'ok' });
    await usarOutro('http://novo:9999');
    await esperarCarga();
    // A pergunta do bloco é "qual endereço o X deve usar para chegar aqui?": o que se digita é o
    // endereço DESTA máquina, para gravar LÁ. Isto era assertado como `base_url` do próprio peer
    // — o lado oposto ao que a frase promete, e o botão consertava o endereço errado.
    // A volta fala com o B da LISTA (srv-b): é por esse id que a resposta limpa a marca de offline.
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'srv-b', baseUrl: 'http://192.168.0.77:8765', token: 'segredo' }),
      expect.objectContaining({ id: 'casa', base_url: 'http://novo:9999' }),
    );
    // e o peer continua registrado aqui no endereço DELE, que o gesto não estava corrigindo
    expect(peersMock.gravarPeer).toHaveBeenCalledWith(
      null,
      expect.objectContaining({ id: 'notebook', base_url: 'http://192.168.0.77:8765', token: 'segredo' }),
    );
    // com os dois lados ok o cartão vira o de sucesso
    expect(cartao()!.textContent).toContain(m.servidores_rec_ok());
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
    await abrir(t.el, 'srv:srv-b');
    await esperarCarga();   // abrir o detalhe mede de novo
    expect(cartao()!.textContent).toContain(m.servidores_rec_sem_registro({ este: 'A', nome: 'Notebook' }));
    unmount(t.comp);
  });

  it('volta na montagem: volta falhou mostra o endereço que o lado de lá tentou e a escolha', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockImplementation(async (alvo: Server | null) => {
      if (alvo === B) return [{ id: 'casa', base_url: 'https://casa.ts.net', token: '••' }];
      return [{ id: 'nb', base_url: 'http://b', token: '••' }];
    });
    peersMock.checkPeer.mockImplementation(async (alvo: Server | null) => (alvo === B ? { estado: 'falhou' } : { estado: 'ok' }));
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    await abrir(t.el, 'srv:srv-b');
    await esperarCarga();
    expect(cartao()!.textContent).toContain(m.servidores_rec_tentou({ endereco: 'https://casa.ts.net' }));
    expect(botaoEm(cartao()!, m.servidores_rec_usar())).toBeDefined();
    unmount(t.comp);
  });

  it('ida rejeitando (falha de rede) não trava a linha em "Testando…" nem gera rejeição não tratada (Critical)', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([{ id: 'nb', base_url: 'http://b', token: '••' }]);
    // a ida (daqui, alvo null) rejeita sempre — também na nova medição que abrir o detalhe dispara
    peersMock.checkPeer.mockImplementation(async (alvo: Server | null) => {
      if (alvo === null) throw new Error('fetch failed');
      return { estado: 'ok' };
    });
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    await abrir(t.el, 'srv:srv-b');
    await esperarCarga();
    expect(cartao()!.textContent).not.toContain(m.peers_estado_testando());
    expect(cartao()!.textContent).toContain(m.servidores_rec_ida_falhou({ este: 'A', nome: 'Notebook' }));
    unmount(t.comp);
  });

  it('remover peer limpa estados[id] e o cartão de resultado sai', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockImplementation(async (alvo: Server | null) => {
      if (alvo === B) return [{ id: 'casa', base_url: 'https://casa.ts.net', token: '••' }];
      return [{ id: 'nb', base_url: 'http://b', token: '••' }];
    });
    peersMock.checkPeer.mockImplementation(async (alvo: Server | null) => (alvo === B ? { estado: 'falhou' } : { estado: 'ok' }));
    const t = montar({ servers: [SRV, B] });
    await esperarCarga();
    // volta falhou de verdade: o cartão pede o endereço da volta
    const linha = await abrir(t.el, 'srv:srv-b');
    await esperarCarga();
    expect(cartao()!.textContent).toContain(m.servidores_rec_so_um_lado({ este: 'A', nome: 'Notebook' }));
    peersMock.listarPeers.mockResolvedValue([]);   // depois de remover, o servidor não conhece mais o peer
    linha.querySelector<HTMLInputElement>('.mq-falar')!.click();
    await tick();
    [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.peers_remover())!.click();
    await esperarCarga();
    // sem peer não há resultado de recados: sem cartão, e a legenda diz que estão desligados
    expect(cartao()).toBeNull();
    expect(document.querySelector('.mq-linha[data-chave="srv:srv-b"]')!.textContent).toContain(m.servidores_rec_desligado({ este: 'A', nome: 'Notebook' }));
    unmount(t.comp);
  });

  it('usarEndereco não sobrescreve peers com o resultado atrasado de um alvo trocado (guard de geração)', async () => {
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' } as Server;
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    const C: Server = { id: 'srv-c', label: 'C', baseUrl: 'http://c', token: 'tc' } as Server;
    authMock.listServers.mockReturnValue([A, B, C]);
    authMock.getActiveId.mockReturnValue('srv-a');
    apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));
    apiMock.getPushSettingsForServer.mockReturnValue(new Promise(() => {}));
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({
      identificador: alvo?.id === 'srv-b' ? 'nb' : alvo?.id === 'srv-c' ? 'c' : 'casa',
    }));
    let chamadasC = 0;
    let resolverStaleC!: (v: unknown) => void;
    const staleC = new Promise((r) => { resolverStaleC = r; });
    peersMock.listarPeers.mockImplementation(async (alvo: Server | null) => {
      // props de criarProps são $state — o nested object chega como PROXY, não a referência
      // literal; comparar por .id (não por ===) é o padrão já usado pelos testes vizinhos.
      if (alvo?.id === 'srv-b') return [{ id: 'casa', base_url: 'https://casa.ts.net', token: '••' }];
      if (alvo?.id === 'srv-c') { chamadasC++; return chamadasC === 1 ? [{ id: 'peer-fresco', base_url: 'http://c-peer', token: '***' }] : (staleC as never); }
      return [{ id: 'nb', base_url: 'http://b', token: '••' }];
    });
    peersMock.checkPeer.mockImplementation(async (alvo: Server | null) => (alvo === B ? { estado: 'falhou' } : { estado: 'ok' }));

    const props = criarProps({ resolvedServer: A as Server | null, apiTarget: A as Server | null, onLogout: vi.fn() });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props });
    const passos = async () => { for (let i = 0; i < 30; i++) { await Promise.resolve(); await tick(); } };
    await passos();

    // volta falhou: o detalhe de B pede o endereço da volta
    await abrir(el, 'srv:srv-b');
    await passos();
    expect(cartao()!.textContent).toContain(m.servidores_rec_so_um_lado({ este: 'A', nome: 'Notebook' }));
    let resolverGravar!: (v: unknown) => void;
    peersMock.gravarPeer.mockImplementation(() => new Promise((r) => { resolverGravar = r; }) as never);
    await usarOutro('http://novo:1');
    await passos();
    expect(peersMock.gravarPeer).toHaveBeenCalled();

    props.resolvedServer = C; props.apiTarget = C;   // troca de alvo com o testarDeNovo em voo
    await passos();   // carga fresca de C (1ª chamada de listarPeers(C))

    resolverGravar([{ id: 'nb', base_url: 'http://b', token: 'tb' }]);   // libera o registro antigo, atrasado
    resolverStaleC([{ id: 'peer-antigo', base_url: 'http://stale', token: '***' }]);
    await passos();

    expect(el.textContent).toContain('peer-fresco');
    expect(el.textContent).not.toContain('peer-antigo');
    unmount(comp as never);
  });
});

describe('MaquinasSettings — neste aparelho', () => {
  const esperar = async () => { const { settled } = await import('svelte'); await settled(); await Promise.resolve(); await tick(); };
  const VPS = { id: 'vps', base_url: 'https://vps', token: '••' };

  it('ligar as sessões de quem só o servidor conhece usa o token que o servidor guarda', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([VPS]);
    peersMock.tokenDoPeer.mockResolvedValue({ token: 'tok-do-servidor' });
    const { el, comp } = montar();
    await esperar();
    (await abrir(el, 'peer:vps')).querySelector<HTMLInputElement>('.mq-acompanhar')!.click();
    await esperar();
    expect(peersMock.tokenDoPeer).toHaveBeenCalledWith(null, 'vps');
    expect(peersMock.getIdentificador).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'https://vps', token: 'tok-do-servidor' }));
    expect(authMock.addServer).toHaveBeenCalledWith('https://vps', 'tok-do-servidor', 'vps', { ativar: false });
    unmount(comp);
  });

  it('sem o token do servidor, o erro aparece e o token digitado é o que se testa', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([VPS]);
    peersMock.tokenDoPeer.mockRejectedValueOnce(new Error('404: sem token'));
    const { el, comp } = montar();
    await esperar();
    const d = await abrir(el, 'peer:vps');
    d.querySelector<HTMLInputElement>('.mq-acompanhar')!.click();
    await esperar();
    expect(d.textContent).toContain('404: sem token');
    expect(authMock.addServer).not.toHaveBeenCalled();
    const inp = d.querySelector<HTMLInputElement>(`input[aria-label="${m.sessao_token()}"]`)!;
    inp.value = 'digitado'; inp.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    inp.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await esperar();
    expect(peersMock.tokenDoPeer).toHaveBeenCalledTimes(1);
    expect(authMock.addServer).toHaveBeenCalledWith('https://vps', 'digitado', 'vps', { ativar: false });
    unmount(comp);
  });

  it('desligar "Mostrar as sessões dele" desliga as entradas, não remove', async () => {
    const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' } as Server;
    peersMock.getIdentificador.mockImplementation(async (alvo) => ({ identificador: alvo?.id === 'srv-b' ? 'nb' : 'casa' }));
    peersMock.listarPeers.mockResolvedValue([]);
    const { el, comp } = montar({ servers: [SRV, B] });
    await esperar();
    (await abrir(el, 'srv:srv-b')).querySelector<HTMLInputElement>('.mq-acompanhar')!.click();
    expect(authMock.setServerDisabled).toHaveBeenCalledWith('srv-b', true);
    expect(authMock.removeServer).not.toHaveBeenCalled();
    unmount(comp);
  });

  it('o servidor escolhido guardado duas vezes: o cartão avisa e o detalhe tira o endereço que sobra', async () => {
    const A2: Server = { id: 'srv-a2', label: 'A (Tailscale)', baseUrl: 'https://a.ts.net', token: 'x2' } as Server;
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([]);
    const { el, comp } = montar({ servers: [SRV, A2] });
    await esperar();
    expect(el.querySelector('.sv-este')!.textContent).toContain(m.servidores_guardado_vezes({ n: 2 }));
    // a mesma máquina não vira uma segunda linha na lista de baixo
    expect(el.querySelectorAll('.sv-linha')).toHaveLength(0);
    const este = await abrirEste(el);
    expect(este.textContent).toContain(m.servidores_guardado_explica({ n: 2 }));
    expect(este.textContent).toContain('https://a.ts.net');
    este.querySelector<HTMLButtonElement>(`.sv-dup button[aria-label="${m.servidores_tirar_aria({ nome: A2.label })}"]`)!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-danger')!.click();
    await tick();
    expect(authMock.removeServer).toHaveBeenCalledWith('srv-a2');
    unmount(comp);
  });

  it('o resultado medido fica salvo: a próxima abertura já mostra, sem esperar medir de novo', async () => {
    peersMock.getIdentificador.mockResolvedValue({ identificador: 'casa' });
    peersMock.listarPeers.mockResolvedValue([VPS]);
    peersMock.checkPeer.mockResolvedValue({ estado: 'ok' });
    const a = montar();
    await esperar();
    expect(peersMock.checkPeer).toHaveBeenCalledWith(null, 'https://vps', 'vps');
    expect(a.el.querySelector('.sv-linha[data-chave="peer:vps"]')!.textContent).toContain(m.servidores_sessoes_sem_token());
    unmount(a.comp);
    // segunda abertura: a medição nova não volta, e mesmo assim a linha não fica em "Testando…"
    peersMock.checkPeer.mockReturnValue(new Promise(() => {}) as never);
    const b = montar();
    await esperar();
    expect(b.el.querySelector('.sv-linha[data-chave="peer:vps"]')!.textContent).toContain(m.servidores_sessoes_sem_token());
    unmount(b.comp);
    peersMock.checkPeer.mockImplementation(async () => ({ estado: 'ok' }));
  });
});

describe('MaquinasSettings — ordem dos blocos', () => {
  it('o cartão deste servidor vem antes da lista dos outros', async () => {
    const { el, comp } = montar();
    await tick();
    const texto = el.textContent ?? '';
    const esta = texto.indexOf(m.peers_esta_maquina());
    const secao = texto.indexOf(m.servidores_secao_maquinas());
    expect(esta).toBeGreaterThanOrEqual(0);
    expect(esta).toBeLessThan(secao);
    unmount(comp);
  });

  it('Parear abre o QR deste servidor numa janela, e Adicionar traz a busca no Tailscale', async () => {
    const { el, comp } = montar();
    await tick();
    const [adicionar, parear] = [...el.querySelectorAll<HTMLButtonElement>('.sv-topo .sv-btn')];
    parear.click();
    await tick();
    expect(document.querySelector('.sd-dialogo')!.textContent).toContain(m.acesso_legenda_qr());
    document.querySelector<HTMLButtonElement>('.sd-dialogo .sv-fechar')!.click();
    await tick();
    expect(document.querySelector('.sd-dialogo')).toBeNull();
    adicionar.click();
    await tick();
    expect(document.body.textContent).toContain(m.maquinas_buscar_tailscale());
    unmount(comp);
  });

  it('sem servidor resolvido, só a lista de máquinas aparece', async () => {
    authMock.listServers.mockReturnValue([]);
    authMock.getActiveId.mockReturnValue(null);
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(MaquinasSettings, { target: el, props: { resolvedServer: null, apiTarget: null, onLogout: vi.fn(async () => {}) } });
    await tick();
    expect(el.textContent).not.toContain(m.peers_esta_maquina());
    expect(el.textContent).toContain(m.servidores_secao_maquinas());
    unmount(comp as never);
  });
});
