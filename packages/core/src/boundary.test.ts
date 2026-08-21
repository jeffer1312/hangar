import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const PROIBIDO = /\b(window|document|localStorage|sessionStorage|navigator|EventSource|WebSocket)\b|from ['"](svelte|react-native|react|@hangar\/core)['"\/]/;
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
