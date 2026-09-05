const test = require('node:test');
const assert = require('node:assert/strict');
const { criarControlador } = require('./preview_ctl.cjs');

function dubleDbg(respostas = {}) {
  const chamadas = [];
  const ouvintes = new Map();
  return {
    chamadas,
    // Mesma forma do Electron: um só evento 'message' com (event, método, params).
    emitir: (ev, p) => (ouvintes.get('message') || []).forEach((cb) => cb(null, ev, p)),
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
  const saida = await ctl.rede();
  assert.equal(saida.split('\n').length, 200);
  assert.match(saida, /r249/);
  assert.doesNotMatch(saida, /r49\b/);
});

test('clicar usa evento de mouse REAL no centro da caixa, nao .click() em JS', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'button' }, name: { value: 'Ok' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [10, 20, 30, 20, 30, 40, 10, 40] } },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  const saida = await ctl.clicar('@e1');
  const mouse = dbg.chamadas.filter(([m]) => m === 'Input.dispatchMouseEvent');
  assert.equal(mouse.length, 2, 'mousePressed e mouseReleased');
  assert.equal(mouse[0][1].type, 'mousePressed');
  assert.equal(mouse[0][1].x, 20);
  assert.equal(mouse[0][1].y, 30);
  assert.match(saida, /^ok: click @e1/);
});

test('clicar em ref desconhecida nao dispara evento nenhum', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  assert.match(await ctl.clicar('@e9'), /^erro: ref @e9 nao existe/);
  assert.equal(dbg.chamadas.filter(([m]) => m === 'Input.dispatchMouseEvent').length, 0);
});

test('ref que ficou velha (no desmontado por re-render) responde como ref inexistente', async () => {
  const dbg = dubleDbg({ 'Accessibility.getFullAXTree': { nodes: [
    { nodeId: '1', role: { value: 'button' }, name: { value: 'Ok' }, childIds: [], backendDOMNodeId: 5 },
  ] } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  // O React trocou a lista sem navegar: o nó sumiu, mas a ref segue no mapa.
  dbg.sendCommand = async (m) => {
    if (m === 'DOM.getBoxModel') throw new Error('Could not find node with given id');
    return {};
  };
  assert.match(await ctl.clicar('@e1'), /^erro: ref @e1 nao existe \(rode snapshot de novo\)/);
});

// A ORDEM é o contrato: focar, selecionar tudo, inserir. Sem o passo da seleção o insertText
// concatena — foi o defeito medido no navegador de verdade.
test('preencher foca, seleciona tudo com o comando de edicao e insere por cima', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'textbox' }, name: { value: 'Nome' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [0, 0, 10, 0, 10, 10, 0, 10] } },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  await ctl.preencher('@e1', 'Jefferson');
  const entrada = dbg.chamadas.filter(([m]) => m.startsWith('Input.'));
  assert.deepEqual(entrada.map(([m]) => m), [
    'Input.dispatchMouseEvent',   // mousePressed
    'Input.dispatchMouseEvent',   // mouseReleased
    'Input.dispatchKeyEvent',     // selectAll
    'Input.insertText',
  ]);
  // O que seleciona é o `commands`, não a tecla: Ctrl+A sem virtual key code não seleciona nada.
  assert.deepEqual(entrada[2][1], { type: 'keyDown', commands: ['selectAll'] });
  assert.equal(entrada[3][1].text, 'Jefferson');
});

test('preencher com texto vazio so limpa: seleciona tudo e insere string vazia', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'textbox' }, name: { value: 'Nome' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [0, 0, 10, 0, 10, 10, 0, 10] } },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  await ctl.preencher('@e1', '');
  const inserir = dbg.chamadas.find(([m]) => m === 'Input.insertText');
  assert.equal(inserir[1].text, '');
  assert.ok(dbg.chamadas.some(([m, p]) => m === 'Input.dispatchKeyEvent' && p.commands?.[0] === 'selectAll'));
});

test('preencher em ref desconhecida nao dispara evento nenhum', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  assert.match(await ctl.preencher('@e9', 'x'), /^erro: ref @e9 nao existe/);
  assert.equal(dbg.chamadas.filter(([m]) => m.startsWith('Input.')).length, 0);
});

