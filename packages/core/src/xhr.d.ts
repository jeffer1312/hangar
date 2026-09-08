// O core roda na PWA e no React Native, então o `lib` do tsconfig fica em ES2022 puro: pôr "DOM"
// ali deixaria `document`/`window` visíveis pra um código que não pode tocar neles. Só o
// `uploadFile` precisa de XMLHttpRequest — é a única API que reporta progresso de upload, e os dois
// ambientes a implementam. Declarada aqui na superfície mínima que ele usa.
interface XhrProgressoLike {
  lengthComputable: boolean;
  loaded: number;
  total: number;
}

interface XMLHttpRequestLike {
  status: number;
  statusText: string;
  responseText: string;
  timeout: number;
  upload: { onprogress: ((e: XhrProgressoLike) => void) | null };
  onload: (() => void) | null;
  onerror: (() => void) | null;
  ontimeout: (() => void) | null;
  open(metodo: string, url: string): void;
  setRequestHeader(nome: string, valor: string): void;
  send(corpo?: unknown): void;
}

declare const XMLHttpRequest: { new (): XMLHttpRequestLike };
