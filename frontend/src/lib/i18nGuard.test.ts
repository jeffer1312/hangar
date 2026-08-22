import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { escanearArquivo, escanearArvore, carregarPermitidas } from '../../scripts/i18nScan.mjs';

const RAIZ = join(import.meta.dirname, '..', '..');
const SRC = join(RAIZ, 'src');
const CORE = join(RAIZ, '..', 'packages', 'core', 'src');
const MOBILE_SRC = join(RAIZ, '..', 'mobile', 'src');
const MOBILE_APP = join(RAIZ, '..', 'mobile', 'app');
const base: Record<string, number> = JSON.parse(readFileSync(join(RAIZ, 'i18n-baseline.json'), 'utf8'));

describe('trava de string crua', () => {
  const permitidas = carregarPermitidas(RAIZ);
  const achadosFront = escanearArvore(SRC, permitidas);
  const achadosCoreRaw = escanearArvore(CORE, permitidas);
  const achadosCore = Object.fromEntries(Object.entries(achadosCoreRaw).map(([k, v]) => [`core/${k}`, v]));
  const achadosMobileSrcRaw = escanearArvore(MOBILE_SRC, permitidas);
  const achadosMobileSrc = Object.fromEntries(Object.entries(achadosMobileSrcRaw).map(([k, v]) => [`mobile/src/${k}`, v]));
  const achadosMobileAppRaw = escanearArvore(MOBILE_APP, permitidas);
  const achadosMobileApp = Object.fromEntries(Object.entries(achadosMobileAppRaw).map(([k, v]) => [`mobile/app/${k}`, v]));
  const achados = { ...achadosFront, ...achadosCore, ...achadosMobileSrc, ...achadosMobileApp };

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
      const j = JSON.parse(readFileSync(join(RAIZ, '..', 'messages', arq), 'utf8'));
      return Object.keys(j).filter((k) => k !== '$schema').sort();
    };
    const en = chaves('en.json');
    const pt = chaves('pt.json');
    expect(pt.filter((k) => !en.includes(k)), 'chave so em pt: aparece como ID cru em ingles').toEqual([]);
    expect(en.filter((k) => !pt.includes(k)), 'chave so em en: aparece em ingles no meio do portugues').toEqual([]);
  });

  // O ramo de markup do extrator so vale pra arquivo COM markup. Num `.ts`, `<algo>` num
  // comentario era lido como tag: a linha era cortada no `<` e o resto virava "string crua".
  // Falso positivo que nao da pra consertar no codigo — empurrava pro i18n-allow, que e global e
  // permanente (duas entradas em um dia, 18/08/2026, as duas por isso).
  it('`<algo>` em comentario de .ts nao vira string crua', () => {
    const fonte = '// marcador "📎 imagem:/arquivo: <path>" + o "—" que liga. Mesma\nexport const X = 1;';
    expect(escanearArquivo('src/lib/exemplo.ts', fonte)).toEqual([]);
    // e o que o extrator existe pra pegar continua sendo pego, no mesmo .ts:
    expect(escanearArquivo('src/lib/exemplo.ts', 'const r = "Salvar alterações";')).toEqual(['Salvar alterações']);
    // ... e o markup de verdade nao muda:
    expect(escanearArquivo('src/components/X.svelte', '<div>Salvar alterações</div>')).toEqual(['Salvar alterações']);
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
    const fonte = readFileSync(join(RAIZ, '..', 'packages', 'core', 'src', 'format.ts'), 'utf8');
    const mapa = /stateLabels\s*:\s*Record<\s*State\s*,\s*string\s*>\s*=\s*\{([^}]*)\}/.exec(fonte);
    expect(mapa?.[1] ?? '', 'stateLabels deve chamar mensagem, nao carregar literal').not.toMatch(/['"][^'"]{3,}['"]/);
  });
});