test('digitar manda Input.insertText com o texto exato', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const saida = await ctl.digitar('hello world');
  const inserir = dbg.chamadas.find(([m]) => m === 'Input.insertText');
  assert.equal(inserir[1].text, 'hello world');
  assert.match(saida, /^ok: type/);
});

test('teclar manda keyDown e keyUp com a tecla pedida nessa ordem', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const saida = await ctl.teclar('Enter');
  const eventos = dbg.chamadas.filter(([m]) => m === 'Input.dispatchKeyEvent');
  assert.equal(eventos.length, 2, 'keyDown e keyUp');
  assert.equal(eventos[0][1].type, 'keyDown');
  assert.equal(eventos[0][1].key, 'Enter');
  assert.equal(eventos[1][1].type, 'keyUp');
  assert.equal(eventos[1][1].key, 'Enter');
  assert.match(saida, /^ok: press Enter/);
});

test('pairar manda mouseMoved no centro da caixa e nada quando ref nao existe', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'button' }, name: { value: 'Hover' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [10, 20, 30, 20, 30, 40, 10, 40] } },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  const saida = await ctl.pairar('@e1');
  const mouse = dbg.chamadas.filter(([m]) => m === 'Input.dispatchMouseEvent');
  assert.equal(mouse.length, 1, 'so mouseMoved');
  assert.equal(mouse[0][1].type, 'mouseMoved');
  assert.equal(mouse[0][1].x, 20);
  assert.equal(mouse[0][1].y, 30);
  assert.match(saida, /^ok: hover @e1/);
});

test('pairar em ref desconhecida nao dispara evento nenhum', async () => {
  const dbg = dubleDbg();
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const saida = await ctl.pairar('@e9');
  assert.match(saida, /^erro: ref @e9 nao existe/);
  assert.equal(dbg.chamadas.filter(([m]) => m === 'Input.dispatchMouseEvent').length, 0);
});

test('avaliar no caminho feliz devolve ok: com valor serializado', async () => {
  const dbg = dubleDbg({
    'Runtime.evaluate': { result: { value: 42 }, exceptionDetails: null },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const saida = await ctl.avaliar('2 + 2 * 20');
  assert.match(saida, /^ok: 42/);
});

test('avaliar quando CDP nao responde antes do teto dispara erro com tempo', async () => {
  const dbg = dubleDbg();
  dbg.sendCommand = async (m, p) => {
    if (m === 'Runtime.evaluate') {
      await new Promise(() => {}); // nunca resolve
    }
    return {};
  };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {}, tetoEspera: 100 });
  const saida = await ctl.avaliar('document.title');
  assert.match(saida, /^erro: eval nao respondeu em 100ms/);
});

test('avaliar prefere exception.description sobre exception.text', async () => {
  const dbg = dubleDbg({
    'Runtime.evaluate': {
      exceptionDetails: {
        text: 'Uncaught',
        exception: { description: 'TypeError: x is not a function' },
      },
    },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const saida = await ctl.avaliar('x()');
  assert.match(saida, /TypeError: x is not a function/);
});

test('wait --text volta assim que o texto aparece', async () => {
  let html = '<p>carregando</p>';
  const dbg = dubleDbg();
  dbg.sendCommand = async (m, p) => {
    if (m === 'Runtime.evaluate' && String(p.expression).includes('innerText')) {
      return { result: { value: html } };
    }
    return {};
  };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  setTimeout(() => { html = '<p>pronto</p>'; }, 30);
  assert.match(await ctl.esperar(['--text', 'pronto']), /^ok: wait/);
});

test('wait estoura o teto e devolve erro em vez de travar', async () => {
  const dbg = dubleDbg({ 'Runtime.evaluate': { result: { value: '' } } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {}, tetoEspera: 60 });
  assert.match(await ctl.esperar(['--text', 'nunca']), /^erro: wait --text nunca nao aconteceu/);
});

test('wait protege contra sendCommand pendurado — devolve erro dentro do teto', async () => {
  const dbg = dubleDbg();
  dbg.sendCommand = async (m) => {
    if (m === 'Runtime.evaluate') {
      await new Promise(() => {}); // nunca resolve
    }
    return {};
  };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {}, tetoEspera: 80 });
  const inicio = Date.now();
  const saida = await ctl.esperar(['--text', 'nunca']);
  const decorrido = Date.now() - inicio;
  assert.match(saida, /^erro: wait --text nunca nao aconteceu/);
  assert.ok(decorrido < 150, `saiu em ${decorrido}ms, deve ser ~80ms do teto`);
});

