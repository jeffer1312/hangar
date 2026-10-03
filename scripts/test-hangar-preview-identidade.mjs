#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

const src = readFileSync(new URL('./hangar-preview', import.meta.url), 'utf8');
const resolver = src.slice(src.indexOf('function nomeSessao() {'), src.indexOf('\nfunction pastaShots()'));
function session(rows, failed = false) {
  return runInNewContext(`${resolver}\nnomeSessao()`, {
    flags: {}, nomeHeadless: () => null, process: { env: { TMUX: '1' } },
    execSync: () => 'observada',
    execFileSync: (program, argv) => {
      assert.equal(program, 'tmux');
      assert.equal(argv[0], 'list-clients');
      if (failed) throw new Error('indisponivel');
      const fields = argv.at(-1).split('\t');
      return rows.map(row => fields.map(field => row[field]).join('\t')).join('\n');
    },
  });
}
const observer = { '#{client_control_mode}': '1', '#{session_name}': 'observada' };
const human = { '#{client_control_mode}': '0', '#{session_name}': 'humana' };
assert.equal(session([observer]), null, 'observador não escolhe navegador');
assert.equal(session([observer, human]), 'humana');
assert.equal(session([human, human]), 'humana');
assert.equal(session([human, { ...human, '#{session_name}': 'outra' }]), null);
assert.equal(session([{ ...human, '#{client_control_mode}': '' }]), null);
assert.equal(session([{ '#{client_control_mode}': '/dev/pts/0: observada: powershell [200x50] (utf8)' }]), null);
assert.equal(session([human], true), null);
console.log('ok: 7 casos');
