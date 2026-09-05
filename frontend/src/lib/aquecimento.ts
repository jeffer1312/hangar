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
}

const portoes = new Map<string, Portao>();

// Teto de segurança. Um histórico que nunca resolve (erro de rede, sessão Kimi antes do 1º
// prompt) não pode deixar o aquecimento preso pra sempre — o popover abriria em "Carregando…"
// eternamente, e uma trava calada é pior que a disputa que este arquivo veio resolver.
const TETO_MS = 6000;

/** Chat: sessão abrindo. Segura o trabalho especulativo DELA até o histórico chegar na tela. */
export function segurarAquecimento(sessao: string): void {
  // Mesma sessão remontando ({#key} na troca de aba): o portão anterior dela sai do caminho, senão
  // quem ficou esperando nele nunca mais é acordado — o `Chat` que o soltaria já morreu.
  soltarAquecimento(sessao);
  let liberar: () => void = () => {};
  const vez = new Promise<void>((resolve) => { liberar = resolve; });
  const relogio = setTimeout(() => {
    // Aparece: o teto disparar quer dizer que o histórico não chegou em 6s, e o aquecimento
    // atrasado que vem depois é sintoma disso, não do aquecimento.
    console.warn(`[hangar] histórico de "${sessao}" não chegou em ${TETO_MS}ms; soltando o aquecimento`);
    soltarAquecimento(sessao);
  }, TETO_MS);
  portoes.set(sessao, { vez, liberar, relogio });
}

/** Chat: histórico na tela (ou desistiu dele). Pode aquecer. Idempotente. */
export function soltarAquecimento(sessao: string): void {
  const p = portoes.get(sessao);
  if (!p) return;
  if (p.relogio) clearTimeout(p.relogio);
  portoes.delete(sessao);
  p.liberar();
}

/** Quem aquece espera aqui antes de tocar no backend. Sessão sem portão (Quadro, Canvas, chat já
 *  carregado) não espera nada. */
export function aoAquecer(sessao: string): Promise<void> {
  return portoes.get(sessao)?.vez ?? Promise.resolve();
}