test('wait --url volta assim que URL contém o trecho', async () => {
  let url = 'http://ex.com/page';
  const dbg = dubleDbg();
  dbg.sendCommand = async (m, p) => {
    if (m === 'Runtime.evaluate' && String(p.expression).includes('location.href')) {
      return { result: { value: url } };
    }
    return {};
  };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  setTimeout(() => { url = 'http://ex.com/resultado'; }, 30);
  assert.match(await ctl.esperar(['--url', 'resultado']), /^ok: wait/);
});

test('wait --idle detecta requisição em voo e bloqueia', async () => {
  const dbg = dubleDbg({ 'Runtime.evaluate': { result: { value: 'complete' } } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {}, tetoEspera: 1200 });
  // Emite requisição em voo
  dbg.emitir('Network.requestWillBeSent', {});
  // Inicia espera por --idle
  const promiseWait = ctl.esperar(['--idle']);
  // Aguarda 100ms: deve estar bloqueado enquanto requisição em voo
  await new Promise((r) => setTimeout(r, 100));
  // Emite resposta e 500ms de silêncio — esperar vai passar
  dbg.emitir('Network.responseReceived', { response: { status: 200, url: 'http://ex.com/api' } });
  await new Promise((r) => setTimeout(r, 520)); // um pouco além de 500ms
  const saida = await promiseWait;
  assert.match(saida, /^ok: wait/);
});

test('criar o controlador NAO liga Network nem Accessibility', () => {
  const dbg = dubleDbg();
  criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  assert.deepEqual(dbg.chamadas.filter(([m]) => m.endsWith('.enable')), []);
});

