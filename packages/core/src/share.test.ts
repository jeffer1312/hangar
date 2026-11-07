import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  parseInviteLink, parsePairLink, inviteAllows, redeemInvite, InviteRedeemError, SharePrerequisiteError, ShareReachError,
  tailscaleHost,
} from './share';
import * as m from './paraglide/messages';
import { configureApi, type ApiEnv } from './apiEnv';
import {
  getConfig, getAtualizacao, checkInviteForServer, createShare, sharePrereqs,
  createPairInvite, acceptPair, listExternalPairs, setPairAccess, leavePair, attachGuest,
} from './api';
import type { Server } from './servers';

const CONVITE: Server = { id: 'srv-i', label: 'Convite · J', baseUrl: 'https://dono.ts.net:8443', token: 'tg', invite: true };

describe('tailscaleHost', () => {
  it('reconhece o nome da tailnet e a faixa 100.64.0.0/10', () => {
    expect(tailscaleHost('https://hangar.tail0000.ts.net')).toBe('hangar.tail0000.ts.net');
    expect(tailscaleHost('http://100.64.0.2:8765')).toBe('100.64.0.2');
  });

  it('não confunde outros endereços', () => {
    for (const outro of ['http://100.128.0.1:8765', 'http://100.63.0.1:8765', 'http://192.168.0.5:8765', 'https://exemplo.com', 'https://ts.net.exemplo.com', 'nao-e-url']) {
      expect(tailscaleHost(outro)).toBeNull();
    }
  });
});

function ambiente(over: Partial<ApiEnv> = {}) {
  configureApi({
    getBaseUrl: () => 'https://dono.ts.net:8443',
    getToken: () => 'tg',
    onUnauthorized: () => {},
    origin: null,
    createEventSource: () => { throw new Error('sem SSE no teste'); },
    ...over,
  });
}
function resposta(status: number, corpo: unknown): Response {
  return new Response(JSON.stringify(corpo), { status, headers: { 'Content-Type': 'application/json' } });
}
afterEach(() => vi.restoreAllMocks());

describe('parseInviteLink', () => {
  it('aceita o link https do Funnel, com barra final e espaços', () => {
    expect(parseInviteLink('  https://dono.ts.net:8443/convite/K7P29QX4/ \n'))
      .toEqual({ address: 'https://dono.ts.net:8443', code: 'K7P29QX4' });
  });
  it('aceita o link hangar:// e devolve o endereço https', () => {
    expect(parseInviteLink('hangar://convite/dono.ts.net:8443/K7P29QX4'))
      .toEqual({ address: 'https://dono.ts.net:8443', code: 'K7P29QX4' });
  });
  it('recusa http, outro caminho, código vazio e texto solto', () => {
    expect(parseInviteLink('http://dono.ts.net:8443/convite/K7P2')).toBeNull();
    expect(parseInviteLink('https://dono.ts.net:8443/api/sessions')).toBeNull();
    expect(parseInviteLink('https://dono.ts.net:8443/convite/')).toBeNull();
    expect(parseInviteLink('K7P29QX4')).toBeNull();
  });
  it('aceita http só com IP da rede local (convite local), nos dois formatos', () => {
    expect(parseInviteLink('http://192.168.77.142:8766/convite/K7P2'))
      .toEqual({ address: 'http://192.168.77.142:8766', code: 'K7P2' });
    expect(parseInviteLink('hangar://convite/10.0.0.5:8766/K7P2'))
      .toEqual({ address: 'http://10.0.0.5:8766', code: 'K7P2' });
    expect(parseInviteLink('http://8.8.8.8:8766/convite/K7P2')).toBeNull();
    expect(parseInviteLink('http://172.32.0.1:8766/convite/K7P2')).toBeNull();
  });
  it('recusa credencial embutida no endereço', () => {
    expect(parseInviteLink('https://a@b/convite/X')).toBeNull();
    expect(parseInviteLink('https://a:p@b:8443/convite/X')).toBeNull();
    expect(parseInviteLink('hangar://convite/a@b/X')).toBeNull();
  });
});

