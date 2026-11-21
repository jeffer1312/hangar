import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { expect, it } from 'vitest';

// Espaço, raio e peso saem do tema (`theme.base.space/radius/weight`, `theme.type.*`), nunca de
// número solto. 0 e 1 (fio) passam; negativo também (margem que devolve área de toque).
const ROOT = join(__dirname, '..', '..');
// Miniaturas desenhadas em escala (prévia de tema/fundo): o número é geometria, não layout.
const DESENHO = new Set(['src/features/config/AppearanceArt.tsx']);
// Só desce: cada passada por uma área baixa o teto para o que sobrou.
const TETO = 7;

const LITERAL = /\b(gap|rowGap|columnGap|padding\w*|margin\w*|border\w*Radius|borderRadius): ?([2-9]|\d{2,})\b|\bfontWeight: ?['"]/;

function files(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name);
    if (e.isDirectory()) return ['node_modules', 'paraglide', '__mocks__'].includes(e.name) ? [] : files(p);
    return /\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [p] : [];
  });
}

it('número solto de espaço, raio e peso não cresce', () => {
  const found = [join(ROOT, 'app'), join(ROOT, 'src')].flatMap(files)
    .map((f) => f.slice(ROOT.length + 1))
    .filter((f) => !DESENHO.has(f) && !f.startsWith('src/theme/'))
    .flatMap((f) => readFileSync(join(ROOT, f), 'utf8').split('\n')
      .flatMap((line, i) => (LITERAL.test(line) ? [`${f}:${i + 1}`] : [])));
  expect(found.length, found.join('\n')).toBeLessThanOrEqual(TETO);
});
