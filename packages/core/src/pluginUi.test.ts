import { describe, expect, it } from 'vitest';
import { activePaneId, applyPluginUiDelta, buttonControl, followLocalTab, hoverProps, inputControl, isHoverScope, isMissingRoute, isPluginServerFailure, pluginFailureText, tabFollowsServer, decodeRaster, inkColor, isEmptyBand, parsePluginToast, parsePluginUi, textOf, type PluginElement } from './pluginUi';
import { mensagemDeErro } from './errosApi';
import amostras from './__fixtures__/plugin-ui-arvores.json';

function cells(words: number[]): string {
  const bytes = new Uint8Array(new Uint32Array(words).buffer);
  return btoa(String.fromCharCode(...bytes));
}

describe('decodeRaster', () => {
  it('lê código, frente e fundo por célula, com a cor padrão como null', () => {
    const grid = decodeRaster(cells([0x2501, 0x5aa6ff, 0x01000000, 0x41, 0x01000000, 0x112233]), 2, 1);
    expect(grid).toEqual([[
      { ch: '━', fg: '#5aa6ff', bg: null },
      { ch: 'A', fg: null, bg: '#112233' },
    ]]);
  });

  it('para no fim dos dados em vez de inventar célula', () => {
    expect(decodeRaster(cells([0x41, 0, 0]), 3, 1)[0]).toHaveLength(1);
  });

  it('não cria linha além dos dados que chegaram, mesmo com `rows` enorme', () => {
    expect(decodeRaster(cells([0x41, 0, 0, 0x42, 0, 0, 0x43, 0, 0]), 2, 1_000)).toHaveLength(2);
    expect(decodeRaster(cells([0x41, 0, 0]), 0, 1_000)).toEqual([]);
  });
});

describe('isEmptyBand', () => {
  it('trata ausência e o marcador do engine como faixa vazia', () => {
    expect(isEmptyBand(null)).toBe(true);
    expect(isEmptyBand({ type: 'engine' })).toBe(true);
    expect(isEmptyBand({ type: 'Box', children: [] })).toBe(false);
  });
});

it('inkColor traduz nomes do Ink e mantém hex', () => {
  expect(inkColor('redBright')).toBe('#f14c4c');
  expect(inkColor('#5aa6ff')).toBe('#5aa6ff');
  expect(inkColor(undefined)).toBeNull();
});

it('inkColor recusa texto que injetaria CSS', () => {
  expect(inkColor('red;position:fixed')).toBeNull();
  expect(inkColor('url(https://x)')).toBeNull();
  expect(inkColor('rgb(1, 2, 3)')).toBe('rgb(1, 2, 3)');
});

it('textOf junta só texto e número', () => {
  expect(textOf(['a', 1, { type: 'Text' }, null])).toBe('a1');
});

describe('parsePluginUi', () => {
  it('lê faixa e painéis e descarta painel sem id', () => {
    const s = parsePluginUi({ above: { type: 'Box' }, panes: [
      { id: 'review-mr', title: 'Review !577', placement: 'dock', columns: 72, tree: { type: 'Box' } },
      { title: 'sem id' },
    ] });
    expect(s.above).toEqual({ type: 'Box' });
    expect(s.panes.map((p) => p.id)).toEqual(['review-mr']);
    expect(s.panes[0].placement).toBe('dock');
  });

  it('aceita o formato antigo, só com a faixa', () => {
    expect(parsePluginUi({ above: null })).toEqual({ above: null, panes: [], shownId: undefined, columns: null, source: null });
  });

  it('placement desconhecido vira inline', () => {
    expect(parsePluginUi({ panes: [{ id: 'a', placement: 'x' }] }).panes[0].placement).toBe('inline');
  });
});

describe('parsePluginToast', () => {
  it('lê o aviso e o mod que o emitiu', () => {
    expect(parsePluginToast({ id: 'ab-1', text: 'Jenkins configurado.', plugin: 'demo', timeoutMs: 9000 }))
      .toEqual({ id: 'ab-1', text: 'Jenkins configurado.', plugin: 'demo', timeoutMs: 9000 });
  });

  it('sem id, sem texto ou sem prazo não é aviso', () => {
    expect(parsePluginToast({ text: 'oi', timeoutMs: 4000 })).toBeNull();
    expect(parsePluginToast({ id: 'ab-3', text: '  ', timeoutMs: 4000 })).toBeNull();
    expect(parsePluginToast({ id: 'ab-4', text: 'oi' })).toBeNull();
    expect(parsePluginToast({ id: 'ab-5', text: 'oi', timeoutMs: 0 })).toBeNull();
    expect(parsePluginToast(null)).toBeNull();
  });
});

