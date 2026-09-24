// @vitest-environment happy-dom
import { describe, it, expect, beforeEach } from 'vitest';
import {
  unirMaquinas, estadoDaLinha, sessionsState, recolhida, recadosCard, rememberedIds, rememberIds,
  savedMotivos, saveMotivos, savedPeerStates, savePeerStates, type LinhaMaquina,
} from './maquinas';
import type { Server } from './auth';
import type { PeerView } from './peers';

const A: Server = { id: 'srv-a', label: 'Casa', baseUrl: 'http://192.168.0.10:8765', token: 'ta' };
const B: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://192.168.0.20:8765', token: 'tb' };
const pB: PeerView = { id: 'notebook', base_url: 'https://notebook.ts.net', token: 'tb••' };
const pC: PeerView = { id: 'vps', base_url: 'https://vps.exemplo.com', token: 'tc••' };

describe('unirMaquinas', () => {
  it('casa navegador e peer pelo identificador, mesmo com URLs diferentes', () => {
    const linhas = unirMaquinas([A, B], { 'srv-a': 'casa', 'srv-b': 'notebook' }, [pB], 'srv-a');
    const nb = linhas.find((l) => l.identificador === 'notebook')!;
    expect(nb.navegador).toBe(B);
    expect(nb.peer).toBe(pB);
    expect(nb.nome).toBe('Notebook');
    expect(nb.chave).toBe('srv:srv-b');
  });

  it('a chave não muda quando o identificador chega depois', () => {
    const antes = unirMaquinas([B], {}, [], null)[0].chave;
    const depois = unirMaquinas([B], { 'srv-b': 'notebook' }, [pB], null)[0].chave;
    expect(antes).toBe('srv:srv-b');
    expect(depois).toBe('srv:srv-b');
  });

  it('peer que só o servidor conhece vira linha própria, sem navegador', () => {
    const linhas = unirMaquinas([A], { 'srv-a': 'casa' }, [pC], 'srv-a');
    const vps = linhas.find((l) => l.identificador === 'vps')!;
    expect(vps.navegador).toBeNull();
    expect(vps.peer).toBe(pC);
    expect(vps.nome).toBe('vps');
    expect(vps.chave).toBe('peer:vps');
  });

  it('navegador sem identificador fica na lista, com chave própria e sem peer', () => {
    const linhas = unirMaquinas([A, B], { 'srv-a': 'casa', 'srv-b': null }, [pB], 'srv-a');
    const b = linhas.find((l) => l.navegador === B)!;
    expect(b.identificador).toBeNull();
    expect(b.peer).toBeNull();
    expect(b.chave).toBe('srv:srv-b');
    // o peer "notebook" não casou com ninguém: vira linha própria
    expect(linhas.find((l) => l.peer === pB)!.navegador).toBeNull();
  });

  it('a máquina escolhida vem primeiro e é marcada', () => {
    const linhas = unirMaquinas([B, A], { 'srv-a': 'casa', 'srv-b': 'notebook' }, [], 'srv-a');
    expect(linhas[0].navegador).toBe(A);
    expect(linhas[0].estaMaquina).toBe(true);
    expect(linhas[1].estaMaquina).toBe(false);
  });

  it('sem servidor escolhido ninguém é "esta máquina" e a ordem é por nome', () => {
    const linhas = unirMaquinas([B, A], {}, [], null);
    expect(linhas.map((l) => l.nome)).toEqual(['Casa', 'Notebook']);
    expect(linhas.every((l) => !l.estaMaquina)).toBe(true);
  });

  it('a mesma máquina guardada por dois endereços é UMA linha, com o peer uma vez só', () => {
    const A2: Server = { id: 'srv-a2', label: 'Casa (Tailscale)', baseUrl: 'https://casa.ts.net', token: 'ta2' };
    const pCasa: PeerView = { id: 'casa', base_url: 'https://casa.ts.net', token: 'ta••' };
    const linhas = unirMaquinas([A, A2], { 'srv-a': 'casa', 'srv-a2': 'casa' }, [pCasa], null);
    expect(linhas.length).toBe(1);
    expect(linhas[0].navegadores).toEqual([A, A2]);
    expect(linhas[0].navegador).toBe(A);
    expect(linhas[0].peer).toBe(pCasa);
  });

  it('a entrada principal é a escolhida; senão a ligada que respondeu; desligada só em último caso', () => {
    const A2: Server = { id: 'srv-a2', label: 'Casa TS', baseUrl: 'https://casa.ts.net', token: 'ta2' };
    const ids = { 'srv-a': 'casa', 'srv-a2': 'casa' };
    expect(unirMaquinas([A, A2], ids, [], 'srv-a2')[0].navegador).toBe(A2);
    expect(unirMaquinas([A, A2], ids, [], null, { 'srv-a': 'sem_resposta', 'srv-a2': 'vazio' })[0].navegador).toBe(A2);
    const off = { ...A, disabled: true };
    const l = unirMaquinas([off, A2], ids, [], null, { 'srv-a': 'sem_resposta' })[0];
    expect(l.navegador).toBe(A2);
    expect(l.chave).toBe('srv:srv-a2');
    expect(l.motivos).toEqual({ 'srv-a': 'sem_resposta' });
  });

  it('máquina desligada (sem identificador) casa pelo endereço, e não vira duas linhas', () => {
    const F: Server = { id: 'srv-f', label: 'Fora', baseUrl: 'https://fora.ts.net/', token: 'tf' };
    const pF: PeerView = { id: 'fora', base_url: 'https://fora.ts.net', token: 'tf••', enabled: false };
    const linhas = unirMaquinas([F], { 'srv-f': null }, [pF, pC], null);
    expect(linhas.length).toBe(2);
    const f = linhas.find((l) => l.navegador === F)!;
    expect(f.peer).toBe(pF);
    expect(f.identificador).toBe('fora');
    expect(f.chave).toBe('srv:srv-f');
  });

  it('sem identificador e sem endereço igual, segue como antes: linha sem peer', () => {
    const linhas = unirMaquinas([B], { 'srv-b': null }, [pB], null);
    expect(linhas.find((l) => l.navegador === B)!.peer).toBeNull();
    expect(linhas.find((l) => l.peer === pB)!.navegador).toBeNull();
  });
});

