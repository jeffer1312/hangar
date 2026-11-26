// scripts/hangar-preview é ESM e fala com o servidor local por HTTP — testável sem Electron,
// como subprocesso de verdade, contra um `subirServidor` real com controlador falso.
//
// spawnSync (síncrono) NÃO SERVE aqui: ele bloqueia o event loop do processo QUE ESTÁ RODANDO O
// TESTE, e é esse mesmo processo que hospeda o servidor HTTP real (`subirServidor`) que o CLI
// filho precisa acessar — child esperando resposta, pai bloqueado esperando o child terminar,
// nunca sai (medido: trava os 30s do teste). `spawn` assíncrono mantém o event loop do pai vivo
// pra atender a requisição do filho.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { spawn } = require('node:child_process');
const { subirServidor } = require('./preview_srv.cjs');
const { nomeSidecar } = require('./navegador.cjs');

const CLI = path.join(__dirname, '..', 'scripts', 'hangar-preview');
const CHAVE = 'srv::sessaoteste';

function homeComSidecar(dadosSrv) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-preview-cli-'));
  const dir = path.join(home, '.hangar', 'nav');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, '_srv.json'), JSON.stringify(dadosSrv));
  fs.writeFileSync(path.join(dir, `${nomeSidecar(CHAVE)}.json`),
    JSON.stringify({ chave: CHAVE, url: 'http://x.test', targetId: null, ts: Date.now() }));
  return home;
}

function rodarCli(args, { entrada = null, home, env = {}, cwd } = {}) {
  return new Promise((resolvePromise, reject) => {
    const filho = spawn(process.execPath, [CLI, ...args], { cwd, env: { ...process.env, HOME: home, ...env } });
    let stdout = '';
    let stderr = '';
    filho.stdout.on('data', (d) => { stdout += d; });
    filho.stderr.on('data', (d) => { stderr += d; });
    filho.on('error', reject);
    filho.on('close', (status) => resolvePromise({ status, stdout, stderr }));
    if (entrada != null) filho.stdin.end(entrada); else filho.stdin.end();
  });
}

test('batch para na primeira linha que falha, sai com codigo 1 e nao roda o resto', async () => {
  const chamadas = [];
  const ctlFalso = {
    enfileirar: (fn) => fn(),
    avaliar: async (js) => {
      chamadas.push(js);
      return js === 'FAIL' ? 'erro: falhou de proposito' : `ok: ${js}`;
    },
  };
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: () => {} });
  const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });

  const r = await rodarCli(['batch', '--sessao', 'sessaoteste'], { entrada: 'eval OK1\neval FAIL\neval OK2\n', home });
  srv.fechar();

  assert.equal(r.status, 1);
  assert.match(r.stdout, /ok: OK1/);
  assert.match(r.stdout, /erro: falhou de proposito/);
  assert.doesNotMatch(r.stdout, /OK2/);
  assert.deepEqual(chamadas, ['OK1', 'FAIL'], 'a terceira linha nunca roda');
  assert.match(r.stderr, /linha 2/, 'informa qual linha parou');
});

test('--aba vai no corpo do pedido e tab vira verbo tab-*', async () => {
  const pedidos = [];
  const srv = await subirServidor({
    controladorDe: (_c, aba) => ({ enfileirar: (fn) => fn(), avaliar: async () => { pedidos.push({ aba }); return 'http://x.test'; } }),
    escrever: () => {},
    abasDe: () => ({ ativa: 1, abas: [{ id: 1, url: 'http://x.test', titulo: 'X' }] }),
    abaTrocar: async (_c, id) => { pedidos.push({ trocou: id }); return { ok: true }; },
  });
  const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });

  await rodarCli(['url', '--aba', '3', '--sessao', 'sessaoteste'], { home });
  const t = await rodarCli(['tab', '2', '--sessao', 'sessaoteste'], { home });
  srv.fechar();

  assert.deepEqual(pedidos[0], { aba: 3 }, '--aba chega no servidor');
  assert.deepEqual(pedidos[1], { trocou: 2 }, 'tab <id> vira tab-switch');
  assert.equal(t.status, 0);
});

