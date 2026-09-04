const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { commitDoCheckout } = require('./versao.cjs');

const HASH = 'a'.repeat(40);
const OUTRO = 'b'.repeat(40);

function repo(estrutura) {
  const raiz = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-versao-'));
  for (const [rel, conteudo] of Object.entries(estrutura)) {
    const p = path.join(raiz, '.git', ...rel.split('/'));
    fs.mkdirSync(path.dirname(p), { recursive: true });
    fs.writeFileSync(p, conteudo);
  }
  return raiz;
}

test('ref solta: HEAD -> refs/heads/main -> hash', () => {
  const r = repo({ HEAD: 'ref: refs/heads/main\n', 'refs/heads/main': `${HASH}\n` });
  assert.equal(commitDoCheckout(r), HASH);
});

test('ref empacotada (packed-refs) quando a solta nao existe', () => {
  const r = repo({
    HEAD: 'ref: refs/heads/main\n',
    'packed-refs': `# pack-refs with: peeled\n${OUTRO} refs/heads/outra\n${HASH} refs/heads/main\n`,
  });
  assert.equal(commitDoCheckout(r), HASH);
});

test('HEAD solto (checkout de commit) devolve o proprio hash', () => {
  const r = repo({ HEAD: `${HASH}\n` });
  assert.equal(commitDoCheckout(r), HASH);
});

test('sem .git devolve null, nunca lanca', () => {
  assert.equal(commitDoCheckout(fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-versao-'))), null);
});
