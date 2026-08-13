// Gera frontend/i18n-baseline.json a partir do estado atual da arvore src/: quantas strings cruas
// cada arquivo ainda tem. O guard (src/lib/i18nGuard.test.ts) falha quando um arquivo passa do
// numero. O numero so desce; rodar de novo so depois de uma fatia de extracao, e o diff da linha de
// base mostra exatamente o que a fatia limpou.
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { escanearArvore, carregarPermitidas } from './i18nScan.mjs';

const raiz = join(import.meta.dirname, '..');
const src = join(raiz, 'src');
const achados = escanearArvore(src, carregarPermitidas(raiz));

// Formato { caminho: quantidade }, ordenado por caminho — o diff fica legivel e cada linha de
// extracao mostra so o que ela limpou.
const linhaDeBase = Object.fromEntries(
  Object.entries(achados).map(([arquivo, strings]) => [arquivo, strings.length]).sort(),
);
writeFileSync(join(raiz, 'i18n-baseline.json'), JSON.stringify(linhaDeBase, null, 2) + '\n');
console.log(`linha de base gravada: ${Object.keys(linhaDeBase).length} arquivos, ${Object.values(linhaDeBase).reduce((a, b) => a + b, 0)} ocorrencias`);
