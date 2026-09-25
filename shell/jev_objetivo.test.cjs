const assert = require('node:assert/strict');
const { test } = require('node:test');
const { parsarSnapshot, montarPerguntas, montarEstado, decidir, rodar, pedidoDeTexto, valorDoCampo, cabecaDoValor, perguntaDeConfere, chegouNoEstado, destinoDoJev, NENHUM, SEM_SELECT } = require('./jev_objetivo.cjs');

const SNAPSHOT = `- RootWebArea "Cadastro"
  - link "Ver ajuda" [ref=@e1]
  - button "Iniciar cadastro" [ref=@e2]
  - StaticText "Nenhum cadastro em andamento"
  - button [ref=@e3]
  - image "Jefferson"
  - textbox "Nome do paciente" [ref=@e4]
  - textbox "Data de nascimento" [ref=@e5]
  - combobox "Convenio" [ref=@e20]
    - option "Unimed" [ref=@e21]
    - option "Bradesco Saude" [ref=@e22]`;

const CANDIDATOS = parsarSnapshot(SNAPSHOT);
const SO_TEXTO = '- RootWebArea "pronto"\n  - StaticText "Cadastro salvo com sucesso."';

test('parsarSnapshot pega so o que da pra acionar e tem nome', () => {
  assert.deepEqual(CANDIDATOS.map((c) => c.ref), ['e1', 'e2', 'e4', 'e5', 'e20', 'e21', 'e22']);
  assert.equal(parsarSnapshot(SNAPSHOT, 2).length, 2);
});

test('opcao e alvo dos DOIS caminhos: SELECT no <select> nativo, CLICK na listbox ARIA', () => {
  const p = montarPerguntas(CANDIDATOS);
  assert.deepEqual(Object.keys(p.click_target.criteria), ['e1', 'e2', 'e20', 'e21', 'e22', NENHUM]);
  assert.deepEqual(Object.keys(p.select_target.criteria), ['e21', 'e22', NENHUM]);
  assert.ok('SELECT' in p.operacao.criteria);
});

test('sem dropdown na pagina, SELECT nem e oferecida', () => {
  const semOpcao = montarPerguntas(CANDIDATOS.filter((c) => c.papel !== 'option'));
  assert.ok(!('SELECT' in semOpcao.operacao.criteria));
  assert.ok(!('select_target' in semOpcao));
});

test('cada operacao ganha seu head de alvo, so com elementos compativeis', () => {
  const p = montarPerguntas(CANDIDATOS);
  assert.deepEqual(Object.keys(p.click_target.criteria), ['e1', 'e2', 'e20', 'e21', 'e22', NENHUM]);
  assert.deepEqual(Object.keys(p.type_text_target.criteria), ['e4', 'e5', NENHUM]);
  assert.ok('DONE' in p.operacao.criteria && 'BLOCKED' in p.operacao.criteria);
});

test('operacao sem elemento compativel nem e oferecida', () => {
  const soBotoes = montarPerguntas(CANDIDATOS.filter((c) => c.papel === 'button'));
  assert.ok(!('TYPE_TEXT' in soBotoes.operacao.criteria));
  assert.ok(!('type_text_target' in soBotoes));
});

const resposta = ({ op = 'CLICK', confOp = 0.9, click = 'e2', confClick = 0.9, campo = 'e4', confCampo = 0.9, opcao = 'e22', confOpcao = 0.9, arriscado = 0.02, concluido = 0.9 } = {}) => ({
  operacao: { choice: op, confidence: confOp },
  concluido: { noul: concluido },
  click_target: { choice: click, confidence: confClick },
  type_text_target: { choice: campo, confidence: confCampo },
  select_target: { choice: opcao, confidence: confOpcao },
  arriscado: { noul: arriscado },
});
const decide = (r) => decidir(r, CANDIDATOS);

test('usa o head que casa com a operacao e ignora o outro', () => {
  const clique = decide(resposta({ op: 'CLICK', click: 'e2', campo: 'e4' }));
  assert.equal(clique.alvo, 'e2');
  const digita = decide(resposta({ op: 'TYPE_TEXT', click: 'e2', campo: 'e4' }));
  assert.equal(digita.alvo, 'e4');
});

