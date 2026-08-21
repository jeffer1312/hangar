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
  origin: string | null;
  createEventSource(url: string, opts: { withCredentials: boolean; headers?: Record<string, string> }): EventSourceLike;
}
let _env: ApiEnv | null = null;
export function configureApi(env: ApiEnv): void { _env = env; }
export function apiEnv(): ApiEnv {
  if (!_env) throw new Error('@hangar/core: chame configureApi() antes de usar a API');
  return _env;
}
// só para testes — permite isolar o "sem configurar"
export function _resetApiEnvForTests(): void { _env = null; }
