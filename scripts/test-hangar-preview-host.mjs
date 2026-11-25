#!/usr/bin/env node
// O app desktop daqui caiu (o `_srv.json` ficou com um pid sem processo): com outro app ligado mostrando a sessão, mesmo
// sem clique (`host` responde 0), o CLI usa o de lá; sem ninguém ligado lá, a recusa segue dizendo que o app daqui reinicia.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import http from 'node:http';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = dirname(dirname(fileURLToPath(import.meta.url)));
const script = join(repo, 'scripts', 'hangar-preview');

// O servidor desta máquina: `answer(verbo)` devolve [status, corpo]; anota os verbos pedidos.
async function backend(answer) {
  const verbs = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (d) => { raw += d; });
    req.on('end', () => {
      const body = JSON.parse(raw || '{}');
      verbs.push(body.verbo);
      const [status, text] = answer(body.verbo);
      res.writeHead(status, { 'Content-Type': status === 200 ? 'text/plain; charset=utf-8' : 'application/json' });
      res.end(text);
    });
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  return { port: server.address().port, verbs, close: () => server.close() };
}

async function home(record = { pid: 999999999, porta: 1, token: 'x' }) {
  const dir = await mkdtemp(join(tmpdir(), 'hangar-preview-host-'));
  const nav = join(dir, '.hangar', 'nav');
  await Promise.all([mkdir(nav, { recursive: true }), mkdir(join(dir, '.hangar', 'config'), { recursive: true })]);
  await Promise.all([
    writeFile(join(dir, '.hangar', 'config', '.env'), 'CP_AUTH_TOKEN=teste\n'),
    writeFile(join(nav, '_srv.json'), JSON.stringify(record)),
    // O caído deixou o sidecar e o foco dele, mais novos que qualquer hora de lá.
    writeFile(join(nav, 'srv-sessaoteste.json'), JSON.stringify({ chave: 'srv::sessaoteste', url: 'http://localhost:3000/' })),
    writeFile(join(nav, '_focus.json'), JSON.stringify({ sessaoteste: Date.now() })),
  ]);
  return dir;
}

function run(dir, port) {
  const env = { ...process.env, HOME: dir, USERPROFILE: dir, PORT: String(port) };
  for (const k of ['CP_SESSION_KEY', 'HANGAR_CANO_KEY', 'TMUX', 'TMUX_PANE', 'CP_SESSION_NAME', 'HANGAR_PREVIEW_CDP']) delete env[k];
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [script, 'url', '--sessao', 'sessaoteste'], { env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => { stdout += d; });
    child.stderr.on('data', (d) => { stderr += d; });
    child.on('close', (code) => resolve({ code, stdout, stderr }));
  });
}

const none = JSON.stringify({ detail: { code: 'browser_host_none', msg: 'nenhum app', params: {} } });
const dirs = [];
try {
  {
    const dir = await home();
    dirs.push(dir);
    const srv = await backend((verb) => (verb === 'host' ? [200, 'ok: host 0'] : [200, 'ok: "http://localhost:3000/"']));
    const r = await run(dir, srv.port);
    srv.close();
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stdout, /http:\/\/localhost:3000\//);
    assert.match(r.stderr, /usando o navegador do app desktop de outra máquina/);
    assert.deepEqual(srv.verbs, ['host', 'url']);
    console.log('ok: app daqui caído e outro com a sessão em foco: o verbo vai ao de lá');
  }
  {
    const dir = await home();
    dirs.push(dir);
    const srv = await backend(() => [409, none]);
    const r = await run(dir, srv.port);
    srv.close();
    assert.equal(r.code, 1);
    assert.match(r.stderr, /reiniciando/);
    assert.deepEqual(srv.verbs, ['host'], 'sem ninguém lá, nada mais vai ao servidor');
    console.log('ok: app daqui caído e ninguém lá: a recusa diz que o app daqui reinicia');
  }
  {
    // O app de lá usou a sessão antes do foco daqui e desligou: a hora dele não substitui o app daqui caído.
    const dir = await home();
    dirs.push(dir);
    const srv = await backend((verb) => (verb === 'host' ? [200, 'ok: host 1 detached'] : [409, none]));
    const r = await run(dir, srv.port);
    srv.close();
    assert.equal(r.code, 1);
    assert.match(r.stderr, /reiniciando/);
    assert.deepEqual(srv.verbs, ['host'], 'nada vai ao app que desligou');
    console.log('ok: app daqui caído e o de lá desligado: a recusa diz que o app daqui reinicia');
  }
  {
    // Mesmo com o uso de lá mais novo que o foco daqui, app desligado não recebe o verbo quando o daqui caiu.
    const dir = await home();
    dirs.push(dir);
    const srv = await backend((verb) => (verb === 'host' ? [200, `ok: host ${Date.now() + 60000} detached`] : [409, none]));
    const r = await run(dir, srv.port);
    srv.close();
    assert.equal(r.code, 1);
    assert.match(r.stderr, /reiniciando/);
    assert.deepEqual(srv.verbs, ['host'], 'nada vai ao app que desligou, mesmo com uso mais novo');
    console.log('ok: app daqui caído e o de lá desligado com uso mais novo: a recusa diz que o app daqui reinicia');
  }
  {
    // Sem pid o registro não decide, como no MCP: o verbo fica no app daqui, mesmo com outro app com a sessão em foco.
    const dir = await home({ porta: 1, token: 'x' });
    dirs.push(dir);
    const srv = await backend((verb) => (verb === 'host' ? [200, 'ok: host 0'] : [200, 'ok: "http://localhost:3000/"']));
    const r = await run(dir, srv.port);
    srv.close();
    assert.notEqual(r.code, 0);
    assert.doesNotMatch(r.stderr, /outra máquina/);
    assert.doesNotMatch(r.stderr, /reiniciando, como na atualizacao/, 'sem pid não é "app fora"');
    assert.deepEqual(srv.verbs, ['host'], 'nada vai ao app de outra máquina');
    console.log('ok: registro sem pid: o verbo fica no app daqui');
  }
} finally {
  await Promise.all(dirs.map((d) => rm(d, { recursive: true, force: true })));
}
