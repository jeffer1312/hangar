// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import ListaMaquinas from './ListaMaquinas.svelte';
import * as m from '../../paraglide/messages';
import type { LinhaMaquina } from '../../lib/maquinas';

const A: LinhaMaquina = { chave: 'srv:srv-a', nome: 'Casa', identificador: 'casa', navegador: { id: 'srv-a', label: 'Casa', baseUrl: 'http://a', token: 'ta' }, peer: null, estaMaquina: true };
const B: LinhaMaquina = { chave: 'srv:srv-b', nome: 'Notebook', identificador: 'notebook', navegador: { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' }, peer: { id: 'notebook', base_url: 'https://nb.ts.net', token: '••' }, estaMaquina: false };
const C: LinhaMaquina = { chave: 'peer:vps', nome: 'vps', identificador: 'vps', navegador: null, peer: { id: 'vps', base_url: 'https://vps', token: '••' }, estaMaquina: false };
const D: LinhaMaquina = { chave: 'srv:srv-d', nome: 'Fora', identificador: null, navegador: { id: 'srv-d', label: 'Fora', baseUrl: 'http://d', token: 'td' }, peer: null, estaMaquina: false };
const E: LinhaMaquina = { chave: 'srv:srv-e', nome: 'Mac', identificador: 'mac', navegador: { id: 'srv-e', label: 'Mac', baseUrl: 'http://e', token: 'te' }, peer: { id: 'mac', base_url: 'https://mac.ts.net', token: '••' }, estaMaquina: false };

function montar(linhas: LinhaMaquina[], over: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const cbs = { onAcompanhar: vi.fn(), onFalar: vi.fn(), onEditar: vi.fn(), onCorrige: vi.fn(), onTestarDeNovo: vi.fn(), onAdicionar: vi.fn() };
  const comp = mount(ListaMaquinas, { target: el, props: { linhas, estados: {}, meuIdentificador: 'casa', carregando: false, corrige: null, ...cbs, ...over } });
  const linha = (chave: string) => el.querySelector<HTMLElement>(`.mq-linha[data-chave="${chave}"]`)!;
  return { el, comp, cbs, linha };
}

describe('ListaMaquinas', () => {
  it('uma linha por máquina, com as duas caixas refletindo as duas listas', () => {
    const t = montar([A, B, C, D]);
    expect(t.el.querySelectorAll('.mq-linha').length).toBe(4);
    const b = t.linha('srv:srv-b');
    expect(b.querySelector<HTMLInputElement>('.mq-acompanhar')!.checked).toBe(true);
    expect(b.querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(true);
    const c = t.linha('peer:vps');
    expect(c.querySelector<HTMLInputElement>('.mq-acompanhar')!.checked).toBe(false);
    expect(c.querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(true);
    expect(c.textContent).toContain(m.maquinas_so_no_servidor());
    unmount(t.comp);
  });

  it('esta máquina mostra a etiqueta no lugar de "servidores se falam"', () => {
    const t = montar([A]);
    expect(t.linha('srv:srv-a').querySelector('.mq-falar')).toBeNull();
    expect(t.linha('srv:srv-a').textContent).toContain(m.maquinas_esta());
    unmount(t.comp);
  });

  it('máquina sem identificador não pode "falar", e diz por quê', () => {
    const t = montar([D]);
    expect(t.linha('srv:srv-d').querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);
    expect(t.linha('srv:srv-d').textContent).toContain(m.maquinas_sem_identificador());
    unmount(t.comp);
  });

  it('sem identificador próprio, nenhuma máquina pode "falar"', () => {
    const t = montar([B], { meuIdentificador: '' });
    expect(t.linha('srv:srv-b').querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);
    unmount(t.comp);
  });

  it('a caixa não muda sozinha: volta ao dado e avisa o dono', () => {
    const t = montar([B]);
    const cb = t.linha('srv:srv-b').querySelector<HTMLInputElement>('.mq-falar')!;
    cb.click();
    expect(cb.checked).toBe(true);   // o dado (peer existe) continua valendo
    expect(t.cbs.onFalar).toHaveBeenCalledWith(B, false);
    const ac = t.linha('srv:srv-b').querySelector<HTMLInputElement>('.mq-acompanhar')!;
    ac.click();
    expect(ac.checked).toBe(true);
    expect(t.cbs.onAcompanhar).toHaveBeenCalledWith(B, false);
    unmount(t.comp);
  });

  it('peer com falha na volta mostra o estado e as pílulas; sem token ou sem registro de lá, diz isso', () => {
    const t = montar([B, C, E], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'falhou' }] },
      vps: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'token' }] },
      mac: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'registro' }] },
    } });
    expect(t.linha('srv:srv-b').textContent).toContain(m.peers_estado_parcial());
    expect(t.linha('srv:srv-b').querySelectorAll('.pr-lado').length).toBe(2);
    expect(t.linha('peer:vps').textContent).toContain(m.maquinas_volta_sem_medir());
    expect(t.linha('peer:vps').textContent).toContain(m.maquinas_so_no_servidor());
    expect(t.linha('peer:vps').textContent).not.toContain(m.peers_estado_parcial());
    expect(t.linha('srv:srv-e').textContent).toContain(m.maquinas_volta_sem_registro());
    unmount(t.comp);
  });

  it('bloco de correção aparece só na linha certa, devolve a URL digitada e null ao deixar só de ida', () => {
    const t = montar([B, C], { corrige: { id: 'notebook', url: 'http://x' } });
    expect(t.linha('srv:srv-b').querySelector('.corrige')).not.toBeNull();
    expect(t.linha('peer:vps').querySelector('.corrige')).toBeNull();
    const inp = t.linha('srv:srv-b').querySelector<HTMLInputElement>('.corrige-input')!;
    inp.value = 'https://casa.ts.net'; inp.dispatchEvent(new Event('input', { bubbles: true }));
    expect(t.cbs.onCorrige).toHaveBeenCalledWith('https://casa.ts.net');
    t.linha('srv:srv-b').querySelector<HTMLButtonElement>('.corrige .btn.primaria')!.click();
    expect(t.cbs.onTestarDeNovo).toHaveBeenCalledWith(B);
    [...t.linha('srv:srv-b').querySelectorAll<HTMLButtonElement>('.corrige .btn')].find((b) => b.textContent?.trim() === m.peers_so_ida())!.click();
    expect(t.cbs.onCorrige).toHaveBeenCalledWith(null);
    unmount(t.comp);
  });

  it('lista vazia: carregando não afirma "nenhuma"; sem carregar, diz e oferece adicionar', () => {
    const a = montar([], { carregando: true });
    expect(a.el.textContent).not.toContain(m.maquinas_vazio());
    unmount(a.comp);
    const b = montar([]);
    expect(b.el.textContent).toContain(m.maquinas_vazio());
    b.el.querySelector<HTMLButtonElement>('.mq-add')!.click();
    expect(b.cbs.onAdicionar).toHaveBeenCalled();
    unmount(b.comp);
  });
});
