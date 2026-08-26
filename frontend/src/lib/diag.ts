// Diário de uso — lado da tela. O par de backend/app/diag.py, onde está a explicação inteira.
//
// Existe porque o defeito acontece na máquina de quem USA, e de lá não chegava nada: o backend
// escreve no journal (que só existe no Linux) e o navegador não escrevia em lugar nenhum. Os três
// defeitos de 25/08/2026 vieram por print e vídeo no Slack, e o que faltou pra achar cada um foi a
// mesma coisa — qual tela, qual sessão, o que a pessoa tocou, e o que voltou.
//
// **Nunca entra conteúdo de conversa aqui.** Nem o texto enviado, nem a resposta do agente, nem
// chave, nem caminho de arquivo do projeto. Entra o VERBO e o DESFECHO. O backend descarta campo
// que não conheça, mas a trava de verdade é esta: não chame `registrar` com texto de ninguém.
import { getBaseUrl, getToken } from './auth';

export type Nivel = 'ok' | 'aviso' | 'erro';

export interface Evento {
  evento: string;
  nivel?: Nivel;
  tela?: string;
  sessao?: string;
  provider?: string;
  codigo?: string;
  detalhe?: string;
  ms?: number;
  /** Id do pedido HTTP — o MESMO valor aparece na linha que o servidor gravou. */
  req?: string;
  /** Primeiras molduras do stack, em erro de JS. */
  pilha?: string;
  // Plataforma: só no `app.abriu`, uma vez por carga de página (ver CLI abaixo).
  so?: string;
  navegador?: string;
  versao?: string;
  vista?: 'desktop' | 'celular';
  tela_px?: string;
}

// Id curto desta aba. Amarra cada linha ao `app.abriu` que carrega a plataforma — mandar sistema,
// navegador e resolução em TODA linha multiplicaria o arquivo por nada.
const CLI = Math.random().toString(36).slice(2, 8);

// Contador crescente desta aba. O `ts` tem milissegundos, mas duas linhas caem no mesmo ms com
// facilidade (o lote é enviado junto) — e ordem errada num diário é como reconstruir causa e efeito
// ao contrário. `seq` desempata sem depender do relógio.
let seq = 0;

/** Id curto de um pedido HTTP. Vai no cabeçalho `X-Hangar-Req` e na linha do diário dos DOIS lados. */
export function novoReq(): string {
  return `${CLI}-${(++seq).toString(36)}`;
}

// Última tela vista. Carimbada em TODA linha que não trouxer uma própria: sem isso um `js.erro` ou
// uma ação que falhou diziam o quê e não ONDE, e "onde a pessoa estava" é metade da análise —
// cruzar o horário do erro com o `tela.ver` anterior era trabalho manual em cima do arquivo.
let telaCorrente = '';

const FILA: Record<string, unknown>[] = [];
const TETO_FILA = 200;        // pico de erro em laço não pode virar consumo de memória
const ESPERA_MS = 4000;       // agrupa; o diário não é tempo real
let timer: ReturnType<typeof setTimeout> | undefined;
let ligado = false;

/** Sistema operacional legível a partir do user-agent. Suficiente pra separar os casos reais. */
function detectarSO(ua: string): string {
  if (/Android/i.test(ua)) return 'Android';
  if (/iPhone|iPad|iPod/i.test(ua)) return 'iOS';
  if (/Windows/i.test(ua)) return 'Windows';
  if (/Mac OS X|Macintosh/i.test(ua)) return 'macOS';
  if (/Linux|X11/i.test(ua)) return 'Linux';
  return 'desconhecido';
}

/** Navegador + versão maior. Electron primeiro: o user-agent dele também casa Chrome. */
export function detectarNavegador(ua: string): string {
  const ordem: [string, RegExp][] = [
    ['Electron', /Electron\/(\d+)/],
    ['Edge', /Edg\/(\d+)/],
    ['Opera', /OPR\/(\d+)/],
    ['Firefox', /Firefox\/(\d+)/],
    ['Samsung', /SamsungBrowser\/(\d+)/],
    ['Chrome', /Chrome\/(\d+)/],
    ['Safari', /Version\/(\d+).*Safari/],
  ];
  for (const [nome, re] of ordem) {
    const m = ua.match(re);
    if (m) return `${nome} ${m[1]}`;
  }
  return 'desconhecido';
}

export { detectarSO };

/**
 * Erro que o NAVEGADOR emite e que não é defeito do app.
 *
 * O `ResizeObserver loop` é o caso: o Chrome dispara isso quando um callback de resize muda o
 * layout e provoca outra passada — o próprio spec diz que é benigno, e o app usa ResizeObserver em
 * vários lugares (terminal, canvas, composer). Na primeira meia hora de diário ele já era metade
 * das linhas de erro, empurrando pra fora da prévia o que interessa. Filtrar ruído conhecido é o
 * que mantém "erro" significando erro.
 */
export function ehRuidoDoNavegador(msg: string | undefined): boolean {
  if (!msg) return false;
  return /ResizeObserver loop/i.test(msg);
}

