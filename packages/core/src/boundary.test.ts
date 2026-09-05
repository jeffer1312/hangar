import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const GLOBAIS = /\b(window|document|localStorage|sessionStorage|navigator|EventSource|WebSocket)\b/;
const IMPORTS = /from ['"](svelte|react-native|react|@hangar\/core)['"\/]/;
// `document` também é nome de ícone do tema (fileIcons). O global do DOM só existe FORA de string,
// então a busca por global roda com o miolo das aspas apagado — o teste de import precisa das
// aspas e continua vendo a linha crua.
const semTexto = (l: string) => l.replace(/(['"`])(?:\\.|(?!\1)[\s\S])*?\1/g, '$1$1');
const PROIBIDO = { test: (l: string) => IMPORTS.test(l) || GLOBAIS.test(semTexto(l)) };
const COMENTARIO = (l: string) => {
  const t = l.trimStart();
  return t.startsWith('//') || t.startsWith('*') || t.startsWith('/*');
};

function arquivos(dir: string): string[] {
  return readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    if (statSync(p).isDirectory()) return n === 'paraglide' ? [] : arquivos(p);
    return /\.ts$/.test(n) && !/\.test\.ts$/.test(n) ? [p] : [];
  });
}

test('core nao toca DOM, svelte, react-native nem o proprio barrel @hangar/core', () => {
  const ruins = arquivos(import.meta.dirname).filter((p) =>
    readFileSync(p, 'utf8')
      .split('\n')
      .some((l) => !COMENTARIO(l) && PROIBIDO.test(l)),
  );
  expect(ruins).toEqual([]);
});
