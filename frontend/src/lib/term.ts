// getBaseUrl/getToken vivem em auth.ts; o api.ts so as IMPORTA (api.ts:1) e nao reexporta.
import { getBaseUrl, getToken } from './auth';

// Coalescencia do resize: arrastar a borda emite dezenas de eventos, e cada um redesenha a janela do
// tmux inteira — o capture-pane concorrente devolveria quadros meio-pintados pro preview e pro
// estado. 150ms e o mesmo numero que a extensao do Pi usa.
export const RESIZE_DEBOUNCE_MS = 150;

export function termUrl(name: string, cols: number, rows: number): string {
  // getBaseUrl() e VAZIO quando o front e servido da mesma origem (auth.ts:166) — o caso do PWA da
  // VPS. Sem o fallback, `new WebSocket('/api/...')` levanta SyntaxError.
  const base = (getBaseUrl() || location.origin).replace(/^http/, 'ws');
  const qs = new URLSearchParams({ token: getToken() ?? '', cols: String(cols), rows: String(rows) });
  return `${base}/api/sessions/${encodeURIComponent(name)}/term?${qs}`;
}

export class TermSocket {
  private ws: WebSocket;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private pendente: { cols: number; rows: number } | null = null;

  constructor(url: string, private on: { data: (b: Uint8Array) => void; close: () => void }) {
    this.ws = new WebSocket(url);
    this.ws.binaryType = 'arraybuffer';
    this.ws.onmessage = (e) => {
      // Quadro binario = bytes do terminal; texto = controle. Separar por TIPO de quadro evita
      // escapar bytes de controle no meio do fluxo (origem classica de bug de acento e de moldura).
      if (e.data instanceof ArrayBuffer) this.on.data(new Uint8Array(e.data));
    };
    this.ws.onclose = () => this.on.close();
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
      if (this.pendente) this.ws.send(JSON.stringify({ t: 'resize', ...this.pendente }));
      this.pendente = null;
    }, RESIZE_DEBOUNCE_MS);
  }

  close() { clearTimeout(this.timer); this.ws.close(); }
}
