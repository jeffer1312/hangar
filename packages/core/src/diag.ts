// Diário de uso — o pedaço que o `apiFetch` precisa, e só ele.
//
// O diário de verdade (fila, envio em lote, captura de erro de JS, plataforma) mora no hospedeiro:
// `frontend/src/lib/diag.ts` depende de `window`, `navigator` e do `auth` da web, e o core roda
// também fora do navegador. Aqui fica apenas a TOMADA: quem tiver um diário se registra, e quem não
// tiver segue sem nenhum — o core nunca pode exigir um.
//
// Mesma regra do outro lado, e ela é a razão do arquivo existir: **nunca entra conteúdo de conversa
// aqui**. Entra o verbo e o desfecho.

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
  pilha?: string;
  operacao?: string;
  tentativa?: number;
  espera_ms?: number;
  quantidade?: number;
}

export interface DiagSink {
  registrar(ev: Evento, destino?: string): void;
  /** Id curto do pedido. Vem do hospedeiro pra os ids não divergirem entre os dois lados. */
  novoReq(): string;
}

let _sink: DiagSink | null = null;
export function configureDiag(sink: DiagSink): void { _sink = sink; }

/** Sem diário registrado, some — o core não depende de haver um. */
export function registrar(ev: Evento, destino?: string): void {
  try { _sink?.registrar(ev, destino); } catch { /* Diário não interrompe a operação. */ }
}

/** Sem diário registrado, devolve '' — o cabeçalho `X-Hangar-Req` sai vazio e nada quebra. */
export function novoReq(): string {
  try { return _sink?.novoReq() ?? ''; } catch { return ''; }
}

/** Uma falha recente contra um servidor, no formato que o relatório de diagnóstico leva. */
export interface RecentFailure {
  /** Segundos desde a época. */
  ts: number;
  /** `METHOD /rota` com os segmentos variáveis trocados por `*`. */
  route: string;
  /** 0 = sem resposta (rede, prazo). */
  status: number;
  code: string | null;
}

const FAILURES_PER_SERVER = 50;
const FAILURE_CODE = /^[a-z0-9_.:-]{1,64}$/;
// Só em memória: o relatório leva o que esta aba viu, sem gravar nada no aparelho.
const _failures = new Map<string, RecentFailure[]>();

export function recordFailure(serverKey: string, failure: RecentFailure): void {
  const code = failure.code && FAILURE_CODE.test(failure.code) ? failure.code : null;
  const list = _failures.get(serverKey) ?? [];
  list.unshift({ ...failure, code });
  if (list.length > FAILURES_PER_SERVER) list.length = FAILURES_PER_SERVER;
  _failures.set(serverKey, list);
}

/** Mais recentes primeiro, até 50. Várias chaves: o mesmo servidor alcançado por mais de um endereço. */
export function recentFailures(...serverKeys: string[]): RecentFailure[] {
  const keys = [...new Set(serverKeys.filter(Boolean))];
  return keys.flatMap((k) => _failures.get(k) ?? [])
    .sort((a, b) => b.ts - a.ts)
    .slice(0, FAILURES_PER_SERVER);
}

// só para testes — permite isolar o "sem configurar"
export function _resetDiagForTests(): void { _sink = null; _failures.clear(); }