describe('estadoDaLinha', () => {
  const linha: LinhaMaquina = { chave: 'srv:srv-b', nome: 'Notebook', identificador: 'notebook', navegador: B, navegadores: [B], motivos: {}, peer: pB, estaMaquina: false };

  it('desligado vence qualquer falha medida', () => {
    const e = estadoDaLinha({ ...linha, peer: { ...pB, enabled: false } }, { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }] });
    expect(e).toMatchObject({ farol: 'neutro', tipo: 'desligada' });
  });

  it('token recusado na volta não é falha parcial', () => {
    const e = estadoDaLinha(linha, { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'recusou', motivo: 'credencial' }] });
    expect(e).toMatchObject({ farol: 'nao', tipo: 'token_recusado' });
  });

  it('volta sem registro é cinza, não vermelho', () => {
    const e = estadoDaLinha(linha, { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'registro' }] });
    expect(e).toMatchObject({ farol: 'test', tipo: 'volta_sem_registro' });
  });

  it('os dois lados ok é verde; sem medição ainda, com peer, está testando', () => {
    expect(estadoDaLinha(linha, { ok: true, lados: [] })).toMatchObject({ farol: 'ok', tipo: 'ok' });
    expect(estadoDaLinha(linha, undefined)).toMatchObject({ farol: 'test', tipo: 'neutro' });
    expect(estadoDaLinha({ ...linha, peer: null }, undefined)).toMatchObject({ farol: 'neutro', tipo: 'neutro' });
  });
});

describe('sessionsState', () => {
  const nav: LinhaMaquina = { chave: 'srv:srv-b', nome: 'Notebook', identificador: 'notebook', motivoId: 'vazio', navegador: B, navegadores: [B], motivos: {}, peer: pB, estaMaquina: false };
  const soPeer: LinhaMaquina = { ...nav, chave: 'peer:notebook', motivoId: undefined, navegador: null, navegadores: [] };

  it('com entrada aqui, fala só do que a entrada respondeu', () => {
    expect(sessionsState(nav, undefined)).toBe('aparecem');
    expect(sessionsState({ ...nav, motivoId: undefined }, undefined)).toBe('testando');
    expect(sessionsState({ ...nav, motivoId: 'token' }, undefined)).toBe('token_recusado');
    expect(sessionsState({ ...nav, motivoId: 'sem_resposta' }, undefined)).toBe('nao_responde');
    // recados falhando não mudam a frase das sessões
    expect(sessionsState(nav, { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }] })).toBe('aparecem');
  });

  it('desligada no servidor vence; todas as entradas desligadas aqui têm frase própria', () => {
    expect(sessionsState({ ...nav, peer: { ...pB, enabled: false } }, undefined)).toBe('desligada');
    const off = { ...B, disabled: true };
    expect(sessionsState({ ...nav, navegador: off, navegadores: [off] }, undefined)).toBe('desligada_aqui');
    expect(sessionsState({ ...nav, navegadores: [off, B] }, undefined)).toBe('aparecem');
  });

  it('só o servidor conhece: depende da ida', () => {
    expect(sessionsState(soPeer, undefined)).toBe('testando');
    expect(sessionsState(soPeer, { ok: false, lados: [], testando: true })).toBe('testando');
    expect(sessionsState(soPeer, { ok: false, lados: [{ lado: 'ida', estado: 'ok' }] })).toBe('sem_token');
    expect(sessionsState(soPeer, { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }] })).toBe('nao_responde');
  });

  it('recolhe só quem não responde ou está desligada no servidor', () => {
    expect(recolhida('nao_responde')).toBe(true);
    expect(recolhida('desligada')).toBe(true);
    for (const s of ['aparecem', 'testando', 'token_recusado', 'sem_token', 'desligada_aqui'] as const) expect(recolhida(s)).toBe(false);
  });
});

