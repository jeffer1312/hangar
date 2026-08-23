import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

describe('FileEditor Saved feedback (BLOQUEADOR 4)', () => {
  it('não desmonta o editor no sucesso — mostra Saved por 2s', () => {
    const p = join(__dirname, 'FileEditor.tsx');
    const src = readFileSync(p, 'utf8');
    // deve ter setSalvo(true) e timeout de 2s
    expect(src).toContain('setSalvo(true)');
    expect(src).toContain('setTimeout(() => setSalvo(false), 2000)');
    // NÃO pode chamar onClose após salvar — senão desmonta antes de pintar
    // o onClose só pode aparecer na prop de Discard, não após setSalvo
    const afterSalvo = src.split('setSalvo(true)')[1] ?? '';
    expect(afterSalvo).not.toContain('onClose()');
  });

  it('files.tsx não fecha o editor automaticamente após salvar', () => {
    const p = join(__dirname, '../../../app/s/[server]/[name]/files.tsx');
    const src = readFileSync(p, 'utf8');
    // handleSalvar não pode fazer setEditando(false) no sucesso
    // se fizer, o editor desmonta e Saved nunca aparece
    const handle = src.split('const handleSalvar')[1]?.split('return r;')[0] ?? '';
    expect(handle).not.toContain('setEditando(false)');
  });

  it('FileEditor mantém interface Save/Discard', () => {
    const p = join(__dirname, 'FileEditor.tsx');
    const src = readFileSync(p, 'utf8');
    expect(src).toContain('m.arq_salvar()');
    expect(src).toContain('m.arq_descartar()');
    expect(src).toContain('m.arq_salvo()');
    expect(src).toContain('m.arq_salvando()');
  });
});
