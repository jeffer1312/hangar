// @vitest-environment happy-dom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import ListaMaquinas from './ListaMaquinas.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import type { LinhaMaquina } from '../../lib/maquinas';

const B: LinhaMaquina = { chave: 'srv:srv-b', nome: 'Notebook', identificador: 'notebook', navegador: { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' }, peer: { id: 'notebook', base_url: 'https://nb.ts.net', token: '••' }, estaMaquina: false };
const C: LinhaMaquina = { chave: 'peer:vps', nome: 'vps', identificador: 'vps', navegador: null, peer: { id: 'vps', base_url: 'https://vps', token: '••' }, estaMaquina: false };
const D: LinhaMaquina = { chave: 'srv:srv-d', nome: 'Fora', identificador: null, motivoId: 'vazio', navegador: { id: 'srv-d', label: 'Fora', baseUrl: 'http://d', token: 'td' }, peer: null, estaMaquina: false };
// A mesma ausência de identificador por motivos diferentes: máquina muda, cada uma com a sua chave.
const MUDA: LinhaMaquina = { ...D, chave: 'srv:srv-m', nome: 'Muda', motivoId: 'sem_resposta', navegador: { id: 'srv-m', label: 'Muda', baseUrl: 'http://mu', token: 'tm' } };
const RECUSA: LinhaMaquina = { ...D, chave: 'srv:srv-r', nome: 'Recusa', motivoId: 'token', navegador: { id: 'srv-r', label: 'Recusa', baseUrl: 'http://re', token: 'tr' } };
const E: LinhaMaquina = { chave: 'srv:srv-e', nome: 'Mac', identificador: 'mac', navegador: { id: 'srv-e', label: 'Mac', baseUrl: 'http://e', token: 'te' }, peer: { id: 'mac', base_url: 'https://mac.ts.net', token: '••' }, estaMaquina: false };

let aberto: { comp: object } | null = null;
afterEach(() => { if (aberto) unmount(aberto.comp); aberto = null; });

function montar(linhas: LinhaMaquina[], over: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const cbs = { onAcompanhar: vi.fn(), onFalar: vi.fn(), onToggleScan: vi.fn(), onEditar: vi.fn(), onCorrige: vi.fn(), onTestarDeNovo: vi.fn(), onRemover: vi.fn(), onSalvarIdentificador: vi.fn() };
  const comp = mount(ListaMaquinas, { target: el, props: { linhas, estados: {}, meuIdentificador: 'casa', carregando: false, corrige: null, idSalvando: '', idErro: {}, ...cbs, ...over } });
  aberto = { comp };
  const linhaCurta = (chave: string) => el.querySelector<HTMLElement>(`.sv-linha[data-chave="${chave}"]`)!;
  // O detalhe vive num portal no <body>: abre pela linha curta e se busca no document.
  const detalhe = (chave: string) => {
    linhaCurta(chave).click();
    flushSync();
    return document.querySelector<HTMLElement>(`.mq-linha[data-chave="${chave}"]`)!;
  };
  return { el, comp, cbs, linhaCurta, detalhe };
}

describe('ListaMaquinas — linha curta', () => {
  it('uma linha por servidor, com nome e uma frase de estado, sem controles', () => {
    const t = montar([B, C, D]);
    expect(t.el.querySelectorAll('.sv-linha').length).toBe(3);
    expect(t.el.querySelector('.mq-acompanhar, .mq-falar, .mq-remover')).toBeNull();
    expect(t.linhaCurta('peer:vps').textContent).toContain(m.servidores_curto_falta_token());
    expect(t.linhaCurta('srv:srv-d').textContent).toContain(m.servidores_curto_sem_id());
  });

  it('faltar o identificador não é uma frase só: quem não respondeu diz isso, quem recusou o token diz aquilo', () => {
    const t = montar([D, MUDA, RECUSA]);
    expect(t.linhaCurta('srv:srv-d').textContent).toContain(m.servidores_curto_sem_id());
    expect(t.linhaCurta('srv:srv-m').textContent).toContain(m.servidores_curto_nao_responde());
    expect(t.linhaCurta('srv:srv-r').textContent).toContain(m.servidores_curto_token_recusado());
    // a máquina que não responde é falha, não espera — o farol neutro dizia "nada a relatar"
    expect(t.linhaCurta('srv:srv-m').querySelector('.mq-farol')!.classList.contains('nao')).toBe(true);
    expect(t.linhaCurta('srv:srv-d').querySelector('.mq-farol')!.classList.contains('neutro')).toBe(true);
  });

  it('a frase curta acompanha o estado medido', () => {
    const t = montar([B, E], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'recusou', motivo: 'credencial' }] },
      mac: { ok: true, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'ok' }] },
    } });
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.servidores_curto_token_recusado());
    expect(t.linhaCurta('srv:srv-e').textContent).toContain(m.servidores_curto_ok());
  });

  it('volta falhando vira "só de ida"; ida falhando diz que não responde daqui', () => {
    const t = montar([B, E], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'falhou' }] },
      mac: { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }, { lado: 'volta', estado: 'ok' }] },
    } });
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.servidores_curto_so_ida());
    expect(t.linhaCurta('srv:srv-e').textContent).toContain(m.servidores_curto_ida_falhou());
  });

  it('lista vazia: carregando não afirma "nenhum"; sem carregar, diz', () => {
    const a = montar([], { carregando: true });
    expect(a.el.textContent).not.toContain(m.maquinas_vazio());
    unmount(a.comp); aberto = null;
    const b = montar([]);
    expect(b.el.textContent).toContain(m.maquinas_vazio());
  });

  it('linha removida fecha o detalhe aberto', () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const props = criarProps({ linhas: [B, C], estados: {}, meuIdentificador: 'casa', carregando: false, corrige: null,
      idSalvando: '', idErro: {},
      onAcompanhar: vi.fn(), onFalar: vi.fn(), onToggleScan: vi.fn(), onEditar: vi.fn(), onCorrige: vi.fn(), onTestarDeNovo: vi.fn(), onRemover: vi.fn(), onSalvarIdentificador: vi.fn() });
    const comp = mount(ListaMaquinas, { target: el, props });
    aberto = { comp };
    el.querySelector<HTMLElement>('.sv-linha[data-chave="peer:vps"]')!.click();
    flushSync();
    expect(document.querySelector('.mq-linha[data-chave="peer:vps"]')).not.toBeNull();
    props.linhas = [B];
    flushSync();
    expect(document.querySelector('.mq-linha[data-chave="peer:vps"]')).toBeNull();
  });
});