test('um head especulativo sem alvo nao atrapalha a operacao escolhida', () => {
  const clique = decide(resposta({ op: 'CLICK', campo: NENHUM, confCampo: 0.1 }));
  assert.equal(clique.alvo, 'e2', 'o type_text_target vazio nao e da operacao CLICK');
});

test('DONE e BLOCKED param o laco', () => {
  assert.match(decide(resposta({ op: 'DONE' })).parar, /objetivo atingido/);
  assert.match(decide(resposta({ op: 'BLOCKED' })).parar, /nao da/);
});

test('DONE com a pagina NAO confirmando nao encerra, mesmo com o choice forte', () => {
  const d = decide(resposta({ op: 'DONE', confOp: 0.95, concluido: 0.2 }));
  assert.ok(!/objetivo atingido/.test(d.parar ?? ''), 'nao pode declarar sucesso sem a pagina confirmar');
});

test('SELECT define o valor do dropdown e dispara change, sem clicar na opcao', async () => {
  const executados = [];
  let volta = 0;
  const r = await rodar({
    objetivo: 'escolher o convenio Bradesco Saude',
    maxPassos: 3,
    executar: navegador(executados),
    perguntar: async () => (volta++ === 0 ? resposta({ op: 'SELECT' }) : resposta({ op: 'DONE' })),
  });
  assert.deepEqual(r.feito, ['SELECT "Bradesco Saude"']);
  const js = executados.find((l) => l.startsWith('eval ') && l.includes('dispatchEvent'));
  assert.match(js, /Bradesco Saude/);
  assert.ok(!executados.some((l) => l === 'click @e22'), 'nao clica na option');
});

test('SELECT que nao acha o dropdown para em vez de seguir', async () => {
  const r = await rodar({
    objetivo: 'escolher o convenio',
    maxPassos: 3,
    executar: async (verbo) => {
      if (verbo === 'snapshot') return SNAPSHOT;
      if (verbo === 'url') return 'http://local/x';
      if (verbo === 'text') return 'pagina';
      if (verbo === 'eval') return 'erro: nenhum select tem a opcao Bradesco Saude';
      return 'ok';
    },
    perguntar: async () => resposta({ op: 'SELECT' }),
  });
  assert.match(r.parou, /SELECT falhou/);
});

test('SCROLL_DOWN nao precisa de alvo', () => {
  assert.deepEqual(decide(resposta({ op: 'SCROLL_DOWN' })), { operacao: 'SCROLL_DOWN' });
});

test('nunca age sozinho numa operacao arriscada', () => {
  assert.match(decide(resposta({ arriscado: 0.6 })).parar, /arriscada/);
});

test('para quando a decisao nao esta confiante', () => {
  assert.match(decide(resposta({ confOp: 0.2 })).parar, /operacao indecisa/);
  assert.match(decide(resposta({ confClick: 0.4 })).parar, /sem alvo confiavel/);
  assert.match(decide(resposta({ click: NENHUM })).parar, /sem alvo confiavel/);
  assert.match(decide(resposta({ click: 'e999' })).parar, /nao esta na pagina/);
  assert.match(decidir({}, CANDIDATOS).parar, /sem operacao/);
});

test('dois botoes de rotulo IDENTICO somam a massa e o primeiro vale', () => {
  const gemeos = parsarSnapshot(`- RootWebArea "Hangar"
  - button "Nova sessão" [ref=@e2]
  - button "Nova sessão" [ref=@e21]
  - button "Configurações" [ref=@e6]`);
  const r = {
    operacao: { choice: 'CLICK', confidence: 0.9 },
    concluido: { noul: 0.1 },
    arriscado: { noul: 0.02 },
    click_target: { choice: 'e21', probabilities: { e21: 0.44, e2: 0.32, e6: 0.13 } },
  };
  const passo = decidir(r, gemeos);
  // Nenhum dos dois chega ao piso de 0.6 sozinho, e os dois abrem a mesma folha.
  assert.equal(passo.alvo, 'e2');
  assert.ok(passo.confianca >= 0.6);
});

test('gemeos sem distribuicao caem no caminho de sempre, nao em massa zero', () => {
  const gemeos = parsarSnapshot(`- RootWebArea "Hangar"
  - button "Nova sessão" [ref=@e2]
  - button "Nova sessão" [ref=@e21]`);
  const r = {
    operacao: { choice: 'CLICK', confidence: 0.9 },
    concluido: { noul: 0.1 },
    arriscado: { noul: 0.02 },
    // Só `confidence`: somar um `probabilities` ausente dava 0 e barrava um alvo de 0.9.
    click_target: { choice: 'e21', confidence: 0.9 },
  };
  const passo = decidir(r, gemeos);
  assert.equal(passo.alvo, 'e21');
  assert.equal(passo.confianca, 0.9);
});

