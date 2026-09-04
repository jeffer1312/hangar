const test = require('node:test');
const assert = require('node:assert/strict');
const { criarControlador } = require('./preview_ctl.cjs');

function dubleDbg(respostas = {}) {
  const chamadas = [];
  const ouvintes = new Map();
  return {
    chamadas,
    emitir: (ev, p) => (ouvintes.get(ev) || []).forEach((cb) => cb(null, p)),
    on: (ev, cb) => ouvintes.set(ev, [...(ouvintes.get(ev) || []), cb]),
    sendCommand: async (m, p) => { chamadas.push([m, p]); return respostas[m] ?? {}; },
  };
}

test('tema aplica prefers-color-scheme e REAPLICA depois de navegar', async () => {
  const dbg = dubleDbg();
  let renavegar = null;
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: (cb) => (renavegar = cb) });
  await ctl.tema('escuro');
  await renavegar();
  const temas = dbg.chamadas.filter(([m]) => m === 'Emulation.setEmulatedMedia');
  assert.equal(temas.length, 2, 'a emulacao se perde na navegacao; tem que ser reaplicada');
  assert.deepEqual(temas[1][1].features, [{ name: 'prefers-color-scheme', value: 'dark' }]);
});

test('refs de antes da navegacao sao invalidadas', async () => {
  const dbg = dubleDbg({ 'Accessibility.getFullAXTree': { nodes: [
    { nodeId: '1', role: { value: 'button' }, name: { value: 'Ok' }, childIds: [], backendDOMNodeId: 5 },
  ] } });
  let renavegar = null;
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: (cb) => (renavegar = cb) });
  await ctl.snapshot();
  assert.equal(ctl.refDe('@e1'), 5);
  await renavegar();
  assert.equal(ctl.refDe('@e1'), null);
});

test('a fila serializa: o segundo comando so comeca quando o primeiro termina', async () => {
  const ctl = criarControlador({ dbg: dubleDbg(), capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const ordem = [];
  const a = ctl.enfileirar(async () => { ordem.push('a1'); await new Promise((r) => setTimeout(r, 20)); ordem.push('a2'); });
  const b = ctl.enfileirar(async () => { ordem.push('b1'); });
  await Promise.all([a, b]);
  assert.deepEqual(ordem, ['a1', 'a2', 'b1']);
});

test('console guarda no maximo o teto e devolve as ultimas', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  for (let i = 0; i < 250; i++) dbg.emitir('Runtime.consoleAPICalled', { type: 'log', args: [{ value: `m${i}` }] });
  const saida = ctl.console(false);
  assert.equal(saida.split('\n').length, 200);
  assert.match(saida, /m249/);
  assert.doesNotMatch(saida, /m49\b/);
});

test('fila continua apos comando do meio rejeitar', async () => {
  const ctl = criarControlador({ dbg: dubleDbg(), capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const ordem = [];
  const a = ctl.enfileirar(async () => { ordem.push('a'); });
  const b = ctl.enfileirar(async () => { ordem.push('b'); throw new Error('falha'); });
  const c = ctl.enfileirar(async () => { ordem.push('c'); });
  await Promise.all([a, c]);
  assert.deepEqual(ordem, ['a', 'b', 'c'], 'terceiro comando rodou mesmo apos rejeicao do meio');
  await assert.rejects(async () => b, /falha/, 'segundo comando rejeitou pra quem chamou');
});

test('rede guarda no maximo o teto e devolve as ultimas', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  for (let i = 0; i < 250; i++) dbg.emitir('Network.responseReceived', { response: { status: 200, url: `http://ex.com/r${i}` } });
  const saida = ctl.rede();
  assert.equal(saida.split('\n').length, 200);
  assert.match(saida, /r249/);
  assert.doesNotMatch(saida, /r49\b/);
});
