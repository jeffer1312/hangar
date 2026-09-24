// @vitest-environment happy-dom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import ListaMaquinas from './ListaMaquinas.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import type { LinhaMaquina } from '../../lib/maquinas';
import type { Server } from '../../lib/auth';

const SB: Server = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://b', token: 'tb' };
const B: LinhaMaquina = { chave: 'srv:srv-b', nome: 'Notebook', identificador: 'notebook', motivoId: 'vazio', navegador: SB, navegadores: [SB], motivos: { 'srv-b': 'vazio' }, peer: { id: 'notebook', base_url: 'https://nb.ts.net', token: '••' }, estaMaquina: false };
const C: LinhaMaquina = { chave: 'peer:vps', nome: 'vps', identificador: 'vps', navegador: null, navegadores: [], motivos: {}, peer: { id: 'vps', base_url: 'https://vps', token: '••' }, estaMaquina: false };
const SD: Server = { id: 'srv-d', label: 'Fora', baseUrl: 'http://d', token: 'td' };
const D: LinhaMaquina = { chave: 'srv:srv-d', nome: 'Fora', identificador: null, motivoId: 'vazio', navegador: SD, navegadores: [SD], motivos: { 'srv-d': 'vazio' }, peer: null, estaMaquina: false };
// A mesma ausência de identificador por motivos diferentes: máquina muda, cada uma com a sua chave.
const SM: Server = { id: 'srv-m', label: 'Muda', baseUrl: 'http://mu', token: 'tm' };
const MUDA: LinhaMaquina = { ...D, chave: 'srv:srv-m', nome: 'Muda', motivoId: 'sem_resposta', navegador: SM, navegadores: [SM], motivos: { 'srv-m': 'sem_resposta' } };
const SR: Server = { id: 'srv-r', label: 'Recusa', baseUrl: 'http://re', token: 'tr' };
const RECUSA: LinhaMaquina = { ...D, chave: 'srv:srv-r', nome: 'Recusa', motivoId: 'token', navegador: SR, navegadores: [SR], motivos: { 'srv-r': 'token' } };
const SE: Server = { id: 'srv-e', label: 'Mac', baseUrl: 'http://e', token: 'te' };
const E: LinhaMaquina = { chave: 'srv:srv-e', nome: 'Mac', identificador: 'mac', motivoId: 'vazio', navegador: SE, navegadores: [SE], motivos: {}, peer: { id: 'mac', base_url: 'https://mac.ts.net', token: '••' }, estaMaquina: false };

let aberto: { comp: object } | null = null;
afterEach(() => { if (aberto) unmount(aberto.comp); aberto = null; });

const callbacks = () => ({
  onAcompanhar: vi.fn(), onFalar: vi.fn(), onToggleScan: vi.fn(), onEditar: vi.fn(), onTirar: vi.fn(), onInformarToken: vi.fn(),
  onTestar: vi.fn(), onUsarEndereco: vi.fn(), onAbrirEste: vi.fn(), onRemover: vi.fn(), onSalvarIdentificador: vi.fn(),
});
const base = { estados: {}, meuIdentificador: 'casa', esteNome: 'Casa', enderecos: [], carregando: false,
  idSalvando: '', idErro: {}, tokenSalvando: '', tokenErro: {} };