test('rotulos DIFERENTES que dividem a massa continuam parando', () => {
  const r = {
    operacao: { choice: 'CLICK', confidence: 0.9 },
    concluido: { noul: 0.1 },
    arriscado: { noul: 0.02 },
    click_target: { choice: 'e1', probabilities: { e1: 0.44, e2: 0.32 } },
  };
  assert.match(decide(r).parar, /sem alvo confiavel/);
});

test('o pedido ao LLM de texto leva campo, objetivo e historico', () => {
  const pedido = pedidoDeTexto('cadastrar o Jefferson', 'Nome do paciente', ['CLICK Iniciar']);
  assert.match(pedido, /Nome do paciente/);
  assert.match(pedido, /cadastrar o Jefferson/);
  assert.match(pedido, /CLICK Iniciar/);
  assert.match(pedido, /APENAS com o valor/);
});

const navegador = (executados, snapshot = SNAPSHOT) => async (verbo, args) => {
  executados.push([verbo, ...args].join(' '));
  if (verbo === 'snapshot') return snapshot;
  if (verbo === 'url') return 'http://local/cadastro';
  if (verbo === 'text') return 'Cadastro de paciente';
  return 'ok';
};

test('o estado leva o valor atual dos campos, lido da pagina', async () => {
  let visto = null;
  await rodar({
    objetivo: 'conferir',
    maxPassos: 1,
    executar: async (verbo) => {
      if (verbo === 'snapshot') return SNAPSHOT;
      if (verbo === 'url') return 'http://local/x';
      if (verbo === 'text') return 'pagina';
      if (verbo === 'eval') return 'ok: "{\\"valores\\":{\\"Nome do paciente\\":\\"Jefferson\\"},\\"obrigatorios_vazios\\":[\\"Convenio*\\"]}"';
      return 'ok';
    },
    perguntar: async (estado) => { visto = estado; return resposta({ op: 'DONE' }); },
  });
  assert.deepEqual(visto.valores_atuais, { 'Nome do paciente': 'Jefferson' });
  assert.deepEqual(visto.obrigatorios_ainda_vazios, ['Convenio*'], 'o que falta preencher e o que impede o submit');
});

test('a regra de nao submeter cedo fica na instrucao, nao no criterio de CLICK', () => {
  const p = montarPerguntas(CANDIDATOS);
  assert.match(p.operacao.instructions, /obrigatorios_ainda_vazios/);
  assert.doesNotMatch(p.operacao.criteria.CLICK, /obrigatorios|fill/, 'criterio de CLICK falando em preencher atrai a escolha errada');
});

test('sonda de valores que falha nao derruba o laco', async () => {
  let visto = null;
  await rodar({
    objetivo: 'conferir',
    maxPassos: 1,
    executar: async (verbo) => {
      if (verbo === 'snapshot') return SNAPSHOT;
      if (verbo === 'url') return 'http://local/x';
      if (verbo === 'text') return 'pagina';
      if (verbo === 'eval') return 'erro: nao deu';
      return 'ok';
    },
    perguntar: async (estado) => { visto = estado; return resposta({ op: 'DONE' }); },
  });
  assert.deepEqual(visto.valores_atuais, {}, 'anotacao e melhoria, nao requisito');
});

test('ref que mudou de elemento entre o snapshot e a decisao nao e usada', async () => {
  const executados = [];
  let volta = 0;
  const OUTRO = SNAPSHOT.replace('button "Iniciar cadastro" [ref=@e2]', 'button "Excluir tudo" [ref=@e2]');
  const r = await rodar({
    objetivo: 'iniciar',
    // Uma volta so: na segunda o Jev ja teria visto a pagina nova e escolher @e2 seria legitimo.
    maxPassos: 1,
    executar: async (verbo, args) => {
      executados.push([verbo, ...args].join(' '));
      // 1o snapshot: pagina original. Do 2o em diante (a reconferencia): @e2 virou outro botao.
      if (verbo === 'snapshot') return volta++ === 0 ? SNAPSHOT : OUTRO;
      if (verbo === 'url') return 'http://local/x';
      if (verbo === 'text') return 'pagina';
      if (verbo === 'eval') return 'ok: "{}"';
      return 'ok';
    },
    perguntar: async () => resposta({ op: 'CLICK', click: 'e2' }),
  });
  assert.ok(!executados.includes('click @e2'), 'nao clica num alvo que trocou de identidade');
  assert.deepEqual(r.feito, []);
});