describe('ListaMaquinas — detalhe do servidor', () => {
  it('as duas caixas refletem as duas listas', () => {
    const t = montar([B, C]);
    const b = t.detalhe('srv:srv-b');
    expect(b.querySelector<HTMLInputElement>('.mq-acompanhar')!.checked).toBe(true);
    expect(b.querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(true);
    const c = t.detalhe('peer:vps');
    expect(c.querySelector<HTMLInputElement>('.mq-acompanhar')!.checked).toBe(false);
    expect(c.querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(true);
    expect(c.textContent).toContain(m.maquinas_so_no_servidor());
  });

  it('servidor sem identificador não pode "falar", e diz por quê', () => {
    const t = montar([D]);
    const d = t.detalhe('srv:srv-d');
    expect(d.querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);
    expect(d.textContent).toContain(m.maquinas_sem_identificador());
  });

  it('o detalhe separa "não respondeu" de "respondeu sem nome", e o token recusado traz o atalho', () => {
    const t = montar([MUDA, RECUSA]);
    const muda = t.detalhe('srv:srv-m');
    expect(muda.textContent).toContain(m.maquinas_nao_responde());
    expect(muda.textContent).not.toContain(m.maquinas_sem_identificador());
    const recusa = t.detalhe('srv:srv-r');
    expect(recusa.textContent).toContain(m.maquinas_token_aparelho_recusado());
    [...recusa.querySelectorAll<HTMLButtonElement>('.sd-acao')].find((x) => x.textContent?.trim() === m.servidores_trocar_token())!.click();
    expect(t.cbs.onEditar).toHaveBeenCalledWith(RECUSA);
  });

  it('endereço que atende como outra máquina diz QUEM atendeu, em vez de "só de ida"', () => {
    const t = montar([B], { estados: {
      notebook: { ok: false, lados: [
        { lado: 'ida', estado: 'ok' },
        { lado: 'volta', estado: 'estranho', motivo: 'identidade', identificador: 'outra', url: 'http://127.0.0.1:8765' },
      ] },
    } });
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.servidores_curto_endereco_outra());
    expect(t.linhaCurta('srv:srv-b').textContent).not.toContain(m.servidores_curto_so_ida());
    const b = t.detalhe('srv:srv-b');
    expect(b.textContent).toContain('outra');
    expect(b.textContent).toContain('http://127.0.0.1:8765');
  });

  it('o identificador do outro servidor é editável daqui, e só com token dele neste aparelho', () => {
    const t = montar([D, C]);
    // peer sem entrada no navegador: sem credencial não há como gravar lá
    expect(t.detalhe('peer:vps').querySelector('.sd-id')).toBeNull();
    const d = t.detalhe('srv:srv-d');
    const campo = d.querySelector<HTMLInputElement>('.sd-id')!;
    expect(campo.value).toBe('');
    campo.value = 'fora'; campo.dispatchEvent(new Event('input', { bubbles: true }));
    campo.dispatchEvent(new Event('blur', { bubbles: true }));
    expect(t.cbs.onSalvarIdentificador).toHaveBeenCalledWith(D, 'fora');
  });

  it('sem mexer no campo, sair dele não regrava; o erro da gravação aparece na linha', () => {
    const t = montar([B], { idErro: { 'srv-b': 'deu ruim' } });
    const b = t.detalhe('srv:srv-b');
    b.querySelector<HTMLInputElement>('.sd-id')!.dispatchEvent(new Event('blur', { bubbles: true }));
    expect(t.cbs.onSalvarIdentificador).not.toHaveBeenCalled();
    expect(b.textContent).toContain('deu ruim');
  });

  it('sem identificador próprio, nenhum servidor pode "falar"', () => {
    const t = montar([B], { meuIdentificador: '' });
    const b = t.detalhe('srv:srv-b');
    expect(b.querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);
    expect(b.textContent).toContain(m.peers_aviso_nao_definido());
  });

  it('a caixa não muda sozinha: volta ao dado e avisa o dono', () => {
    const t = montar([B]);
    const b = t.detalhe('srv:srv-b');
    const cb = b.querySelector<HTMLInputElement>('.mq-falar')!;
    cb.click();
    expect(cb.checked).toBe(true);
    expect(t.cbs.onFalar).toHaveBeenCalledWith(B, false);
    const ac = b.querySelector<HTMLInputElement>('.mq-acompanhar')!;
    ac.click();
    expect(ac.checked).toBe(true);
    expect(t.cbs.onAcompanhar).toHaveBeenCalledWith(B, false);
  });

  it('"ligado neste servidor" segue o enabled do peers.json e só avisa o dono ao clicar', () => {
    const desligado: LinhaMaquina = { ...C, peer: { ...C.peer!, enabled: false } };
    const t = montar([desligado, D]);
    const cb = t.detalhe('peer:vps').querySelector<HTMLInputElement>('.mq-varredura')!;
    expect(cb.checked).toBe(false);
    cb.click();
    expect(cb.checked).toBe(false);
    expect(t.cbs.onToggleScan).toHaveBeenCalledWith(desligado, true);
    // Sem registro no peers.json não há o que ligar.
    expect(t.detalhe('srv:srv-d').querySelector('.mq-varredura')).toBeNull();
  });

  it('falha na volta mostra o estado e as pílulas; sem token ou sem registro de lá, diz isso', () => {
    const t = montar([B, C, E], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'falhou' }] },
      vps: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'token' }] },
      mac: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'registro' }] },
    } });
    const b = t.detalhe('srv:srv-b');
    expect(b.textContent).toContain(m.peers_estado_parcial());
    expect(b.querySelectorAll('.pr-lado').length).toBe(2);
    const c = t.detalhe('peer:vps');
    expect(c.textContent).toContain(m.maquinas_volta_sem_medir());
    expect(c.textContent).toContain(m.maquinas_so_no_servidor());
    expect(c.textContent).not.toContain(m.peers_estado_parcial());
    expect(t.detalhe('srv:srv-e').textContent).toContain(m.maquinas_volta_sem_registro());
  });

  it('bloco de correção aparece só no servidor certo, devolve a URL digitada e null ao deixar só de ida', () => {
    const t = montar([B, C], { corrige: { id: 'notebook', url: 'http://x' } });
    expect(t.detalhe('peer:vps').querySelector('.corrige')).toBeNull();
    const b = t.detalhe('srv:srv-b');
    expect(b.querySelector('.corrige')).not.toBeNull();
    const inp = b.querySelector<HTMLInputElement>('.corrige-input')!;
    inp.value = 'https://casa.ts.net'; inp.dispatchEvent(new Event('input', { bubbles: true }));
    expect(t.cbs.onCorrige).toHaveBeenCalledWith('https://casa.ts.net');
    b.querySelector<HTMLButtonElement>('.corrige .btn.primaria')!.click();
    expect(t.cbs.onTestarDeNovo).toHaveBeenCalledWith(B);
    [...b.querySelectorAll<HTMLButtonElement>('.corrige .btn')].find((x) => x.textContent?.trim() === m.peers_so_ida())!.click();
    expect(t.cbs.onCorrige).toHaveBeenCalledWith(null);
  });

  it('volta recusada por token ganha dica própria e o atalho de trocar o token', () => {
    const t = montar([B], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'recusou', motivo: 'credencial' }] },
    } });
    const b = t.detalhe('srv:srv-b');
    expect(b.textContent).toContain(m.maquinas_volta_token_recusado());
    expect(b.textContent).not.toContain(m.peers_estado_parcial());
    [...b.querySelectorAll<HTMLButtonElement>('.sd-acao')].find((x) => x.textContent?.trim() === m.servidores_trocar_token())!.click();
    expect(t.cbs.onEditar).toHaveBeenCalledWith(B);
  });

  it('só o servidor conhece: "Informar token" pede para acompanhar neste aparelho', () => {
    const t = montar([C]);
    const c = t.detalhe('peer:vps');
    [...c.querySelectorAll<HTMLButtonElement>('.sd-acao')].find((x) => x.textContent?.trim() === m.servidores_informar_token())!.click();
    expect(t.cbs.onAcompanhar).toHaveBeenCalledWith(C, true);
  });

  it('farol: só navegador é neutro; testando e falha real aparecem mesmo sem peer', () => {
    const t = montar([D]);
    const farol = t.linhaCurta('srv:srv-d').querySelector('.mq-farol')!;
    expect(farol.textContent?.trim()).toBe('·');
    expect(farol.classList.contains('neutro')).toBe(true);
    unmount(t.comp); aberto = null;
    const semPeer: LinhaMaquina = { ...D, identificador: 'd' };
    const t1 = montar([semPeer], { estados: { d: { lados: [], ok: false, testando: true } } });
    expect(t1.linhaCurta('srv:srv-d').querySelector('.mq-farol')!.textContent?.trim()).toBe('◌');
    unmount(t1.comp); aberto = null;
    const t2 = montar([semPeer], { estados: { d: { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }] } } });
    const farolN = t2.linhaCurta('srv:srv-d').querySelector('.mq-farol')!;
    expect(farolN.textContent?.trim()).toBe('●');
    expect(farolN.classList.contains('nao')).toBe(true);
  });

  it('motivo da falha vai no title da pílula', () => {
    const t = montar([B], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'falhou', motivo: 'fetch failed' }] },
    } });
    expect(t.detalhe('srv:srv-b').querySelectorAll<HTMLElement>('.pr-lado')[1].title).toBe('fetch failed');
  });

  it('Remover devolve a linha inteira; Editar só existe com entrada neste aparelho, e fecha o detalhe', () => {
    const t = montar([B, C]);
    const c = t.detalhe('peer:vps');
    expect(c.querySelector('.mq-editar')).toBeNull();
    expect(c.querySelector('.mq-remover')!.textContent).toContain(m.lista_remover());
    c.querySelector<HTMLButtonElement>('.mq-remover')!.click();
    expect(t.cbs.onRemover).toHaveBeenCalledWith(C);
    const b = t.detalhe('srv:srv-b');
    b.querySelector<HTMLButtonElement>('.mq-editar')!.click();
    flushSync();
    expect(t.cbs.onEditar).toHaveBeenCalledWith(B);
    expect(document.querySelector('.mq-linha[data-chave="srv:srv-b"]')).toBeNull();
  });

  it('a etiqueta "Este servidor" fica no lado do servidor, e só nele', () => {
    const t = montar([B]);
    const b = t.detalhe('srv:srv-b');
    const campos = [...b.querySelectorAll<HTMLElement>('.sd-campo')];
    // A caixa do navegador grava no localStorage deste aparelho: sem etiqueta.
    expect(campos.find((c) => c.querySelector('.mq-acompanhar'))!.querySelector('.escopo')).toBeNull();
    // Recados grava no peers.json DO SERVIDOR.
    expect(campos.find((c) => c.querySelector('.mq-falar'))!.querySelector('.escopo')!.textContent).toBe(m.config_escopo_servidor());
    // Remover apaga o peer no servidor antes de mexer no navegador.
    expect(b.querySelector('.mq-remover .escopo')!.textContent).toBe(m.config_escopo_servidor());
    // Editar abre a entrada do NAVEGADOR: etiqueta ali seria falso.
    expect(b.querySelector('.mq-editar .escopo')).toBeNull();
  });

  it('o ✕ segue os lados que a linha tem, no chip e no nome acessível', () => {
    // Sem peer a remoção é só deste navegador; só do servidor, só do registro dele; com os dois, os dois.
    const t = montar([D, B, C]);
    const d = t.detalhe('srv:srv-d');
    expect(d.querySelector('.mq-remover .escopo')).toBeNull();
    expect(d.querySelector('.mq-remover')!.getAttribute('aria-label')).toBe(m.maquinas_remover_aria_local({ nome: D.nome }));
    expect(t.detalhe('srv:srv-b').querySelector('.mq-remover')!.getAttribute('aria-label')).toBe(m.maquinas_remover_aria({ nome: B.nome }));
    expect(t.detalhe('peer:vps').querySelector('.mq-remover')!.getAttribute('aria-label')).toBe(m.maquinas_remover_aria_servidor({ nome: C.nome }));
  });

  it('peer desligado no servidor: farol cinza e dica própria, mesmo com estado de falha', () => {
    const G: LinhaMaquina = { ...C, chave: 'peer:mac', identificador: 'mac', peer: { id: 'mac', base_url: 'https://mac', token: '••', enabled: false } };
    const t = montar([G], { estados: { mac: { ok: false, lados: [{ lado: 'ida', estado: 'falhou', motivo: 'timeout' }] } } });
    expect(t.linhaCurta('peer:mac').querySelector('.mq-farol')!.classList.contains('neutro')).toBe(true);
    expect(t.linhaCurta('peer:mac').textContent).toContain(m.servidores_curto_desligado());
    const g = t.detalhe('peer:mac');
    expect(g.textContent).toContain(m.maquinas_peer_desligado());
    expect(g.textContent).not.toContain(m.peers_estado_parcial());
  });
});