describe('inviteAllows', () => {
  it('deixa passar rotas de sessão, o resgate e as globais do chat', () => {
    expect(inviteAllows('/api/sessions')).toBe(true);
    expect(inviteAllows('/api/sessions/events?x=1')).toBe(true);
    expect(inviteAllows('/api/sessions/s1/history?limit=20')).toBe(true);
    expect(inviteAllows('/api/guest/redeem')).toBe(true);
    expect(inviteAllows('/api/model-options?x=1')).toBe(true);
    expect(inviteAllows('/api/tts/audio/abc123')).toBe(true);
  });
  it('barra rotas do servidor inteiro, inclusive /api/config', () => {
    expect(inviteAllows('/api/config')).toBe(false);
    expect(inviteAllows('/api/atualizacao')).toBe(false);
    expect(inviteAllows('/api/cotas')).toBe(false);
    expect(inviteAllows('/api/claude-configs')).toBe(false);
  });
});

describe('redeemInvite', () => {
  it('posta código e aparelho no endereço do link e devolve o token', async () => {
    const f = vi.fn(async () => resposta(200, { token: 'tg', session: 's1', owner: 'Jefferson', address: 'https://dono.ts.net:8443' }));
    const r = await redeemInvite('https://dono.ts.net:8443/convite/K7P2', 'Navegador · Linux', f as unknown as typeof fetch);
    expect(r.token).toBe('tg');
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('https://dono.ts.net:8443/api/guest/redeem');
    expect(init.method).toBe('POST');
    expect(JSON.parse(String(init.body))).toEqual({ code: 'K7P2', device: 'Navegador · Linux' });
  });
  it('agrega sessão ao token de convite existente e nunca segue redirect com esse token', async () => {
    const f = vi.fn(async () => resposta(200, { token: 'tg', session: 's2', owner: 'Ana', address: 'https://dono.ts.net:8443' }));
    await redeemInvite('https://dono.ts.net:8443/convite/NOVO', 'Android', f as unknown as typeof fetch, 'tg');
    const [, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(JSON.parse(String(init.body))).toEqual({ code: 'NOVO', device: 'Android', token: 'tg' });
    expect(init.redirect).toBe('error');
  });
  it.each([
    'http://dono.ts.net:8443',
    'ftp://dono.ts.net:8443',
    'https://outro.ts.net:8443',
    'https://dono.ts.net:8766',
  ])('origem diferente na resposta conserva o endereço do link: %s', async (address) => {
    const f = vi.fn(async () => resposta(200, { token: 'tg', session: 's1', owner: 'Ana', address }));
    const result = await redeemInvite('https://dono.ts.net:8443/convite/K7P2', 'browser', f as unknown as typeof fetch);
    expect(result).toEqual({ token: 'tg', session: 's1', owner: 'Ana', address: 'https://dono.ts.net:8443' });
  });
  it.each([
    ['https://dono.ts.net:8443/convite/K7P2', 'https://dono.ts.net:8443/'],
    ['http://192.168.77.142:8766/convite/K7P2', 'http://192.168.77.142:8766/'],
  ])('preserva endereço válido da mesma origem sem token opcional: %s', async (link, address) => {
    const f = vi.fn(async () => resposta(200, { token: 'tg', session: 's1', owner: 'Ana', address }));
    expect(await redeemInvite(link, 'browser', f as unknown as typeof fetch))
      .toEqual({ token: 'tg', session: 's1', owner: 'Ana', address });
    const [, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(JSON.parse(String(init.body))).toEqual({ code: 'K7P2', device: 'browser' });
  });
  it('410 vira o motivo certo (envelope com params ou detail cru)', async () => {
    const usado = vi.fn(async () => resposta(410, { detail: { code: 'erro_convite_usado', params: { reason: 'used' }, msg: 'x' } }));
    await expect(redeemInvite('https://d:8443/convite/A', 'x', usado as unknown as typeof fetch))
      .rejects.toMatchObject({ reason: 'used' });
    const vencido = vi.fn(async () => resposta(410, { detail: { reason: 'expired' } }));
    await expect(redeemInvite('https://d:8443/convite/A', 'x', vencido as unknown as typeof fetch))
      .rejects.toMatchObject({ reason: 'expired' });
  });
  it('404, link inválido e falha de rede têm motivo próprio', async () => {
    const nao = vi.fn(async () => resposta(404, { detail: 'not found' }));
    await expect(redeemInvite('https://d:8443/convite/A', 'x', nao as unknown as typeof fetch)).rejects.toMatchObject({ reason: 'unknown' });
    await expect(redeemInvite('lixo', 'x', nao as unknown as typeof fetch)).rejects.toBeInstanceOf(InviteRedeemError);
    const rede = vi.fn(async () => { throw new TypeError('fetch failed'); });
    await expect(redeemInvite('https://d:8443/convite/A', 'x', rede as unknown as typeof fetch)).rejects.toMatchObject({ reason: 'network' });
  });
  it('503 (túnel do dono fora do ar, código não gasto) é indisponível, não usado nem vencido', async () => {
    const f = vi.fn(async () => resposta(503, { detail: { code: 'erro_sessao_indisponivel', params: {}, msg: 'x' } }));
    await expect(redeemInvite('https://d:8443/convite/A', 'x', f as unknown as typeof fetch))
      .rejects.toMatchObject({ reason: 'unavailable' });
  });
});

describe('trava do servidor de convite em apiFetchRes', () => {
  it('servidor ativo de convite: rota global barrada sem ir à rede', async () => {
    ambiente({ isInvite: () => true });
    const f = vi.spyOn(globalThis, 'fetch');
    await expect(getAtualizacao()).rejects.toMatchObject({ status: 403, code: 'erro_fora_do_convite' });
    expect(f).not.toHaveBeenCalled();
  });
  it('servidor ativo de convite: /api/config é barrado sem ir à rede (o Chat cai no padrão)', async () => {
    ambiente({ isInvite: () => true });
    const f = vi.spyOn(globalThis, 'fetch');
    await expect(getConfig()).rejects.toMatchObject({ status: 403, code: 'erro_fora_do_convite' });
    expect(f).not.toHaveBeenCalled();
  });
  it('410 de um servidor de convite avisa quem ouve, com o id dele', async () => {
    const onInviteEnded = vi.fn();
    ambiente({ onInviteEnded });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(410, { detail: { code: 'erro_convite_encerrado', params: {}, msg: 'x' } }));
    expect(await checkInviteForServer(CONVITE)).toBe(true);
    expect(onInviteEnded).toHaveBeenCalledWith('srv-i');
  });
  it('401 de um servidor de convite também encerra (registro apagado pelo dono)', async () => {
    const onInviteEnded = vi.fn();
    ambiente({ onInviteEnded });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(401, { detail: 'x' }));
    expect(await checkInviteForServer(CONVITE)).toBe(true);
    expect(onInviteEnded).toHaveBeenCalledWith('srv-i');
  });
  it('401 de servidor que NÃO é convite não chama onInviteEnded', async () => {
    const onInviteEnded = vi.fn();
    ambiente({ onInviteEnded });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(401, { detail: 'x' }));
    await checkInviteForServer({ ...CONVITE, invite: false });
    expect(onInviteEnded).not.toHaveBeenCalled();
  });
  it('pergunta ao convite nunca aceita resposta guardada pelo navegador', async () => {
    ambiente({});
    const f = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, []));
    await checkInviteForServer(CONVITE);
    expect(f).toHaveBeenCalledWith('https://dono.ts.net:8443/api/sessions', expect.objectContaining({ cache: 'no-store' }));
  });
  it('503 de um servidor de convite NÃO encerra: é indisponibilidade passageira', async () => {
    const onInviteEnded = vi.fn();
    ambiente({ onInviteEnded });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(503, { detail: { code: 'erro_sessao_indisponivel', params: {}, msg: 'x' } }));
    expect(await checkInviteForServer(CONVITE)).toBe(false);
    expect(onInviteEnded).not.toHaveBeenCalled();
  });
  it('502 (Funnel com o servidor Rust reiniciando) também é tentar de novo, nunca fim do convite', async () => {
    const onInviteEnded = vi.fn();
    ambiente({ onInviteEnded });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('', { status: 502 }));
    expect(await checkInviteForServer(CONVITE)).toBe(false);
    expect(onInviteEnded).not.toHaveBeenCalled();
  });
  it('conexão recusada num servidor de convite não encerra o convite', async () => {
    const onInviteEnded = vi.fn();
    ambiente({ onInviteEnded });
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('Failed to fetch'));
    await expect(checkInviteForServer(CONVITE)).rejects.toThrow('Failed to fetch');
    expect(onInviteEnded).not.toHaveBeenCalled();
  });
});