test('rodar clica e para no DONE, sem LLM entre os passos', async () => {
  const executados = [];
  let volta = 0;
  const r = await rodar({
    objetivo: 'iniciar o cadastro',
    maxPassos: 5,
    executar: navegador(executados),
    perguntar: async () => (volta++ === 0 ? resposta() : resposta({ op: 'DONE' })),
  });
  assert.deepEqual(r.feito, ['CLICK button "Iniciar cadastro"']);
  assert.match(r.parou, /objetivo atingido/);
  assert.ok(executados.includes('click @e2'));
});

test('TYPE_TEXT pede o valor ao LLM pequeno, vendo o campo', async () => {
  const executados = [];
  const pedidos = [];
  let volta = 0;
  await rodar({
    objetivo: 'cadastrar o paciente Jefferson Felizardo',
    maxPassos: 3,
    executar: navegador(executados),
    escreverTexto: async (pedido) => { pedidos.push(pedido); return 'Jefferson Felizardo\n'; },
    perguntar: async () => (volta++ === 0 ? resposta({ op: 'TYPE_TEXT' }) : resposta({ op: 'DONE' })),
  });
  assert.ok(executados.includes('fill @e4 Jefferson Felizardo'), 'digita o valor sem o \\n');
  assert.match(pedidos[0], /Campo a preencher: Nome do paciente/);
});

test('TYPE_TEXT sem dados e sem modelo de texto para em vez de chutar', async () => {
  const r = await rodar({
    objetivo: 'cadastrar',
    executar: navegador([]),
    perguntar: async () => resposta({ op: 'TYPE_TEXT' }),
  });
  assert.match(r.parou, /nenhum dado foi passado.*nao ha modelo de texto/);
});

test('com dados, ha uma pergunta de valor POR campo de texto', () => {
  const p = montarPerguntas(CANDIDATOS, { nome: 'Jefferson', plano: 'Unimed' });
  assert.deepEqual(Object.keys(p[cabecaDoValor('e4')].criteria), ['nome', 'plano', NENHUM]);
  assert.ok(cabecaDoValor('e5') in p, 'cada campo tem a sua, porque a resposta nao pode depender de outra');
  assert.ok(!(cabecaDoValor('e2') in p), 'botao nao ganha pergunta de valor');
  assert.equal(p[cabecaDoValor('e4')].criteria.plano, 'plano = "Unimed"');
});

test('campo de credencial nunca vai ao modelo de texto — senha nao se inventa', async () => {
  let pediu = false;
  const r = await valorDoCampo({
    campo: { nome: 'Senha de acesso' },
    escolha: { choice: NENHUM, probabilities: { [NENHUM]: 0.9 } },
    cofre: { usuario: 'jefferson' },
    objetivo: 'fazer login',
    feito: [],
    escreverTexto: async () => { pediu = true; return 'Abc12345!'; },
  });
  assert.equal(pediu, false, 'o modelo de texto nem pode ser chamado para um campo de senha');
  assert.equal(r.valor, undefined);
  assert.equal(r.falta, 'Senha de acesso');
  assert.match(r.parou, /credencial/);
});

test('senha que veio em --dados continua sendo usada — a recusa e so contra inventar', async () => {
  const r = await valorDoCampo({
    campo: { nome: 'Senha de acesso' },
    escolha: { choice: 'senha', probabilities: { senha: 0.9 } },
    cofre: { senha: 'valor-do-usuario' },
    objetivo: 'fazer login',
    feito: [],
    escreverTexto: async () => 'nao devia chegar aqui',
  });
  assert.equal(r.valor, 'valor-do-usuario');
});

test('valor de chave sensivel nao cruza a rede', () => {
  const p = montarPerguntas(CANDIDATOS, { usuario: 'jeff', senha: 'hunter2' });
  assert.equal(p[cabecaDoValor('e4')].criteria.usuario, 'usuario = "jeff"');
  assert.match(p[cabecaDoValor('e4')].criteria.senha, /nao enviado/);
  assert.ok(!JSON.stringify(p).includes('hunter2'));
});

