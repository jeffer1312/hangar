import { describe, it, expect } from 'vitest';
import { pseudoCaminhoPorConteudo as det } from './detectarLinguagem';

// Todas as amostras abaixo são trechos REAIS deste repo ou saídas reais desta sessão.

describe('acerta o que é claramente de uma linguagem', () => {
  it('TypeScript', () => {
    expect(det(`export interface Task {
  id: string;
  subject: string;
  description: string;
  activeForm: string;
  status: TaskStatus;
}`)).toBe('saida.ts');
  });

  it('Python', () => {
    expect(det(`def _write_marker(base: str, subdir: str, key: str, payload: dict) -> None:
    d = os.path.join(base, subdir)
    os.makedirs(d, exist_ok=True)
    tmp = os.path.join(d, key + ".json.tmp")
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(payload, fh)
    os.replace(tmp, os.path.join(d, key + ".json"))`)).toBe('saida.py');
  });

  it('CSS', () => {
    expect(det(`.tc-row {
  display: flex;
  align-items: center;
  gap: 8px;
  border-radius: 8px;
  background: var(--fill-subtle);
}`)).toBe('saida.css');
  });

  it('JSON confirmado por parse, não por chute', () => {
    expect(det(`{
  "name": "frontend",
  "version": "0.0.0",
  "dependencies": {
    "uplot": "^1.6.32",
    "shiki": "^4.3.1"
  }
}`)).toBe('saida.json');
  });

  it('shebang é prova', () => {
    expect(det(`#!/usr/bin/env python3
x = 1
y = 2
z = 3
w = 4
v = 5`)).toBe('saida.py');
  });
});

describe('enxerga através do prefixo que a ferramenta põe', () => {
  it('saída de grep -n', () => {
    expect(det(`12:.tc-row {
13:  display: flex;
14:  align-items: center;
15:  gap: 8px;
16:  border-radius: 8px;
17:  background: var(--fill-subtle);
18:}`)).toBe('saida.css');
  });

  it('saída de cat -n (número + tab)', () => {
    expect(det(`1\tdef churn(x):
2\t    y = x + 1
3\t    for i in y:
4\t        print(i)
5\t    return y
6\tclass A:`)).toBe('saida.py');
  });
});

describe('na dúvida devolve null — erra pra menos', () => {
  it('texto curto demais', () => {
    expect(det('const a = 1;\nconst b = 2;')).toBeNull();
  });

  it('prosa não vira código', () => {
    expect(det(`Rodei a suíte inteira e passou.
O relatório está no diretório de sempre.
Amanhã eu vejo o resto com calma.
Nada disso muda o que já foi combinado.
Confere aí quando puder.
Depois me diz o que achou.`)).toBeNull();
  });

  it('saída de comando comum não vira código', () => {
    expect(det(`total 2596
drwxr-xr-x 1 jefferson jefferson  2852 ago 11 13:22 .
drwxr-xr-x 1 jefferson jefferson   470 ago 11 09:12 ..
-rw-r--r-- 1 jefferson jefferson    45 ago 11 14:21 marcador.json
-rw-r--r-- 1 jefferson jefferson   128 ago 11 14:20 outro.txt
-rw-r--r-- 1 jefferson jefferson  1108 ago 11 14:19 mais.bin`)).toBeNull();
  });

  it('JSON malformado NÃO vira json', () => {
    // Começa e termina como objeto, mas não parseia — o `try` existe pra isto.
    expect(det(`{
  "a": 1,
  "b": 2,
  "c": [1,2,3
  "d": 4,
  "e": 5,
}`)).not.toBe('saida.json');
  });

  it('vazio e nulo', () => {
    expect(det('')).toBeNull();
    expect(det(null)).toBeNull();
    expect(det(undefined)).toBeNull();
  });
});

describe('gutter de número + espaços (grep filtrado deste ambiente)', () => {
  it('enxerga através dele', () => {
    expect(det(`17  // espremido entre sidebar e painel de contexto
61  // Numeros de linha sao monotonicos
153  .ed { display: flex; flex-direction: column; }
162    max-width: 100%;
172  .ed-chip-file { min-width: 0; overflow: hidden; }
178    display: flex; min-width: 0;`)).toBe('saida.css');
  });

  it('mas não come "1 item" de lista em prosa (um espaço só)', () => {
    expect(det(`1 primeiro item da lista
2 segundo item da lista
3 terceiro item da lista
4 quarto item da lista
5 quinto item da lista
6 sexto item da lista`)).toBeNull();
  });
});