test('snapshot liga Accessibility na primeira chamada, antes da arvore, e nunca de novo', async () => {
  const dbg = dubleDbg({ 'Accessibility.getFullAXTree': { nodes: [] } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  await ctl.snapshot();
  const metodos = dbg.chamadas.map(([m]) => m);
  assert.deepEqual(metodos, ['Accessibility.enable', 'Accessibility.getFullAXTree', 'Accessibility.getFullAXTree']);
});

test('network e wait --idle compartilham UM Network.enable', async () => {
  const dbg = dubleDbg({ 'Runtime.evaluate': { result: { value: 'complete' } } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {}, tetoEspera: 1500 });
  await ctl.rede();
  await new Promise((r) => setTimeout(r, 520));
  assert.match(await ctl.esperar(['--idle']), /^ok: wait/);
  await ctl.rede();
  assert.equal(dbg.chamadas.filter(([m]) => m === 'Network.enable').length, 1);
});

test('wait --idle liga Network e o silencio conta a partir do ligamento', async () => {
  const dbg = dubleDbg({ 'Runtime.evaluate': { result: { value: 'complete' } } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {}, tetoEspera: 1500 });
  await new Promise((r) => setTimeout(r, 600));   // controlador velho, rede nunca observada
  const inicio = Date.now();
  assert.match(await ctl.esperar(['--idle']), /^ok: wait/);
  assert.ok(Date.now() - inicio >= 500, 'nao pode dizer "parada" sobre uma rede que acabou de comecar a observar');
  assert.equal(dbg.chamadas.filter(([m]) => m === 'Network.enable').length, 1);
});

test('enable que falha vira erro nomeado no verbo, e a proxima chamada tenta de novo', async () => {
  let falhas = 1;
  const dbg = dubleDbg({ 'Accessibility.getFullAXTree': { nodes: [] } });
  const original = dbg.sendCommand;
  dbg.sendCommand = async (m, p) => {
    if (m === 'Accessibility.enable' && falhas-- > 0) { dbg.chamadas.push([m, p]); throw new Error('Target closed'); }
    return original(m, p);
  };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await assert.rejects(() => ctl.snapshot(), /Accessibility\.enable falhou: Target closed/);
  await ctl.snapshot();
  assert.equal(dbg.chamadas.filter(([m]) => m === 'Accessibility.enable').length, 2);
});

test('avaliar reavalia declaracoes cru (replMode) quando a expressao da SyntaxError', async () => {
  const dbg = dubleDbg();
  dbg.sendCommand = async (m, p) => {
    dbg.chamadas.push([m, p]);
    if (m !== 'Runtime.evaluate') return {};
    if (p.replMode) return { result: { value: 2 } };
    return { exceptionDetails: { text: 'Uncaught', exception: { description: "SyntaxError: Unexpected token 'const'" } } };
  };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  const saida = await ctl.avaliar('const a=1; a+1');
  assert.equal(saida, 'ok: 2');
  const cru = dbg.chamadas.find(([m, p]) => m === 'Runtime.evaluate' && p.replMode);
  assert.equal(cru[1].expression, 'const a=1; a+1');
});

test('avaliar com SyntaxError de verdade devolve o erro da segunda tentativa, nao pendura', async () => {
  const dbg = dubleDbg({
    'Runtime.evaluate': { exceptionDetails: { text: 'Uncaught', exception: { description: 'SyntaxError: Unexpected end of input' } } },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  assert.match(await ctl.avaliar('foo('), /^erro: SyntaxError/);
});

test('click e shot esperam um quadro pintado antes de responder', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'button' }, name: { value: 'Ok' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [10, 20, 30, 20, 30, 40, 10, 40] } },
  });
  let capturas = 0;
  const ctl = criarControlador({ dbg, capturarPagina: async () => { capturas++; return Buffer.alloc(0); }, aoNavegar: () => {} });
  await ctl.snapshot();
  await ctl.clicar('@e1');
  const ordem = dbg.chamadas.map(([m, p]) => (m === 'Runtime.evaluate' ? `frame:${/requestAnimationFrame/.test(p.expression)}` : m));
  assert.deepEqual(ordem.slice(-3), ['Input.dispatchMouseEvent', 'Input.dispatchMouseEvent', 'frame:true']);
  const antes = dbg.chamadas.length;
  await ctl.capturarPagina();
  assert.equal(capturas, 1);
  assert.equal(dbg.chamadas[antes][0], 'Runtime.evaluate', 'o quadro vem ANTES da captura');
});

test('clicar rola ate o elemento ANTES de medir a caixa', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'button' }, name: { value: 'Ok' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [10, 20, 30, 20, 30, 40, 10, 40] } },
  });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  await ctl.clicar('@e1');
  const metodos = dbg.chamadas.map(([m]) => m);
  const rolou = metodos.indexOf('DOM.scrollIntoViewIfNeeded');
  assert.ok(rolou >= 0 && rolou < metodos.indexOf('DOM.getBoxModel'));
  assert.equal(dbg.chamadas[rolou][1].backendNodeId, 5);
});

test('texto devolve o innerText cru da pagina', async () => {
  const dbg = dubleDbg({ 'Runtime.evaluate': { result: { value: 'Olá\nmundo' } } });
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  assert.equal(await ctl.texto(), 'Olá\nmundo');
  assert.match(dbg.chamadas[0][1].expression, /innerText/);
});

test('quadro que falha (contexto destruido pela navegacao do clique) nao derruba o click', async () => {
  const dbg = dubleDbg({
    'Accessibility.getFullAXTree': { nodes: [
      { nodeId: '1', role: { value: 'link' }, name: { value: 'Ir' }, childIds: [], backendDOMNodeId: 5 },
    ] },
    'DOM.getBoxModel': { model: { content: [10, 20, 30, 20, 30, 40, 10, 40] } },
  });
  const base = dbg.sendCommand;
  dbg.sendCommand = async (m, p) => { if (m === 'Runtime.evaluate') throw new Error('Cannot find context with specified id'); return base(m, p); };
  const ctl = criarControlador({ dbg, capturarPagina: async () => Buffer.alloc(0), aoNavegar: () => {} });
  await ctl.snapshot();
  assert.equal(await ctl.clicar('@e1'), 'ok: click @e1');
});