const campo = { ref: 'e4', papel: 'textbox', nome: 'Nome do paciente' };

test('dados vem antes do LLM', async () => {
  const r = await valorDoCampo({
    campo,
    escolha: { choice: 'nome', confidence: 0.9 },
    cofre: { nome: 'Jefferson Felizardo' },
    escreverTexto: async () => assert.fail('nao devia chamar o LLM tendo o valor em dados'),
  });
  assert.deepEqual(r, { valor: 'Jefferson Felizardo', origem: 'dados.nome' });
});

test('campo que dados nao cobre cai no LLM', async () => {
  for (const escolha of [{ choice: NENHUM, confidence: 0.9 }, { choice: 'nome', confidence: 0.2 }, undefined]) {
    const r = await valorDoCampo({
      campo, escolha, cofre: { nome: 'Jefferson' }, objetivo: 'cadastrar', feito: [],
      escreverTexto: async () => '  do modelo \n',
    });
    assert.deepEqual(r, { valor: 'do modelo', origem: 'llm' });
  }
});

test('sem dados e sem LLM, diz qual campo faltou — em frase e em campo isolado', async () => {
  const r = await valorDoCampo({ campo, escolha: undefined, cofre: {} });
  assert.match(r.parou, /Nome do paciente/);
  assert.equal(r.falta, 'Nome do paciente', 'quem chamou reinvoca com este nome, sem ler a frase');
});

test('o campo que faltou sobe ate o resultado do laco', async () => {
  const r = await rodar({
    objetivo: 'cadastrar',
    executar: navegador([]),
    perguntar: async () => resposta({ op: 'TYPE_TEXT' }),
  });
  assert.equal(r.falta, 'Nome do paciente');
});

test('o LLM que nao sabe o valor para, em vez de escrever prosa no campo', async () => {
  for (const saida of ['SEM_VALOR', '  SEM_VALOR\n', 'SEM_VALOR — nao consta no objetivo', '']) {
    const r = await valorDoCampo({
      campo, escolha: undefined, cofre: {}, objetivo: 'cadastrar', feito: [],
      escreverTexto: async () => saida,
    });
    assert.ok(r.parou, `deveria parar com a saida ${JSON.stringify(saida)}`);
    assert.equal(r.valor, undefined);
  }
});

test('o pedido ao LLM ensina a saida SEM_VALOR', () => {
  assert.match(pedidoDeTexto('cadastrar', 'Data de nascimento', []), /SEM_VALOR/);
});

test('pagina sem nada acionavel ainda pergunta se concluiu', async () => {
  const feito = await rodar({
    objetivo: 'salvar',
    executar: navegador([], SO_TEXTO),
    perguntar: async () => ({ concluido: { noul: 0.96 } }),
  });
  assert.match(feito.parou, /objetivo atingido/);

  const naoFeito = await rodar({
    objetivo: 'salvar',
    executar: navegador([], SO_TEXTO),
    perguntar: async () => ({ concluido: { noul: 0.1 } }),
  });
  assert.match(naoFeito.parou, /nao tem elemento acionavel/);
});

test('erro do navegador vira parada explicita, nao "pagina vazia"', async () => {
  const r = await rodar({
    objetivo: 'qualquer coisa',
    executar: async (verbo) => (verbo === 'snapshot' ? 'erro: a aba escondida nao descongelou' : 'ok'),
    perguntar: async () => assert.fail('nao deveria chegar a perguntar ao Jev'),
  });
  assert.match(r.parou, /o navegador falhou no snapshot/);
});

test('insistir no mesmo alvo para, mesmo com a pagina alternando', async () => {
  let n = 0;
  const r = await rodar({
    objetivo: 'nunca termina',
    maxPassos: 12,
    // Snapshot alternando de tamanho a cada volta, como um dropdown que abre e fecha: a guarda
    // NAO pode olhar o estado da pagina, so o alvo repetido.
    executar: async (verbo) => {
      if (verbo === 'snapshot') return n++ % 2 ? `${SNAPSHOT}\n  - StaticText "x"` : SNAPSHOT;
      if (verbo === 'url') return 'http://local/x';
      if (verbo === 'text') return 'pagina';
      if (verbo === 'eval') return 'ok: "{}"';
      return 'ok';
    },
    perguntar: async () => resposta(),
  });
  assert.ok(r.feito.length <= 3, `agiu ${r.feito.length}x; devia parar em 3`);
  assert.match(r.parou, /repetiu 3x/);
});

