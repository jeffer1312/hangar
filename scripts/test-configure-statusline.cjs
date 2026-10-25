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

test('no pacote a barra aponta para o current, não para a pasta da versão', (t) => {
  const casa = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-statusline-casa-'));
  t.after(() => fs.rmSync(casa, { recursive: true, force: true }));
  const versao = path.join(casa, '.hangar', 'versions', '0.1.0.42');
  fs.mkdirSync(path.join(versao, 'scripts'), { recursive: true });
  fs.copyFileSync(path.join(__dirname, 'configure-statusline.cjs'), path.join(versao, 'scripts', 'configure-statusline.cjs'));
  const current = path.join(casa, '.hangar', 'current');
  fs.symlinkSync(versao, current, 'junction');
  const config = path.join(casa, '.claude');
  const r = spawnSync(process.execPath, [path.join(current, 'scripts', 'configure-statusline.cjs')], {
    env: { ...process.env, HOME: casa, USERPROFILE: casa, CLAUDE_CONFIG_DIR: config }, encoding: 'utf8',
  });
  assert.equal(r.status, 0, r.stderr);
  const comando = JSON.parse(fs.readFileSync(path.join(config, 'settings.json'), 'utf8')).statusLine.command;
  assert.ok(comando.includes(path.join(current, 'scripts', 'omniroute-statusline.js')), comando);
});

for (const runtime of ['{', 'null', '{"claude_statusline_update":"false"}', '{"claude_statusline_update":null}']) {
  test(`preferência inválida não autoriza substituir a barra: ${runtime}`, (t) => {
    const { settings, anterior, rodar } = instalacao(t, runtime);
    assert.notEqual(rodar().status, 0);
    assert.equal(fs.readFileSync(settings, 'utf8'), anterior);
  });
}
