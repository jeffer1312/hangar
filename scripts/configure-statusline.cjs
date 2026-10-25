// Linux e Windows compartilham a preferência e a escrita da barra; o backend pode estar parado.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

function ler(arquivo) {
  try { return fs.readFileSync(arquivo, 'utf8'); }
  catch (e) { if (e.code === 'ENOENT') return null; throw e; }
}

function objeto(raw) {
  const valor = JSON.parse(raw ?? '{}');
  if (!valor || typeof valor !== 'object' || Array.isArray(valor)) throw new Error('Objeto esperado');
  return valor;
}

// Pacote: o Node resolve o link do `current` no __dirname, e a pasta da versão a poda apaga.
function pastaScripts() {
  const current = path.join(os.homedir(), '.hangar', 'current');
  try {
    if (fs.realpathSync(current) === fs.realpathSync(path.join(__dirname, '..'))) return path.join(current, 'scripts');
  } catch (e) { if (e.code !== 'ENOENT') throw e; }
  return __dirname;
}

function configurar() {
  const dir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  const preferencias = objeto(ler(path.join(dir, 'runtime-config.json')));
  const padrao = !['0', 'false', 'no', 'off'].includes((process.env.CP_CLAUDE_STATUSLINE_UPDATE || '').toLowerCase());
  const atualizar = Object.hasOwn(preferencias, 'claude_statusline_update') ? preferencias.claude_statusline_update : padrao;
  if (typeof atualizar !== 'boolean') throw new Error('Preferência inválida');
  if (!atualizar) {
    console.log('Barra de status preservada conforme a preferência do Claude Code.');
    return;
  }
  const arquivo = path.join(dir, 'settings.json');
  const anterior = ler(arquivo);
  const config = objeto(anterior);
  const quote = process.platform === 'win32' ? (s) => `"${s}"` : (s) => `'${s.replaceAll("'", "'\"'\"'")}'`;
  const comando = `${quote(process.execPath)} ${quote(path.join(pastaScripts(), 'omniroute-statusline.js'))}`;
  if (config.statusLine?.type === 'command' && config.statusLine.command === comando) {
    console.log('Barra de status já configurada.');
    return;
  }
  config.statusLine = { type: 'command', command: comando };
  fs.mkdirSync(dir, { recursive: true });
  if (anterior !== null) fs.writeFileSync(arquivo + '.bak', anterior, { mode: 0o600 });
  const tmp = `${arquivo}.${process.pid}.tmp`;
  try {
    fs.writeFileSync(tmp, JSON.stringify(config, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
    if (ler(arquivo) !== anterior) throw new Error('Configuração alterada durante a instalação');
    fs.renameSync(tmp, arquivo);
  } finally {
    fs.rmSync(tmp, { force: true });
  }
  console.log('Barra de status do Hangar configurada no Claude Code.');
}

try { configurar(); }
catch (e) {
  console.error(`Não foi possível configurar a barra de status (${e.message}); confira settings.json e runtime-config.json.`);
  process.exitCode = 1;
}