describe('createShare', () => {
  it('409 de pré-requisito vira SharePrerequisiteError com o que falta e o conserto', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(409, { detail: {
      code: 'erro_compartilhar_pre_requisito', params: { missing: ['operator'], fix: 'sudo tailscale set --operator=$USER' }, msg: 'x',
    } }));
    const e = await createShare('s1').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(SharePrerequisiteError);
    expect((e as SharePrerequisiteError).missing).toEqual(['operator']);
    expect((e as SharePrerequisiteError).fix).toBe('sudo tailscale set --operator=$USER');
    expect((e as SharePrerequisiteError).enableUrl).toBeNull();
  });
  it('409 com funnel faltando traz o link de liberar', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(409, { detail: {
      code: 'erro_compartilhar_pre_requisito', msg: 'x',
      params: { missing: ['funnel'], fix: 'libere', enable_url: 'https://login.tailscale.com/f/funnel?node=n1' },
    } }));
    const e = await createShare('s1').catch((x: unknown) => x);
    expect((e as SharePrerequisiteError).enableUrl).toBe('https://login.tailscale.com/f/funnel?node=n1');
  });
  it('409 com enable_url fora do Tailscale descarta o link', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(409, { detail: {
      code: 'erro_compartilhar_pre_requisito', msg: 'x',
      params: { missing: ['funnel'], fix: 'libere', enable_url: 'javascript:alert(1)' },
    } }));
    const e = await createShare('s1').catch((x: unknown) => x);
    expect((e as SharePrerequisiteError).enableUrl).toBeNull();
  });
  it('409 share_reach_failed diz a etapa em que o link não abriu de fora', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(409, { detail: {
      code: 'share_reach_failed', msg: 'o link não abriu pela internet: sem nome',
      params: { stage: 'dns', detail: 'sem nome' },
    } }));
    const e = await createShare('s1').catch((x: unknown) => x);
    expect(e).toBeInstanceOf(ShareReachError);
    expect((e as ShareReachError).stage).toBe('dns');
    expect((e as ShareReachError).detail).toBe('sem nome');
    expect((e as ShareReachError).message).toBe(m.share_reach_failed_dns());
  });
  it.each([404, 405])('%i na criação = servidor sem o Rust', async (status) => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"Not Found"}', { status }));
    const e = await createShare('s1').catch((x: unknown) => x);
    expect(e).toMatchObject({ status, code: 'share_create_needs_rust', message: m.share_create_needs_rust() });
  });
});

