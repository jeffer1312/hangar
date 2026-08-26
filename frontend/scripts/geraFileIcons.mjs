// Gera src/lib/fileIcons.generated.ts a partir do material-icon-theme (node_modules), só os
// nomes de src/lib/fileIcons.lista.json. O gerado é commitado: o build da VPS não instala o
// pacote. Rodar: `npm run icons`.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const raiz = dirname(dirname(fileURLToPath(import.meta.url)));
const pacote = join(raiz, 'node_modules', 'material-icon-theme');
const lista = JSON.parse(readFileSync(join(raiz, 'src', 'lib', 'fileIcons.lista.json'), 'utf8'));
const licenca = readFileSync(join(pacote, 'LICENSE'), 'utf8').trim();
const versao = JSON.parse(readFileSync(join(pacote, 'package.json'), 'utf8')).version;

const out = {};
for (const nome of lista) {
  const svg = readFileSync(join(pacote, 'icons', `${nome}.svg`), 'utf8').trim();
  // Vai pra tela por {@html}: a única barreira é esta. Sem script e sem handler inline.
  if (/<script|\son[a-z]+\s*=|javascript:/i.test(svg)) throw new Error(`svg suspeito: ${nome}`);
  out[nome] = svg.replace(/>\s+</g, '><');
}

const cabecalho = `// GERADO por scripts/geraFileIcons.mjs — não edite à mão. Fonte: material-icon-theme ${versao}.\n` +
  licenca.split('\n').map((l) => `// ${l}`.trimEnd()).join('\n') + '\n\n';
writeFileSync(join(raiz, 'src', 'lib', 'fileIcons.generated.ts'),
  cabecalho + 'export const ICONES: Record<string, string> = ' + JSON.stringify(out, null, 0) + ';\n');
console.log(`${lista.length} ícones gerados (material-icon-theme ${versao})`);
