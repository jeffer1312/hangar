const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('crypto');
const { decifrar } = require('./senhas_chrome.cjs');

// Cifra uma senha do jeito que o Chrome faz no Linux (v10), pra o decifrar fechar o ciclo.
function cifrarV10(texto, chaveSenha) {
  const chave = crypto.pbkdf2Sync(chaveSenha, 'saltysalt', 1, 16, 'sha1');
  const c = crypto.createCipheriv('aes-128-cbc', chave, Buffer.alloc(16, ' '));
  return Buffer.concat([Buffer.from('v10'), c.update(Buffer.from(texto)), c.final()]);
}

test('decifra v10 com a chave peanuts', () => {
  assert.equal(decifrar(cifrarV10('segredo123', 'peanuts'), Buffer.from('peanuts')), 'segredo123');
});

test('senha com aspas e barra sobrevive (vai como JSON no fill, não é o ponto aqui, mas o round-trip tem que bater)', () => {
  const s = 'a"b\\c\'d`e';
  assert.equal(decifrar(cifrarV10(s, 'peanuts'), Buffer.from('peanuts')), s);
});

test('chave errada -> null, não lixo', () => {
  assert.equal(decifrar(cifrarV10('x', 'peanuts'), Buffer.from('outra-chave-1234')), null);
});

test('sem prefixo v10/v11 -> null', () => {
  assert.equal(decifrar(Buffer.from('texto puro'), Buffer.from('peanuts')), null);
  assert.equal(decifrar(Buffer.alloc(0), Buffer.from('peanuts')), null);
});
