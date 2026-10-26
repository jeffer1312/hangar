#!/usr/bin/env node
// Hook (PreToolUse, Bash) — antes de `gh pr create`, lembra o modelo de escrever a novidade quando
// o PR muda tela do app sem arquivo em docs/novidades/. Só lembra: nunca bloqueia.
// O check é scripts/check-novidade.sh, o mesmo que o CI roda.
import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';

let dados;
try {
  dados = JSON.parse(readFileSync(0, 'utf8'));
} catch {
  process.exit(0);
}
// Mesma quebra do hook-pr-verificado.mjs: só o trecho que É a chamada conta, não texto entre aspas.
function trechos(texto) {
  const saida = [];
  let atual = '', aspas = '';
  for (let i = 0; i < texto.length; i++) {
    const c = texto[i];
    if (aspas) {
      if (c === '\\' && aspas === '"') { atual += c + (texto[++i] ?? ''); continue; }
      if (c === aspas) aspas = '';
      atual += c;
    } else if (c === "'" || c === '"') {
      aspas = c; atual += c;
    } else if (c === '\\') {
      atual += c + (texto[++i] ?? '');
    } else if (';&|(\n'.includes(c)) {
      saida.push(atual); atual = '';
    } else {
      atual += c;
    }
  }
  saida.push(atual);
  return saida;
}
let pasta = dados?.cwd || process.cwd();
let achado = null;
for (const t of trechos(dados?.tool_input?.command ?? '')) {
  const cd = t.match(/^\s*cd\s+(['"]?)([^'"\s]+)\1\s*$/);
  if (cd) pasta = resolve(pasta, cd[2].replace(/^~(?=\/|$)/, homedir()));
  achado = t.match(/^\s*((?:(?:\w+=\S*|then|do|else|command|env|sudo|time|!|\{)\s+)*)gh\s+(?:(?:-R|--repo)[ =]\S+\s+)?pr\s+create\b(.*)$/s);
  if (achado) break;
}
if (!achado) process.exit(0);
const comando = achado[2].replace(/'[^']*'|"(?:\\.|[^"\\])*"/g, m => (/^['"][^\s'"]+['"]$/.test(m) ? m.slice(1, -1) : '""'));
if (/(?:^|\s)(?:-h|--help|--dry-run)\b/.test(comando)) process.exit(0);

const git = (cwd, ...args) =>
  execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
const opcao = (curta, longa) => comando.match(new RegExp(`(?:^|\\s)(?:${curta}|${longa})[ =]+(\\S+)`))?.[1];

let raiz, ponta, origem;
try {
  raiz = git(pasta, 'rev-parse', '--show-toplevel');
  const head = opcao('-H', '--head');
  const base = opcao('-B', '--base');
  let alvo = 'origin/main';
  try { alvo = git(raiz, 'rev-parse', '--abbrev-ref', 'origin/HEAD'); } catch {}
  if (base) alvo = `origin/${base}`;
  ponta = git(raiz, 'rev-parse', '--verify', `${head ? `origin/${head}` : 'HEAD'}^{commit}`);
  origem = git(raiz, 'merge-base', alvo, ponta);
} catch {
  // Sem base resolvível o lembrete se cala; quem barra PR sem base é o hook-pr-verificado.
  process.exit(0);
}
const script = join(raiz, 'scripts', 'check-novidade.sh');
if (!existsSync(script)) process.exit(0);

const env = { ...process.env };
delete env.GITHUB_ACTIONS;
const r = spawnSync('bash', [script, `${origem}..${ponta}`], { cwd: raiz, encoding: 'utf8', env });
const aviso = (r.stderr || '').trim();
if (r.status !== 0 || !aviso.startsWith('aviso:')) process.exit(0);

console.log(JSON.stringify({
  hookSpecificOutput: {
    hookEventName: 'PreToolUse',
    additionalContext:
      `[novidade] ${aviso.replace(/^aviso:\s*/, '')}\n` +
      'Antes de abrir o PR, invoque a skill hangar-novidade (escreve docs/novidades/<data>-<nome>.md e, ' +
      'se for recurso novo visível, a animação). Se não há nada que a pessoa veja, siga com o PR e ' +
      "registre 'HANGAR_SEM_NOVIDADE: <motivo>' num commit.",
  },
}));
process.exit(0);