it('buttonControl só para Button com key em texto e o mod do press', () => {
  const press = { plugin: 'pm-mock', handle: 1 };
  expect(buttonControl({ type: 'Button', props: { key: 'cp-1' }, press })).toEqual({ plugin: 'pm-mock', key: 'cp-1' });
  expect(buttonControl({ type: 'Button', props: { key: 'cp-1' } })).toBeNull();
  expect(buttonControl({ type: 'Button', props: {}, press })).toBeNull();
  expect(buttonControl({ type: 'Text', props: { key: 'x' }, press })).toBeNull();
});

describe('parsePluginUi: campos novos da fase 1', () => {
  const evento = (extra: Record<string, unknown>) => ({
    above: amostras.faixaPm,
    panes: amostras.rolPm.panes.map((p) => ({ ...p, placement: 'dock', columns: 58, tree: { type: 'engine', ref: 0 } })),
    ...extra,
  });

  it('lê shown_id, columns e source quando o servidor manda', () => {
    const s = parsePluginUi(evento({ shown_id: 'pm-mock-mr', columns: 110, source: 'surface' }));
    expect([s.shownId, s.columns, s.source]).toEqual(['pm-mock-mr', 110, 'surface']);
    expect(s.panes.map((p) => p.id)).toEqual(['pm-mock-pm', 'pm-mock-mr', 'pm-mock-jenkins']);
  });

  it('servidor de hoje: sem os campos, shownId fica undefined e o resto null', () => {
    const s = parsePluginUi(evento({}));
    expect(s.shownId).toBeUndefined();
    expect(s.columns).toBeNull();
    expect(s.source).toBeNull();
  });

  it('shown_id null quer dizer "sem painel"; valores estranhos valem como ausentes', () => {
    expect(parsePluginUi({ shown_id: null }).shownId).toBeNull();
    const s = parsePluginUi({ shown_id: 7, columns: -3, source: 'mobile' });
    expect([s.shownId, s.columns, s.source]).toEqual([undefined, null, null]);
    expect(parsePluginUi({ shown_id: '' }).shownId).toBeUndefined();
  });
});

describe('aba ativa', () => {
  const ids = ['pm-mock-pm', 'pm-mock-mr', 'pm-mock-jenkins'];

  it('segue o shown_id quando ele nomeia um painel da lista', () => {
    expect(tabFollowsServer(ids, 'pm-mock-pm')).toBe(true);
    expect(activePaneId(ids, 'pm-mock-pm', 'pm-mock-mr')).toBe('pm-mock-pm');
  });

  it('shown_id de painel que ainda não chegou: vale a escolha local, nunca nada', () => {
    expect(tabFollowsServer(ids, 'pm-mock-novo')).toBe(false);
    expect(activePaneId(ids, 'pm-mock-novo', 'pm-mock-mr')).toBe('pm-mock-mr');
    expect(activePaneId(ids, 'pm-mock-novo', null)).toBe('pm-mock-jenkins');
  });

  it('sem shown_id (servidor antigo), escolha local; sem ela, o último aberto; sem painel, null', () => {
    expect(activePaneId(ids, undefined, 'pm-mock-pm')).toBe('pm-mock-pm');
    expect(activePaneId(ids, null, null)).toBe('pm-mock-jenkins');
    expect(activePaneId([], 'x', 'y')).toBeNull();
  });

  it('escolha local que não está mais na lista cai no último aberto', () => {
    expect(activePaneId(ids, undefined, 'fechado')).toBe('pm-mock-jenkins');
  });
});

describe('escolha local da aba', () => {
  it('começa no último painel aberto', () => {
    expect(followLocalTab([], ['a', 'b', 'c'], null)).toBe('c');
  });

  it('sobrevive a um redesenho sem painel novo', () => {
    expect(followLocalTab(['a', 'b', 'c'], ['a', 'b', 'c'], 'a')).toBe('a');
  });

  it('painel que acaba de abrir vai para a frente, como no terminal', () => {
    expect(followLocalTab(['a', 'b'], ['a', 'b', 'd'], 'a')).toBe('d');
  });

  it('fechado o escolhido, fica o vizinho anterior; sem anterior, o seguinte', () => {
    expect(followLocalTab(['a', 'b', 'c'], ['a', 'c'], 'b')).toBe('a');
    expect(followLocalTab(['a', 'b'], ['b'], 'a')).toBe('b');
    expect(followLocalTab(['a'], [], 'a')).toBeNull();
  });
});

describe('rota ausente', () => {
  it('404 e 405 são servidor anterior à rota; 409 e erro sem status não são', () => {
    expect(isMissingRoute(Object.assign(new Error('Not Found'), { status: 404 }))).toBe(true);
    expect(isMissingRoute(Object.assign(new Error('Method Not Allowed'), { status: 405 }))).toBe(true);
    expect(isMissingRoute(Object.assign(new Error('x'), { status: 409, code: 'erro_mod_dialogo_aberto' }))).toBe(false);
    expect(isMissingRoute(new Error('rede'))).toBe(false);
    expect(isMissingRoute(null)).toBe(false);
  });
});

