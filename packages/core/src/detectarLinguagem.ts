// Adivinha a linguagem de um texto de saída, pra dar realce a código que veio de comando composto
// (`cat x | head`, `grep ... | tail`) — onde o alvo não dá pra ler do comando (ver codeFromBash.ts).
//
// Aqui erra às vezes, e o desenho assume isso: erra pra MENOS. Só chuta com sinal forte e com folga
// sobre o segundo colocado; na dúvida devolve null e o texto sai cru, como sempre saiu. Pintar com a
// gramática errada é pior que não pintar — a cor passa a mentir sobre o que aquilo é.

// Prefixos que ferramentas põem na frente da linha e que não fazem parte do código:
//   `123:conteudo`        grep -n / rg -n
//   `123\tconteudo`       cat -n / Read do Claude
//   `caminho:123:conteudo` grep -n em vários arquivos
// `123   conteudo` (numero + 2+ espacos) e o gutter que varias ferramentas usam, inclusive o grep
// filtrado deste ambiente. Exige 2 espacos pra nao comer "1 item" de uma lista em prosa.
const PREFIXO = /^(?:[^\s:]*:)?\d+(?:[:\t]|\s{2,})/;

function limpar(texto: string): string[] {
  return texto
    .replace(/\n$/, '')
    .split('\n')
    .map((l) => l.replace(PREFIXO, ''))
    .filter((l) => l.trim() !== '');
}

// Cada sinal vale PESO pontos por linha em que aparece. Escolhidos por serem RAROS fora da
// linguagem — `import` sozinho não serve (existe em quase tudo), `import ... from '...'` serve.
//
// Peso existe porque nem todo sinal vale o mesmo. `algo: valor;` é ambíguo por natureza: corpo de
// `interface` do TS e declaração CSS são a MESMA forma, e com peso igual os dois empatavam e o
// detector desistia dos dois. Já `export interface X {` é decisivo — CSS nunca tem isso. Sinal
// decisivo vale 5; pista comum vale 1.
const DECISIVO = 5;
type Regra = RegExp | [RegExp, number];
const peso = (r: Regra) => (Array.isArray(r) ? r[1] : 1);
const exp = (r: Regra) => (Array.isArray(r) ? r[0] : r);

