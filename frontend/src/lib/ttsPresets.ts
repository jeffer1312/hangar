// Presets de instrucao da narracao guiada (fase 2 do TTS) + o predicado que os distingue de texto
// digitado. Arquivo PURO (sem $state) de proposito: vitest neste repo nao tem o plugin svelte
// cadastrado, entao um .svelte.ts com runes nao importa em teste (ReferenceError: $state is not
// defined) — extrair pra ca e o que deixa isto testavel sem montar esse ambiente so pra um teste.

export const PRESET_LER = '';
export const PRESET_CODIGO = 'Explique a lógica do código em vez de descrevê-lo literalmente.';

/** So o texto DIGITADO pelo usuario deve virar prefill da proxima selecao; o de um atalho (preset)
 * e sempre um destes dois literais. */
export function ehInstrucaoDigitada(instrucao: string): boolean {
  return instrucao !== PRESET_LER && instrucao !== PRESET_CODIGO;
}