function montar(linhas: LinhaMaquina[], over: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const cbs = callbacks();
  const comp = mount(ListaMaquinas, { target: el, props: { linhas, ...base, ...cbs, ...over } });
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

const botao = (raiz: Element, texto: string) =>
  [...raiz.querySelectorAll<HTMLButtonElement>('button')].find((x) => x.textContent?.trim() === texto)!;
const cartao = (d: Element) => d.querySelector<HTMLElement>('.sd-res[role="status"]');

describe('ListaMaquinas — linha curta', () => {
  it('uma linha por máquina, com nome e se as sessões aparecem, sem controles', () => {
    const t = montar([B, C, D], { estados: { vps: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }] } } });
    expect(t.el.querySelectorAll('.sv-linha').length).toBe(3);
    expect(t.el.querySelector('.mq-acompanhar, .mq-falar, .mq-remover')).toBeNull();
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.servidores_sessoes_aparecem());
    expect(t.linhaCurta('srv:srv-d').textContent).toContain(m.servidores_sessoes_aparecem());
    // só o servidor conhece e ela responde: falta o token neste aparelho
    expect(t.linhaCurta('peer:vps').textContent).toContain(m.servidores_sessoes_sem_token());
  });

  it('o identificador aparece como chip só quando difere do nome', () => {
    const t = montar([B, C]);
    expect(t.linhaCurta('srv:srv-b').querySelector('.sv-id')!.textContent).toBe('notebook');
    expect(t.linhaCurta('peer:vps').querySelector('.sv-id')).toBeNull();
  });

  it('a mesma máquina guardada várias vezes diz quantas', () => {
    const S2: Server = { ...SB, id: 'srv-b2', baseUrl: 'http://b2' };
    const t = montar([{ ...B, navegadores: [SB, S2] }]);
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.servidores_guardado_vezes({ n: 2 }));
  });

  it('sem medição ainda diz testando; token recusado é falha', () => {
    const t = montar([{ ...B, motivoId: undefined }, RECUSA]);
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.acesso_testando());
    expect(t.linhaCurta('srv:srv-b').querySelector('.mq-farol')!.textContent?.trim()).toBe('◌');
    expect(t.linhaCurta('srv:srv-r').textContent).toContain(m.servidores_sessoes_token_recusado());
    expect(t.linhaCurta('srv:srv-r').querySelector('.mq-farol')!.classList.contains('nao')).toBe(true);
    expect(t.linhaCurta('srv:srv-b').querySelector('.mq-farol')!.classList.contains('ok')).toBe(false);
  });

  it('desligada neste aparelho continua na lista, com frase própria', () => {
    const t = montar([{ ...B, navegador: { ...SB, disabled: true }, navegadores: [{ ...SB, disabled: true }] }]);
    expect(t.linhaCurta('srv:srv-b').textContent).toContain(m.servidores_sessoes_desligadas());
    expect(t.el.querySelector('.mq-fora')).toBeNull();
  });

  it('quem não responde ou está desligada no servidor vai para "Não respondem", com Remover', () => {
    const G: LinhaMaquina = { ...C, chave: 'peer:mac', nome: 'mac', identificador: 'mac', peer: { id: 'mac', base_url: 'https://mac', token: '••', enabled: false } };
    const t = montar([B, MUDA, G]);
    const fora = t.el.querySelector<HTMLDetailsElement>('details.mq-fora')!;
    expect(fora.open).toBe(false);
    expect(fora.querySelector('summary')!.textContent).toBe(m.servidores_nao_respondem({ n: 2 }));
    expect(fora.querySelector('.sv-linha[data-chave="srv:srv-m"]')!.textContent).toContain(m.servidores_nao_respondeu());
    expect(fora.querySelector('.sv-linha[data-chave="peer:mac"]')!.textContent).toContain(m.servidores_desligada());
    expect(fora.querySelector('.sv-linha[data-chave="srv:srv-b"]')).toBeNull();
    fora.querySelectorAll<HTMLButtonElement>('.mq-remover')[0].click();
    expect(t.cbs.onRemover).toHaveBeenCalledTimes(1);
    expect(fora.querySelectorAll('.mq-remover').length).toBe(2);
  });

  it('peer que não responde nem tem entrada aqui diz que só falha nos recados', () => {
    const t = montar([C], { estados: { vps: { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }] } } });
    expect(t.linhaCurta('peer:vps').textContent).toContain(m.servidores_nao_respondeu_so_recados());
  });

  it('lista vazia: carregando não afirma "nenhum"; sem carregar, diz; todas fora do ar, diz isso', () => {
    const a = montar([], { carregando: true });
    expect(a.el.textContent).not.toContain(m.maquinas_vazio());
    unmount(a.comp); aberto = null;
    const b = montar([]);
    expect(b.el.textContent).toContain(m.maquinas_vazio());
    unmount(b.comp); aberto = null;
    const c = montar([MUDA]);
    expect(c.el.querySelector('.mq-vazio')!.textContent).toBe(m.servidores_nenhuma_respondendo());
  });

  it('linha removida fecha o detalhe aberto', () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const props = criarProps({ linhas: [B, C], ...base, ...callbacks() });
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
    expect(c.textContent).toContain(m.servidores_sem_token_aqui());
  });

  it('abrir o detalhe mede de novo, só com peer ligado', () => {
    const G: LinhaMaquina = { ...E, peer: { ...E.peer!, enabled: false } };
    const t = montar([B]);
    t.detalhe('srv:srv-b');
    expect(t.cbs.onTestar).toHaveBeenCalledWith(B);
    for (const l of [D, G]) {
      unmount(aberto!.comp); aberto = null;
      const t2 = montar([l]);
      t2.detalhe(l.chave);
      expect(t2.cbs.onTestar).not.toHaveBeenCalled();
    }
  });

  it('a caixa não muda sozinha: volta ao dado e avisa o dono; desligar só desliga', () => {
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
    expect(t.cbs.onRemover).not.toHaveBeenCalled();
  });

  it('entrada desligada neste aparelho: caixa desmarcada, ligar religa a mesma entrada', () => {
    const off: LinhaMaquina = { ...B, navegador: { ...SB, disabled: true }, navegadores: [{ ...SB, disabled: true }] };
    const t = montar([off]);
    const ac = t.detalhe('srv:srv-b').querySelector<HTMLInputElement>('.mq-acompanhar')!;
    expect(ac.checked).toBe(false);
    ac.click();
    expect(t.cbs.onAcompanhar).toHaveBeenCalledWith(off, true);
    expect(t.cbs.onInformarToken).not.toHaveBeenCalled();
  });

  it('só o servidor conhece: ligar as sessões busca o token do servidor; falhando, pede o token', () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const cbs = callbacks();
    const props = criarProps({ linhas: [C], ...base, ...cbs, tokenErro: {} as Record<string, string> });
    const comp = mount(ListaMaquinas, { target: el, props });
    aberto = { comp };
    el.querySelector<HTMLElement>('.sv-linha[data-chave="peer:vps"]')!.click();
    flushSync();
    const c = document.querySelector<HTMLElement>('.mq-linha[data-chave="peer:vps"]')!;
    c.querySelector<HTMLInputElement>('.mq-acompanhar')!.click();
    expect(cbs.onInformarToken).toHaveBeenCalledWith(C, '');
    expect(cbs.onAcompanhar).not.toHaveBeenCalled();
    flushSync();
    // sem erro, o campo não aparece
    expect(c.querySelector(`input[aria-label="${m.sessao_token()}"]`)).toBeNull();
    props.tokenErro = { 'peer:vps': 'sem token lá' };
    flushSync();
    expect(c.textContent).toContain('sem token lá');
    const inp = c.querySelector<HTMLInputElement>(`input[aria-label="${m.sessao_token()}"]`)!;
    inp.value = ' tok '; inp.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    inp.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    expect(cbs.onInformarToken).toHaveBeenLastCalledWith(C, 'tok');
  });

  it('uma linha por endereço guardado: Nome e token abre a entrada; Tirar só com mais de uma', () => {
    const S2: Server = { ...SB, id: 'srv-b2', label: 'Notebook LAN', baseUrl: 'http://b2' };
    const t = montar([B, { ...E, navegadores: [SE] }]);
    expect(t.detalhe('srv:srv-e').querySelector('.sd-tirar')).toBeNull();
    unmount(t.comp); aberto = null;
    const dup = { ...B, navegadores: [SB, S2] };
    const t2 = montar([dup]);
    const b = t2.detalhe('srv:srv-b');
    expect(b.textContent).toContain(m.servidores_guardado_explica({ n: 2 }));
    expect(b.textContent).toContain('http://b2');
    const tirar = b.querySelectorAll<HTMLButtonElement>('.sd-tirar');
    expect(tirar.length).toBe(2);
    tirar[1].click();
    flushSync();
    expect(t2.cbs.onTirar).toHaveBeenCalledWith(S2);
    expect(document.querySelector('.mq-linha[data-chave="srv:srv-b"]')).toBeNull();
    const b2 = t2.detalhe('srv:srv-b');
    botao(b2, m.servidores_nome_token()).click();
    flushSync();
    expect(t2.cbs.onEditar).toHaveBeenCalledWith(SB);
    expect(document.querySelector('.mq-linha[data-chave="srv:srv-b"]')).toBeNull();
  });

  it('servidor sem identificador não pode "falar", e o cartão pede o nome dele', () => {
    const t = montar([D]);
    const d = t.detalhe('srv:srv-d');
    expect(d.querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);
    expect(cartao(d)!.textContent).toContain(m.servidores_rec_sem_id_dele({ nome: 'Fora' }));
    // sem identificador não há campo no avançado: o nome se dá pelo cartão
    expect(d.querySelector('.sd-id')).toBeNull();
    const campo = cartao(d)!.querySelector<HTMLInputElement>('.sd-in')!;
    campo.value = 'fora'; campo.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    botao(cartao(d)!, m.ctx_salvar()).click();
    expect(t.cbs.onSalvarIdentificador).toHaveBeenCalledWith(D, 'fora');
  });

  it('o detalhe separa "não respondeu" de "respondeu sem nome", e o token recusado traz o atalho', () => {
    const t = montar([MUDA, RECUSA]);
    const muda = t.detalhe('srv:srv-m');
    expect(cartao(muda)!.textContent).toContain(m.servidores_rec_sem_resposta({ nome: 'Muda' }));
    expect(muda.textContent).not.toContain(m.servidores_rec_sem_id_dele({ nome: 'Muda' }));
    const recusa = t.detalhe('srv:srv-r');
    expect(cartao(recusa)!.textContent).toContain(m.servidores_rec_token_recusado({ nome: 'Recusa' }));
    botao(recusa, m.servidores_trocar_token()).click();
    expect(t.cbs.onEditar).toHaveBeenCalledWith(SR);
  });

  it('funcionando nos dois sentidos mostra o ok e a medida de cada lado', () => {
    const t = montar([B], { estados: {
      notebook: { ok: true, lados: [{ lado: 'ida', estado: 'ok', tempo_ms: 12.4 }, { lado: 'volta', estado: 'ok', tempo_ms: 30 }] },
    } });
    const c = cartao(t.detalhe('srv:srv-b'))!;
    expect(c.classList.contains('ok')).toBe(true);
    expect(c.textContent).toContain(m.servidores_rec_ok());
    expect(c.textContent).toContain(m.servidores_rec_medida({ de: 'Casa', para: 'Notebook', ms: '12' }));
    expect(c.textContent).toContain(m.servidores_rec_medida({ de: 'Notebook', para: 'Casa', ms: '30' }));
    botao(c, m.peers_testar_novamente()).click();
    expect(t.cbs.onTestar).toHaveBeenCalledTimes(2);
  });

  it('endereço que atende como outra máquina diz QUEM atendeu, em vez de "só de ida"', () => {
    const t = montar([B], { estados: {
      notebook: { ok: false, lados: [
        { lado: 'ida', estado: 'ok' },
        { lado: 'volta', estado: 'estranho', motivo: 'identidade', identificador: 'outra', url: 'http://127.0.0.1:8765' },
      ] },
    } });
    const c = cartao(t.detalhe('srv:srv-b'))!;
    expect(c.textContent).toContain(m.servidores_rec_volta_outra({ este: 'Casa', nome: 'Notebook' }));
    expect(c.textContent).toContain(m.servidores_rec_responde_como({ endereco: 'http://127.0.0.1:8765', outro: 'outra' }));
    expect(c.textContent).not.toContain(m.servidores_rec_so_um_lado({ este: 'Casa', nome: 'Notebook' }));
    // sem "só de ida" aqui: o endereço está errado, não faltando
    expect(botao(c, m.peers_so_ida())).toBeUndefined();
  });

  it('volta falhando: escolhe o endereço desta máquina, usa e testa, ou deixa só de ida', () => {
    const enderecos = [
      { tipo: 'nesta_maquina', url: 'http://127.0.0.1:8765', estado: 'ok', tempo_ms: 1 },
      { tipo: 'rede_local', url: 'http://192.168.0.2:8765', estado: 'falhou', tempo_ms: null },
      { tipo: 'tailscale', url: 'https://casa.ts.net', estado: 'ok', tempo_ms: 20 },
    ];
    const t = montar([B], { enderecos, estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'falhou', url: 'http://velho' }] },
    } });
    const d = t.detalhe('srv:srv-b');
    const c = cartao(d)!;
    expect(c.textContent).toContain(m.servidores_rec_so_um_lado({ este: 'Casa', nome: 'Notebook' }));
    expect(c.textContent).toContain(m.servidores_rec_tentou({ endereco: 'http://velho' }));
    // "nesta máquina" não serve para a outra chegar; o que responde vem primeiro e marcado
    expect(c.textContent).not.toContain('http://127.0.0.1:8765');
    const radios = c.querySelectorAll<HTMLInputElement>('input[type="radio"]');
    expect(radios.length).toBe(3);
    expect(radios[0].checked).toBe(true);
    botao(c, m.servidores_rec_usar()).click();
    expect(t.cbs.onUsarEndereco).toHaveBeenCalledWith(B, 'https://casa.ts.net');
    // "Outro" com endereço digitado
    radios[2].click();
    flushSync();
    const outro = c.querySelector<HTMLInputElement>(`input[aria-label="${m.servidores_rec_outro()}"]`)!;
    outro.value = ' http://novo '; outro.dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    botao(c, m.servidores_rec_usar()).click();
    expect(t.cbs.onUsarEndereco).toHaveBeenLastCalledWith(B, 'http://novo');
    botao(c, m.peers_so_ida()).click();
    flushSync();
    expect(cartao(d)!.textContent).toContain(m.servidores_rec_so_ida({ este: 'Casa', nome: 'Notebook' }));
    botao(cartao(d)!, m.servidores_rec_escolher()).click();
    flushSync();
    expect(cartao(d)!.querySelectorAll('input[type="radio"]').length).toBe(3);
  });

  it('volta sem token: usa o token que este servidor guarda; sem registro: cadastra de novo', () => {
    const t = montar([C, E], { estados: {
      vps: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'token' }] },
      mac: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'nao_configurado', motivo: 'registro' }] },
    } });
    const c = cartao(t.detalhe('peer:vps'))!;
    expect(c.textContent).toContain(m.servidores_rec_falta_token({ este: 'Casa', nome: 'vps' }));
    botao(c, m.servidores_usar_token_servidor({ este: 'Casa' })).click();
    expect(t.cbs.onInformarToken).toHaveBeenCalledWith(C, '');
    const e = cartao(t.detalhe('srv:srv-e'))!;
    expect(e.textContent).toContain(m.servidores_rec_sem_registro({ este: 'Casa', nome: 'Mac' }));
    botao(e, m.servidores_rec_cadastrar_de_novo()).click();
    expect(t.cbs.onFalar).toHaveBeenCalledWith(E, true);
  });

  it('volta recusada por token ganha cartão próprio e o atalho de trocar o token', () => {
    const t = montar([B], { estados: {
      notebook: { ok: false, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'recusou', motivo: 'credencial' }] },
    } });
    const c = cartao(t.detalhe('srv:srv-b'))!;
    expect(c.textContent).toContain(m.servidores_rec_token_recusado({ nome: 'Notebook' }));
    expect(c.textContent).not.toContain(m.servidores_rec_so_um_lado({ este: 'Casa', nome: 'Notebook' }));
    botao(c, m.servidores_trocar_token()).click();
    expect(t.cbs.onEditar).toHaveBeenCalledWith(SB);
  });

  it('ida falhando diz que não chega lá, com testar de novo', () => {
    const t = montar([E], { estados: { mac: { ok: false, lados: [{ lado: 'ida', estado: 'falhou' }, { lado: 'volta', estado: 'ok' }] } } });
    const c = cartao(t.detalhe('srv:srv-e'))!;
    expect(c.textContent).toContain(m.servidores_rec_ida_falhou({ este: 'Casa', nome: 'Mac' }));
    expect(botao(c, m.peers_testar_novamente())).toBeDefined();
  });

  it('sem identificador próprio, nenhum servidor pode "falar", e o cartão leva a dar o nome', () => {
    const X: LinhaMaquina = { ...B, peer: null };
    const t = montar([X], { meuIdentificador: '' });
    const b = t.detalhe('srv:srv-b');
    expect(b.querySelector<HTMLInputElement>('.mq-falar')!.disabled).toBe(true);
    expect(cartao(b)!.textContent).toContain(m.servidores_rec_sem_meu_id({ este: 'Casa' }));
    botao(b, m.servidores_rec_dar_nome()).click();
    flushSync();
    expect(t.cbs.onAbrirEste).toHaveBeenCalled();
    expect(document.querySelector('.mq-linha[data-chave="srv:srv-b"]')).toBeNull();
  });

  it('recados desligados: sem cartão, legenda diz que estão desligados', () => {
    const X: LinhaMaquina = { ...B, peer: null };
    const d = montar([X]).detalhe('srv:srv-b');
    expect(cartao(d)).toBeNull();
    expect(d.textContent).toContain(m.servidores_rec_desligado({ este: 'Casa', nome: 'Notebook' }));
    expect(d.querySelector<HTMLInputElement>('.mq-falar')!.checked).toBe(false);
  });

  it('o identificador do outro servidor é editável no avançado, e só com token dele neste aparelho', () => {
    const t = montar([B, C]);
    expect(t.detalhe('peer:vps').querySelector('.sd-id')).toBeNull();
    const b = t.detalhe('srv:srv-b');
    const campo = b.querySelector<HTMLInputElement>('.sd-avan .sd-id')!;
    expect(campo.value).toBe('notebook');
    campo.value = 'nb'; campo.dispatchEvent(new Event('input', { bubbles: true }));
    campo.dispatchEvent(new Event('blur', { bubbles: true }));
    expect(t.cbs.onSalvarIdentificador).toHaveBeenCalledWith(B, 'nb');
  });

  it('sem mexer no campo, sair dele não regrava; o erro da gravação aparece', () => {
    const t = montar([B], { idErro: { 'srv-b': 'deu ruim' } });
    const b = t.detalhe('srv:srv-b');
    b.querySelector<HTMLInputElement>('.sd-id')!.dispatchEvent(new Event('blur', { bubbles: true }));
    expect(t.cbs.onSalvarIdentificador).not.toHaveBeenCalled();
    expect(b.textContent).toContain('deu ruim');
  });

  it('o avançado mostra os dois endereços e "Trocar" abre a escolha da volta', () => {
    const t = montar([B], { enderecos: [{ tipo: 'tailscale', url: 'https://casa.ts.net', estado: 'ok', tempo_ms: 5 }], estados: {
      notebook: { ok: true, lados: [{ lado: 'ida', estado: 'ok' }, { lado: 'volta', estado: 'ok', url: 'http://casa.lan' }] },
    } });
    const avan = t.detalhe('srv:srv-b').querySelector<HTMLElement>('.sd-avan')!;
    expect(avan.textContent).toContain('https://nb.ts.net');
    expect(avan.textContent).toContain('http://casa.lan');
    botao(avan, m.servidores_rec_trocar()).click();
    flushSync();
    botao(avan, m.servidores_rec_usar()).click();
    expect(t.cbs.onUsarEndereco).toHaveBeenCalledWith(B, 'https://casa.ts.net');
  });

  it('"ligado neste servidor" segue o enabled do peers.json e só avisa o dono ao clicar', () => {
    const desligado: LinhaMaquina = { ...C, peer: { ...C.peer!, enabled: false } };
    const t = montar([desligado, D]);
    t.el.querySelector<HTMLDetailsElement>('details.mq-fora')!.open = true;
    const cb = t.detalhe('peer:vps').querySelector<HTMLInputElement>('.mq-varredura')!;
    expect(cb.checked).toBe(false);
    cb.click();
    expect(cb.checked).toBe(false);
    expect(t.cbs.onToggleScan).toHaveBeenCalledWith(desligado, true);
    // Sem registro no peers.json não há o que ligar.
    expect(t.detalhe('srv:srv-d').querySelector('.mq-varredura')).toBeNull();
  });

  it('peer desligado no servidor: cartão "pausada" com Religar, mesmo com estado de falha', () => {
    const G: LinhaMaquina = { ...E, peer: { id: 'mac', base_url: 'https://mac', token: '••', enabled: false } };
    const t = montar([G], { estados: { mac: { ok: false, lados: [{ lado: 'ida', estado: 'falhou', motivo: 'timeout' }] } } });
    const c = cartao(t.detalhe('srv:srv-e'))!;
    expect(c.textContent).toContain(m.servidores_desligada());
    expect(c.textContent).not.toContain(m.servidores_rec_ida_falhou({ este: 'Casa', nome: 'Mac' }));
    botao(c, m.servidores_rec_religar()).click();
    expect(t.cbs.onToggleScan).toHaveBeenCalledWith(G, true);
  });

  it('Remover devolve a linha inteira', () => {
    const t = montar([C]);
    const c = t.detalhe('peer:vps');
    expect(c.querySelector('.mq-editar')).toBeNull();
    expect(c.querySelector('.mq-remover')!.textContent).toContain(m.servidores_remover_maquina());
    c.querySelector<HTMLButtonElement>('.mq-remover')!.click();
    expect(t.cbs.onRemover).toHaveBeenCalledWith(C);
  });

  it('a etiqueta "Este servidor" fica no lado do servidor, e só nele', () => {
    const b = montar([B]).detalhe('srv:srv-b');
    const campos = [...b.querySelectorAll<HTMLElement>('.sd-campo')];
    // A caixa do navegador grava no localStorage deste aparelho: sem etiqueta.
    expect(campos.find((c) => c.querySelector('.mq-acompanhar'))!.querySelector('.escopo')).toBeNull();
    // Recados grava no peers.json DO SERVIDOR: etiqueta no título do bloco.
    const grupo = [...b.querySelectorAll<HTMLElement>('.sd-grupo')].find((g) => g.textContent?.includes(m.maquinas_falar()))!;
    expect(grupo.querySelector('.escopo')!.textContent).toBe(m.config_escopo_servidor());
    // Remover apaga o peer no servidor antes de mexer no navegador.
    expect(b.querySelector('.mq-remover .escopo')!.textContent).toBe(m.config_escopo_servidor());
    // Editar abre a entrada do NAVEGADOR: etiqueta ali seria falso.
    expect(b.querySelector('.mq-editar .escopo')).toBeNull();
  });

  it('o Remover segue os lados que a linha tem, no chip e no nome acessível', () => {
    const t = montar([D, B, C]);
    const d = t.detalhe('srv:srv-d');
    expect(d.querySelector('.mq-remover .escopo')).toBeNull();
    expect(d.querySelector('.mq-remover')!.getAttribute('aria-label')).toBe(m.maquinas_remover_aria_local({ nome: D.nome }));
    expect(t.detalhe('srv:srv-b').querySelector('.mq-remover')!.getAttribute('aria-label')).toBe(m.maquinas_remover_aria({ nome: B.nome }));
    expect(t.detalhe('peer:vps').querySelector('.mq-remover')!.getAttribute('aria-label')).toBe(m.maquinas_remover_aria_servidor({ nome: C.nome }));
  });
});