test('layout encaminha largura e altura juntas ao shell', async () => {
  const chamadas = [];
  const ctlFalso = {
    enfileirar: (fn) => fn(),
    layout: async (...args) => { chamadas.push(args); return `layout: ${args.join('x')}`; },
    layoutAtual: () => 'desktop',
  };
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: () => {} });
  const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });

  const r = await rodarCli(['layout', '1366', '768', '--sessao', 'sessaoteste'], { home });
  srv.fechar();

  assert.equal(r.status, 0);
  assert.equal(r.stdout.trim(), 'layout: 1366x768');
  assert.deepEqual(chamadas, [['1366', '768']]);
});

// O `list` confere no CDP da 9223 quem está vivo, então o teste precisa de ALGUÉM respondendo ali.
// Sobe um falso; com a porta ocupada (app do usuário aberto) usa quem já responde, e só devolve
// null — para o teste pular — quando ninguém responde.
async function cdpParaOList() {
  const falso = http.createServer((_req, res) => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end('[]');
  });
  const subiu = await new Promise((r) => {
    falso.once('error', () => r(false));
    falso.listen(9223, '127.0.0.1', () => r(true));
  });
  if (subiu) return () => falso.close();
  try { await (await fetch('http://127.0.0.1:9223/json/list')).json(); return () => {}; } catch { return null; }
}

test('list mostra a aba ativa e a contagem, e le sidecar antigo como uma aba', async (t) => {
  const parar = await cdpParaOList();
  if (!parar) return t.skip('a 9223 esta ocupada por quem nao responde /json/list: o `list` erraria por falta de CDP, nao pelo formato');
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-preview-list-'));
  const dir = path.join(home, '.hangar', 'nav');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, `${nomeSidecar('srv::nova')}.json`), JSON.stringify({
    chave: 'srv::nova', url: 'http://dois.test', targetId: null, ativa: 2,
    abas: [{ id: 1, url: 'http://um.test', titulo: 'Um', targetId: null },
      { id: 2, url: 'http://dois.test', titulo: 'Dois', targetId: null }],
  }));
  fs.writeFileSync(path.join(dir, `${nomeSidecar('srv::velha')}.json`), JSON.stringify({
    chave: 'srv::velha', url: 'http://tres.test', targetId: null,
  }));

  const r = await rodarCli(['list'], { home });
  parar();
  assert.match(r.stdout, /srv::nova.*\(aba 2 de 2\)/);
  assert.doesNotMatch(r.stdout.split('\n').find((l) => l.includes('srv::velha')) || '', /aba/,
    'sidecar sem abas (shell antigo) sai como hoje');
});

test('sidecar com pid morto e recusado com a mesma mensagem do sidecar ausente', async () => {
  const pidMorto = 999999; // improvável de existir; se existir por acaso, o teste é inconclusivo
  const home = homeComSidecar({ porta: 1, token: 'x', pid: pidMorto });

  const r = await rodarCli(['url', '--sessao', 'sessaoteste'], { home });

  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /nao esta rodando nesta maquina/);
});

// Só o cabeçalho que o CLI lê (assinatura + IHDR com largura e altura).
function png(width, height) {
  const b = Buffer.alloc(33);
  Buffer.from('89504e470d0a1a0a', 'hex').copy(b, 0);
  b.writeUInt32BE(13, 8);
  b.write('IHDR', 12, 'latin1');
  b.writeUInt32BE(width, 16);
  b.writeUInt32BE(height, 20);
  return b;
}

const ctlWithScreenshot = (shots = []) => ({
  enfileirar: (fn) => fn(),
  snapshot: async () => '- RootWebArea "Lista"\n  - button "Novo" [ref=@e1]',
  capturarPagina: async () => { shots.push(Date.now()); return { isEmpty: () => false, toPNG: () => png(1280, 800) }; },
});