const SINAIS: { ext: string; regras: Regra[] }[] = [
  { ext: 'ts', regras: [
    [/^\s*(export\s+)?(interface|type)\s+\w+\s*[=<{]/, DECISIVO], /:\s*(string|number|boolean|void|unknown|never)\b/,
    /^\s*(export\s+)?(const|let)\s+\w+\s*:\s*\w/, /\bas\s+(const|unknown|\w+\[\])/,
  ] },
  { ext: 'js', regras: [
    [/^\s*(export\s+)?(async\s+)?function\s+\w+\s*\(/, DECISIVO], /^\s*(const|let|var)\s+\w+\s*=/,
    /=>\s*[{(]/, /^\s*import\s+.*\bfrom\s+['"]/, /\bconsole\.(log|error|debug)\(/,
  ] },
  { ext: 'py', regras: [
    [/^\s*def\s+\w+\s*\(/, DECISIVO], /^\s*class\s+\w+\s*[(:]/, [/^\s*from\s+[\w.]+\s+import\s/, DECISIVO],
    /\bself\./, /^\s*(if|for|while|with|try|elif|else)\b.*:\s*$/,
  ] },
  { ext: 'svelte', regras: [[/\{#(if|each|await|key)\b/, DECISIVO], /\{[:/](if|each|else|await)\b/, /\bbind:\w+/, /\buse:\w+/] },
  { ext: 'css', regras: [
    [/^\s*[.#&][\w-]+[^{;]*\{\s*$/, DECISIVO], /^\s*[\w-]+\s*:\s*[^;{}]+;\s*$/, /\bvar\(--[\w-]+\)/, [/^\s*@(media|keyframes|supports)\b/, DECISIVO],
  ] },
  { ext: 'html', regras: [/<\/(div|span|p|body|html|section|button)>/, /<(div|span|section|button)\b[^>]*>/] },
  { ext: 'sh', regras: [
    /^\s*(if|for|while)\b.*;\s*then\b|^\s*(fi|done|esac)\s*$/, /^\s*\w+=\S+$/, /\$\{\w+\}/, /^\s*echo\s+/,
  ] },
  { ext: 'sql', regras: [[/\bSELECT\b[\s\S]*\bFROM\b/i, DECISIVO], /\b(INSERT\s+INTO|UPDATE\s+\w+\s+SET|CREATE\s+TABLE)\b/i] },
  // YAML é o sinal mais traiçoeiro do conjunto: `  chave: valor` indentado é IGUAL a anotação de
  // tipo do TS (`id: string;`) e a declaração CSS (`display: flex;`). Sem excluir a linha terminada
  // em `;` ou `{`, ele empatava com os dois e a folga fazia o detector devolver null pros três.
  { ext: 'yaml', regras: [/^\s*-\s+\w+:\s/, /^\s{2,}[\w-]+:\s*[^;{}]*[^;{}\s]\s*$/, [/^---\s*$/, DECISIVO]] },
  { ext: 'md', regras: [/^#{1,6}\s+\S/, /^\s*[-*]\s+\S/, /^```/] },
];

// Shebang é prova, não pista.
const SHEBANG: [RegExp, string][] = [
  [/^#!.*\b(bash|sh|zsh)\b/, 'sh'],
  [/^#!.*\bpython\d?\b/, 'py'],
  [/^#!.*\bnode\b/, 'js'],
];

/** Mínimo de linhas: em 3 linhas qualquer coisa casa um sinal por acaso. */
const MIN_LINHAS = 6;
/** Densidade mínima: o vencedor tem que aparecer em pelo menos 12% das linhas. */
const MIN_DENSIDADE = 0.12;
/** Folga sobre o segundo: sem ela, ts e js empatam o tempo todo e a escolha vira sorteio. */
const FOLGA = 1.5;

/**
 * Devolve um PSEUDO-CAMINHO (`saida.ts`) pra alimentar o realce, ou null.
 * Pseudo-caminho e não o nome da linguagem porque quem escolhe a gramática é `langFromPath`.
 */
export function pseudoCaminhoPorConteudo(texto: string | null | undefined): string | null {
  const linhas = limpar(texto ?? '');
  if (linhas.length < MIN_LINHAS) return null;

  for (const [re, ext] of SHEBANG) {
    if (re.test(linhas[0])) return `saida.${ext}`;
  }

  // JSON é o único que dá pra CONFIRMAR em vez de estimar.
  const junto = linhas.join('\n').trim();
  if ((junto.startsWith('{') && junto.endsWith('}')) || (junto.startsWith('[') && junto.endsWith(']'))) {
    try { JSON.parse(junto); return 'saida.json'; } catch { /* não era */ }
  }

  const placar = SINAIS.map(({ ext, regras }) => ({
    ext,
    // Por LINHA vale o maior peso que casou nela — somar todas as regras da mesma linha faria
    // uma linha rica valer por cinco e desequilibraria a comparação.
    pontos: linhas.reduce((s, l) => {
      const casou = regras.filter((r) => exp(r).test(l));
      return s + (casou.length ? Math.max(...casou.map(peso)) : 0);
    }, 0),
  })).sort((a, b) => b.pontos - a.pontos);

  const [primeiro, segundo] = placar;
  if (!primeiro || primeiro.pontos === 0) return null;
  if (primeiro.pontos / linhas.length < MIN_DENSIDADE) return null;
  if (segundo && segundo.pontos > 0 && primeiro.pontos < segundo.pontos * FOLGA) return null;
  return `saida.${primeiro.ext}`;
}
