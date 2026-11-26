#!/usr/bin/env node
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, copyFile, writeFile, rm } from 'node:fs/promises';
import { closeSync, openSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = dirname(dirname(fileURLToPath(import.meta.url)));
const temp = await mkdtemp(join(tmpdir(), 'hangar-preview-headless-'));
const home = join(temp, 'home');

try {
  await Promise.all([
    mkdir(join(temp, 'scripts'), { recursive: true }),
    mkdir(join(temp, 'shell'), { recursive: true }),
    mkdir(join(temp, 'backend'), { recursive: true }),
    mkdir(join(home, '.hangar', 'codex-sessions'), { recursive: true }),
    mkdir(join(home, '.hangar', 'nav'), { recursive: true }),
  ]);
  await Promise.all([
    copyFile(join(repo, 'scripts', 'hangar-preview'), join(temp, 'scripts', 'hangar-preview')),
    copyFile(join(repo, 'shell', 'preview_fmt.cjs'), join(temp, 'shell', 'preview_fmt.cjs')),
    copyFile(join(repo, 'shell', 'folha.cjs'), join(temp, 'shell', 'folha.cjs')),
    writeFile(join(temp, 'backend', '.env'), 'CP_AUTH_TOKEN=teste\n'),
    writeFile(join(home, '.hangar', 'codex-sessions', 'cx.json'),
      JSON.stringify({ name: 'cx-sem-terminal', provider: 'codex', headless: true, key: 'chave-cx' })),
    writeFile(join(home, '.hangar', 'nav', 'cx.json'),
      JSON.stringify({ chave: 'cx-sem-terminal', url: 'http://localhost:3000' })),
    writeFile(join(home, '.hangar', 'nav', '_srv.json'),
      JSON.stringify({ pid: 999999999, porta: 1, token: 'teste' })),
  ]);

  const env = { ...process.env, HOME: home, HANGAR_CANO_KEY: 'chave-cx' };
  delete env.CP_SESSION_KEY;
  delete env.TMUX;
  delete env.TMUX_PANE;
  delete env.CP_SESSION_NAME;

  const outputPath = join(temp, 'saida.txt');
  const output = openSync(outputPath, 'w');
  const result = await new Promise((resolve) => {
    const child = spawn(process.execPath, [join(temp, 'scripts', 'hangar-preview'),
      'url'], { env, stdio: ['ignore', output, output] });
    child.on('close', (code) => resolve({ code }));
  });
  closeSync(output);
  const text = readFileSync(outputPath, 'utf8');

  assert.equal(result.code, 1);
  assert.match(text, /app desktop do Hangar nao esta rodando/);
  assert.doesNotMatch(text, /fora do tmux/);
  console.log('ok: Codex headless resolve o próprio navegador sem tmux');
} finally {
  await rm(temp, { recursive: true, force: true });
}