// Jev falso do `confere`: `semImagem` responde o pedido sem print; `comImagem` o com print (null = 400).
async function jevDoConfere(semImagem, comImagem) {
  const corpos = [];
  const server = http.createServer((req, res) => {
    let bruto = '';
    req.on('data', (d) => { bruto += d; });
    req.on('end', () => {
      const corpo = JSON.parse(bruto);
      corpos.push(corpo);
      const p = corpo.state?.tela ? comImagem : semImagem;
      if (p === null) { res.writeHead(400); res.end('imagem recusada'); return; }
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ answers: { chegou: { noul: p } } }));
    });
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const env = { JEV_ENDPOINT: `http://127.0.0.1:${server.address().port}/v1/systemone`, TYPESAFE_API_KEY: 'chave-teste', JEV_MODEL: '' };
  return { corpos, env, fechar: () => server.close() };
}

for (const [nome, semImagem, comImagem, saida, codigo, comPrint] of [
  ['fora da faixa de duvida: responde sem capturar', 0.9, 0.1, 'confere: 0.90', 0, false],
  ['abaixo da faixa de duvida: responde sem capturar', 0.1, 0.9, 'confere: 0.10', 1, false],
  ['na faixa de duvida: refaz com o print e decide pela resposta com imagem', 0.5, 0.7, 'confere: 0.70', 0, true],
]) {
  test(`confere ${nome}`, async () => {
    const jevFalso = await jevDoConfere(semImagem, comImagem);
    const shots = [];
    const srv = await subirServidor({ controladorDe: () => ctlWithScreenshot(shots), escrever: () => {} });
    const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });

    const r = await rodarCli(['confere', 'a lista mostra o botao Novo', '--sessao', 'sessaoteste'], { home, env: jevFalso.env });
    srv.fechar();
    jevFalso.fechar();

    assert.equal(r.status, codigo, r.stderr);
    assert.equal(r.stdout.trim(), saida);
    assert.equal(shots.length, comPrint ? 1 : 0);
    assert.equal(jevFalso.corpos.length, comPrint ? 2 : 1);
    assert.equal(typeof jevFalso.corpos[0].state, 'string', 'a primeira pergunta e a de antes, sem print');
    if (comPrint) assert.ok(jevFalso.corpos[1].state.tela);
    assert.doesNotMatch(r.stderr, /sem o print/);
  });
}

test('confere na faixa com o print recusado: responde pela pergunta sem imagem e avisa no stderr', async () => {
  const { corpos, env: jevEnv, fechar } = await jevDoConfere(0.5, null);
  const jevFalso = { close: fechar };
  const shots = [];
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-preview-tmp-'));
  const naCaptura = [];
  const base = ctlWithScreenshot(shots);
  const ctl = {
    ...base,
    capturarPagina: async () => {
      naCaptura.push(...fs.readdirSync(tmp).map((n) => ({ n, modo: fs.statSync(path.join(tmp, n)).mode & 0o777 })));
      return base.capturarPagina();
    },
  };
  const srv = await subirServidor({ controladorDe: () => ctl, escrever: () => {} });
  const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });

  const r = await rodarCli(['confere', 'a lista mostra o botao Novo', '--sessao', 'sessaoteste'], {
    home,
    env: { ...jevEnv, TMPDIR: tmp, TEMP: tmp, TMP: tmp },
  });
  srv.fechar();
  jevFalso.close();

  // O print vive numa pasta só do dono, que some com o processo.
  assert.equal(naCaptura.length, 1);
  assert.match(naCaptura[0].n, /^hangar-jev-/);
  if (process.platform !== 'win32') assert.equal(naCaptura[0].modo, 0o700);
  assert.deepEqual(fs.readdirSync(tmp), []);

  assert.equal(r.status, 1, r.stderr);
  assert.equal(r.stdout.trim(), 'confere: 0.50');
  assert.equal(shots.length, 1);
  assert.equal(corpos.length, 2, 'a pergunta sem imagem ja respondida nao se repete');
  assert.equal(typeof corpos[0].state, 'string', 'sem imagem, o corpo e o de antes');
  assert.match(corpos[1].state.pagina, /button "Novo"/);
  assert.equal(corpos[1].assets[corpos[1].state.tela].format, 'png');
  assert.match(r.stderr, /confere sem o print/);
  assert.match(r.stderr, /400/);
});

