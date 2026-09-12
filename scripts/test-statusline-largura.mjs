// Régua de largura do teste da statusline — INDEPENDENTE da que o omniroute-statusline.js usa pra
// decidir a quebra. Conferir a largura com a mesma função que decide onde quebrar não provaria nada:
// um erro na régua passaria nos dois lados ao mesmo tempo (foi exatamente o que aconteceu).
//
// Uso: node test-statusline-largura.mjs <arquivo com a saída da barra> <colunas>
// Imprime a largura real de cada linha que PASSOU das colunas (vazio = tudo caberia no terminal).
import { readFileSync } from 'node:fs';

// Lista fixa: os glifos que a barra usa, e quanto cada um ocupa no terminal. Escrita à mão de
// propósito — é o oráculo. ⚡ ⏳ ⏱ ♻ ⚠ são de UMA unidade de código UTF-16 e DUAS colunas, que é
// a armadilha inteira.
const LARGOS = ['🤖', '📁', '📟', '💬', '💵', '⚡', '📅', '🕐', '⏱', '♻', '👤', '🤝', '⏳', '⚠'];
const INVISIVEIS = ['️', '‍'];   // seletor de variação e ZWJ não ocupam coluna

const ANSI = /\x1b\[[0-9;:?]*[ -/]*[@-~]/g;

export function larguraVisivel(linha) {
    let n = 0;
    for (const g of linha.replace(ANSI, '')) {
        if (LARGOS.includes(g)) n += 2;
        else if (!INVISIVEIS.includes(g)) n += 1;
    }
    return n;
}

// Só age como comando quando recebe argumento: quem importa a régua (uma sonda, outro teste) não
// pode disparar leitura de arquivo no import.
const [arquivo, colunas] = process.argv.slice(2);
if (arquivo) {
    const cols = Number(colunas);
    const estouraram = readFileSync(arquivo, 'utf8')
        .split('\n')
        .map(larguraVisivel)
        .filter((n) => n > cols);
    process.stdout.write(estouraram.join(','));
}
