const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { ler, gravar } = require('./settings.cjs');

test('ler() em diretório inexistente devolve {}', () => {
  const dir = path.join(os.tmpdir(), 'cp-shell-settings-missing-' + Date.now());
  assert.deepEqual(ler(dir), {});
});

test('gravar() seguido de ler() faz round-trip', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'cp-shell-settings-'));
  // Diretorio ANINHADO que ainda nao existe: mkdtempSync ja cria o diretorio raiz sozinho, entao
  // gravar direto nele nunca exercita o `mkdirSync({recursive: true})` — o caminho que existe
  // justamente pra primeira execucao, quando o userData do Electron ainda nao foi criado.
  const dir = path.join(base, 'aninhado', 'userdata');
  try {
    const dados = { url: 'http://127.0.0.1:8765', janela: { x: 10, y: 20, width: 1280, height: 800 } };
    gravar(dir, dados);
    assert.deepEqual(ler(dir), dados);
  } finally {
    fs.rmSync(base, { recursive: true, force: true });
  }
});

test('ler() em arquivo corrompido devolve {}', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cp-shell-settings-'));
  try {
    fs.writeFileSync(path.join(dir, 'settings.json'), 'isto nao e json {{{');
    assert.deepEqual(ler(dir), {});
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