test('objetivo com o print recusado: o passo 1 fraco registra a falha, o passo 2 fraco ja sai sem imagem', async () => {
  const corpos = [];
  let naListaSemPrint = 0;
  const jevFalso = http.createServer((req, res) => {
    let bruto = '';
    req.on('data', (d) => { bruto += d; });
    req.on('end', () => {
      const corpo = JSON.parse(bruto);
      corpos.push(corpo);
      if (corpo.state?.tela) { res.writeHead(400); res.end('imagem recusada'); return; }
      const naLista = /Lista de pacientes/.test(corpo.state?.texto_da_pagina ?? '');
      // Na lista, a primeira resposta é indecisa (leva ao print) e a refeita sem print é firme.
      const firme = naLista && (naListaSemPrint += 1) > 1;
      const answers = naLista
        ? { operacao: { choice: 'CLICK', confidence: firme ? 0.95 : 0.2 }, click_target: { choice: 'e1', confidence: 0.95 },
          concluido: { noul: 0.05 }, arriscado: { noul: 0.01 }, click_risk: { choice: '_nenhum', probabilities: { _nenhum: 0.99, e1: 0.01 } } }
        // No cadastro, DONE sem a página confirmar: fraco, e sem print disponível o laço para.
        : { operacao: { choice: 'DONE', confidence: 0.95 }, concluido: { noul: 0.3 }, arriscado: { noul: 0.01 } };
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({ answers }));
    });
  });
  await new Promise((r) => jevFalso.listen(0, '127.0.0.1', r));
  let clicou = false;
  const ctl = {
    ...ctlWithScreenshot(),
    snapshot: async () => (clicou ? '- RootWebArea "Cadastro"\n  - button "Voltar" [ref=@e2]' : '- RootWebArea "Lista"\n  - button "Novo" [ref=@e1]'),
    texto: async () => (clicou ? 'Cadastro concluido' : 'Lista de pacientes'),
    avaliar: async (js) => (js === 'location.href' ? (clicou ? 'http://x.test/cadastro' : 'http://x.test/lista') : 'ok'),
    clicar: async () => { clicou = true; return 'ok'; },
    esperar: async () => 'ok',
  };
  const srv = await subirServidor({ controladorDe: () => ctl, escrever: () => {} });
  const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });
  const pasta = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-preview-roteiro-'));

  const r = await rodarCli(['objetivo', 'abrir o cadastro novo', '--pasta', pasta, '--sessao', 'sessaoteste'], {
    home,
    env: { JEV_ENDPOINT: `http://127.0.0.1:${jevFalso.address().port}/v1/systemone`, TYPESAFE_API_KEY: 'chave-teste', JEV_MODEL: '',
      JEV_TEXTO_BASE_URL: '', JEV_TEXTO_API_KEY: '', JEV_TEXTO_MODELO: '' },
  });
  srv.fechar();
  jevFalso.close();

  assert.equal(r.status, 1, r.stderr + r.stdout);
  const arquivo = r.stdout.match(/^trajeto: (.+)$/m)?.[1];
  assert.ok(arquivo, r.stdout);
  const bruto = fs.readFileSync(arquivo, 'utf8');
  assert.ok(!bruto.includes(png(1280, 800).toString('base64')), 'o trajeto nunca leva o PNG');
  const decisoes = bruto.trim().split('\n').map((l) => JSON.parse(l)).filter((e) => e.tipo === 'decisao');
  assert.equal(decisoes.length, 2);
  assert.equal(decisoes[0].imagem, 'falhou');
  assert.match(decisoes[0].motivo, /400/);
  assert.equal(decisoes[0].operacao, 'CLICK');
  assert.equal(decisoes[1].imagem, 'desligada');
  assert.match(decisoes[1].parar, /^a pagina nao confirma que acabou/);
  assert.equal(corpos.length, 4, 'passo 1 sem print, com print e refeito sem; passo 2 so sem');
  assert.equal(corpos[0].state.tela, undefined);
  assert.equal(corpos[0].questions.bloqueado, undefined);
  assert.ok(corpos[1].state.tela);
  assert.ok(corpos[1].questions.bloqueado, 'a pergunta com imagem leva o bloqueio');
  assert.equal(corpos[2].state.tela, undefined);
  assert.equal(corpos[2].questions.bloqueado, undefined);
  assert.equal(corpos[3].state.tela, undefined);
  assert.equal(corpos[3].questions.bloqueado, undefined);
  assert.ok(corpos[3].questions.operacao, 'o passo 2 e o pedido normal do laco');
});

