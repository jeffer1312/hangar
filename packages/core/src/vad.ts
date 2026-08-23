// Decide "a pessoa parou de falar" a partir da energia (RMS) do microfone. Sem DOM e sem relogio
// proprio — o tempo entra como argumento —, entao o vitest alimenta com os envelopes dos ditados
// reais do usuario (__fixtures__/ditado-envelopes.json). MUTA o estado que recebe.
//
// PICO COM ATAQUE LENTO E DECAIMENTO, nao janela deslizante. A primeira versao deste desenho usava
// "pico = maximo dos ultimos 3s" e morria: quando a fala termina a janela vai esvaziando; em t=2s
// ela ainda cobre 1s de fala e a condicao dispara — mas essa e uma janela de UM segundo. Perdido
// esse instante, a partir de t=3s a janela so tem silencio, o pico VIRA o piso de ruido, a razao
// sobe pra perto de 1 e `< 0,25` nunca mais e satisfeita: gravaria 3 minutos e nao enviaria nada.

export interface EstadoVad {
  /** Maior energia recente, com ataque lento e decaimento — a referencia do que e "alto" AQUI. */
  pico: number;
  /** Quando o silencio continuo comecou (ms), ou null se nao estamos em silencio. */
  silencioDesde: number | null;
  /** Timestamp do passo anterior, pra decair proporcional ao TEMPO e nao a contagem de passos. */
  ultimoTs: number | null;
}

/** Energia minima pra considerar que houve FALA e nao so ruido ambiente. Substitui a trava separada
 *  de "so arma depois de ouvir fala": com regra puramente relativa, o silencio inicial e ruido
 *  contra ruido e o resultado e oscilacao, nao decisao.
 *  PISO minimo de 0,004 por quantizacao: o detector ao vivo le `getByteTimeDomainData` (8 bits),
 *  cujo degrau ja vale ~0,002 de RMS — um piso abaixo disso e um piso que o sinal vivo nunca cruza,
 *  e `armado` ficaria verdadeiro desde o primeiro quadro em qualquer sala.
 *  Medido nos 3 ditados reais (diagnostico do Step 6): p10(ruido)/mediana(fala) ficaram em
 *  0,0005/0,0347 | 0,0005/0,0364 | 0,0012/0,0223. 0,02 (ponto de partida) ainda passava nos testes,
 *  mas ficava colado na menor mediana (0,0223) — pouca folga. Baixado pra 0,01: fica bem entre o
 *  maior p10 (0,0012) e a menor mediana (0,0223), e 2,5x acima do minimo de quantizacao (0,004). */
export const PISO_ABSOLUTO = 0.01;

/** Quanto o pico cai a cada 55ms sem fala. Medido: com 0,995 (meia-vida ~7,6s) o pico mal se move
 *  numa pausa de fala e o teste do audio real "1785615417-f9f5b3.m4a" falhava — esse ditado tem uma
 *  pausa de respiracao de 2,035s no meio (janelas 860-897, medido com o script de diagnostico), mais
 *  longa que os 2s de corte, e o pico ficava alto demais (~0,09) pro ruido de respiracao (~0,008)
 *  cruzar a fracao de silencio antes do corte — resultado: parava a gravacao no meio da frase.
 *  Com 0,98 (meia-vida ~1,9s) o pico decai rapido o bastante pra esses pios de respiracao voltarem
 *  a cruzar 25% do pico e resetar o cronometro de silencio ANTES de bater 2s — sem decair tao rapido
 *  a ponto de a fala continua (nivel constante) ou o silencio final serem lidos errado. */
export const DECAIMENTO_POR_55MS = 0.98;

/** Quanto o pico SOBE em direcao a uma energia maior, por passo. Ataque instantaneo (=1) faz um
 *  estouro (buzina, porta batendo) virar a referencia na hora — testado em vad.test.ts com um pico
 *  de RMS 1,0 no meio de uma fala real de 0,08: com ataque=1 o pico salta pra 1,0 e, mesmo com o
 *  decaimento atual (0,98), a fala normal fica abaixo de 25% dele por tempo demais — a regra encerra
 *  falso ~4,3s depois do estouro (medido). Ataque lento evita isso: com 0,08 o mesmo estouro so
 *  levanta o pico pra ~0,14 (medido), que fica abaixo do patamar necessario pra a fala real contar
 *  como silencio, e a regra nunca encerra.
 *  Mantido no ponto de partida (0,08): o grid de calibracao do Step 6 (PISO x DECAIMENTO x ATAQUE
 *  contra os 3 audios reais + os 5 casos sinteticos) achou combinacoes validas com 0,08 assim que
 *  DECAIMENTO_POR_55MS foi ajustado — nao precisou mexer aqui. */
export const ATAQUE_PICO = 0.08;

/** Silencio = energia abaixo desta fracao do pico. */
export const FRACAO_SILENCIO = 0.25;

/** Quanto tempo de silencio continuo encerra. Mantido em 2000ms — os 2000ms sao contados a partir de
 *  onde o RMS cruza o criterio de silencio, e esse ponto NAO e o fim do arquivo real: os proprios
 *  ditados ja terminam com uma cauda quase-silenciosa (medida no Step 1: 0,8s / 0,5s / 0,6s antes do
 *  ultimo byte). O cronometro arma dentro dessa cauda, entao o corte medido (Step 6) cai antes dos
 *  2000ms completos contados desde o FIM DO ARQUIVO: 1265ms/1540ms/1485ms depois do ultimo byte dos
 *  3 ditados reais — e isso e o comportamento certo (a regra ja estava "quieta" havia 0,5-0,8s antes
 *  do arquivo acabar). Continua sendo 0,4s de folga sobre a maior pausa real medida no meio de uma
 *  fala (1,6s, caso sintetico). */
export const SILENCIO_MS = 2000;

export function novoEstadoVad(): EstadoVad {
  return { pico: 0, silencioDesde: null, ultimoTs: null };
}

export function passoVad(estado: EstadoVad, rms: number, ts: number): 'continua' | 'encerra' {
  // Decaimento por TEMPO decorrido: o loop do medidor e requestAnimationFrame, que varia com a
  // carga do aparelho — contar passos faria o pico cair mais rapido num celular sobrecarregado.
  const dt = estado.ultimoTs === null ? 0 : Math.max(0, ts - estado.ultimoTs);
  estado.ultimoTs = ts;
  const decaido = estado.pico * DECAIMENTO_POR_55MS ** (dt / 55);
  // Sobe devagar, desce pelo decaimento. Um estouro isolado quase nao move o pico.
  estado.pico = rms > decaido ? decaido + (rms - decaido) * ATAQUE_PICO : decaido;

  const armado = estado.pico > PISO_ABSOLUTO;
  const quieto = armado && rms < estado.pico * FRACAO_SILENCIO;

  if (!quieto) {
    estado.silencioDesde = null;
    return 'continua';
  }
  if (estado.silencioDesde === null) {
    estado.silencioDesde = ts;
    return 'continua';
  }
  return ts - estado.silencioDesde >= SILENCIO_MS ? 'encerra' : 'continua';
}
