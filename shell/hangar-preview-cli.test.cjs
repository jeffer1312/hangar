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

function rodarCli(args, { entrada = null, home } = {}) {
  return new Promise((resolvePromise, reject) => {
    const filho = spawn(process.execPath, [CLI, ...args], { env: { ...process.env, HOME: home } });
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

test('sidecar com pid morto e recusado com a mesma mensagem do sidecar ausente', async () => {
  const pidMorto = 999999; // improvável de existir; se existir por acaso, o teste é inconclusivo
  const home = homeComSidecar({ porta: 1, token: 'x', pid: pidMorto });

  const r = await rodarCli(['url', '--sessao', 'sessaoteste'], { home });

  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /nao esta no ar/);
});
