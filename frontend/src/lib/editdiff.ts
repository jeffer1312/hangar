// Diff linha-a-linha pro card de Edit/MultiEdit do chat (o "diff estilo Pi": old a esquerda, new a
// direita). Fonte = old_string/new_string do tool_input — NAO e um unified diff de arquivo, entao os
// numeros de linha sao relativos ao TRECHO editado (1-based), nao ao arquivo inteiro.
//
// Myers O(ND) com trace pra backtrack. Acima de um teto de tamanho cai no fallback prefixo/sufixo
// (diff valido, so nao-minimo) pra nunca travar a UI num pathologico gigante.

export type EditOp = 'ctx' | 'del' | 'add';
export interface OpLine { op: EditOp; text: string }

export interface SplitCell { num: number; text: string }
export interface SplitRow {
  left: SplitCell | null;    // null = lado vazio (o app desenha o hachurado)
  right: SplitCell | null;
}

export interface EditDiff {
  ops: OpLine[];             // visao unificada (celular): ctx/del/add na ordem
  rows: SplitRow[];          // visao lado a lado (desktop): del[i] pareado com add[i]
  add: number;               // linhas adicionadas
  del: number;               // linhas removidas
}

// Abaixo disto vai no Myers completo; acima, so trim de prefixo/sufixo (meio vira del+add em bloco).
// 1M pares ~ string de 1000x1000 linhas — Edit de verdade raramente passa de 100.
const MYERS_MAX_PRODUCT = 1_000_000;

function splitLines(s: string): string[] {
  if (s === '') return [];
  const t = s.endsWith('\n') ? s.slice(0, -1) : s;
  return t.split('\n');
}

function myers(a: string[], b: string[]): OpLine[] {
  const n = a.length, m = b.length;
  const max = n + m;
  const off = max;
  const v = new Int32Array(2 * max + 1);
  const trace: Int32Array[] = [];
  let found = 0;

  outer: for (let d = 0; d <= max; d++) {
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && v[k - 1 + off] < v[k + 1 + off])) x = v[k + 1 + off];
      else x = v[k - 1 + off] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) { x++; y++; }
      v[k + off] = x;
      if (x >= n && y >= m) { found = d; break outer; }
    }
    trace.push(v.slice());
  }

  // Backtrack do fim pro inicio, remontando as operacoes.
  const ops: OpLine[] = [];
  let x = n, y = m;
  for (let d = found; d > 0; d--) {
    const vPrev = trace[d - 1];
    const k = x - y;
    const prevK = (k === -d || (k !== d && vPrev[k - 1 + off] < vPrev[k + 1 + off])) ? k + 1 : k - 1;
    const prevX = vPrev[prevK + off];
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) { ops.push({ op: 'ctx', text: a[--x] }); y--; }
    if (x === prevX) ops.push({ op: 'add', text: b[--y] });
    else ops.push({ op: 'del', text: a[--x] });
  }
  while (x > 0 && y > 0) { ops.push({ op: 'ctx', text: a[--x] }); y--; }
  while (x > 0) ops.push({ op: 'del', text: a[--x] });
  while (y > 0) ops.push({ op: 'add', text: b[--y] });
  return ops.reverse();
}

function prefixSuffixFallback(a: string[], b: string[]): OpLine[] {
  let pre = 0;
  while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre++;
  let suf = 0;
  while (suf < a.length - pre && suf < b.length - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++;
  const ops: OpLine[] = [];
  for (let i = 0; i < pre; i++) ops.push({ op: 'ctx', text: a[i] });
  for (let i = pre; i < a.length - suf; i++) ops.push({ op: 'del', text: a[i] });
  for (let i = pre; i < b.length - suf; i++) ops.push({ op: 'add', text: b[i] });
  for (let i = a.length - suf; i < a.length; i++) ops.push({ op: 'ctx', text: a[i] });
  return ops;
}

/** Pareia as ops em linhas lado a lado: bloco del+add vira linhas "alteradas" (1 a 1 pelo indice);
 * o que sobra de um lado fica com o outro lado vazio. Numeros relativos ao trecho (1-based). */
export function pairRows(ops: OpLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let oldNum = 1, newNum = 1, i = 0;
  while (i < ops.length) {
    if (ops[i].op === 'ctx') {
      rows.push({ left: { num: oldNum++, text: ops[i].text }, right: { num: newNum++, text: ops[i].text } });
      i++;
      continue;
    }
    const dels: string[] = [], adds: string[] = [];
    while (i < ops.length && ops[i].op !== 'ctx') {
      (ops[i].op === 'del' ? dels : adds).push(ops[i].text);
      i++;
    }
    const paired = Math.min(dels.length, adds.length);
    for (let j = 0; j < paired; j++) rows.push({ left: { num: oldNum++, text: dels[j] }, right: { num: newNum++, text: adds[j] } });
    for (let j = paired; j < dels.length; j++) rows.push({ left: { num: oldNum++, text: dels[j] }, right: null });
    for (let j = paired; j < adds.length; j++) rows.push({ left: null, right: { num: newNum++, text: adds[j] } });
  }
  return rows;
}

/** Diff de um par old_string/new_string. Identicos -> tudo ctx (replace no-op, raro mas valido). */
export function computeEditDiff(oldText: string, newText: string): EditDiff {
  const a = splitLines(oldText);
  const b = splitLines(newText);
  const ops = a.length * b.length > MYERS_MAX_PRODUCT ? prefixSuffixFallback(a, b) : myers(a, b);
  let add = 0, del = 0;
  for (const o of ops) { if (o.op === 'add') add++; else if (o.op === 'del') del++; }
  return { ops, rows: pairRows(ops), add, del };
}

// Dois dialetos no transcript: Claude Code (Edit/MultiEdit com file_path + old_string/new_string)
// e Pi (edit com path + edits[] de oldText/newText — sempre em lista, mesmo edit unico).
function normEdit(e: unknown): { oldText: string; newText: string } | null {
  if (!e || typeof e !== 'object') return null;
  const r = e as Record<string, unknown>;
  const oldText = typeof r.old_string === 'string' ? r.old_string : typeof r.oldText === 'string' ? r.oldText : null;
  const newText = typeof r.new_string === 'string' ? r.new_string : typeof r.newText === 'string' ? r.newText : null;
  return oldText !== null && newText !== null ? { oldText, newText } : null;
}

/** Extrai a lista de edicoes do tool_input (Edit/MultiEdit do Claude, edit do Pi — case-insensitive).
 * null = shape desconhecido (provider mudou o formato) -> o card cai no <pre> cru de sempre. */
export function extractEdits(toolName: string | null | undefined, input: unknown): { oldText: string; newText: string }[] | null {
  if (!input || typeof input !== 'object') return null;
  const name = (toolName ?? '').toLowerCase();
  if (name !== 'edit' && name !== 'multiedit') return null;
  const rec = input as Record<string, unknown>;
  const single = normEdit(rec);
  if (single) return [single];
  if (Array.isArray(rec.edits)) {
    const out: { oldText: string; newText: string }[] = [];
    for (const e of rec.edits) {
      const ne = normEdit(e);
      if (!ne) return null;
      out.push(ne);
    }
    return out.length ? out : null;
  }
  return null;
}

/** file_path (Claude) ou path (Pi) do arquivo que a ferramenta toca (Edit, Read) — usado pra detectar a linguagem do highlight. */
export function extractFilePath(input: unknown): string {
  if (!input || typeof input !== 'object') return '';
  const rec = input as Record<string, unknown>;
  return String(rec.file_path ?? rec.path ?? '');
}
