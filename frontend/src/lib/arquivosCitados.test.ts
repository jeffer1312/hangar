import { describe, expect, it } from 'vitest';
import { acumularCitados, estadoVazio, parseCodePaths } from './arquivosCitados';
import type { ChatEvent } from './types';

const CWD = '/home/jefferson/Projetos/hangar';
const ev = (kind: ChatEvent['kind'], extra: Partial<ChatEvent>, ts: number): ChatEvent =>
  ({ kind, id: `${kind}-${ts}`, ts, ...extra });

describe('parseCodePaths', () => {
  it('absoluto e ~ casam sem pasta; relativo exige dir/; prosa e URL não casam', () => {
    expect(parseCodePaths('leia /abs/x.py e ~/.claude/x.md')).toEqual(['/abs/x.py', '~/.claude/x.md']);
    expect(parseCodePaths('mexi em backend/app/api.py e no app.main; config.py solto')).toEqual(['backend/app/api.py']);
    expect(parseCodePaths('clone https://github.com/u/repo.git e veja https://x.io/a/b.ts')).toEqual([]);
    expect(parseCodePaths('cat src/a.ts && rm b.md; docker build -f docker/Dockerfile .')).toEqual(['src/a.ts', 'docker/Dockerfile']);
    expect(parseCodePaths('lixo em node_modules/x/y.js e .git/config.txt')).toEqual([]);
  });
});

describe('acumularCitados', () => {
  const eventos: ChatEvent[] = [
    ev('tool_use', { tool_name: 'Read', tool_input: { file_path: `${CWD}/backend/app/api.py` } }, 10),
    ev('tool_use', { tool_name: 'Edit', tool_input: { file_path: `${CWD}/backend/app/api.py`, old_string: 'a', new_string: 'b' } }, 20),
    ev('tool_use', { tool_name: 'Bash', tool_input: { command: 'cat src/a.ts && rm b.md' } }, 30),
    ev('tool_use', { tool_name: 'shell', tool_input: { command: ['bash', '-lc', 'sed -n 1p frontend/src/x.ts'] } }, 40),
    ev('user_msg', { text: 'olha ~/.claude/x.md' }, 50),
    ev('assistant_msg', { text: 'o app.main não casa, mas backend/app/api.py sim' }, 60),
  ];

  it('conta por origem, resolve relativo/fora do cwd e ordena por recência', () => {
    const st = acumularCitados(estadoVazio(), eventos, 0, CWD);
    expect(st.desde).toBe(6);
    const api = st.porCru.get(`${CWD}/backend/app/api.py`)!;
    expect(api.origens).toEqual({ Read: 1, Edit: 1 });
    expect(api.relativo).toBe('backend/app/api.py');
    expect(api.pasta).toBe('backend/app');
    expect(api.nome).toBe('api.py');
    // a citação em prosa é outra string crua (relativa) — chave separada, origem 'citado'
    expect(st.porCru.get('backend/app/api.py')!.origens).toEqual({ citado: 1 });
    expect(st.porCru.get('src/a.ts')!.origens).toEqual({ Bash: 1 });
    expect(st.porCru.get('frontend/src/x.ts')!.origens).toEqual({ tool: 1 });
    const fora = st.porCru.get('~/.claude/x.md')!;
    expect(fora.relativo).toBeNull();
    expect(fora.pasta).toBe('~/.claude');
    expect(fora.origens).toEqual({ voce: 1 });
    expect(st.porCru.has('b.md')).toBe(false); // sem pasta = prosa, não caminho
    expect(st.lista.map((c) => c.ultimoTs)).toEqual([60, 50, 40, 30, 20]);
  });

  it('é incremental: a segunda chamada só processa o que chegou depois', () => {
    const a = acumularCitados(estadoVazio(), eventos.slice(0, 2), 0, CWD);
    expect(a.desde).toBe(2);
    const b = acumularCitados(a, eventos, a.desde, CWD);
    expect(b.desde).toBe(6);
    expect(b.porCru.get(`${CWD}/backend/app/api.py`)!.origens).toEqual({ Read: 1, Edit: 1 });
    expect(a.porCru.size).toBe(1); // o estado antigo não foi mutado
    expect(b.porCru.size).toBe(5);
  });
});