describe('sharePrereqs', () => {
  it('consulta a rota do dono sem ligar nada', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, { missing: [], fix: '', enable_url: null }));
    expect(await sharePrereqs()).toEqual({ missing: [], fix: '', enable_url: null });
    const [url, init] = spy.mock.calls[0];
    expect(url).toBe('https://dono.ts.net:8443/api/share/prereqs');
    expect(init?.method).toBeUndefined();
  });
  it('só a página do Tailscale passa como enable_url', async () => {
    ambiente();
    const ok = 'https://login.tailscale.com/f/funnel?node=n1';
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(resposta(200, { missing: ['funnel'], fix: 'x', enable_url: 'javascript:alert(1)' }))
      .mockResolvedValueOnce(resposta(200, { missing: ['funnel'], fix: 'x', enable_url: 'https://login.tailscale.com.evil.io/f' }))
      .mockResolvedValueOnce(resposta(200, { missing: ['funnel'], fix: 'x', enable_url: ok }));
    expect((await sharePrereqs()).enable_url).toBeNull();
    expect((await sharePrereqs()).enable_url).toBeNull();
    expect((await sharePrereqs()).enable_url).toBe(ok);
  });
});

describe('parsePairLink', () => {
  it('aceita só o link https do Funnel na 8443, com /par/CÓDIGO', () => {
    expect(parsePairLink('  https://Casa.tail1.ts.net:8443/par/AB12cd/ \n')).toBe('https://casa.tail1.ts.net:8443/par/AB12cd');
  });
  it('recusa outro host, porta, esquema, caminho, credencial, query e código torto', () => {
    for (const bad of [
      'https://casa.example.com:8443/par/AB12', 'https://.ts.net:8443/par/AB12', 'https://casa.ts.net/par/AB12',
      'https://casa.ts.net:443/par/AB12', 'http://casa.ts.net:8443/par/AB12', 'https://casa.ts.net:8443/convite/AB12',
      'https://u@casa.ts.net:8443/par/AB12', 'https://casa.ts.net:8443/par/AB12?x=1', 'https://casa.ts.net:8443/par/AB12#x',
      'https://casa.ts.net:8443/par/AB-12', 'https://casa.ts.net:8443/par/', `https://casa.ts.net:8443/par/${'a'.repeat(65)}`,
      'https://casa.ts.net:8443/par/AB12/mais', 'lixo',
    ]) expect(parsePairLink(bad), bad).toBeNull();
  });
  it('aceita a entrada de convidado do Connect, sem porta', () => {
    expect(parsePairLink(' https://PC1--guest.acme.hangar.dev.br/par/AB12cd/ \n')).toBe('https://pc1--guest.acme.hangar.dev.br/par/AB12cd');
  });
  it('recusa do Connect tudo que não é a entrada de convidado', () => {
    for (const bad of [
      'https://pc1.acme.hangar.dev.br/par/AB12', 'https://pc1--preview.acme.hangar.dev.br/par/AB12',
      'https://--guest.acme.hangar.dev.br/par/AB12', 'https://pc1---guest.acme.hangar.dev.br/par/AB12',
      'https://pc1--guest.hangar.dev.br/par/AB12', 'https://a.pc1--guest.acme.hangar.dev.br/par/AB12',
      'https://pc1--guest.acme.hangar.dev.br.evil.com/par/AB12', 'https://pc1--guest.acme.hangar.dev.br:443/par/AB12',
      'https://pc1--guest.acme.hangar.dev.br:8443/par/AB12', 'http://pc1--guest.acme.hangar.dev.br/par/AB12',
      'https://u@pc1--guest.acme.hangar.dev.br/par/AB12', 'https://pc1--guest.acme.hangar.dev.br/par/AB12?x=1',
      'https://pc1--guest.acme.hangar.dev.br/par/AB12#x', 'https://pc1--guest.acme.hangar.dev.br/convite/AB12',
      'https:\\\\pc1--guest.acme.hangar.dev.br:9000/par/AB12', 'https:\\\\pc1--guest.acme.hangar.dev.br:443/par/AB12',
    ]) expect(parsePairLink(bad), bad).toBeNull();
  });
});

