const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

function instalacao(t, runtime) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-statusline-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const settings = path.join(dir, 'settings.json');
  const anterior = JSON.stringify({ statusLine: { type: 'command', command: 'barra-pessoal' }, env: { TESTE: 'preservar' } });
  fs.writeFileSync(settings, anterior);
  if (runtime !== undefined) fs.writeFileSync(path.join(dir, 'runtime-config.json'), runtime);
  const rodar = () => spawnSync(process.execPath, [path.join(__dirname, 'configure-statusline.cjs')], {
    env: { ...process.env, CLAUDE_CONFIG_DIR: dir }, encoding: 'utf8',
  });
  return { settings, anterior, rodar };
}

test('preferência desligada preserva a barra pessoal e não cria backup', (t) => {
  const { settings, anterior, rodar } = instalacao(t, '{"claude_statusline_update":false}');
  const r = rodar();
  assert.equal(r.status, 0, r.stderr);
  assert.equal(fs.readFileSync(settings, 'utf8'), anterior);
  assert.equal(fs.existsSync(settings + '.bak'), false);
});

for (const runtime of [undefined, '{}', '{"claude_statusline_update":true}']) {
  test(`instala com preferência habilitada ou ausente: ${runtime}`, (t) => {
    const { settings, anterior, rodar } = instalacao(t, runtime);
    const r = rodar();
    assert.equal(r.status, 0, r.stderr);
    const atual = JSON.parse(fs.readFileSync(settings, 'utf8'));
    assert.match(atual.statusLine.command, /omniroute-statusline\.js/);
    assert.deepEqual(atual.env, { TESTE: 'preservar' });
    assert.equal(fs.readFileSync(settings + '.bak', 'utf8'), anterior);
    const mtime = fs.statSync(settings).mtimeMs;
    assert.equal(rodar().status, 0);
    assert.equal(fs.statSync(settings).mtimeMs, mtime);
  });
}

for (const runtime of ['{', 'null', '{"claude_statusline_update":"false"}', '{"claude_statusline_update":null}']) {
  test(`preferência inválida não autoriza substituir a barra: ${runtime}`, (t) => {
    const { settings, anterior, rodar } = instalacao(t, runtime);
    assert.notEqual(rodar().status, 0);
    assert.equal(fs.readFileSync(settings, 'utf8'), anterior);
  });
}
