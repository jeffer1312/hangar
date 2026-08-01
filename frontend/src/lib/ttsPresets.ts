// Presets de instrucao da narracao guiada (fase 2 do TTS) + o predicado que os distingue de texto
// digitado. Arquivo PURO (sem $state) de proposito: vitest neste repo nao tem o plugin svelte
// cadastrado, entao um .svelte.ts com runes nao importa em teste (ReferenceError: $state is not
// defined) — extrair pra ca e o que deixa isto testavel sem montar esse ambiente so pra um teste.

export const PRESET_LER = '';

/** O PADRAO da feature: adaptar pra fala, sem resumir.
 *
 * Texto de chat lido cru soa estranho — caminho de arquivo virando ladainha de barras, identificador
 * em camelCase soletrado, sigla lida letra a letra, termo em ingles com fonetica errada no meio da
 * frase em portugues. E exatamente o que a skill `falar` do usuario ja fazia ("ADAPTE pra fala,
 * mantendo TODA a info — nao e resumo"), e o que ele pediu desde o inicio.
 *
 * O "nao e resumo" e a parte que mais importa e a mais facil de o modelo desobedecer: quem manda
 * ouvir um plano quer o plano inteiro, nao a ideia geral dele. */
export const PRESET_FALA = [
  'Reescreva o texto abaixo para ser LIDO EM VOZ ALTA em português do Brasil.',
  'NÃO é resumo: mantenha TODA a informação, todos os passos e todos os nomes.',
  'Exceção: identificador que ninguém decora de ouvido — hash de commit, id, token, caminho de URL —',
  'não deve ser soletrado; diga o papel dele ("o commit", "o identificador") ou omita se a frase',
  'continuar fazendo sentido sem ele.',
  'Adapte só a FORMA: troque caminho de arquivo pelo nome do arquivo falado, separe identificador',
  'em camelCase ou snake_case em palavras, escreva sigla e abreviação por extenso na primeira vez,',
  'aportuguese a pronúncia de termo técnico em inglês quando isso ajudar a voz, e transforme',
  'marcação e símbolo solto em pontuação que gere pausa natural.',
  'Não comente o texto, não adicione introdução nem conclusão: devolva só a versão falável.',
].join(' ');

export const PRESET_CODIGO = 'Explique a lógica do código em vez de descrevê-lo literalmente.';

/** So o texto DIGITADO pelo usuario deve virar prefill da proxima selecao; o de um atalho (preset)
 * e sempre um destes literais. */
export function ehInstrucaoDigitada(instrucao: string): boolean {
  return instrucao !== PRESET_LER && instrucao !== PRESET_CODIGO && instrucao !== PRESET_FALA;
}

/** Preset que vale quando o usuario nao escolheu nada explicitamente.
 *
 * Adaptar pra fala e o padrao SEMPRE — inclusive com bloco de codigo no alvo, porque ali o codigo ja
 * saiu trocado por marcador antes de chegar aqui e o que sobra e prosa. "Explicar o código" e
 * escolha explicita, nao padrao: quem seleciona um trecho com codigo geralmente quer ouvir o texto,
 * e so as vezes quer a explicacao da logica. */
export function presetPadrao(_temCodigo: boolean): string {
  return PRESET_FALA;
}