test('alvos diferentes a cada passo nao disparam a guarda', async () => {
  const alvos = ['e1', 'e2', 'e1', 'e2'];
  let i = 0;
  const r = await rodar({
    objetivo: 'alterna alvo',
    maxPassos: 4,
    executar: navegador([]),
    perguntar: async () => resposta({ click: alvos[i++ % alvos.length] }),
  });
  assert.equal(r.feito.length, 4);
  assert.match(r.parou, /estourou 4 passos/);
});

// --- combobox editavel (autocomplete): a lista so existe depois de digitar ---

const EDITAVEIS = new Set(['convenio']);

// O navegador de mentira que sabe responder as sondas de `eval` do laco.
const navegadorComSondas = (executados, { aberto = false, editaveis = ['Convenio'], semSelect = false } = {}) => async (verbo, args) => {
  executados.push([verbo, ...args].join(' ').slice(0, 60));
  if (verbo === 'snapshot') return SNAPSHOT;
  if (verbo === 'url') return 'http://local/cadastro';
  if (verbo === 'text') return 'Cadastro de paciente';
  if (verbo === 'eval') {
    const js = args[0];
    if (js.includes('obrigatorios_vazios')) return JSON.stringify({ valores: {}, obrigatorios_vazios: [], editaveis });
    if (js.includes('aria-expanded')) return String(aberto);
    if (js.includes('role=option')) return 'opcoes: 3';
    if (js.includes('querySelectorAll(\'select\')')) return semSelect ? SEM_SELECT : 'valor-definido';
  }
  return 'ok';
};

const digitaNoCombo = {
  operacao: { choice: 'TYPE_TEXT', probabilities: { TYPE_TEXT: 0.9 } },
  type_text_target: { choice: 'e20', probabilities: { e20: 0.9 } },
  [cabecaDoValor('e20')]: { choice: 'convenio', probabilities: { convenio: 0.9 } },
  arriscado: { noul: 0.01 },
};

test('combobox editavel e alvo de TYPE_TEXT e continua sendo de CLICK', () => {
  const p = montarPerguntas(CANDIDATOS, {}, EDITAVEIS);
  assert.ok('e20' in p.type_text_target.criteria, 'o combobox editavel precisa poder ser digitado');
  assert.ok('e20' in p.click_target.criteria, 'e continua clicavel, porque abrir e um passo proprio');
  // Sem a marca de editavel ele e so um botao: digitar nele nao faria sentido.
  assert.ok(!('e20' in montarPerguntas(CANDIDATOS, {}).type_text_target.criteria));
});

test('TYPE_TEXT em combobox editavel abre, digita e espera a lista — nao usa fill', async () => {
  const feitos = [];
  await rodar({
    objetivo: 'escolher o convenio',
    dados: { convenio: 'Unimed' },
    maxPassos: 1,
    executar: navegadorComSondas(feitos),
    perguntar: async () => digitaNoCombo,
  });
  assert.ok(feitos.includes('click @e20'), 'precisa abrir o popover antes de digitar');
  assert.ok(feitos.includes('type Unimed'), 'digita no que ganhou foco, porque a busca nao tem ref');
  assert.ok(!feitos.some((f) => f.startsWith('fill ')), 'fill miraria o botao, nao a busca');
  assert.ok(feitos.some((f) => f.startsWith('eval') && f.includes('Promise')), 'espera a lista aparecer');
});

test('popover ja aberto nao leva clique — o clique o fecharia', async () => {
  const feitos = [];
  await rodar({
    objetivo: 'escolher o convenio',
    dados: { convenio: 'Unimed' },
    maxPassos: 1,
    executar: navegadorComSondas(feitos, { aberto: true }),
    perguntar: async () => digitaNoCombo,
  });
  assert.ok(!feitos.includes('click @e20'));
  assert.ok(feitos.includes('type Unimed'));
});

