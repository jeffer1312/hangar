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
  FILA.push({ ...e, nivel: e.nivel ?? 'ok', cli: CLI });
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
    registrar({ evento: 'js.erro', nivel: 'erro',
                detalhe: `${ev.message} @ ${ev.filename ?? '?'}:${ev.lineno ?? 0}` });
  });
  window.addEventListener('unhandledrejection', (ev) => {
    const r = ev.reason;
    registrar({ evento: 'js.promessa', nivel: 'erro',
                detalhe: r instanceof Error ? `${r.name}: ${r.message}` : String(r) });
  });
  // `pagehide`, não `beforeunload`: no iOS o segundo não dispara ao trocar de app, e é justamente
  // o fim de sessão do celular que interessa registrar.
  window.addEventListener('pagehide', () => { clearTimeout(timer); void enviar(); });
}

/** Qual tela está à vista. É o "onde foi usado" — a pergunta de qual parte do app é usada. */
export function telaAtiva(tela: string): void {
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
