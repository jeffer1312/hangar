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
}

export interface DiagSink {
  registrar(ev: Evento): void;
  /** Id curto do pedido. Vem do hospedeiro pra os ids não divergirem entre os dois lados. */
  novoReq(): string;
}

let _sink: DiagSink | null = null;
export function configureDiag(sink: DiagSink): void { _sink = sink; }

/** Sem diário registrado, some — o core não depende de haver um. */
export function registrar(ev: Evento): void { _sink?.registrar(ev); }

/** Sem diário registrado, devolve '' — o cabeçalho `X-Hangar-Req` sai vazio e nada quebra. */
export function novoReq(): string { return _sink?.novoReq() ?? ''; }

// só para testes — permite isolar o "sem configurar"
export function _resetDiagForTests(): void { _sink = null; }
