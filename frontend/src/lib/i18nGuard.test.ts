import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { escanearArvore, carregarPermitidas } from '../../scripts/i18nScan.mjs';

const RAIZ = join(import.meta.dirname, '..', '..');
const SRC = join(RAIZ, 'src');
const base: Record<string, number> = JSON.parse(readFileSync(join(RAIZ, 'i18n-baseline.json'), 'utf8'));

describe('trava de string crua', () => {
  const achados = escanearArvore(SRC, carregarPermitidas(RAIZ));

  it('nenhum arquivo passa do seu limite na linha de base', () => {
    const piorou: string[] = [];
    for (const [arquivo, strings] of Object.entries(achados)) {
      // Arquivo que nao esta na linha de base tem limite ZERO: e o que obriga tela nova a nascer
      // com chave, em vez de acumular divida que alguem extrai de novo daqui a seis meses.
      const limite = base[arquivo] ?? 0;
      if (strings.length > limite) {
        piorou.push(`${arquivo}: ${strings.length} > ${limite}\n    ${strings.slice(0, 8).join('\n    ')}`);
      }
    }
    expect(piorou.join('\n\n')).toBe('');
  });

  // Com baseLocale = 'en', chave que existe SO no pt.json nao tem para onde cair: o Paraglide
  // compila a funcao, gera o .d.ts (entao o svelte-check e cego) e a tela em ingles mostra o ID DA
  // CHAVE na cara do usuario. Nenhuma das outras camadas da trava pega isso.
  it('pt.json e en.json tem exatamente as mesmas chaves', () => {
    const chaves = (arq: string) => {
      const j = JSON.parse(readFileSync(join(RAIZ, 'messages', arq), 'utf8'));
      return Object.keys(j).filter((k) => k !== '$schema').sort();
    };
    const en = chaves('en.json');
    const pt = chaves('pt.json');
    expect(pt.filter((k) => !en.includes(k)), 'chave so em pt: aparece como ID cru em ingles').toEqual([]);
    expect(en.filter((k) => !pt.includes(k)), 'chave so em en: aparece em ingles no meio do portugues').toEqual([]);
  });

  // lib/format.ts nao tem mapa de rotulo com valor literal.
  // stateLabels era `Record<State, string>` com 'em execucao'/'pronto'/'aguardando'/'encerrado'
  // escritos ali. Um Record de literais e exatamente o formato que vaza sem ninguem ver.
  //
  // A busca e ANCORADA NO NOME de proposito. Uma regex generica por `Record<State, string>` com
  // `.exec` devolve o PRIMEIRO match: hoje e o stateLabels, mas assim que a Task 3 o transforma em
  // funcao o primeiro passa a ser o stateColors (format.ts:68-73), cujos valores ('var(--accent)')
  // tambem casam o teste de literal — e a suite fecharia em vermelho justo quando o problema foi
  // consertado. Cor nao e idioma; so o mapa de ROTULO e vigiado aqui.
  it('lib/format.ts nao tem mapa de rotulo com valor literal', () => {
    const fonte = readFileSync(join(SRC, 'lib', 'format.ts'), 'utf8');
    const mapa = /stateLabels\s*:\s*Record<\s*State\s*,\s*string\s*>\s*=\s*\{([^}]*)\}/.exec(fonte);
    expect(mapa?.[1] ?? '', 'stateLabels deve chamar mensagem, nao carregar literal').not.toMatch(/['"][^'"]{3,}['"]/);
  });
});
