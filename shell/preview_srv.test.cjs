const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const { subirServidor } = require('./preview_srv.cjs');

const ctlFalso = { enfileirar: (fn) => fn(), snapshot: async () => '- button "Ok" [ref=@e1]' };

test('recusa sem token e atende com token', async () => {
  let sidecar = null;
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: (d) => (sidecar = d) });
  const url = `http://127.0.0.1:${srv.porta}/cmd`;
  const corpo = JSON.stringify({ chave: 'srv::a', verbo: 'snapshot', args: [] });

  const sem = await fetch(url, { method: 'POST', body: corpo });
  assert.equal(sem.status, 401);

  const com = await fetch(url, {
    method: 'POST', body: corpo, headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(com.status, 200);
  assert.equal(await com.text(), '- button "Ok" [ref=@e1]');
  assert.equal(sidecar.porta, srv.porta);
  srv.fechar();
});

test('token de tamanho diferente do certo recusa sem lancar (timingSafeEqual)', async () => {
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: () => {} });
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: JSON.stringify({ chave: 'srv::x', verbo: 'snapshot', args: [] }),
    headers: { Authorization: 'Bearer curto' },
  });
  assert.equal(r.status, 401);
  srv.fechar();
});

test('corpo com caractere multibyte cortado na fronteira do pedaço chega integro', async () => {
  let recebido = null;
  const ctl = { enfileirar: (fn) => fn(), digitar: async (texto) => { recebido = texto; return 'ok: type'; } };
  const srv = await subirServidor({ controladorDe: () => ctl, escrever: () => {} });
  const textoEsperado = 'João da Silva çãé';
  const corpoTexto = JSON.stringify({ chave: 'srv::f', verbo: 'type', args: [textoEsperado] });
  const corpoBuf = Buffer.from(corpoTexto, 'utf8');
  // corta no MEIO de um caractere multibyte (depois do 1º byte de um 0xC3), não em fronteira de char
  const corte = corpoBuf.indexOf(0xc3) + 1;
  const parte1 = corpoBuf.subarray(0, corte);
  const parte2 = corpoBuf.subarray(corte);

  await new Promise((resolve, reject) => {
    const socket = net.connect(srv.porta, '127.0.0.1', () => {
      socket.write(
        `POST /cmd HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer ${srv.token}\r\n` +
        `Content-Type: application/json\r\nContent-Length: ${corpoBuf.length}\r\nConnection: close\r\n\r\n`,
      );
      socket.write(parte1);
      setTimeout(() => socket.write(parte2), 20);
    });
    socket.on('end', resolve);
    socket.on('error', reject);
    socket.resume();
  });

  assert.equal(recebido, textoEsperado);
  srv.fechar();
});

test('corpo acima do teto responde 413 e nao derruba o servidor', async () => {
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: () => {} });
  const corpoGrande = 'a'.repeat(200 * 1024);
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: corpoGrande,
    headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(r.status, 413);
  assert.match(await r.text(), /corpo grande demais/);
  srv.fechar();
});

test('sessao sem navegador aberto responde 404 com recado util', async () => {
  const srv = await subirServidor({ controladorDe: () => null, escrever: () => {} });
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: JSON.stringify({ chave: 'srv::b', verbo: 'snapshot', args: [] }),
    headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(r.status, 404);
  assert.match(await r.text(), /nao tem navegador aberto/);
  srv.fechar();
});

test('so escuta em loopback', async () => {
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: () => {} });
  assert.equal(srv.endereco, '127.0.0.1');
  srv.fechar();
});

test('verbo url devolve location.href via avaliar', async () => {
  const ctl = { enfileirar: (fn) => fn(), avaliar: async (js) => (js === 'location.href' ? 'ok: "https://exemplo.test/"' : 'erro') };
  const srv = await subirServidor({ controladorDe: () => ctl, escrever: () => {} });
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: JSON.stringify({ chave: 'srv::c', verbo: 'url', args: [] }),
    headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(r.status, 200);
  assert.equal(await r.text(), 'ok: "https://exemplo.test/"');
  srv.fechar();
});

test('verbo shot grava PNG no caminho pedido', async () => {
  const destino = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'preview-shot-')), 'print.png');
  const ctl = { enfileirar: (fn) => fn(), capturarPagina: async () => ({ toPNG: () => Buffer.from('png') }) };
  const srv = await subirServidor({ controladorDe: () => ctl, escrever: () => {} });
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: JSON.stringify({ chave: 'srv::d', verbo: 'shot', args: [destino] }),
    headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(r.status, 200);
  assert.equal(await r.text(), `ok: shot ${destino}`);
  assert.ok(fs.existsSync(destino));
  srv.fechar();
});

test('verbo "constructor" nao alcanca o prototype de VERBOS', async () => {
  const srv = await subirServidor({ controladorDe: () => ctlFalso, escrever: () => {} });
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: JSON.stringify({ chave: 'srv::g', verbo: 'constructor', args: [] }),
    headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(r.status, 400);
  assert.match(await r.text(), /verbo desconhecido/);
  srv.fechar();
});

test('verbo shot sem caminho devolve erro', async () => {
  const ctl = { enfileirar: (fn) => fn(), capturarPagina: async () => ({ toPNG: () => Buffer.from('png') }) };
  const srv = await subirServidor({ controladorDe: () => ctl, escrever: () => {} });
  const r = await fetch(`http://127.0.0.1:${srv.porta}/cmd`, {
    method: 'POST',
    body: JSON.stringify({ chave: 'srv::e', verbo: 'shot', args: [] }),
    headers: { Authorization: `Bearer ${srv.token}` },
  });
  assert.equal(r.status, 200);
  assert.match(await r.text(), /shot precisa de um caminho de arquivo/);
  srv.fechar();
});
