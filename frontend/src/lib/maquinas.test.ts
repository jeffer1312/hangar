import { describe, it, expect } from 'vitest';
import { unirMaquinas } from './maquinas';
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

  it('dois servidores do navegador com o mesmo identificador não duplicam o peer', () => {
    const A2: Server = { id: 'srv-a2', label: 'Casa (Tailscale)', baseUrl: 'https://casa.ts.net', token: 'ta2' };
    const pCasa: PeerView = { id: 'casa', base_url: 'https://casa.ts.net', token: 'ta••' };
    const linhas = unirMaquinas([A, A2], { 'srv-a': 'casa', 'srv-a2': 'casa' }, [pCasa], null);
    expect(linhas.find((l) => l.navegador === A)!.peer).toBe(pCasa);
    expect(linhas.find((l) => l.navegador === A2)!.peer).toBeNull();
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
