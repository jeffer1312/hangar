// Aquecimento de cache NUNCA disputa o backend com o histórico da conversa.
//
// Abrir uma sessão disparava DEZ requisições no mesmo instante — histórico, cotas, comandos,
// catálogo de modelos, modos de permissão, runners, orquestração, subagentes. O backend atende
// essas praticamente em fila (cada uma segura o GIL, roda `tmux` ou lê `/proc` por sessão), então
// a única que PINTA a tela ficava atrás das outras nove.
//
// Medido em 28/08/2026 na mesma sessão e no mesmo servidor: o `/history` saía em 0,29s sozinho,
// 0,53s com o `/commands` junto, 0,43s com a política de orquestração, e 2,00s no meio do bando
// inteiro. No navegador, do clique até a conversa aparecer eram 3,2s, dos quais 2,9s eram o
// histórico esperando a vez.
//
// Os aquecimentos não somem — eles existem pra a pílula e o painel abrirem PRONTOS ao toque, e
// isso continua valendo. Eles só passam a esperar a conversa pintar, que é o que a pessoa está
// olhando: nenhum deles é para agora, todos são para o toque seguinte.
//
// Quem NÃO passa por aqui: o próprio `/history`, o SSE e qualquer coisa que a pessoa pediu com um
// clique. A fila é só pro trabalho especulativo.
//
// UM PORTÃO POR SESSÃO, e isso não é zelo: o app monta VÁRIOS `Chat` ao mesmo tempo de propósito —
// o split do desktop (`DesktopShell`, até três) e o chat do par (`PairChatModal`). Com um portão
// só pro app inteiro, o `Chat` que abrisse depois trocava a promessa por baixo do primeiro: o
// histórico do primeiro chegava e soltava o portão do SEGUNDO, deixando o Composer daquele
// esperando pra sempre (sem lista de `/`, sem catálogo de modelo) — e ainda cancelava o teto que
// cobriria isso. Achado da revisão, antes de ir pro repositório.

interface Portao {
  vez: Promise<void>;
  liberar: () => void;
  relogio: ReturnType<typeof setTimeout> | null;
  // Quem espera a vez, na ordem de chegada. Soltar o portão NÃO acorda todos de uma vez: o
  // histórico chegando disparava os onze aquecimentos no mesmo instante, e eles disputavam com
  // o SSE (o 1º evento de estado levava ~600ms atrás deles, medido 10/09/2026 no Windows, onde
  // cada um custa 200–600ms de backend). Saem um por vez, com folga entre eles.
  fila: Array<() => void>;
  aberto: boolean;
  drenando: ReturnType<typeof setTimeout> | null;
}

const portoes = new Map<string, Portao>();

// Depois do histórico: folga pro SSE abrir e o 1º estado chegar antes do 1º aquecimento, e o
// espaço entre um aquecimento e o seguinte.
const ATRASO_INICIAL_MS = 700;
const ESPACO_MS = 300;

function drenar(sessao: string, p: Portao): void {
  p.drenando = null;
  const proximo = p.fila.shift();
  if (proximo) {
    proximo();
    p.drenando = setTimeout(() => drenar(sessao, p), ESPACO_MS);
    return;
  }
  // Fila vazia: portão some, e quem chegar depois corre na hora (chat já carregado).
  if (portoes.get(sessao) === p) portoes.delete(sessao);
}

// Teto de segurança. Um histórico que nunca resolve (erro de rede, sessão Kimi antes do 1º
// prompt) não pode deixar o aquecimento preso pra sempre — o popover abriria em "Carregando…"
// eternamente, e uma trava calada é pior que a disputa que este arquivo veio resolver.
const TETO_MS = 6000;

/** Chat: sessão abrindo. Segura o trabalho especulativo DELA até o histórico chegar na tela. */
export function segurarAquecimento(sessao: string): void {
  // Mesma sessão remontando ({#key} na troca de aba): o portão anterior dela sai do caminho, senão
  // quem ficou esperando nele nunca mais é acordado — o `Chat` que o soltaria já morreu.
  soltarAquecimento(sessao, true);
  let liberar: () => void = () => {};
  const vez = new Promise<void>((resolve) => { liberar = resolve; });
  const relogio = setTimeout(() => {
    // Aparece: o teto disparar quer dizer que o histórico não chegou em 6s, e o aquecimento
    // atrasado que vem depois é sintoma disso, não do aquecimento.
    console.warn(`[hangar] histórico de "${sessao}" não chegou em ${TETO_MS}ms; soltando o aquecimento`);
    soltarAquecimento(sessao);
  }, TETO_MS);
  portoes.set(sessao, { vez, liberar, relogio, fila: [], aberto: false, drenando: null });
}

/** Chat: histórico na tela (ou desistiu dele). Pode aquecer. Idempotente.
 *  Quem espera sai um por vez (ver `drenar`); `imediato` acorda todos de uma vez — é o caso do
 *  portão substituído por uma remontagem, cujos esperadores pertencem a um Chat que já morreu. */
export function soltarAquecimento(sessao: string, imediato = false): void {
  const p = portoes.get(sessao);
  if (!p) return;
  if (p.relogio) { clearTimeout(p.relogio); p.relogio = null; }
  if (p.aberto && !imediato) return;
  p.aberto = true;
  p.liberar();
  if (imediato) {
    if (p.drenando) clearTimeout(p.drenando);
    portoes.delete(sessao);
    for (const acorda of p.fila.splice(0)) acorda();
    return;
  }
  if (!p.drenando) p.drenando = setTimeout(() => drenar(sessao, p), ATRASO_INICIAL_MS);
}

/** Quem aquece espera aqui antes de tocar no backend. Sessão sem portão (Quadro, Canvas, chat já
 *  carregado) não espera nada; com portão, espera a vez na fila. */
export function aoAquecer(sessao: string): Promise<void> {
  const p = portoes.get(sessao);
  if (!p) return Promise.resolve();
  return new Promise<void>((resolve) => { p.fila.push(resolve); });
}
