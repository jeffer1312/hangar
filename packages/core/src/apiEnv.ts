export interface EventSourceLike {
  addEventListener(type: string, fn: (ev: { data: string; lastEventId?: string }) => void): void;
  removeEventListener(type: string, fn: (ev: { data: string; lastEventId?: string }) => void): void;
  close(): void;
  onerror: ((ev: unknown) => void) | null;
  onopen: ((ev: unknown) => void) | null;
  readyState: number;
}
export interface ApiEnv {
  getBaseUrl(): string;
  getToken(): string | null;
  onUnauthorized(): void;
  /** Avisos de uma sessão já criada: exibir sem tratar a criação como falha. */
  onSessionWarnings?(warnings: string[]): void;
  origin: string | null;
  createEventSource(url: string, opts: { withCredentials: boolean; headers?: Record<string, string> }): EventSourceLike;
  /** O servidor ATIVO é de convite (token de convidado). Ausente = nunca. */
  isInvite?(): boolean;
  /** Um servidor de convite respondeu 410: o dono revogou ou a sessão acabou. */
  onInviteEnded?(serverId: string | null): void;
  /** Guarda o endereço da rede local aprendido para um servidor. Ausente = só em memória. */
  rememberLan?(serverId: string, lan: import('./servers').LanInfo, source?: import('./servers').Server): void;
}
let _env: ApiEnv | null = null;
export function configureApi(env: ApiEnv): void { _env = env; }
export function apiEnv(): ApiEnv {
  if (!_env) throw new Error('@hangar/core: chame configureApi() antes de usar a API');
  return _env;
}
// só para testes — permite isolar o "sem configurar"
export function _resetApiEnvForTests(): void { _env = null; }