// Sem app nesta máquina (sem `_srv.json`), o servidor daqui repassa cada verbo ao app de outra máquina. O falso grava o
// print do `shot` no caminho absoluto pedido, como o servidor de verdade faz na máquina da sessão.
async function remoteServer() {
  const verbs = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (d) => { raw += d; });
    req.on('end', () => {
      const { verbo: verb, args = [] } = JSON.parse(raw || '{}');
      verbs.push(verb);
      const text = (status, body) => { res.writeHead(status, { 'Content-Type': 'text/plain; charset=utf-8' }); res.end(body); };
      if (verb === 'host') {
        res.writeHead(409, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ detail: { code: 'browser_host_none', msg: 'x', params: {} } }));
      } else if (verb === 'snapshot') {
        text(200, '- RootWebArea "Lista"\n  - button "Novo" [ref=@e1]');
      } else if (verb === 'shot') {
        fs.writeFileSync(args[0], png(1280, 800));
        text(200, `ok: shot ${args[0]}`);
      } else {
        text(200, 'erro: verbo inesperado');
      }
    });
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  return { verbs, port: server.address().port, close: () => server.close() };
}

test('confere com a sessao no app de outra maquina le o print que o servidor gravou', async () => {
  const fakeJev = await jevDoConfere(0.5, 0.7);
  const srv = await remoteServer();
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-preview-remoto-'));
  fs.mkdirSync(path.join(home, '.hangar', 'config'), { recursive: true });
  fs.writeFileSync(path.join(home, '.hangar', 'config', '.env'), 'CP_AUTH_TOKEN=teste\n');

  const r = await rodarCli(['confere', 'a lista mostra o botao Novo', '--sessao', 'sessaoteste'], {
    home,
    env: { ...fakeJev.env, PORT: String(srv.port), USERPROFILE: home },
  });
  srv.close();
  fakeJev.fechar();

  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.stdout.trim(), 'confere: 0.70');
  assert.match(r.stderr, /usando o navegador do app desktop de outra máquina/);
  assert.deepEqual(srv.verbs, ['host', 'snapshot', 'shot']);
  assert.equal(fakeJev.corpos.length, 2);
  assert.ok(fakeJev.corpos[1].state.tela, 'o print gravado pelo servidor vai ao Jev');
});

test('shot com --max e sem caminho grava no caminho padrao, nunca num arquivo chamado --max', async () => {
  const srv = await subirServidor({ controladorDe: () => ctlWithScreenshot(), escrever: () => {} });
  const home = homeComSidecar({ porta: srv.porta, token: srv.token, pid: process.pid });

  const r = await rodarCli(['shot', '--max', '1568', '--sessao', 'sessaoteste'], { home, cwd: home });
  srv.fechar();

  assert.equal(r.status, 0, r.stderr);
  const gravado = r.stdout.trim().replace(/^ok: shot /, '');
  assert.equal(path.dirname(gravado), path.join(home, '.hangar', 'nav', 'shots', 'sessaoteste'));
  assert.ok(fs.existsSync(gravado));
  assert.equal(fs.existsSync(path.join(home, '--max')), false);
});
