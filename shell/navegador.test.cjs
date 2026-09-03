const test = require('node:test');
const assert = require('node:assert/strict');
const { uaDeChrome, normalizaBounds, urlNavegavel, nomeSidecar } = require('./navegador.cjs');

test('uaDeChrome remove marcas de app e vira Chrome vanilla', () => {
  const ua = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) hangar/0.1.3 Chrome/142.0.0.0 Electron/43.3.0 Safari/537.36 hangar-shell';
  assert.equal(
    uaDeChrome(ua),
    'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36',
  );
});

test('uaDeChrome em UA já limpo não muda nada', () => {
  const ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36';
  assert.equal(uaDeChrome(ua), ua);
});

test('normalizaBounds arredonda float de getBoundingClientRect e corta negativo', () => {
  assert.deepEqual(
    normalizaBounds({ x: 10.6, y: -3, width: 800.4, height: 600 }),
    { x: 11, y: 0, width: 800, height: 600 },
  );
  assert.deepEqual(normalizaBounds(undefined), { x: 0, y: 0, width: 0, height: 0 });
});

test('urlNavegavel aceita http(s) e recusa o resto', () => {
  assert.equal(urlNavegavel('http://localhost:3000/x'), 'http://localhost:3000/x');
  assert.equal(urlNavegavel('https://exemplo.com'), 'https://exemplo.com/');
  assert.equal(urlNavegavel('file:///etc/passwd'), null);
  assert.equal(urlNavegavel('javascript:alert(1)'), null);
  assert.equal(urlNavegavel('não é url'), null);
});

test('nomeSidecar: chave vira nome de arquivo seguro, com sufixo casável', () => {
  assert.equal(nomeSidecar('srv-abc123::hangar'), 'srv-abc123--hangar');
  assert.equal(nomeSidecar('srv-x::minha sessao/noite'), 'srv-x--minha-sessao-noite');
  // o CLI casa pelo sufixo `--<nome da sessão>.json`
  assert.ok(nomeSidecar('srv-x::hangar').endsWith('--hangar'));
});