const DONO: Server = { id: 'srv-1', label: 'note', baseUrl: 'https://note.ts.net', token: 'td' };

describe('createPairInvite', () => {
  it('posta o nível e devolve o link', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, { id: 'p1', link: 'https://note.ts.net:8443/par/X', expires_at: 1 }));
    expect(await createPairInvite('s 1', 'full', DONO)).toEqual({ id: 'p1', link: 'https://note.ts.net:8443/par/X', expires_at: 1 });
    const [url, init] = spy.mock.calls[0];
    expect(url).toBe('https://note.ts.net/api/sessions/s%201/pair-invite');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(String(init?.body))).toEqual({ level: 'full' });
  });
  it('409 de pré-requisito e de alcance saem como no compartilhar', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(resposta(409, { detail: { code: 'erro_compartilhar_pre_requisito', params: { missing: ['funnel'], fix: 'f' }, msg: 'x' } }))
      .mockResolvedValueOnce(resposta(409, { detail: { code: 'share_reach_failed', params: { stage: 'funnel', detail: 'd' }, msg: 'x' } }));
    expect(await createPairInvite('s', 'view', DONO).catch((x: unknown) => x)).toBeInstanceOf(SharePrerequisiteError);
    const e = await createPairInvite('s', 'view', DONO).catch((x: unknown) => x);
    expect(e).toBeInstanceOf(ShareReachError);
    expect((e as ShareReachError).stage).toBe('funnel');
  });
  it.each([404, 405])('%i = servidor sem o Rust', async (status) => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"Not Found"}', { status }));
    expect(await createPairInvite('s', 'view', DONO).catch((x: unknown) => x)).toMatchObject({ status, code: 'share_create_needs_rust' });
  });
});

