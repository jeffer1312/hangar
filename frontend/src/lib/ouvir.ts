import { ttsPlayer } from './ttsPlayer.svelte';
import { sintetizarTts, ttsAudioUrl } from './api';

/**
 * Toca um texto. `confirmar` e chamado quando o servidor responde 409 (acima do limite de aviso):
 * devolver true repete o pedido com confirm=true.
 *
 * O unlock roda SINCRONO aqui, antes de qualquer await — no WebKit o gesto do usuario expira quando
 * a pilha desenrola, e sintetizar leva segundos. Por isso `ouvirTexto` PRECISA ser chamada direto do
 * handler do toque, nunca de dentro de um then().
 *
 * `instrucao` (fase 2): a instrucao que JA tratou este `texto` (via ttsNarracao.pedir/Groq), ou ""
 * quando foi lido como esta. So repassada pro hash do cache do backend — nao dispara nada aqui.
 */
export function ouvirTexto(
  texto: string,
  confirmar: (msg: string) => Promise<boolean>,
  instrucao: string = '',
): void {
  // unlock('') ANTES do fail: fail() so seta error, e a TtsBar so renderiza com active=true — sem
  // o unlock, "nao ha texto" era um erro INVISIVEL (o atalho Ctrl+Shift+Espaco do Chat chama aqui
  // direto, sem o painel, e uma bolha que achata pra vazio — so anexo/midia — falhava calada).
  if (!texto) { ttsPlayer.unlock(''); ttsPlayer.fail('não há texto pra ler'); return; }
  // Guard de duplo-toque: sem ele, dois toques rapidos no mesmo trecho antes do primeiro terminar
  // pagam credito duas vezes, porque o cache so existe depois da primeira resposta.
  if (ttsPlayer.loading) return;
  ttsPlayer.unlock(texto);

  // Anotacao de retorno explicita: `pedir` chama a si mesma no braco do 409 (repete com confirm=true),
  // e sem o tipo declarado o TS nao infere o retorno de uma funcao recursiva.
  const pedir = (confirm: boolean): Promise<void> =>
    sintetizarTts({ text: texto, confirm, instruction: instrucao })
      .then((r) => { ttsPlayer.playUrl(ttsAudioUrl(r.url), r.provider); })
      .catch((e: Error & { status?: number }) => {
        if (e.status === 409) {
          // Recusar (ou window.confirm suprimido no PWA, que devolve false calado) NAO pode
          // fechar a barra em silencio — regra do projeto: falha aparece, nunca some.
          return confirmar(e.message)
            .then((ok) => {
              if (ok) return pedir(true);
              // e.message ja vem limpo (ensureOk em api.ts nao embute mais o status no texto).
              ttsPlayer.fail(`acima do limite de leitura — ${e.message}`);
            })
            .catch(() => ttsPlayer.fail('não deu pra confirmar a leitura'));
        }
        ttsPlayer.fail(e.message);
      });

  void pedir(false);
}

/**
 * Toca uma AMOSTRA já cortada (ver `cortarAmostra`) com uma voz explícita — a voz do RASCUNHO
 * escolhida na tela de Config, não a salva. Sem `confirm`: a amostra é sempre curta (bem abaixo do
 * limite de aviso), então não há susto de custo pra confirmar.
 */
export function ouvirAmostra(texto: string, voz: string): void {
  if (!texto) { ttsPlayer.fail('não há texto pra ouvir'); return; }
  if (ttsPlayer.loading) return;
  ttsPlayer.unlock(texto);
  sintetizarTts({ text: texto, voice: voz })
    .then((r) => { ttsPlayer.playUrl(ttsAudioUrl(r.url), r.provider); })
    .catch((e: Error) => { ttsPlayer.fail(e.message); });
}
