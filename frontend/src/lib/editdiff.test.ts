import { describe, it, expect } from 'vitest';
import { computeEditDiff, extractEdits, extractEditPath, pairRows } from './editdiff';

describe('computeEditDiff', () => {
  it('troca simples de 1 linha vira par alterado', () => {
    const d = computeEditDiff('envFrom:\n  - secretRef:', 'env:\n  - name: X');
    expect(d.del).toBe(2);
    expect(d.add).toBe(2);
    expect(d.rows).toEqual([
      { left: { num: 1, text: 'envFrom:' }, right: { num: 1, text: 'env:' } },
      { left: { num: 2, text: '  - secretRef:' }, right: { num: 2, text: '  - name: X' } },
    ]);
  });

  it('insercao pura: so lado direito, esquerdo vazio', () => {
    const d = computeEditDiff('a\nb', 'a\nx\ny\nb');
    expect(d.add).toBe(2);
    expect(d.del).toBe(0);
    expect(d.rows).toEqual([
      { left: { num: 1, text: 'a' }, right: { num: 1, text: 'a' } },
      { left: null, right: { num: 2, text: 'x' } },
      { left: null, right: { num: 3, text: 'y' } },
      { left: { num: 2, text: 'b' }, right: { num: 4, text: 'b' } },
    ]);
  });

  it('remocao pura: so lado esquerdo, direito vazio', () => {
    const d = computeEditDiff('a\nx\ny\nb', 'a\nb');
    expect(d.del).toBe(2);
    expect(d.add).toBe(0);
    expect(d.rows[1]).toEqual({ left: { num: 2, text: 'x' }, right: null });
    expect(d.rows[2]).toEqual({ left: { num: 3, text: 'y' }, right: null });
  });

  it('bloco trocado com sobra: pareia pelo indice e o resto fica sem par', () => {
    // 2 linhas saem, 4 entram (o caso do print: envFrom -> env)
    const d = computeEditDiff('old1\nold2', 'new1\nnew2\nnew3\nnew4');
    expect(d.rows).toEqual([
      { left: { num: 1, text: 'old1' }, right: { num: 1, text: 'new1' } },
      { left: { num: 2, text: 'old2' }, right: { num: 2, text: 'new2' } },
      { left: null, right: { num: 3, text: 'new3' } },
      { left: null, right: { num: 4, text: 'new4' } },
    ]);
  });

  it('old vazio (arquivo/trecho novo): tudo adicionado', () => {
    const d = computeEditDiff('', 'linha1\nlinha2');
    expect(d.del).toBe(0);
    expect(d.add).toBe(2);
    expect(d.rows.every((r) => r.left === null)).toBe(true);
  });

  it('identicos: tudo contexto, sem add/del', () => {
    const d = computeEditDiff('a\nb', 'a\nb');
    expect(d.add).toBe(0);
    expect(d.del).toBe(0);
    expect(d.rows).toHaveLength(2);
  });

  it('trailing newline nao vira linha fantasma', () => {
    const d = computeEditDiff('a\n', 'a\nb\n');
    expect(d.add).toBe(1);
    expect(d.rows).toHaveLength(2);
  });

  it('linhas movidas voltam como del+add (diff valido, nao necessariamente "move")', () => {
    const d = computeEditDiff('a\nb\nc', 'b\na\nc');
    // toda sequencia del/add fecha: reconstruir old e new a partir das ops
    const oldLines = d.ops.filter((o) => o.op !== 'add').map((o) => o.text);
    const newLines = d.ops.filter((o) => o.op !== 'del').map((o) => o.text);
    expect(oldLines).toEqual(['a', 'b', 'c']);
    expect(newLines).toEqual(['b', 'a', 'c']);
  });

  it('grande demais pro Myers cai no fallback prefixo/sufixo sem travar', () => {
    const n = 1200; // 1200*1200 > MYERS_MAX_PRODUCT
    const a = Array.from({ length: n }, (_, i) => `a${i}`);
    const b = Array.from({ length: n }, (_, i) => `b${i}`);
    a[0] = b[0] = 'igual';
    const d = computeEditDiff(a.join('\n'), b.join('\n'));
    expect(d.rows[0]).toEqual({ left: { num: 1, text: 'igual' }, right: { num: 1, text: 'igual' } });
    expect(d.del).toBe(n - 1);
    expect(d.add).toBe(n - 1);
  });
});

describe('pairRows', () => {
  it('ops vazias -> sem linhas', () => {
    expect(pairRows([])).toEqual([]);
  });

  it('del nao adjacente a add nao pareia', () => {
    const rows = pairRows([
      { op: 'del', text: 'x' },
      { op: 'ctx', text: 'meio' },
      { op: 'add', text: 'y' },
    ]);
    expect(rows).toEqual([
      { left: { num: 1, text: 'x' }, right: null },
      { left: { num: 2, text: 'meio' }, right: { num: 1, text: 'meio' } },
      { left: null, right: { num: 2, text: 'y' } },
    ]);
  });
});

describe('extractEdits', () => {
  it('Edit: extrai old/new do input', () => {
    const edits = extractEdits('Edit', { file_path: '/x', old_string: 'a', new_string: 'b' });
    expect(edits).toEqual([{ oldText: 'a', newText: 'b' }]);
  });

  it('MultiEdit: extrai a lista edits[]', () => {
    const edits = extractEdits('MultiEdit', {
      file_path: '/x',
      edits: [
        { old_string: 'a', new_string: 'b' },
        { old_string: 'c', new_string: 'd', replace_all: true },
      ],
    });
    expect(edits).toHaveLength(2);
    expect(edits?.[1]).toEqual({ oldText: 'c', newText: 'd' });
  });

  it('shape desconhecido -> null (card cai no pre cru)', () => {
    expect(extractEdits('Edit', { file_path: '/x' })).toBeNull();
    expect(extractEdits('MultiEdit', { edits: [{ old_string: 1 }] })).toBeNull();
    expect(extractEdits('MultiEdit', { edits: [] })).toBeNull();
    expect(extractEdits('Bash', { command: 'ls' })).toBeNull();
    expect(extractEdits('Edit', null)).toBeNull();
  });

  it('Pi (edit minusculo): edits[] com oldText/newText + path', () => {
    const edits = extractEdits('edit', {
      path: '/x.ts',
      edits: [{ oldText: 'a', newText: 'b' }, { oldText: 'c', newText: 'd' }],
    });
    expect(edits).toEqual([
      { oldText: 'a', newText: 'b' },
      { oldText: 'c', newText: 'd' },
    ]);
  });

  it('nome case-insensitive e misto de dialetos', () => {
    expect(extractEdits('EDIT', { oldText: 'a', newText: 'b' })).toEqual([{ oldText: 'a', newText: 'b' }]);
    expect(extractEdits('multiedit', { edits: [{ oldText: 'a', newText: 'b' }] })).toHaveLength(1);
  });

  it('extractEditPath: file_path (Claude) ou path (Pi)', () => {
    expect(extractEditPath({ file_path: '/a.ts' })).toBe('/a.ts');
    expect(extractEditPath({ path: '/b.ts' })).toBe('/b.ts');
    expect(extractEditPath({})).toBe('');
    expect(extractEditPath(null)).toBe('');
  });
});