describe('falha de servidor numa rota de mod', () => {
  it('sem status ou 5xx é falha do servidor ou da rede; 4xx traz o motivo da recusa', () => {
    expect(isPluginServerFailure(new Error('rede'))).toBe(true);
    expect(isPluginServerFailure(Object.assign(new Error('x'), { status: 500 }))).toBe(true);
    expect(isPluginServerFailure(Object.assign(new Error('x'), { status: 503 }))).toBe(true);
    expect(isPluginServerFailure(Object.assign(new Error('409: frase'), { status: 409 }))).toBe(false);
    expect(isPluginServerFailure(Object.assign(new Error('x'), { status: 404 }))).toBe(false);
  });
});

describe('frase da falha numa rota de mod', () => {
  const generica = () => 'frase-generica';
  it('código conhecido vira a frase dele em qualquer status, inclusive o 503 do dono único', () => {
    const guarda = Object.assign(new Error('503: motivo — erro_mod_guarda_indisponivel'), { status: 503, code: 'erro_mod_guarda_indisponivel' });
    expect(pluginFailureText(guarda, generica)).toBe(mensagemDeErro('erro_mod_guarda_indisponivel'));
    const recusa = Object.assign(new Error('409: x'), { status: 409, code: 'erro_mod_sem_digitacao' });
    expect(pluginFailureText(recusa, generica)).toBe(mensagemDeErro('erro_mod_sem_digitacao'));
  });
  it('sem código: 5xx ou sem resposta é a frase genérica; 4xx é o motivo que veio', () => {
    expect(pluginFailureText(Object.assign(new Error('500: Internal Server Error'), { status: 500 }), generica)).toBe('frase-generica');
    expect(pluginFailureText(new Error('rede'), generica)).toBe('frase-generica');
    expect(pluginFailureText(Object.assign(new Error('503: x'), { status: 503, code: 'codigo_desconhecido' }), generica)).toBe('frase-generica');
    expect(pluginFailureText(Object.assign(new Error('409: motivo'), { status: 409 }), generica)).toBe('409: motivo');
  });
});

describe('hover', () => {
  it('Box com key é escopo; sem key ou outro tipo, não', () => {
    expect(isHoverScope(amostras.hoverV29 as unknown as PluginElement)).toBe(true);
    expect(isHoverScope({ type: 'Box', props: {} })).toBe(false);
    expect(isHoverScope({ type: 'Text', props: { key: 'x' } })).toBe(false);
  });

  it('com o escopo aceso, o hover do nó vence as props; apagado, as props valem', () => {
    const cartao = amostras.hoverV29.children[1] as unknown as PluginElement;
    expect(hoverProps(cartao, false).display).toBe('none');
    expect(hoverProps(cartao, true).display).toBe('flex');
    expect(hoverProps(cartao, true).position).toBe('absolute');
  });

  it('hover com scope (grupo entre lugares) fica para depois: o nó segue sem hover', () => {
    const v30 = { type: 'Text', hover: { scope: 'vitrine-V30', color: '#e8a33d' } } as PluginElement;
    expect(hoverProps(v30, true)).toEqual({});
  });

  it('inputControl só para Input com key em texto e o mod do press', () => {
    expect(inputControl(amostras.campoV18 as unknown as PluginElement)).toEqual({ plugin: 'vitrine', key: 'V18-campo' });
    expect(inputControl({ type: 'Input', props: { key: 'V18-campo' } })).toBeNull();
    expect(inputControl({ type: 'Input', props: {}, press: { plugin: 'vitrine' } })).toBeNull();
    expect(inputControl({ type: 'Button', props: { key: 'x' }, press: { plugin: 'vitrine' } })).toBeNull();
  });
});

describe('applyPluginUiDelta', () => {
  const prev = { above: { type: 'Text', children: ['1'] }, panes: [{ id: 'a', tree: { type: 'Text', children: ['grande'] } },
    { id: 'b', tree: null }], shown_id: 'a', source: 'terminal' };

  it('mantém a faixa ausente e repõe o painel igual pelo id', () => {
    const full = applyPluginUiDelta(prev, { panes: [{ id: 'b', tree: { type: 'Text', children: ['novo'] } }, { id: 'a', same: true }],
      shown_id: 'b', source: 'terminal' });
    expect(full).toEqual({ above: prev.above, panes: [{ id: 'b', tree: { type: 'Text', children: ['novo'] } }, prev.panes[0]],
      shown_id: 'b', source: 'terminal' });
    expect(parsePluginUi(full).panes.map((p) => p.id)).toEqual(['b', 'a']);
  });

  it('troca a faixa que veio, inclusive nula, e some com o painel que não veio', () => {
    expect(applyPluginUiDelta(prev, { above: null, panes: [] })).toEqual({ above: null, panes: [] });
  });
});
