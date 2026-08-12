// Saída de comando que É código: descobre CONTRA QUAL ARQUIVO ela foi tirada, pra o resultado poder
// usar o mesmo visualizador do Read (ReadView) em vez de um <pre> cru.
//
// O ReadView só precisa do caminho pra escolher a linguagem do realce. Então a pergunta aqui é
// estreita: "este comando despejou UM arquivo de código?". Não é adivinhação de linguagem por
// conteúdo — é ler o alvo do comando.
//
// A regra é deliberadamente conservadora. Realce errado é pior que realce nenhum: pinta o texto com
// gramática de outra coisa e o usuário lê cor mentindo sobre o que aquilo é.

// Comandos cuja saída É o conteúdo do arquivo (inteiro, uma faixa, ou as linhas que casaram).
// O verbo importa e não dá pra dispensar: `python3 script.py` também cita um .py, mas o que sai
// dali é o que o programa IMPRIME, não o código dele — realçar seria pintar a coisa errada.
const DESPEJAM = new Set(['cat', 'sed', 'head', 'tail', 'bat', 'grep', 'rg', 'nl', 'tac']);

// Extensões que o realce conhece e que aparecem neste repo. Sem lista, `.log`/`.txt`/`.csv` entrariam
// como se fossem código.
const CODIGO = new Set([
  'ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'svelte', 'py', 'rs', 'go', 'java', 'cs', 'rb', 'php',
  'c', 'h', 'cpp', 'hpp', 'sh', 'bash', 'fish', 'zsh', 'sql', 'css', 'scss', 'html', 'xml',
  'json', 'yaml', 'yml', 'toml', 'ini', 'md', 'pas', 'dart', 'kt', 'swift', 'lua', 'vim',
]);

/**
 * Devolve o caminho do arquivo de código que o comando despejou, ou null.
 * null = mantém o `<pre>` cru, que é o comportamento de sempre.
 */
export function caminhoDeCodigoNoComando(cmd: string | null | undefined): string | null {
  const s = (cmd ?? '').trim();
  if (!s) return null;

  // Comando composto (pipe, &&, ;, redirecionamento) — a saída deixa de ser o arquivo puro: pode ter
  // passado por tr/cut/wc, ou vir de dois arquivos. Recusa em vez de arriscar.
  if (/[|;&><]|\$\(|`/.test(s)) return null;

  const tokens = s.split(/\s+/);
  const verbo = tokens[0]?.split('/').pop() ?? '';   // aceita /usr/bin/cat
  if (!DESPEJAM.has(verbo)) return null;

  // Candidatos: token com extensão conhecida, fora de aspas simples (o `sed -n '1,20p'` tem ponto
  // no meio e não pode virar caminho).
  const achados = tokens
    .slice(1)
    .filter((t) => !t.startsWith('-') && !/^['"]/.test(t))
    .map((t) => t.replace(/^['"]|['"]$/g, ''))
    .filter((t) => {
      const ext = t.includes('.') ? t.split('.').pop()!.toLowerCase() : '';
      return CODIGO.has(ext);
    });

  const distintos = [...new Set(achados)];
  // Exatamente UM: dois arquivos no mesmo comando (`cat a.ts b.py`) produzem uma saída concatenada
  // que não é nenhum dos dois.
  return distintos.length === 1 ? distintos[0] : null;
}
