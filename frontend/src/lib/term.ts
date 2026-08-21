// getBaseUrl/getToken vivem em auth.ts; o api.ts so as IMPORTA (api.ts:1) e nao reexporta.
import type { Server } from './auth';
import { fetchSessionsForServer } from '@hangar/core';

// Coalescencia do resize: arrastar a borda emite dezenas de eventos, e cada um redesenha a janela do
// tmux inteira — o capture-pane concorrente devolveria quadros meio-pintados pro preview e pro
// estado. 150ms e o mesmo numero que a extensao do Pi usa.
export const RESIZE_DEBOUNCE_MS = 150;

// A sessao existe no servidor? Probe ANTES/DURANTE a abertura do socket: o backend recusa sessao
// inexistente FECHANDO ANTES do accept (termsock.py: close 1008, reason "sessao nao existe"), e esse
// fechamento chega no navegador como handshake recusado (onclose 1006, reason vazio) — o TermSocket
// nao tem como saber POR QUE caiu, e a tela so dizia "desconectado". O probe e a unica forma de
// transformar a recusa em texto legivel (Task 2, Step 6). Fala com a lista do servidor (o MESMO
// /api/sessions que a visao agregada ja consume), nao com um endpoint novo — zero backend tocado.
// I/O isolado aqui de proposito: o teste troca fetchSessionsForServer por um fake (regra do grupo).
export async function sessionExistsOnServer(s: Server, name: string): Promise<boolean> {
  const sessoes = await fetchSessionsForServer(s);
  return sessoes.some((x) => x.name === name);
}

// Endereco do terminal de UM servidor EXPLICITO, no molde de apiFetchForServer (api.ts:132) e
// fetchSessionsForServer: o painel da sessao de B tem que conectar em B, nao no servidor ATIVO — com
// sessoes homonimas nos dois, montar pelo ativo anexava silenciosamente ao terminal da homonima do
// outro servidor (o defeito que esta Task conserta). `location.origin` quando o baseUrl e VAZIO: o
// servidor da mesma origem (front servido pelo proprio backend, o PWA da VPS) guarda baseUrl vazio e
// `new WebSocket('/api/...')` sozinho levanta SyntaxError — mesmo fallback do termUrl antigo.
export function termUrlForServer(s: Server, name: string, cols: number, rows: number): string {
  const base = (s.baseUrl || location.origin).replace(/^http/, 'ws');
  const qs = new URLSearchParams({ token: s.token, cols: String(cols), rows: String(rows) });
  return `${base}/api/sessions/${encodeURIComponent(name)}/term?${qs}`;
}

export class TermSocket {
  private ws: WebSocket;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private pendente: { cols: number; rows: number } | null = null;

  // `close` recebe o MOTIVO quando o backend manda um: ele fecha com texto legivel em
  // "outra conexao assumiu" (termsock.py), e sem repassar a UI dizia so "desconectado · reconectar"
  // — indistinguivel de queda de rede. Ressalva medida: fechamento ANTES do `accept` (sessao que
  // nao existe, cols/rows invalidos) NAO chega como close frame no navegador, e sim como handshake
  // recusado (onclose 1006, reason vazio) — por isso o motivo e opcional, nunca garantido.
  constructor(url: string, private on: { data: (b: Uint8Array) => void; close: (motivo?: string) => void }) {
    this.ws = new WebSocket(url);
    this.ws.binaryType = 'arraybuffer';
    this.ws.onmessage = (e) => {
      // Quadro binario = bytes do terminal; texto = controle. Separar por TIPO de quadro evita
      // escapar bytes de controle no meio do fluxo (origem classica de bug de acento e de moldura).
      if (e.data instanceof ArrayBuffer) this.on.data(new Uint8Array(e.data));
    };
    // `e?.reason`: o CloseEvent sempre existe no navegador, mas o duble de teste chama onclose sem
    // argumento — e um `undefined.reason` aqui derrubaria o handler de fechamento inteiro.
    this.ws.onclose = (e) => this.on.close(e?.reason || undefined);
  }

  // Uint8Array<ArrayBuffer>, nao Uint8Array generico: o lib.dom.d.ts atual distingue ArrayBuffer
  // de SharedArrayBuffer no generico e WebSocket.send so aceita o primeiro.
  // readyState !== OPEN -> no-op: depois que a conexao cai (onclose ja disparou, mas o componente
  // que consome ainda nao reagiu ao `close` no mesmo tick), cada tecla digitada jogava
  // InvalidStateError direto no console.
  send(b: Uint8Array<ArrayBuffer>) { if (this.ws.readyState === WebSocket.OPEN) this.ws.send(b); }

  resize(cols: number, rows: number) {
    this.pendente = { cols, rows };
    clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      // Mesma guarda do send(): o debounce de 150ms pode vencer DEPOIS da conexao cair (tmux morreu
      // no meio do timer) -> sem o readyState, InvalidStateError.
      if (this.pendente && this.ws.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({ t: 'resize', ...this.pendente }));
      }
      this.pendente = null;
    }, RESIZE_DEBOUNCE_MS);
  }

  close() { clearTimeout(this.timer); this.ws.close(); }
}