describe('recadosCard', () => {
  const nav: LinhaMaquina = { chave: 'srv:srv-b', nome: 'Notebook', identificador: 'notebook', motivoId: 'vazio', navegador: B, navegadores: [B], motivos: {}, peer: pB, estaMaquina: false };
  const semPeer: LinhaMaquina = { ...nav, peer: null };
  const st = (ida: string, volta?: { estado: string; motivo?: string }) => ({
    ok: ida === 'ok' && volta?.estado === 'ok',
    lados: [{ lado: 'ida' as const, estado: ida as 'ok' }, ...(volta ? [{ lado: 'volta' as const, ...volta } as never] : [])],
  });

  it('sem peer: falta o meu nome, o dele, ou recados estão só desligados', () => {
    expect(recadosCard(semPeer, undefined, '')).toBe('sem_meu_id');
    expect(recadosCard({ ...semPeer, identificador: null }, undefined, 'casa')).toBe('sem_id_dele');
    expect(recadosCard({ ...semPeer, identificador: null, motivoId: 'sem_resposta' }, undefined, 'casa')).toBe('sem_resposta');
    expect(recadosCard({ ...semPeer, identificador: null, motivoId: 'token' }, undefined, 'casa')).toBe('token_recusado');
    expect(recadosCard(semPeer, undefined, 'casa')).toBe('desligado');
  });

  it('com peer, um cartão por resultado medido', () => {
    expect(recadosCard(nav, undefined, 'casa')).toBe('testando');
    expect(recadosCard(nav, st('ok', { estado: 'ok' }), 'casa')).toBe('ok');
    expect(recadosCard(nav, st('ok', { estado: 'falhou' }), 'casa')).toBe('endereco_volta');
    expect(recadosCard(nav, st('falhou', { estado: 'ok' }), 'casa')).toBe('ida_falhou');
    expect(recadosCard(nav, st('ok', { estado: 'recusou', motivo: 'credencial' }), 'casa')).toBe('token_recusado');
    expect(recadosCard(nav, st('ok', { estado: 'estranho' }), 'casa')).toBe('volta_outra');
    expect(recadosCard(nav, st('estranho', { estado: 'ok' }), 'casa')).toBe('ida_outra');
    expect(recadosCard(nav, st('ok', { estado: 'nao_configurado', motivo: 'token' }), 'casa')).toBe('falta_token');
    expect(recadosCard(nav, st('ok', { estado: 'nao_configurado', motivo: 'registro' }), 'casa')).toBe('sem_registro');
    expect(recadosCard({ ...nav, peer: { ...pB, enabled: false } }, st('falhou'), 'casa')).toBe('pausada');
  });
});

describe('o que fica guardado neste aparelho', () => {
  beforeEach(() => localStorage.clear());

  it('identificadores lembrados sobrevivem e lixo no storage vira vazio', () => {
    expect(rememberedIds()).toEqual({});
    rememberIds({ 'srv-b': 'notebook' });
    expect(rememberedIds()).toEqual({ 'srv-b': 'notebook' });
    localStorage.setItem('cp_server_ids', '{quebrado');
    expect(rememberedIds()).toEqual({});
  });

  it('resultados salvos: motivos e peers por alvo, sem os que ainda estão testando', () => {
    saveMotivos({ 'srv-b': 'sem_resposta' });
    savePeerStates('srv-a', {
      notebook: { ok: true, lados: [{ lado: 'ida', estado: 'ok' }], em: 1 },
      vps: { ok: false, lados: [], testando: true },
    });
    expect(savedMotivos()).toEqual({ 'srv-b': 'sem_resposta' });
    expect(Object.keys(savedPeerStates('srv-a'))).toEqual(['notebook']);
    expect(savedPeerStates('outro')).toEqual({});
    // gravar um não apaga o outro
    saveMotivos({});
    expect(Object.keys(savedPeerStates('srv-a'))).toEqual(['notebook']);
  });
});