/**
 * As primeiras molduras do stack, numa linha só.
 *
 * A mensagem sozinha ("Cannot read properties of undefined") não localiza nada: o app tem dezenas
 * de lugares que poderiam produzi-la. O stack aponta o arquivo e a linha do bundle, e quem analisa
 * tem o repositório e o sourcemap do mesmo build (`build.sourcemap` no vite.config) pra chegar no
 * arquivo de origem. Três molduras: a primeira raramente basta (costuma ser um utilitário), e o
 * stack inteiro estoura o teto de 300 do campo.
 */
export function molduras(erro: unknown, n = 3): string | undefined {
  const stack = erro instanceof Error ? erro.stack : undefined;
  if (!stack) return undefined;
  return stack.split('\n').slice(1, 1 + n).map((l) => l.trim()).join(' <- ') || undefined;
}

async function enviar(): Promise<void> {
  timer = undefined;
  if (!FILA.length) return;
  const lote = FILA.splice(0, FILA.length);
  const token = getToken();
  if (!token) return;   // deslogado: o diário não é motivo pra bater numa API sem credencial
  try {
    await fetch(`${getBaseUrl()}/api/diag`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
      body: JSON.stringify({ eventos: lote }),
      keepalive: true,   // o lote do `pagehide` precisa sobreviver à página fechando
    });
  } catch {
    // Diário que estoura na cara de quem usa é pior que diário nenhum. Perder o lote é aceitável:
    // o que interessa neste arquivo é padrão ao longo de dias, não uma linha específica.
  }
}

/** Registra um evento. Nunca levanta, nunca espera. */
export function registrar(e: Evento): void {
  if (!ligado) return;
  if (FILA.length >= TETO_FILA) return;
  // Horário do EVENTO, não do envio. O lote sai até ESPERA_MS depois e o backend carimbava um
  // único instante no lote inteiro: eventos separados por segundos apareciam colados, e a ordem —
  // que é tudo quando se investiga corrida entre remontagem, recarga e reconexão — sumia do
  // arquivo (medido 26/08/2026: 4s de diferença entre o diário e o log do servidor pro MESMO
  // evento). Em UTC porque é o que o `toISOString` dá de graça; o backend converte pro fuso dele.
  FILA.push({ ts: new Date().toISOString(), tela: telaCorrente || undefined, ...e,
              nivel: e.nivel ?? 'ok', cli: CLI, seq: ++seq });
  // Erro vai na hora: se a página estiver prestes a quebrar, um lote de 4s depois não sai.
  if (e.nivel === 'erro') { clearTimeout(timer); void enviar(); return; }
  if (timer === undefined) timer = setTimeout(() => void enviar(), ESPERA_MS);
}

/**
 * Liga o diário: manda a plataforma uma vez e passa a capturar erro de JS.
 *
 * `versao`/`vista` vêm de quem chama porque só o app sabe (`__HANGAR_VERSION__` e o resultado do
 * matchMedia de 820px, que é o que separa os dois caminhos de UI que sempre divergem).
 */
export function iniciar(versao: string, vista: 'desktop' | 'celular'): void {
  if (ligado || typeof window === 'undefined') return;
  ligado = true;
  const ua = navigator.userAgent;
  registrar({
    evento: 'app.abriu',
    so: detectarSO(ua),
    navegador: detectarNavegador(ua),
    versao,
    vista,
    tela_px: `${window.screen?.width ?? 0}x${window.screen?.height ?? 0}`,
  });

  window.addEventListener('error', (ev) => {
    if (ehRuidoDoNavegador(ev.message)) return;
    registrar({ evento: 'js.erro', nivel: 'erro',
                detalhe: `${ev.message} @ ${ev.filename ?? '?'}:${ev.lineno ?? 0}:${ev.colno ?? 0}`,
                pilha: molduras(ev.error) });
  });
  window.addEventListener('unhandledrejection', (ev) => {
    const r = ev.reason;
    registrar({ evento: 'js.promessa', nivel: 'erro',
                detalhe: r instanceof Error ? `${r.name}: ${r.message}` : String(r),
                pilha: molduras(r) });
  });
  // `pagehide`, não `beforeunload`: no iOS o segundo não dispara ao trocar de app, e é justamente
  // o fim de sessão do celular que interessa registrar.
  window.addEventListener('pagehide', () => { clearTimeout(timer); void enviar(); });
}

/** Qual tela está à vista. É o "onde foi usado" — e o carimbo de ONDE de tudo que vier depois. */
export function telaAtiva(tela: string): void {
  telaCorrente = tela;
  registrar({ evento: 'tela.ver', tela });
}

/** Envolve uma ação e registra o desfecho: quanto demorou e, falhando, com qual código. */
export async function comRegistro<T>(
  evento: string, contexto: Omit<Evento, 'evento' | 'nivel' | 'ms'>, acao: () => Promise<T>,
): Promise<T> {
  const t0 = Date.now();
  try {
    const r = await acao();
    registrar({ ...contexto, evento, nivel: 'ok', ms: Date.now() - t0 });
    return r;
  } catch (e) {
    const status = (e as { status?: number })?.status;
    registrar({
      ...contexto, evento, nivel: 'erro', ms: Date.now() - t0,
      codigo: status ? String(status) : undefined,
      // A mensagem do backend é diagnóstico (código + motivo), não conteúdo de conversa. Cortada
      // curta pelo mesmo motivo que o backend corta: o arquivo tem de continuar mandável.
      detalhe: e instanceof Error ? e.message.slice(0, 200) : String(e).slice(0, 200),
    });
    throw e;
  }
}