test('SELECT numa listbox ARIA cai no clique em vez de parar o laco', async () => {
  const feitos = [];
  const r = await rodar({
    objetivo: 'escolher o convenio',
    maxPassos: 1,
    executar: navegadorComSondas(feitos, { semSelect: true }),
    perguntar: async () => ({
      operacao: { choice: 'SELECT', probabilities: { SELECT: 0.9 } },
      select_target: { choice: 'e22', probabilities: { e22: 0.9 } },
      arriscado: { noul: 0.01 },
    }),
  });
  assert.deepEqual(r.feito, ['SELECT "Bradesco Saude"']);
  assert.ok(feitos.includes('click @e22'), 'sem <select> nativo, so o clique escolhe');
});

test('lista aberta encolhe os alvos para ela — nada de digitar noutro campo', () => {
  const p = montarPerguntas(CANDIDATOS, {}, EDITAVEIS, 'Convenio*');
  assert.deepEqual(Object.keys(p.click_target.criteria), ['e21', 'e22', NENHUM], 'so as opcoes da lista');
  assert.deepEqual(Object.keys(p.type_text_target.criteria), ['e20', NENHUM], 'so o campo que esta aberto');
});

test('DONE fraco nao encerra: cai na segunda colocada, que e reversivel', () => {
  const d = decide({
    operacao: { choice: 'DONE', probabilities: { DONE: 0.5, CLICK: 0.45 } },
    click_target: { choice: 'e2', probabilities: { e2: 0.9 } },
  });
  assert.equal(d.operacao, 'CLICK');
  assert.equal(d.alvo, 'e2');
});

test('sem confirmacao e sem alternativa aceitavel, para dizendo as duas coisas', () => {
  const d = decidir({
    operacao: { choice: 'DONE', probabilities: { DONE: 0.5, CLICK: 0.2 } },
    concluido: { noul: 0.3 },
  }, CANDIDATOS);
  assert.match(d.parar, /pagina nao confirma/);
});

test('DONE com a pagina confirmando encerra', () => {
  const d = decide({
    operacao: { choice: 'DONE', probabilities: { DONE: 0.8 } },
    concluido: { noul: 0.88 },
  });
  assert.match(d.parar, /objetivo atingido, pagina confirma/);
});

// O CLI sai com `process.exit(resultado.sucesso ? 0 : 1)`. Sem este campo, um script encadeando
// `hangar-preview objetivo` seguia em frente achando que tinha dado certo.
test('so a conclusao confirmada devolve sucesso; toda parada devolve falha', async () => {
  const pronto = await rodar({
    objetivo: 'salvar',
    executar: navegador([], SO_TEXTO),
    perguntar: async () => ({ concluido: { noul: 0.96 } }),
  });
  assert.equal(pronto.sucesso, true);

  const semConfirmar = await rodar({
    objetivo: 'salvar',
    executar: navegador([], SO_TEXTO),
    perguntar: async () => ({ concluido: { noul: 0.1 } }),
  });
  assert.equal(semConfirmar.sucesso, false);

  const alvos = ['e1', 'e2', 'e1', 'e2'];
  let i = 0;
  const estourou = await rodar({
    objetivo: 'alterna alvo',
    maxPassos: 2,
    executar: navegador([]),
    perguntar: async () => resposta({ click: alvos[i++ % alvos.length] }),
  });
  assert.equal(estourou.sucesso, false, 'teto de passos e falha, nao sucesso');

  const navegadorQuebrado = await rodar({
    objetivo: 'salvar',
    executar: async (verbo) => (verbo === 'snapshot' ? 'erro: a aba nao descongelou' : 'ok'),
    perguntar: async () => resposta(),
  });
  assert.equal(navegadorQuebrado.sucesso, false);
});

test('o sucesso nasce no decidir, nao da frase em portugues', () => {
  assert.equal(decide(resposta({ op: 'DONE' })).sucesso, true);
  assert.notEqual(decide(resposta({ op: 'BLOCKED' })).sucesso, true);
});

test('o texto da pagina guarda as DUAS pontas — a prova pode estar no fim', () => {
  const longo = `${'a'.repeat(5000)}FOI CRIADO`;
  const e = montarEstado('salvar', 'http://local', [], longo);
  assert.ok(e.texto_da_pagina.includes('FOI CRIADO'), 'o fim da pagina e onde vive o toast de sucesso');
  assert.ok(e.texto_da_pagina.startsWith('aaa'), 'e o comeco tambem entra');
  assert.ok(e.texto_da_pagina.length < 4200, 'mas com teto: pagina inteira no estado e context rot');
});

