import { narrarSelecao } from './api';
import { ehInstrucaoDigitada } from './ttsPresets';

// Narracao guiada (fase 2 do TTS): pede pra Groq tratar o texto falavel de uma selecao ANTES de
// virar audio, e guarda o resultado pra revisao — requisito explicito do desenho: "LLM falando
// sobre a selecao, em vez de le-la, tem que ser conferivel de olho". Modulo (nao componente), pelo
// mesmo motivo do ttsPlayer: sobrevive a remontagem do Chat/pill entre sessoes.

let carregando = $state(false);
let erro = $state('');
let textoTratado = $state('');   // '' = nada pra revisar ainda (ou ainda carregando)
let instrucaoUsada = $state(''); // a que gerou textoTratado — repassada pro hash do /api/tts
// Ultima instrucao usada, PRA PREENCHER o campo livre na proxima selecao — mesma decisao do
// ttsPlayer.ultimoTexto: so em memoria, nao persiste entre recargas.
let ultimaInstrucao = $state('');

export const ttsNarracao = {
  get carregando() { return carregando; },
  get erro() { return erro; },
  get textoTratado() { return textoTratado; },
  get instrucaoUsada() { return instrucaoUsada; },
  get ultimaInstrucao() { return ultimaInstrucao; },
  get pendente() { return textoTratado.length > 0; },

  limpar() { textoTratado = ''; erro = ''; instrucaoUsada = ''; carregando = false; },

  /**
   * "Ler como está" (instrucao vazia) devolve o texto na hora, SEM chamar a Groq — caminho comum
   * nao paga token nem latencia. Instrucao de verdade chama /api/tts/narrar e guarda o resultado
   * pra revisao (ttsNarracao.textoTratado), que so entao pode virar audio.
   */
  async pedir(texto: string, blocos: string[], instrucao: string): Promise<void> {
    // So digitada persiste (prefill da proxima selecao) — o texto de um preset (ex: "Explicar o
    // codigo") grudava aqui e reaparecia numa selecao seguinte sem codigo nenhum, gastando Groq a
    // toa e sem o usuario perceber que a instrucao vinha de um toque antigo.
    if (ehInstrucaoDigitada(instrucao)) ultimaInstrucao = instrucao;
    if (!instrucao) {
      textoTratado = texto; instrucaoUsada = ''; erro = ''; carregando = false;
      return;
    }
    carregando = true; erro = ''; textoTratado = '';
    try {
      const r = await narrarSelecao({ text: texto, code_blocks: blocos, instruction: instrucao });
      textoTratado = r.text;
      instrucaoUsada = instrucao;
    } catch (e) {
      erro = (e as Error).message;
    } finally {
      carregando = false;
    }
  },
};