describe('acceptPair', () => {
  it('posta link e nível; resposta sem level é Hangar antigo = ver e recado', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(resposta(200, { ok: true, alias: 'casa', owner: 'Ana', session: 'hangar', level: 'full' }))
      .mockResolvedValueOnce(resposta(200, { ok: true, alias: 'casa', owner: 'Ana', session: 'hangar' }));
    expect(await acceptPair('s', ' https://casa.ts.net:8443/par/AB12 ', 'full', DONO))
      .toEqual({ alias: 'casa', owner: 'Ana', session: 'hangar', level: 'full' });
    const [url, init] = spy.mock.calls[0];
    expect(url).toBe('https://note.ts.net/api/sessions/s/pair-accept');
    expect(JSON.parse(String(init?.body))).toEqual({ link: 'https://casa.ts.net:8443/par/AB12', level: 'full' });
    expect((await acceptPair('s', 'https://casa.ts.net:8443/par/AB12', 'view', DONO)).level).toBe('view');
  });
  it('pré-requisito do Funnel deste lado sai como no compartilhar', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(409, { detail: {
      code: 'erro_compartilhar_pre_requisito', params: { missing: ['funnel'], fix: 'f' }, msg: 'x' } }));
    expect(await acceptPair('s', 'https://casa.ts.net:8443/par/AB12', 'view', DONO).catch((x: unknown) => x))
      .toBeInstanceOf(SharePrerequisiteError);
  });
  it('link torto nem sai daqui', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch');
    expect(await acceptPair('s', 'https://casa.ts.net:8443/convite/AB12', 'view', DONO).catch((x: unknown) => x))
      .toMatchObject({ code: 'erro_par_link_invalido', message: m.erro_par_link_invalido() });
    expect(spy).not.toHaveBeenCalled();
  });
  it('erro do servidor sai com o código', async () => {
    ambiente();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(502, { detail: { code: 'erro_par_incerto', params: {}, msg: 'x' } }));
    expect(await acceptPair('s', 'https://casa.ts.net:8443/par/AB12', 'view', DONO).catch((x: unknown) => x))
      .toMatchObject({ status: 502, code: 'erro_par_incerto' });
  });
});

describe('pares externos: lista, liberar, sair e attach', () => {
  it('lista os pares do servidor', async () => {
    ambiente();
    const par = { local_session: 's', alias: 'casa', owner: 'Ana', session: 'h', address: 'https://casa.ts.net:8443', token: 't', level_given: 'full' };
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, [par]));
    expect(await listExternalPairs(DONO)).toEqual([par]);
    expect(spy.mock.calls[0][0]).toBe('https://note.ts.net/api/external-pairs');
  });
  it('liberar posta alias e nível na rota do dono', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, { ok: true }));
    await setPairAccess('s', 'casa', 'full', DONO);
    const [url, init] = spy.mock.calls[0];
    expect(url).toBe('https://note.ts.net/api/sessions/s/pair-access');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(String(init?.body))).toEqual({ alias: 'casa', level: 'full' });
  });
  it('sair/desfazer o par: DELETE na rota do dono do MEU servidor', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, { ok: true }));
    await leavePair('casa', 'han gar', DONO);
    const [url, init] = spy.mock.calls[0];
    expect(url).toBe('https://note.ts.net/api/external-pairs/casa?session=han%20gar');
    expect(init?.method).toBe('DELETE');
    expect((init?.headers as Record<string, string>).Authorization).toBe('Bearer td');
  });
  it('attach liga o token do par à entrada que já existe', async () => {
    ambiente();
    const spy = vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta(200, { ok: true }));
    await attachGuest({ id: 'e', label: 'x', baseUrl: 'https://casa.ts.net:8443', token: 'tconvite', invite: true }, 'tpar');
    const [url, init] = spy.mock.calls[0];
    expect(url).toBe('https://casa.ts.net:8443/api/guest/attach');
    expect((init?.headers as Record<string, string>).Authorization).toBe('Bearer tconvite');
    expect(JSON.parse(String(init?.body))).toEqual({ token: 'tpar' });
  });
});
