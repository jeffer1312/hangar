const test = require('node:test');
const assert = require('node:assert/strict');
const net = require('node:net');
const { importarCookiesDoChrome, casaDominio, paraElectron } = require('./cookies_chrome.cjs');

test('dominio: igual, sufixo com ponto, com ou sem ponto inicial; nunca sufixo parcial', () => {
  assert.equal(casaDominio('.google.com', 'mail.google.com'), true);
  assert.equal(casaDominio('google.com', 'mail.google.com'), true);
  assert.equal(casaDominio('mail.google.com', 'mail.google.com'), true);
  assert.equal(casaDominio('ogle.com', 'mail.google.com'), false);
  assert.equal(casaDominio('mail.google.com', 'google.com'), false);
  assert.equal(casaDominio('', 'google.com'), false);
});

test('sameSite do CDP vira o do Electron; sessao (expires -1) sem expirationDate', () => {
  const base = { name: 'a', value: '1', domain: '.x.com', path: '/p', secure: true, httpOnly: false, expires: 123 };
  assert.equal(paraElectron({ ...base, sameSite: 'Strict' }, 'x.com').sameSite, 'strict');
  assert.equal(paraElectron({ ...base, sameSite: 'Lax' }, 'x.com').sameSite, 'lax');
  assert.equal(paraElectron({ ...base, sameSite: 'None' }, 'x.com').sameSite, 'no_restriction');
  const c = paraElectron({ ...base, expires: -1 }, 'x.com');
  assert.equal(c.sameSite, 'unspecified');
  assert.equal('expirationDate' in c, false);
  assert.equal(c.url, 'https://x.com/p');
});

test('porta fechada -> erro chrome_fechado', async () => {
  // Porta livre: abre e fecha um servidor só pra descobrir uma que ninguém escuta.
  const srv = net.createServer();
  await new Promise((r) => srv.listen(0, '127.0.0.1', r));
  const porta = srv.address().port;
  await new Promise((r) => srv.close(r));
  await assert.rejects(importarCookiesDoChrome({ porta, dominio: 'x.com' }), (e) => e.code === 'chrome_fechado');
});

test('Chrome headless na porta (automacao de outra sessao) e recusado como chrome_fechado', async () => {
  const http = require('node:http');
  const srv = http.createServer((_req, res) => {
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify({ 'User-Agent': 'Mozilla/5.0 HeadlessChrome/150.0', webSocketDebuggerUrl: 'ws://127.0.0.1:1/x' }));
  });
  await new Promise((r) => srv.listen(0, '127.0.0.1', r));
  const porta = srv.address().port;
  try {
    await assert.rejects(importarCookiesDoChrome({ porta, dominio: 'x.com' }),
      (e) => e.code === 'chrome_fechado' && e.motivo === 'headless');
  } finally {
    await new Promise((r) => srv.close(r));
  }
});
