import { describe, it, expect } from 'vitest';
import { caminhoDeCodigoNoComando as alvo } from './codeFromBash';

describe('caminhoDeCodigoNoComando', () => {
  it('pega o arquivo dos comandos que despejam conteúdo', () => {
    // Todos estes rodaram de verdade nesta sessão.
    expect(alvo("sed -n '196,240p' frontend/src/components/EditDiff.svelte"))
      .toBe('frontend/src/components/EditDiff.svelte');
    expect(alvo('cat backend/app/pqueue.py')).toBe('backend/app/pqueue.py');
    expect(alvo('head -20 src/lib/tasks.ts')).toBe('src/lib/tasks.ts');
    expect(alvo('grep -n "kind" lib/types.ts')).toBe('lib/types.ts');
    expect(alvo('/usr/bin/cat a/b/c.rs')).toBe('a/b/c.rs');   // caminho absoluto do binário
  });

  it('a faixa do sed não vira caminho', () => {
    // "'196,240p'" tem ponto? não — mas "'1.5p'" teria. Aspas simples ficam de fora de qualquer jeito.
    expect(alvo("sed -n '1.5p' arquivo.ts")).toBe('arquivo.ts');
  });

  it('recusa comando que só MENCIONA o arquivo', () => {
    // O que sai de um script é o que ele imprime, não o código dele.
    expect(alvo('python3 scripts/gen.py')).toBeNull();
    expect(alvo('node build.js')).toBeNull();
    expect(alvo('npx vitest run src/lib/tasks.test.ts')).toBeNull();
  });

  it('recusa comando composto — a saída deixou de ser o arquivo', () => {
    expect(alvo('cat a.ts | head -5')).toBeNull();
    expect(alvo('cat a.ts && cat b.ts')).toBeNull();
    expect(alvo('cat a.ts > /tmp/x')).toBeNull();
    expect(alvo('cat $(ls *.ts)')).toBeNull();
  });

  it('recusa dois arquivos: a saída concatenada não é nenhum dos dois', () => {
    expect(alvo('cat a.ts b.py')).toBeNull();
  });

  it('recusa extensão que não é código', () => {
    expect(alvo('cat saida.log')).toBeNull();
    expect(alvo('head -c 60 raw-0.txt')).toBeNull();
    expect(alvo('cat dados.csv')).toBeNull();
  });

  it('recusa comando sem arquivo nenhum', () => {
    expect(alvo('ls -la')).toBeNull();
    expect(alvo('git status')).toBeNull();
    expect(alvo('')).toBeNull();
    expect(alvo(null)).toBeNull();
  });
});

describe('flags que mudam a natureza da saída (achado da revisão pré-push)', () => {
  it('grep que não imprime conteúdo', () => {
    expect(alvo('grep -c foo x.ts')).toBeNull();      // conta
    expect(alvo('grep -l foo x.ts')).toBeNull();      // só o nome do arquivo
    expect(alvo('grep -q foo x.ts')).toBeNull();      // nada
    expect(alvo('grep -rn foo x.ts')).toBe('x.ts');   // este imprime, continua valendo
  });

  it('sed que edita no lugar não imprime nada', () => {
    expect(alvo("sed -i 's/a/b/' x.ts")).toBeNull();
    expect(alvo("sed -n '1,20p' x.ts")).toBe('x.ts');
  });

  it('nl saiu da lista: numera com padding e o gutter não reconhece', () => {
    expect(alvo('nl x.py')).toBeNull();
  });
});