test('texto curto entra inteiro, sem marca de corte', () => {
  const e = montarEstado('salvar', 'http://local', [], 'Servico criado com sucesso.');
  assert.equal(e.texto_da_pagina, 'Servico criado com sucesso.');
});

test('o estado nao repete os rotulos que ja estao nos criterios dos heads', () => {
  const e = montarEstado('objetivo', 'http://local', [], 'texto');
  assert.ok(!('elementos' in e));
  assert.equal(e.objetivo, 'objetivo');
});

test('confere: noul no piso da conclusao passa; abaixo, torto ou ausente nao', () => {
  const q = perguntaDeConfere('a lista mostra o item novo');
  assert.equal(q.chegou.type, 'noul');
  assert.match(q.chegou.instructions, /a lista mostra o item novo/);
  assert.deepEqual(chegouNoEstado({ chegou: { noul: 0.8 } }), { p: 0.8, ok: true });
  assert.deepEqual(chegouNoEstado({ chegou: { noul: 0.4 } }), { p: 0.4, ok: false });
  assert.deepEqual(chegouNoEstado({ chegou: { noul: 'x' } }), { p: 0, ok: false });
  assert.deepEqual(chegouNoEstado(undefined), { p: 0, ok: false });
});

test('destino do Jev: endpoint e modelo do ambiente vencem o padrao', () => {
  assert.deepEqual(destinoDoJev({}, 'jev-latest'),
    { url: 'https://api.typesafe.ai/v1/systemone', modelo: 'jev-latest' });
  assert.deepEqual(destinoDoJev({ JEV_ENDPOINT: 'https://openrouter.ai/api/alpha/decisions', JEV_MODEL: 'typesafe/jev-1.13-20260917' }, 'jev-latest'),
    { url: 'https://openrouter.ai/api/alpha/decisions', modelo: 'typesafe/jev-1.13-20260917' });
  assert.deepEqual(destinoDoJev({ JEV_ENDPOINT: '  ', JEV_MODEL: '' }, 'jev-1.13.0'),
    { url: 'https://api.typesafe.ai/v1/systemone', modelo: 'jev-1.13.0' });
});

const http = require('node:http');
const { askJev } = require('./jev_objetivo.cjs');

async function jevServer(respond) {
  const srv = http.createServer(respond);
  await new Promise((ok) => srv.listen(0, '127.0.0.1', ok));
  return { srv, url: `http://127.0.0.1:${srv.address().port}/v1/systemone` };
}

test('askJev: endpoint pendurado vira erro no prazo, sem prender o comando', async () => {
  const { srv, url } = await jevServer(() => {});
  try {
    await assert.rejects(
      askJev({ url, modelo: 'm', chave: 'k', estado: 's', perguntas: {}, deadlineMs: 100 }),
      { message: 'o Jev não respondeu em 0.1 s' },
    );
  } finally {
    srv.closeAllConnections();
    srv.close();
  }
});

test('askJev: devolve answers e manda modelo, estado e chave', async () => {
  let pedido;
  const { srv, url } = await jevServer((req, res) => {
    let corpo = '';
    req.on('data', (c) => { corpo += c; });
    req.on('end', () => {
      pedido = { corpo: JSON.parse(corpo), auth: req.headers.authorization };
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify({ answers: { ok: { noul: 0.9 } } }));
    });
  });
  try {
    const r = await askJev({ url, modelo: 'jev-x', chave: 'k', estado: 'tela', perguntas: { ok: { type: 'noul' } } });
    assert.deepEqual(r, { ok: { noul: 0.9 } });
    assert.deepEqual(pedido, { corpo: { model: 'jev-x', state: 'tela', questions: { ok: { type: 'noul' } } }, auth: 'Bearer k' });
  } finally {
    srv.close();
  }
});

test('askJev: status de erro vira "o Jev recusou"', async () => {
  const { srv, url } = await jevServer((req, res) => { res.statusCode = 529; res.end('lotado'); });
  try {
    await assert.rejects(
      askJev({ url, modelo: 'm', chave: 'k', estado: 's', perguntas: {} }),
      { message: 'o Jev recusou: 529 lotado' },
    );
  } finally {
    srv.close();
  }
});
