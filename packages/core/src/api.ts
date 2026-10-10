import { apiEnv, type EventSourceLike } from './apiEnv';
import type { Server } from './servers';
import { baseOf } from './rota';
import type { CodexAccount, CodexIntegracaoEstado, CodexLoginAttempt, CodexResetOutcome, Credencial } from './credenciais';
import * as m from './paraglide/messages';
import { localeAtual } from './i18n';
import { basename } from './format';
import { mensagemDeErro, formataErro, type EnvelopeErro } from './errosApi';
// diag NÃO importa api (ele usa `fetch` direto) — é o que mantém esta dependência de mão única.
import { registrar as registrarDiag, novoReq } from './diag';
import { dispensaPrazo, retryAfterMs, registrarFalha, registrarSucesso } from './esfriamento';
import {
  inviteAllows, SharePrerequisiteError, tailscaleEnableUrl, type ShareCreated, type ShareInfo, type SharePrereqs,
} from './share';
import type { CotaContaResumo } from './cotaResumo';
import type { Shortcut, ProjectShortcut, ProjectShortcuts } from './shortcuts';
import type { UsoFiltros, UsoReport } from './uso';
import { isMissingRoute, type PluginControl } from './pluginUi';
import type { ConfigSyncItem, ConfigSyncManifest, ConfigSyncProgress, ConfigSyncReport } from './configSync';
import type {
  Atualizacao,
  MigrationStatus,
  SessionInfo,
  Provider,
  ChatEvent,
  CommandInfo,
  ConfigDirInfo,
  FsRoot,
  FsScanResult,
  FsEntry,
  FsScanError,
  WorkflowSummary,
  SubagentRun,
  WorkflowDetail,
  WorkflowAgentDetail,
  AnswerItem,
  CostReport,
  OrqConductor,
  OrqPanel,
  OrqExecucao,
  OrqLista,
  ResumeResult,
  RunnersResponse,
  RunInfo,
  Runner,
  SessionLimits,
  CodexModelsResponse,
  PiModelsResponse,
  LoopState,
  UploadFile,
  PlanDetail,
  TreeListing,
  FileContent,
  SearchResult,
  PathDiff,
} from './types';

export interface CodexOpcoes {
  contexto_estendido: boolean;
  codex_voice_beta: boolean;
  contexto_configurado: number | null;
  compactacao: number | null;
  modelos: { model: string; default: number; max: number }[];
}

export function codexVoiceUrlForServer(server: Server, name: string, origin: string): string {
  const base = (baseOf(server) || origin).replace(/\/$/, '').replace(/^http/, 'ws');
  return `${base}/api/sessions/${encodeURIComponent(name)}/codex/voice?${new URLSearchParams({ token: server.token })}`;
}

export async function getCodexVoicesForServer(server: Server, name: string): Promise<string[]> {
  const result = await apiFetchForServer<{ voices: string[] }>(server, `/api/sessions/${encodeURIComponent(name)}/codex/voices`);
  return result.voices;
}

export function codexOpcoes(
  alvo: Server | null,
  signal: AbortSignal,
  mudancas?: boolean | Pick<CodexOpcoes, 'contexto_estendido' | 'codex_voice_beta'>,
): Promise<CodexOpcoes> {
  const body = typeof mudancas === 'boolean' ? { contexto_estendido: mudancas } : mudancas;
  const init = {
    signal: comTeto(signal, 8000), method: mudancas === undefined ? 'GET' : 'POST',
    body: body === undefined ? undefined : JSON.stringify(body),
  };
  return alvo ? apiFetchForServer(alvo, '/api/harness/codex/opcoes', init)
              : apiFetch('/api/harness/codex/opcoes', init);
}

// Safari antigo precisa combinar o cancelamento e o prazo sem AbortSignal.any.
export function comTeto(signal: AbortSignal | undefined, ms: number): AbortSignal {
  const teto = AbortSignal.timeout(ms);
  if (!signal) return teto;
  if (typeof AbortSignal.any === 'function') return AbortSignal.any([signal, teto]);
  const juncao = new AbortController();
  for (const s of [signal, teto]) {
    if (s.aborted) juncao.abort(s.reason);
    else s.addEventListener('abort', () => juncao.abort(s.reason), { once: true });
  }
  return juncao.signal;
}

// URL da idx-ésima imagem (colada no terminal) de uma msg do transcript. `?token` porque a tag img
// não manda header Authorization e cross-origin (multi-PC) não leva cookie — o backend aceita ?token.
export function transcriptImageUrl(name: string, id: string, idx: number, server?: Server | null): string {
  const t = (server ? server.token : apiEnv().getToken()) ?? '';
  return `${server ? baseOf(server) : apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/transcript-image/${encodeURIComponent(id)}/${idx}?token=${encodeURIComponent(t)}`;
}

// URL pra servir um arquivo CITADO na conversa (video/html/pdf/img por caminho). `?token` p/ <img>/
// <video>/<iframe> (sem header). O backend so serve se o path estiver no transcript da sessao.
export function fileUrl(name: string, path: string, download = false, server?: Server | null): string {
  const t = (server ? server.token : apiEnv().getToken()) ?? '';
  return `${server ? baseOf(server) : apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/file?path=${encodeURIComponent(path)}&token=${encodeURIComponent(t)}${download ? '&download=1' : ''}`;
}

// URL nativa (sem token na query) — para WebView/Image nativo que manda Authorization header.
// PWA/browser continua no fileUrl com ?token porque <img> não manda header.
export function fileUrlNative(name: string, path: string, server?: Server): string {
  const base = server ? baseOf(server) : apiEnv().getBaseUrl();
  return `${base}/api/sessions/${encodeURIComponent(name)}/file?path=${encodeURIComponent(path)}`;
}

// Header Authorization para caminho nativo (mesmo token do fileUrl, mas só no header).
export function fileAuthHeader(server?: Server): Record<string, string> {
  return server ? { Authorization: `Bearer ${server.token}` } : authHeaders();
}

// URL de uma imagem ENVIADA do phone (upload), servida do cofre (~/.hangar/uploads/<projeto>/<sessão>/).
// `?token` igual as de cima: <img> nao manda header Authorization e cross-origin nao leva cookie.
export function uploadUrl(name: string, filename: string, download = false, server?: Server | null): string {
  const t = (server ? server.token : apiEnv().getToken()) ?? '';
  return `${server ? baseOf(server) : apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/uploads/${encodeURIComponent(filename)}?token=${encodeURIComponent(t)}${download ? '&download=1' : ''}`;
}

export function uploadUrlNative(name: string, filename: string, server?: Server): string {
  const base = server ? baseOf(server) : apiEnv().getBaseUrl();
  return `${base}/api/sessions/${encodeURIComponent(name)}/uploads/${encodeURIComponent(filename)}`;
}

// Par nativo do transcriptImageUrl: o token vai no header (fileAuthHeader), nunca na URL.
export function transcriptImageUrlNative(name: string, id: string, idx: number): string {
  return `${apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/transcript-image/${encodeURIComponent(id)}/${idx}`;
}

// URL do mp3 gerado. `?token` porque <audio> nao manda header Authorization e o front vem de outra
// origem (PWA servido pela VPS, backend no Tailscale) — cookie tambem nao viaja.
export function ttsAudioUrl(path: string): string {
  const t = apiEnv().getToken() ?? '';
  return `${apiEnv().getBaseUrl()}${path}?token=${encodeURIComponent(t)}`;
}

function authHeaders(): Record<string, string> {
  const token = apiEnv().getToken();
  if (!token) return {};
  return { Authorization: `Bearer ${token}` };
}

// Mensagem de erro legivel a partir do corpo de uma resposta !ok. FastAPI devolve {"detail": "..."}
// -> extrai a mensagem limpa em vez do JSON cru (esse texto vai direto pra UI). Fallbacks: corpo
// nao-JSON vira o texto cru; corpo vazio/ilegivel cai no res.statusText. Compartilhado pelo ensureOk
// (caminho do servidor ativo) e pelas funcoes *ForServer, pra que o MESMO 404 do backend produza a
// MESMA string nos dois caminhos.
//
// Task 10: o backend passou a poder mandar o detail como dict {code, params, msg}
// (backend/app/mensagens.py). O `code` e o contrato: mensagemDeErro traduz no idioma do app;
// codigo desconhecido (backend mais novo que o front) cai no `msg` em portugues — mostrar o codigo
// cru na tela seria pior que mostrar a lingua errada. Endpoint ainda nao migrado manda string,
// e o caminho antigo continua inteiro.
export async function errorDetail(res: Response): Promise<string> {
  return (await lerErro(res)).msg;
}

// Mensagem traduzida + o CÓDIGO cru do backend (`erro_*`). O código é o que deixa quem chama
// decidir por identidade e não por texto — a mensagem muda com o idioma.
async function lerErro(res: Response): Promise<{ msg: string; code?: string; envelope?: Record<string, unknown> }> {
  const text = await res.text().catch(() => '');
  try {
    const j = JSON.parse(text);
    if (j && typeof j.detail === 'string') return { msg: j.detail };
    const envelope = j?.detail ?? j;
    if (envelope && typeof envelope.code === 'string') {
      return { msg: formataErro(envelope)!, code: envelope.code, envelope };
    }
  } catch { /* corpo nao-JSON: cai no texto cru abaixo */ }
  // text e statusText podem os DOIS vir vazios (502 de infra sem corpo JSON, servidor HTTP/2 que
  // nao popula statusText) — sem este ultimo fallback, quem le `.message` (TtsBar, ServerSettings)
  // trata string vazia como "sem erro" e desenha a UI de sucesso por cima de uma falha real.
  return { msg: text || res.statusText || `falha ${res.status} sem detalhe do servidor` };
}

// Rotas GET que estão falhando por rede AGORA. O diário ganha uma linha na primeira falha e uma
// no retorno, não uma por poll: 4 abas × poll de 4s deram 455 linhas iguais numa tarde em que o
// backend só estava lento (máquina saturada), e afogaram o resto do dia.
const _semRede = new Set<string>();

// Trata a resposta compartilhada por apiFetch e uploadFile. Self-heal de token invalido/rotacionado:
// isAuthenticated() so checa se EXISTE token, nao se vale. Num 401 COM token salvo, limpamos a
// credencial e recarregamos -> cai no Login pra re-parear (QR). O guard apiEnv().getToken() evita loop quando
// ja estamos deslogados (Login nao chama a API). Qualquer outro !ok vira erro com o corpo.
// Com `server` explícito, o 401 só derruba a credencial ativa quando é dela (mesmo token).
async function ensureOk(res: Response, server?: Server | null): Promise<void> {
  if (res.status === 401 && ehCredencialAtiva(server)) {
    apiEnv().onUnauthorized();
    throw Object.assign(new Error(m.sessao_expirada()), { status: 401 });
  }
  // `status` no proprio erro: sem ele quem chama (ex: ouvir.ts) nao consegue distinguir um 409
  // (acima do limite de aviso, pede confirmacao) de qualquer outra falha so pela mensagem. A
  // MENSAGEM fica limpa (sem o "409: " na frente) — quem precisa do numero le `.status`, nao
  // texto que o usuario acaba vendo cru (ex: window.confirm da confirmacao de custo do TTS).
  if (!res.ok) {
    const { msg, code, envelope } = await lerErro(res);
    // `envelope` = detail cru: quem precisa de campo extra do erro (ex: `sessao`) lê daqui.
    throw Object.assign(new Error(msg), { status: res.status, code, envelope });
  }
}

// Segmentos cujo VALOR seguinte não pode ir pro diário como está.
//
// Duas razões diferentes, e a segunda é a que importa:
//  - agrupar: `/api/sessions/api-front/select` vira `/api/sessions/*/select`, e aí dá pra contar
//    "quantas vezes o /select falhou" em vez de ler uma lista de caminhos únicos;
//  - NÃO VAZAR: em `/api/archive/<projeto>` o `<projeto>` é o slug do diretório do Claude Code, ou
//    seja, o caminho real do projeto na máquina (`-home-fulano-Projetos-cliente-x`); em
//    `/api/claude-configs/<nome>` é o rótulo da conta. O diário promete, no topo do lib/diag.ts,
//    não guardar caminho de arquivo do projeto — e sem `archive` nesta lista ele guardava, em toda
//    retomada de conversa arquivada (é POST, então entra mesmo dando certo).
const SEGMENTOS_OPACOS = /\/(sessions|servers|agents|archive|claude-configs|projects|codex-contas|conta-estado)\/[^/]+/g;

export function rotaGenerica(path: string): string {
  return path.split('?')[0].replace(SEGMENTOS_OPACOS, '/$1/*');
}

function ehCredencialAtiva(server?: Server | null): boolean {
  const ativo = apiEnv().getToken();
  return !!ativo && (!server || server.token === ativo);
}

// Erro das chamadas com servidor explícito: "status: detalhe", o contrato das *ForServer. O 401 só
// derruba a credencial ativa quando é dela.
async function throwForServer(res: Response, server: Server): Promise<never> {
  if (res.status === 401 && ehCredencialAtiva(server)) apiEnv().onUnauthorized();
  throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
}

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await apiFetchRes(path, init);
  await ensureOk(res);
  return res.json() as Promise<T>;
}

// Chamada de UMA sessão. Com `server`, vai à máquina da sessão e não ao ativo do momento, que pode
// mudar sob um chat aberto; o erro sai na mesma forma do apiFetch (mensagem, status, code).
async function sessionFetch<T>(server: Server | null | undefined, path: string, init?: RequestInit): Promise<T> {
  if (!server) return apiFetch<T>(path, init);
  const res = await apiFetchRes(path, init, server);
  await ensureOk(res, server);
  return res.json() as Promise<T>;
}

// A metade de baixo do apiFetch: entrega a `Response` crua, sem `ensureOk` nem `json()`. Existe pra
// quem precisa do STATUS ou de um header — hoje o histórico condicional (304 + ETag), que não tem
// corpo pra desserializar e cujo status não é erro. O diário e o rastreio de "sem rede/voltou"
// ficam aqui: um fetch escrito à mão sairia do registro sem ninguém notar.
async function apiFetchRes(path: string, init?: RequestInit, server?: Server, probe = false): Promise<Response> {
  const destino = server?.baseUrl ?? apiEnv().getBaseUrl();
  const base = server ? baseOf(server) : destino;
  // Servidor de convite só atende a sessão compartilhada e as rotas do chat: o resto nem sai
  // daqui, senão cada tela aberta enche a rede e o diário de 403.
  const convite = server ? server.invite === true : apiEnv().isInvite?.() === true;
  if (convite && !inviteAllows(path)) {
    throw Object.assign(new Error(m.erro_fora_do_convite()), { status: 403, code: 'erro_fora_do_convite' });
  }
  if (server?.removed) {
    throw Object.assign(new Error(m.servidor_nao_existe()), { status: 410, code: 'servidor_nao_existe' });
  }
  const url = `${base}${path}`;
  const t0 = Date.now();
  // Id do pedido: vai no cabeçalho e na linha do diário dos DOIS lados, pra quem analisa seguir a
  // cadeia (o toque na tela -> o que o servidor fez) sem depender de comparar horário.
  const req = novoReq();
  // Durante a espera, só uma verificação explícita pode antecipar a nova tentativa. O ativo e a
  // máquina de um chat aberto não esperam: o prazo pode ter sido gravado antes de o chat abrir.
  if (server && retryAfterMs(server.id) > 0 && !probe && !dispensaPrazo(server.id)) {
    throw new Error(m.esfriamento_servidor_desligado({ servidor: server.label },
                                                     { locale: localeAtual() }));
  }
  let res: Response;
  try {
    res = await fetch(url, {
      // O navegador guarda o 410 do convite antigo e o devolveria ao convite novo do mesmo dono.
      ...(convite ? { cache: 'no-store' as const } : {}),
      ...init,
      headers: {
        'Content-Type': 'application/json',
        'X-Hangar-Req': req,
        ...(server ? { Authorization: `Bearer ${server.token}` } : authHeaders()),
        ...(init?.headers ?? {}),
      },
    });
  } catch (e) {
    // Rede caiu / servidor fora: nunca chegou a haver status. Distinguir isto de um 500 é metade
    // do diagnóstico de "sumiu do nada".
    //
    // CANCELAMENTO não entra: o Chat aborta o /history em voo a cada troca de sessão e a cada
    // remontagem da tela, e o `fetch` lança igual. Registrado como erro, cada navegação virava uma
    // "queda de rede" no diário — em 26/08/2026 as duas únicas do dia eram exatamente isso, e a
    // queda de verdade, se tivesse havido, estaria indistinguível no meio. `TimeoutError` (o teto
    // de tempo) segue entrando: aquilo é falha, não navegação — a mesma linha que `isAbortError`
    // já traça pro resto do app. Quem for abortar por um motivo NOVO (um teto de tempo escrito à
    // mão, por exemplo, em vez do `AbortSignal.timeout`) precisa saber disto: por este caminho a
    // falha some do diário sem deixar rastro.
    const timedOut = init?.signal?.aborted && init.signal.reason?.name === 'TimeoutError';
    if (!isAbortError(e) || timedOut) {
      const rota = `${(init?.method ?? 'GET').toUpperCase()} ${rotaGenerica(path)}`;
      const poll = rota.startsWith('GET ');
      if (!poll || !_semRede.has(`${destino}|${rota}`)) {
        registrarDiag({ evento: 'api.sem_rede', nivel: 'erro', ms: Date.now() - t0, req, detalhe: rota,
          codigo: timedOut || (e instanceof Error && e.name === 'TimeoutError') ? 'timeout' : 'rede' }, destino);
      }
      if (poll) _semRede.add(`${destino}|${rota}`);
      // Só falha de REDE esfria (o `isAbortError` acima já tirou o cancelamento de quem chamou).
      if (server) registrarFalha(server.id);
    }
    throw e;
  }
  // Respondeu — inclusive com erro HTTP: a máquina está de pé, e é isso que o esfriamento mede.
  if (server) registrarSucesso(server.id);
  // 410 e 401 encerram (o dono revogou, ou apagou o registro do convite): 401 aqui nunca é
  // "token a repareamento". O 503 (erro_sessao_indisponivel) é a trava do dono numa troca de modo
  // ou com o tmux mudo, e passa sozinho.
  if (convite && (res.status === 410 || res.status === 401)) apiEnv().onInviteEnded?.(server?.id ?? null);
  {
    const rota = `${(init?.method ?? 'GET').toUpperCase()} ${rotaGenerica(path)}`;
    if (_semRede.delete(`${destino}|${rota}`)) {
      registrarDiag({ evento: 'api.voltou', nivel: 'ok', ms: Date.now() - t0, req, detalhe: rota }, destino);
    }
  }
  // O que entra no diário, e por quê:
  //  - toda AÇÃO (POST/PUT/PATCH/DELETE), dando certo ou não. É o uso: enviar mensagem, responder
  //    opção, criar/matar sessão, trocar modelo, salvar motor. Sem o caso de SUCESSO o arquivo só
  //    responde "o que quebrou", e a pergunta era como o app está sendo usado.
  //  - toda LEITURA que FALHA. O sucesso de GET fica de fora de propósito: o poll da lista e o
  //    histórico dariam milhares de linhas por hora e afogariam o resto.
  const metodo = (init?.method ?? 'GET').toUpperCase();
  const acao = metodo !== 'GET';
  // 304 não é falha: é a resposta certa pra "o que eu tenho ainda vale". Sem esta exceção, toda
  // entrada em sessão sem novidade viraria uma linha de aviso no diário.
  if (acao || (!res.ok && res.status !== 304)) {
    // Código estável explica a recusa sem copiar texto livre do servidor; o clone preserva a resposta da tela.
    let motivo = '';
    if (!res.ok) {
      try {
        const { code } = await lerErro(res.clone());
        motivo = code && /^[a-z][a-z0-9_]{0,79}$/.test(code) ? code : '';
      } catch {
        motivo = '';
      }
    }
    registrarDiag({
      evento: acao ? 'acao' : 'leitura',
      nivel: res.ok ? 'ok' : (res.status >= 500 ? 'erro' : 'aviso'),
      codigo: String(res.status),
      ms: Date.now() - t0,
      req,
      detalhe: [`${metodo} ${rotaGenerica(path)}`, motivo].filter(Boolean).join(' — '),
    }, destino);
  }
  return res;
}

/** Verificação explícita: pode consultar um servidor offline, usando o mesmo registro de rede. */
export function probeServerResponse(server: Server, path: string, init?: RequestInit): Promise<Response> {
  return apiFetchRes(path, { signal: AbortSignal.timeout(8000), ...init }, server, true);
}

// Configurações abertas a partir da visão agregada precisam continuar no servidor capturado, sem
// trocar o servidor global. Um 401 aqui é erro local da sheet: nunca remove a credencial ativa,
// que pode pertencer a outra máquina.
// Arquivos, diff e planos da sessão: o chat sempre passa o servidor, e 8 s cortava diff e leitura
// de repositório grande. Teto ainda existe para máquina fora do ar atrás de VPN não prender a tela.
const SESSION_FILE_TIMEOUT_MS = 60_000;

// `comCodigo`: o erro também leva o `code` e o `envelope` do servidor, como o do `apiFetch`. Opcional porque
// quem já trata o erro destas chamadas pelo texto ou pelo status não muda de comportamento sem querer.
async function apiFetchForServer<T>(s: Server, path: string, init?: RequestInit, prazoMs = 8000, comCodigo = false): Promise<T> {
  let res: Response;
  // Prazo por PADRAO. Esta funcao fala com OUTRO servidor, e servidor offline atras de VPN nao
  // recusa a conexao — o socket fica pendurado e a promessa nunca resolve (o comentario do
  // getSessions ja registrava isso pro poll). Sem prazo, abrir Configuracoes de um servidor
  // desligado prendia a folha em "Carregando..." pra sempre, sem erro nenhum na tela.
  // Antes do spread do `init`: quem precisar de outro prazo (ou de nenhum) passa o proprio sinal.
  const teto = AbortSignal.timeout(prazoMs);
  try {
    res = await apiFetchRes(path, { signal: teto, ...init }, s);
  } catch (e) {
    // "signal timed out" (o texto que o navegador poe no TimeoutError) nao diz nada pra quem le a
    // tela. O WebKit ainda entrega o estouro como AbortError "Fetch is aborted": e o NOSSO teto
    // disparado que diz que foi tempo. Abort pedido POR QUEM CHAMOU continua passando cru — quem
    // cancela sabe que cancelou.
    if (e instanceof DOMException && (e.name === 'TimeoutError' || (e.name === 'AbortError' && teto.aborted))) {
      throw new Error(`${s.label} não respondeu em ${Math.round(prazoMs / 1000)}s — servidor fora do ar?`);
    }
    throw e;
  }
  if (!res.ok) {
    if (!comCodigo) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
    const { msg, code, envelope } = await lerErro(res);
    throw Object.assign(new Error(`${res.status}: ${msg}`), { status: res.status, code, envelope });
  }
  return res.json() as Promise<T>;
}

export function getSessions(server?: Server | null): Promise<SessionInfo[]> {
  if (server) return fetchSessionsForServer(server);
  // Timeout curto: este alimenta polls (ex. nav do Chat a cada 5s) — socket pendurado (tailscale
  // pra nó morto nao recusa) empilhava um fetch por tick até esgotar as 6 conexões do host.
  return apiFetch<SessionInfo[]>('/api/sessions', { signal: AbortSignal.timeout(4000) });
}

// Lista sessões de UM servidor específico (baseUrl+token explícitos), sem mexer no ativo. A visão
// agregada chama um por um e renderiza cada resposta assim que chega (sem esperar os outros), então
// um servidor lento/offline não segura os demais. Timeout de 4s: servidor morto falha rápido (< o
// intervalo de poll de 5s) em vez de pendurar no timeout default do browser.
export async function fetchSessionsForServer(s: Server): Promise<SessionInfo[]> {
  const res = await apiFetchRes('/api/sessions', {
    signal: AbortSignal.timeout(4000),
  }, s);
  if (!res.ok) throw Object.assign(new Error(`${res.status}`), { status: res.status });
  return res.json() as Promise<SessionInfo[]>;
}

// O shell criou o view do navegador embutido da sessão: o marcador 'nav' daquele servidor sai, e
// nenhuma outra conexão (celular, outra janela) o recebe de novo.
export async function confirmarNavForServer(s: Server, name: string): Promise<void> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/nav`, {
    method: 'DELETE',
    signal: AbortSignal.timeout(4000),
  }, s);
  if (!res.ok) throw new Error(`${res.status}`);
}

// Custo de UM servidor (baseUrl+token explicitos), sem mexer no ativo. Igual fetchSessionsForServer:
// a visao agregada chama todos em paralelo; um servidor lento/offline falha rapido (timeout 4s) e e
// pulado, sem segurar os demais.
// `period` OBRIGATÓRIO: o servidor ecoa em `applied` o período que aplicou, e o merge recusa
// somar quem não ecoou o pedido. Deixar o parâmetro opcional é convidar o chamador a esquecê-lo,
// receber de volta o default do backend e jogar a malha INTEIRA em `mismatched` — a tela diria
// "todos os servidores desatualizados" quando o bug é do front.
// Devolve `Partial<CostReport>` porque é isto que chega DO FIO: um servidor da malha em versão
// antiga responde sem os campos novos, e prometer o objeto completo aqui é como o front
// quebrava em runtime com o `check` verde.
// O teto NÃO é os 4s dos outros fan-outs: medido em 27/08/2026, o próprio servidor local, saudável,
// leva 12,6s neste endpoint com o cache do backend frio (0,28s quente) — ou seja, a PRIMEIRA carga
// de custos estourava sempre, e a máquina aparecia na tela como "não respondeu". Servidor offline
// não paga este tempo: conexão recusada volta em milissegundos. Quem espera são os lentos de
// verdade, e é exatamente por eles que este número existe.
// `fresco` = botão "Atualizar dados": o servidor coleta agora em vez de servir a última leitura.
// `summary` = tela inicial: só totals/by_day/by_model/sem_tarifa/applied/usd_brl, sem `combos`.
// Servidor antigo ignora o parâmetro e manda o relatório inteiro, que tem os mesmos campos.
export async function fetchCostsForServer(s: Server, period: string, fresco = false, summary = false): Promise<Partial<CostReport>> {
  const res = await apiFetchRes(`/api/costs?period=${encodeURIComponent(period)}${fresco ? '&fresco=1' : ''}${summary ? '&view=summary' : ''}`, {
    signal: AbortSignal.timeout(20000),
  }, s);
  if (res.status === 202) throw await Aquecendo.de(res);
  if (!res.ok) throw await falhaDeCustos(res);
  return res.json() as Promise<Partial<CostReport>>;
}

export interface SessionCostEstimate {
  cost_usd: number | null;
  missing_models: string[];
  has_usage: boolean;
}

export function getSessionCostForServer(s: Server, name: string, signal: AbortSignal): Promise<SessionCostEstimate> {
  return apiFetchForServer(s, `/api/sessions/${encodeURIComponent(name)}/cost`, { signal }, 25_000);
}

// Só a cotação USD/BRL do servidor ativo (cache de 1h no backend). Existe à parte do /api/costs
// porque quem mostra o custo de UMA sessão — painel, card do quadro, folha de uso — não pode
// esperar a varredura de transcript do relatório só pra saber a taxa. null = sem cotação; quem
// formata cai pro dólar em vez de converter por um número que não tem.
export async function fetchCotacao(): Promise<number | null> {
  try {
    const r = await apiFetch<{ usd_brl: number | null }>('/api/cotacao');
    return typeof r.usd_brl === 'number' ? r.usd_brl : null;
  } catch (e) {
    // Cair pro dólar é o comportamento certo, mas sem rastro nenhum não dá pra responder
    // "por que o real nunca fica disponível aqui". O backend loga a falha dele; este é o lado
    // do cliente.
    registrarDiag({ evento: 'cotacao.falhou', nivel: 'erro', detalhe: String(e) });
    return null;
  }
}

// Uso de skills/tools/hooks de UMA máquina: mesmo cache e mesmo 202 "aquecendo" do /api/costs.
export async function fetchUsoForServer(s: Server, period: string, filtros: UsoFiltros = {}, fresco = false): Promise<Partial<UsoReport>> {
  const partes = [`period=${encodeURIComponent(period)}`];
  // Filtros de lista vão repetidos (`conta=a&conta=b`): o backend lê `list[str]`.
  for (const [k, v] of Object.entries(filtros)) {
    for (const x of Array.isArray(v) ? v : v ? [v] : []) partes.push(`${k}=${encodeURIComponent(x)}`);
  }
  if (fresco) partes.push('fresco=1');
  const q = partes.join('&');
  const res = await apiFetchRes(`/api/uso?${q}`, {
    signal: AbortSignal.timeout(20000),
  }, s);
  if (res.status === 202) throw await Aquecendo.de(res);
  if (!res.ok) throw await falhaDeCustos(res);
  return res.json() as Promise<Partial<UsoReport>>;
}

// 503 do servidor de custos traz código e motivo: a frase traduzida vai no `message` e o código
// no erro, como no resto do dono único. Sem envelope, `message` continua sendo o status.
async function falhaDeCustos(res: Response): Promise<Error> {
  const { msg, code } = await lerErro(res);
  return Object.assign(new Error(code ? msg : `${res.status}`), { status: res.status, code });
}

// 202 do /api/costs e /api/uso: a primeira leitura do histórico daquela máquina ainda está rodando no
// backend (máquina nova varre 1 GB+ de transcript). Não é falha: a tela mostra o progresso e
// pergunta de novo em alguns segundos.
export class Aquecendo extends Error {
  constructor(public readonly lidos: number, public readonly total: number) {
    super(`aquecendo ${lidos}/${total}`);
    this.name = new.target.name;
  }

  static async de(res: Response): Promise<Aquecendo> {
    let j: unknown = {};
    try { j = await res.json(); } catch { /* corpo vazio ou não-JSON: progresso desconhecido */ }
    const o = (j && typeof j === 'object' ? j : {}) as { lidos?: unknown; total?: unknown };
    const n = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? v : 0);
    return new Aquecendo(n(o.lidos), n(o.total));
  }
}

// Execuções de orquestração de UM servidor (baseUrl+token explícitos), sem mexer no ativo — a tela
// Orq soma TODOS os servidores da malha, como o `sessionsStore` já faz com as sessões; buscar só do
// ativo mostraria meia verdade sem dizer que é meia. Prazo maior que o dos fan-outs de 4s porque
// aqui o servidor lê disco (um diretório por execução) em vez de responder de memória.
export function getOrqForServer(s: Server): Promise<OrqLista> {
  return apiFetchForServer<OrqLista>(s, '/api/orq');
}

export function getOrqDetalheForServer(s: Server, id: string): Promise<OrqExecucao> {
  return apiFetchForServer<OrqExecucao>(s, `/api/orq/${encodeURIComponent(id)}`);
}

export function getOrqConductorForServer(s: Server, id: string): Promise<OrqConductor> {
  return apiFetchForServer<OrqConductor>(s, `/api/orq/${encodeURIComponent(id)}/conductor`);
}

export function getOrqPanelForServer(s: Server, name: string): Promise<OrqPanel> {
  return apiFetchForServer<OrqPanel>(s, `/api/sessions/${encodeURIComponent(name)}/orq/panel`);
}

export function getOrqHistoryPanelForServer(s: Server, runId: string): Promise<OrqPanel> {
  return apiFetchForServer<OrqPanel>(s, `/api/orq/${encodeURIComponent(runId)}/panel`);
}

// Cauda do histórico de UMA sessão de um servidor específico — cards do quadro kanban.
// limit dispara o tail-read no backend (parseia só o fim do jsonl). Timeout de 8s mantido: disco
// frio + arquivo grande ainda pode passar dos 4s dos fan-outs acima.
export async function getHistoryTailForServer(s: Server, name: string, limit: number): Promise<ChatEvent[]> {
  const res = await apiFetchRes(
    `/api/sessions/${encodeURIComponent(name)}/history?limit=${limit}`,
    { signal: AbortSignal.timeout(8000) }, s,
  );
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<ChatEvent[]>;
}

// Cache de cauda pros cards do board/canvas. Vive no MÓDULO de propósito: trocar de view no
// DesktopShell desmonta board/canvas inteiros, então um cache em componente morreria junto — e é
// exatamente a troca de view que dispara a tempestade de 50 GET /history. A chave inclui STATE e
// LAST_ACTIVITY: só state deixava buraco — ciclo idle→working→idle completo dentro do TTL voltava
// pra MESMA chave e servia a cauda pré-troca como atual; last_activity (mtime do jsonl, re-emitido
// pelo stream de lista a cada mudança real) muda junto com conteúdo novo e fura o cache na volta.
// `at` volta pro chamador: o retire de eco pendente do BoardCard só pode aposentar echos
// confirmados ANTES da cauda ser buscada (não do hit) — senão msg entregue some da UI até o
// próximo fetch real. `evs` sai como CÓPIA rasa: o chamador joga o array num $state (proxy sobre a
// própria referência) — devolver o array do cache criaria aliasing e uma mutação futura no
// componente corromperia a entrada pros demais consumidores da chave.
const _tailCache = new Map<string, { at: number; evs: ChatEvent[] }>();
const _TAIL_TTL = 30_000;

export async function getHistoryTailCached(
  s: Server, name: string, limit: number, state: string, lastActivity: number | null | undefined,
): Promise<{ evs: ChatEvent[]; at: number }> {
  const key = `${s.id}::${name}::${state}::${lastActivity ?? 0}::${limit}`;
  const hit = _tailCache.get(key);
  if (hit && Date.now() - hit.at < _TAIL_TTL) return { at: hit.at, evs: [...hit.evs] };
  const entry = { at: Date.now(), evs: await getHistoryTailForServer(s, name, limit) };
  _tailCache.set(key, entry);
  if (_tailCache.size > 300) {
    for (const [k, v] of _tailCache) if (Date.now() - v.at >= _TAIL_TTL) _tailCache.delete(k);
  }
  return { at: entry.at, evs: [...entry.evs] };
}

// Upload/transcrição pra sessão de um servidor específico — o composer COMPLETO do card do
// board/canvas (mesmos endpoints/headers dos uploadFile/transcribeFile do servidor ativo; aqui
// baseUrl+token vêm do Server dono do card, que pode não ser o ativo).
export async function uploadFileForServer(
  s: Server,
  name: string,
  file: File,
  // Ditado: mesmo motivo do `audioOnly` do `uploadFile`.
  opts?: { audioOnly?: boolean },
): Promise<{ path: string }> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/upload${opts?.audioOnly ? '?audio_only=1' : ''}`, {
    method: 'POST',
    headers: {
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'arquivo'),
    },
    body: file,
    // Sem teto, uma foto grande num link ruim (tablet em relay) deixava o composer preso em
    // "enviando…" pra sempre. 3min cobre upload legítimo lento; estourou -> erro visível + retry.
    signal: AbortSignal.timeout(180_000),
  }, s);
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ path: string }>;
}

// Opções do ditado que viajam na query do /transcribe. `estilo` é o que a PILL mostrava na hora de
// falar: o backend só lê a config quando ele não vem, e sem isso uma troca feita noutra aba/aparelho
// fazia o ditado voltar em briefing com a tela dizendo "Só limpar".
export type OpcoesTranscribe = { limpar?: boolean; estilo?: string };

function queryTranscribe(opts?: OpcoesTranscribe): string {
  if (!opts?.limpar) return '';   // áudio anexado: nem limpeza, nem estilo
  return opts.estilo ? `?limpar=1&estilo=${encodeURIComponent(opts.estilo)}` : '?limpar=1';
}

export type TranscribeResult = {
  path: string; text: string; raw?: string; aviso?: string | null; estilo_aplicado?: string;
  /** Serviço da lista que transcreveu. O `aviso` já explica quando um anterior falhou. */
  provider?: string;
};

function queryArquivo(opts: OpcoesTranscribe | undefined, arquivo: string): string {
  const qs = queryTranscribe(opts);
  return `${qs ? `${qs}&` : '?'}arquivo=${encodeURIComponent(arquivo)}`;
}

// Transcreve um áudio que JÁ está no servidor, sem receber nem gravar outra cópia. `arquivo` é o
// nome solto na pasta da sessão, ou o caminho absoluto devolvido antes (depois de `/clear` o áudio
// fica na pasta do transcript anterior). O ditado sobe o áudio antes por `uploadFile*`: o caminho
// fica guardado mesmo que a transcrição nunca volte.
export async function transcribeUploadedForServer(
  s: Server,
  name: string,
  arquivo: string,
  opts?: OpcoesTranscribe,
): Promise<TranscribeResult> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/transcribe${queryArquivo(opts, arquivo)}`, {
    method: 'POST',
    // Corpo vazio: o `apiFetchRes` poria `application/json` por padrão, e não há JSON nenhum aqui.
    headers: { 'Content-Type': 'application/octet-stream' },
    signal: AbortSignal.timeout(300_000),   // mesmo teto do transcribeFile (ver o comentário lá)
  }, s);
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  return res.json() as Promise<TranscribeResult>;
}

// `limpar` monta a MESMA query do transcribeFile (?limpar=1), de propósito: é o mesmo botão de
// microfone, e ditar num card não pode devolver texto pior que ditar no chat. Só o mic manda o
// flag — áudio anexado (arquivo de até 10min) não paga a limpeza, mesma regra do backend.
// O `aviso` vem junto porque a limpeza pode desistir e devolver o cru (LLM fora do ar, resposta
// rejeitada pelas travas): quem chama tem que poder dizer isso, e não fingir que limpou.
export async function transcribeFileForServer(
  s: Server,
  name: string,
  file: File,
  opts?: OpcoesTranscribe,
): Promise<TranscribeResult> {
  const qs = queryTranscribe(opts);
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/transcribe${qs}`, {
    method: 'POST',
    headers: {
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'audio.webm'),
    },
    body: file,
    signal: AbortSignal.timeout(300_000),   // mesmo teto do transcribeFile (ver o comentário lá)
  }, s);
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<TranscribeResult>;
}

// Ditado da tela de nova conversa: a sessão ainda não existe, então o áudio só é transcrito e
// limpo (sem pasta onde guardar), com o mesmo `estilo` que a pílula mostrava.
export async function transcribeDictationForServer(
  s: Server,
  file: File,
  estilo?: string,
): Promise<{ text: string; raw?: string; aviso?: string | null }> {
  const qs = estilo ? `?estilo=${encodeURIComponent(estilo)}` : '';
  const res = await apiFetchRes(`/api/dictation/transcribe${qs}`, {
    method: 'POST',
    headers: {
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'audio.m4a'),
    },
    body: file,
    signal: AbortSignal.timeout(300_000),
  }, s);
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ text: string; raw?: string; aviso?: string | null }>;
}

// Envia prompt pra sessão de um servidor específico (input do card do quadro). 404 = sessão morta:
// o chamador REMOVE o eco pendente e sinaliza — mensagem nunca "some" calada (mesmo contrato do
// feedback de entrega do Chat). SEM timeout de propósito (igual ao sendInput por-servidor-ativo):
// abortar um POST já em voo não desfaz o envio, e reportaria "não entregue" pra recado entregue.
export async function sendInputForServer(s: Server, name: string, text: string): Promise<void> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/input`, {
    method: 'POST',
    body: JSON.stringify({ text }),
  }, s);
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
}

// Responde uma opção do picker (awaiting_input) direto do card. Mesma convenção de índice do
// selectOption por-servidor-ativo (api.ts): option é 1-BASED (1 = primeira opção) — o backend
// valida ge=1 e traduz pra (option-1)×Down + Enter no tmux.
export async function selectOptionForServer(s: Server, name: string, option: number): Promise<void> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/select`, {
    method: 'POST',
    body: JSON.stringify({ option }),
  }, s);
  // Mesmo tratamento do sendInputForServer: o erro do picker tambem e renderizado no card.
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
}

/** submitSelected no servidor da sessão, com o erro legível como o selectOptionForServer. */
export async function submitSelectedForServer(s: Server, name: string): Promise<void> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/select/submit`, { method: 'POST' }, s);
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
}

export function listClaudeConfigs(): Promise<ConfigDirInfo[]> {
  return apiFetch<ConfigDirInfo[]>('/api/claude-configs');
}
export function listClaudeConfigsForServer(server: Server): Promise<ConfigDirInfo[]> {
  return apiFetchForServer<ConfigDirInfo[]>(server, '/api/claude-configs');
}

export function getClaudeAccountSuggestion(): Promise<{ path: string }> {
  return apiFetch<{ path: string }>('/api/cotas/sugestao');
}

/** Cota de cada credencial do servidor ativo (backend/app/cotas.py). Ver `cotaResumo`. */
export function listarCotasResumo(): Promise<CotaContaResumo[]> {
  return apiFetch<CotaContaResumo[]>('/api/cotas');
}

export interface CreateSessionBody {
  name: string;
  cwd?: string | null;
  config_dir?: string | null;
  provider?: Provider;
  remember_provider?: boolean;
  engine?: string | null;
  model?: string | null;
  effort?: string | null;
  permission_mode?: string | null;
  omp_profile?: string | null;
  codex_account?: string | null;
  // Claude ou Codex sem terminal (processo gerenciado pelo Hangar, sem tmux).
  headless?: boolean;
  // CLAUDE_CODE_SUBAGENT_MODEL da sessão. Só claude na conta Anthropic (motor já define o seu).
  subagent_model?: string | null;
  // Jev no `hangar-preview objetivo`: ligado, a sessão nasce com a chave no ambiente. Escolha da
  // abertura — é assim que se roda a mesma tarefa com e sem, sem apagar a configuração.
  jev?: boolean;
  // Conta ChatGPT do CLIProxyAPI local (só Claude com motor) e Fast (`priority`) da sessão nova.
  engine_account?: string;
  service_tier?: 'default' | 'priority';
  // Branch já existente (local ou remota) em que a sessão nasce; vazio = a atual da pasta.
  branch?: string | null;
  // Com new_branch, `branch` é a branch NOVA criada a partir de `base` (vazio = a atual da pasta).
  new_branch?: boolean;
  base?: string | null;
}

export type SessionOpeningExtras = Pick<CreateSessionBody, 'engine_account' | 'service_tier'>;

export function buildCreateSessionBody(body: CreateSessionBody): CreateSessionBody {
  const { codex_account, branch, new_branch, base, ...rest } = body;
  const out: CreateSessionBody = rest.provider === 'codex' && codex_account ? { ...rest, codex_account } : rest;
  if (!branch) return out;
  return new_branch ? { ...out, branch, new_branch: true, ...(base ? { base } : {}) } : { ...out, branch };
}

// A MESMA regra do backend (`app/names.py:sanitize_session_name`): NFKD antes do filtro, senão a
// letra acentuada vira `-` e o aparo das pontas a come junto ("Área" -> "rea").
export function sanitizeSessionName(name: string): string {
  return name.normalize('NFKD').replace(/\p{M}/gu, '').replace(/[^A-Za-z0-9_-]/g, '-').replace(/^-+|-+$/g, '');
}

// Nome único p/ tmux: sanitiza e, se já existir, sufixa -2/-3...
export function uniqueSessionName(base: string, taken: Set<string>): string {
  const clean = sanitizeSessionName(base) || 'sessao';
  if (!taken.has(clean)) return clean;
  let i = 2;
  while (taken.has(`${clean}-${i}`)) i++;
  return `${clean}-${i}`;
}

// Abrir ou retomar sessão leva bem mais que o prazo padrão: estourar antes faz a tela acusar erro
// com a sessão nascendo e o reenvio duplicá-la.
const SESSION_BIRTH_MS = 120_000;

function withCreationWarnings<T extends { avisos?: string[] }>(result: T): T {
  if (result?.avisos?.length) {
    try { apiEnv().onSessionWarnings?.(result.avisos); }
    catch (error) { console.error('onSessionWarnings:', error); }
  }
  return result;
}

export function createSessionForServer(server: Server, body: CreateSessionBody): Promise<SessionInfo> {
  return apiFetchForServer<SessionInfo>(server, '/api/sessions', { method: 'POST', body: JSON.stringify(buildCreateSessionBody(body)) }, SESSION_BIRTH_MS)
    .then(withCreationWarnings);
}

export function createSession(
  name: string,
  cwd?: string,
  configDir?: string | null,
  provider?: Provider,
  engine?: string | null,
  model?: string | null,
  effort?: string | null,
  permissionMode?: string | null,
  ompProfile?: string | null,
  codexAccount?: string | null,
  headless?: boolean,
  subagentModel?: string | null,
  jev?: boolean,
  worktree?: WorktreeChoice,
  opening?: SessionOpeningExtras,
): Promise<SessionInfo> {
  // `model`/`effort`/`permissionMode`/`ompProfile` no FIM de propósito: chamador antigo com 5 argumentos continua válido e abre
  // no padrão, byte por byte (o backend valida None = comportamento de hoje).
  const body: CreateSessionBody = { name, cwd, config_dir: configDir ?? null, provider, engine: engine ?? null,
                           model: model ?? null, effort: effort ?? null, codex_account: codexAccount, remember_provider: true };
  if (permissionMode) body.permission_mode = permissionMode;
  if (ompProfile) body.omp_profile = ompProfile;
  if (headless !== undefined && (provider === 'claude' || provider === 'codex')) body.headless = headless;
  if (subagentModel && provider === 'claude') body.subagent_model = subagentModel;
  if (jev) body.jev = true;
  if (worktree?.branch) Object.assign(body, worktree);
  if (opening) Object.assign(body, opening);
  return apiFetch<SessionInfo>('/api/sessions', {
    method: 'POST',
    body: JSON.stringify(buildCreateSessionBody(body)),
  }).then(withCreationWarnings);
}

// ── Passagem de bastão ──────────────────────────────────────────────────────
// O dossiê de continuidade que o backend monta lendo o transcript/git/plano da sessão. Volta como
// `text/markdown` cru, e não JSON — por isso NÃO passa pelo apiFetch, que sempre faz `res.json()`.
// Só leitura: o GET não cria nem grava nada, então serve de AMOSTRA (a origem segue trabalhando).
export async function getBastao(name: string): Promise<string> {
  const res = await fetch(`${apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/bastao`, {
    headers: authHeaders(),
  });
  await ensureOk(res);
  return res.text();
}

// O dossiê que ESTA sessão recebeu, lido do disco — não um novo montado agora (é o que separa esta
// rota do `getBastao` acima). Mesma resposta em markdown cru, mesmo motivo pra não passar pelo
// apiFetch.
export async function getBastaoDossie(name: string, server?: Server | null): Promise<string> {
  const res = await fetch(`${server ? baseOf(server) : apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/bastao/dossie`, {
    headers: server ? { Authorization: `Bearer ${server.token}` } : authHeaders(),
  });
  await ensureOk(res, server);
  return res.text();
}

// Resposta INTEIRA da rota, não só o que a tela lê hoje (`name`, pra navegar). Declarar meia
// resposta é o convite pro próximo `as any` quando alguém precisar do `dossie` — e os quatro
// campos são o contrato do endpoint, não campos inventados por precaução.
export interface BastaoResult {
  name: string;      // nome da sessão criada (já sanitizado pelo backend)
  dossie: string;    // caminho do .md gravado no disco DAQUELA máquina
  texto: string;     // o dossiê que foi realmente gravado (o da prévia era outro, mais velho)
  kickoff: string;   // as linhas que entraram na fila durável da sessão nova
  // Só quando a reescrita pelo modelo foi pedida e não deu (cota, tempo, CLI ausente): a sessão
  // nasceu com o resumo montado por código, e quem pediu tem de saber que recebeu o outro.
  aviso?: string | null;
  avisos?: string[];
}

// Passo em curso da criação de `name` (sessão nova ou sucessora do bastão); `step` null = nada em curso.
export interface CreationProgress { step: string | null; params: Record<string, string> }

export function getCreationProgress(name: string, server?: Server | null): Promise<CreationProgress> {
  const path = `/api/sessions/creation-progress?name=${encodeURIComponent(name)}`;
  return server ? apiFetchForServer(server, path) : apiFetch<CreationProgress>(path);
}

// Cria a sessão sucessora COM o dossiê: o backend monta → grava → cria → enfileira o kick-off.
// Não é o POST /api/sessions normal; mandar aquele deixaria a sessão nova sem dossiê nenhum.
export function passarBastao(
  name: string,
  body: {
    name: string;
    cwd?: string | null;
    config_dir?: string | null;
    provider?: Provider;
    remember_provider?: boolean;
    engine?: string | null;
    model?: string | null;
    effort?: string | null;
    permission_mode?: string | null;
    omp_profile?: string | null;
    codex_account?: string | null;
    engine_account?: string;
    // Modo de execução da sessão que recebe o trabalho: ela é nova e nasce onde a pessoa
    // escolher, sem herdar o modo da origem.
    headless?: boolean | null;
    // Pedir ao modelo que reescreva o resumo antes de gravar. Gasta cota da origem.
    resumo_por_modelo?: boolean;
  },
  server?: Server | null,
): Promise<BastaoResult> {
  if (server) return apiFetchForServer<BastaoResult>(server, `/api/sessions/${encodeURIComponent(name)}/bastao`, {
    method: 'POST', body: JSON.stringify(body),
  }).then(withCreationWarnings);
  return apiFetch<BastaoResult>(`/api/sessions/${encodeURIComponent(name)}/bastao`, {
    method: 'POST',
    body: JSON.stringify(body),
  }).then(withCreationWarnings);
}

// Modelo oferecido na tela de ABERTURA (GET /api/model-options). O backend devolve QUATRO formatos
// do campo `models` conforme o ramo (pi, motor, cache do Claude, aliases reduzidos) — os campos são
// todos opcionais porque nenhum formato tem todos; quem renderiza usa `m.name ?? m.id` e só mostra
// a etiqueta do que a fonte informou.
export interface ModelOption {
  id: string;
  name?: string;
  provider?: string;
  context_length?: number | null;
  context?: string;
  vision?: boolean | null;
  images?: boolean;
  // Níveis de esforço DAQUELE modelo (Codex e Kimi): variam por modelo e vêm do provedor, então a
  // tela não pode ter lista fechada — ver app/codex_models.py e app/kimi_models.py.
  efforts?: string[];
  default_effort?: string | null;
  // Fast: no Codex vem como tier `priority`; no motor GPT do CLIProxyAPI local, como `supports_fast`.
  service_tiers?: { id: string; hidden?: boolean }[];
  supports_fast?: boolean;
}

// Orquestração: política de contas da máquina e papéis do grupo (tipos em ./orquestracao.ts).
export async function getOrqPolitica(server?: Server | null): Promise<import('./orquestracao').OrqPolitica> {
  return sessionFetch(server, '/api/orquestracao/politica');
}
export async function putOrqConta(
  conta: string,
  body: { provider: string; apelido?: string; modelos?: string[]; trocar?: boolean; ligada?: boolean; mtime: number },
  server?: Server | null,
): Promise<{ ok: boolean; mtime: number }> {
  return sessionFetch(server, `/api/orquestracao/politica/${encodeURIComponent(conta)}`, { method: 'PUT', body: JSON.stringify(body) });
}
export async function getOrqGrupo(name: string, server?: Server | null): Promise<import('./orquestracao').OrqGrupo> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/orq`);
}
// Vários papéis numa escrita só.
export async function postOrqPapeis(
  name: string,
  // `avisar` é aceito e ignorado: salvar nunca acorda o árbitro.
  body: { papeis: ({ papel: string; sessao?: string; provider: string; conta: string; modelo?: string; esforco?: string; vez?: string; janela?: string }
    & Partial<import('./orquestracao').AberturaPapel>)[]; mtime: number; avisar?: boolean }, server?: Server | null,
): Promise<import('./orquestracao').RespostaPapel> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/orq/papeis`, { method: 'POST', body: JSON.stringify(body) });
}

/**
 * Põe a PRÓPRIA sessão pra tocar a orquestração, a partir do passo que falta (`fase`: planner,
 * prepare, launch ou arbiter). 409 só quando o recado não chega à sessão.
 */
export async function comecarOrq(name: string, server?: Server | null): Promise<{ ok: boolean; entregue: boolean; fase: string; plano: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/orq/comecar`, { method: 'POST', body: JSON.stringify({}) });
}

/**
 * Tira UMA linha da tabela de papéis: o papel inteiro (sem `vez`) ou uma conta do rodízio dele.
 * Não avisa o árbitro — quem mexe na fila mexe em várias linhas seguidas, e o aviso sai no fim.
 */
export async function removerPapel(
  name: string,
  body: { papel: string; vez: string; mtime: number }, server?: Server | null,
): Promise<{ papeis: import('./orquestracao').Papel[]; mtime: number }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/orq/papel`, { method: 'DELETE', body: JSON.stringify(body) });
}

// Lista de modelos da tela de nova sessão, onde ainda não existe sessão viva. O front manda
// CAMINHO de config dir (nunca rótulo de conta): é o que faz a chave do cache do backend casar com
// a da sessão viva — sem isto a lista quente de uma conta nunca seria aproveitada na abertura.
export async function modelOptions(
  provider: string, engine?: string | null, configDir?: string | null,
  codexAccount?: string | null, engineAccount?: string | null,
): Promise<{ kind: string; reduced: boolean; models: ModelOption[] }> {
  return apiFetch(modelOptionsPath(provider, engine, configDir, codexAccount, engineAccount));
}

function modelOptionsPath(provider: string, engine?: string | null, configDir?: string | null, codexAccount?: string | null,
  engineAccount?: string | null): string {
  const q = new URLSearchParams({ provider });
  if (engine) q.set('engine', engine);
  if (configDir) q.set('config_dir', configDir);
  if (provider === 'codex' && codexAccount) q.set('codex_account', codexAccount);
  // O catálogo de um motor do CLIProxyAPI depende da conta ChatGPT escolhida.
  if (provider === 'claude' && engine && engineAccount) q.set('engine_account', engineAccount);
  return `/api/model-options?${q}`;
}

export function modelOptionsForServer(server: Server, provider: string, engine?: string | null,
  configDir?: string | null, codexAccount?: string | null, signal?: AbortSignal, engineAccount?: string | null): ReturnType<typeof modelOptions> {
  return apiFetchForServer(server, modelOptionsPath(provider, engine, configDir, codexAccount, engineAccount), { signal: comTeto(signal, 8000) });
}

export function getCredentialsForServer(server: Server, force = false, signal?: AbortSignal): Promise<Credencial[]> {
  return apiFetchForServer(server, `/api/credenciais${force ? '?forcar=true' : ''}`, { signal: comTeto(signal, 8000) });
}
export function listarCredenciais(server: Server | null, force = false): Promise<Credencial[]> {
  return server ? getCredentialsForServer(server, force) : apiFetch(`/api/credenciais${force ? '?forcar=true' : ''}`);
}
export function getCodexAccountsForServer(server: Server, signal?: AbortSignal): Promise<CodexAccount[]> {
  return apiFetchForServer(server, '/api/codex-contas', { signal: comTeto(signal, 8000) });
}
export function createCodexAccountForServer(server: Server, name: string): Promise<CodexAccount> {
  return apiFetchForServer(server, '/api/codex-contas', { method: 'POST', body: JSON.stringify({ name }) });
}
export function deleteCodexAccountForServer(server: Server, id: string): Promise<void> {
  // Apagar a pasta leva segundos: os clones de marketplace do Codex somam milhares de arquivos.
  return apiFetchForServer(server, `/api/codex-contas/${encodeURIComponent(id)}`, { method: 'DELETE' }, 120_000);
}
function codexAccountPath(id: string, action: string): string {
  return `/api/codex-contas/${encodeURIComponent(id)}/${action}`;
}
export function prepareCodexAccountForServer(server: Server, id: string, force = false): Promise<CodexAccount['sync']> {
  const query = force ? '?forcar=true' : '';
  return apiFetchForServer(server, `${codexAccountPath(id, 'prepare')}${query}`, { method: 'POST' });
}
export function getCodexPreparationForServer(server: Server, id: string, signal?: AbortSignal): Promise<CodexAccount['sync']> {
  return apiFetchForServer(server, codexAccountPath(id, 'prepare'), { signal: comTeto(signal, 8000) });
}
export function startCodexAccountLoginForServer(server: Server, id: string): Promise<CodexLoginAttempt> {
  return apiFetchForServer(server, codexAccountPath(id, 'login'), { method: 'POST' });
}
export function getCodexAccountLoginForServer(server: Server, id: string, signal?: AbortSignal): Promise<CodexLoginAttempt | null> {
  return apiFetchForServer(server, codexAccountPath(id, 'login'), { signal: comTeto(signal, 8000) });
}
export function cancelCodexAccountLoginForServer(server: Server, id: string, attemptId: string): Promise<CodexLoginAttempt> {
  return apiFetchForServer(server, `${codexAccountPath(id, 'login')}?attempt_id=${encodeURIComponent(attemptId)}`, { method: 'DELETE' });
}
export function consumeCodexRateLimitResetForServer(
  server: Server, id: string, creditId: string | null, idempotencyKey: string,
): Promise<{ outcome: CodexResetOutcome }> {
  return apiFetchForServer(server, codexAccountPath(id, 'rate-limit-reset'), {
    method: 'POST',
    body: JSON.stringify({ credit_id: creditId, idempotency_key: idempotencyKey }),
  }, 90_000);
}
export function consumeCodexRateLimitReset(
  id: string, creditId: string | null, idempotencyKey: string,
): Promise<{ outcome: CodexResetOutcome }> {
  return apiFetch(codexAccountPath(id, 'rate-limit-reset'), {
    method: 'POST',
    body: JSON.stringify({ credit_id: creditId, idempotency_key: idempotencyKey }),
  });
}
export function getRootsForServer(server: Server, signal?: AbortSignal): Promise<FsRoot[]> {
  return apiFetchForServer(server, '/api/fs/roots', { signal: comTeto(signal, 8000) });
}
// Git da pasta (`/api/fs/branches`, `/api/fs/git*`): as rotas pedem `root` (uma raiz liberada, igual
// à lista de `/api/fs/roots`) e `path` (a pasta). O cliente recebe só a pasta e acha a raiz que a contém.
export interface FolderBranches {
  current: string | null;
  branches: string[];
  remotes: string[];   // nome curto, sem a local correspondente
  dirty: boolean;
}

// Base padrão da branch nova: a atual, ou com HEAD solto a primeira branch local (o backend recusa base vazia).
export function defaultBase(info: FolderBranches): string {
  return info.current ?? info.branches[0] ?? '';
}

export interface FolderGit {
  repo: boolean;       // false = pasta fora de repositório; os demais campos não vêm
  current?: string | null;
  upstream?: string | null;
  toplevel?: string | null;
  dirty?: number;
  ahead?: number | null;
  behind?: number | null;
  last_fetch?: number | null;   // epoch s do último fetch
  sessions?: string[];          // sessões vivas no mesmo checkout
}

// Compara por forma normalizada (barra, sem separador final, sem caixa em caminho de unidade do
// Windows), mas devolve a string ORIGINAL da raiz: o backend exige igualdade com a dele.
export function pickFolderRoot(roots: { path: string }[], cwd: string): string | null {
  const norm = (p: string) => {
    const f = p.replace(/\\/g, '/').replace(/\/+$/, '');
    return /^[A-Za-z]:/.test(f) ? f.toLowerCase() : f;
  };
  const target = norm(cwd);
  let best: string | null = null;
  for (const r of roots) {
    const root = norm(r.path);
    if ((target === root || target.startsWith(root + '/')) && (best === null || r.path.length > best.length)) best = r.path;
  }
  return best;
}

const FOLDER_READ_MS = 30_000;
const FOLDER_ACTION_MS = 150_000;

// `root` explícito dispensa a consulta a /api/fs/roots (quem escolheu a pasta já sabe a raiz).
async function folderRoot(server: Server, cwd: string, signal?: AbortSignal, root?: string): Promise<string> {
  if (root) return root;
  const found = pickFolderRoot(await getRootsForServer(server, signal), cwd);
  if (!found) throw new Error('root not allowed');
  return found;
}

export async function getFolderBranchesForServer(server: Server, cwd: string, signal?: AbortSignal, root?: string): Promise<FolderBranches> {
  const q = new URLSearchParams({ root: await folderRoot(server, cwd, signal, root), path: cwd });
  return apiFetchForServer(server, `/api/fs/branches?${q}`, { signal: comTeto(signal, FOLDER_READ_MS) }, FOLDER_READ_MS);
}

export async function getFolderGitForServer(server: Server, cwd: string, signal?: AbortSignal, root?: string): Promise<FolderGit> {
  const q = new URLSearchParams({ root: await folderRoot(server, cwd, signal, root), path: cwd });
  return apiFetchForServer(server, `/api/fs/git?${q}`, { signal: comTeto(signal, FOLDER_READ_MS) }, FOLDER_READ_MS);
}

// Fetch/pull devolvem o mesmo estado da leitura, já relido.
export async function folderGitActionForServer(server: Server, cwd: string, action: 'fetch' | 'pull', root?: string): Promise<FolderGit> {
  const body = JSON.stringify({ root: await folderRoot(server, cwd, undefined, root), path: cwd });
  return apiFetchForServer(server, `/api/fs/git/${action}`, { method: 'POST', body }, FOLDER_ACTION_MS);
}

export interface WorktreeChoice { branch: string; new_branch?: boolean; base?: string | null }
export interface WorktreeStatus {
  path: string; repo: string; exists: boolean; branch: string | null; base: string | null;
  main_branch: string | null;   // branch da pasta principal, onde as conversas retomam
  merged: boolean; ahead: number; dirty: number; ignored: string[]; sessions: string[]; closed: number;
  degraded?: boolean;   // leitura do git falhou: dirty/ignored podem estar zerados sem ser verdade
  behind?: number;      // commits novos na base que a branch ainda não tem
  dirty_files?: { code: string; path: string }[];
  last_commit?: WorktreeCommit | null;
  commits?: WorktreeCommit[];   // os últimos da branch que não estão na base
  created_at?: number | null;   // epoch em segundos
  size?: number | null;         // bytes ocupados; null enquanto o backend mede
  size_biggest?: { name: string; bytes: number } | null;
  size_pending?: boolean;
  size_error?: boolean;     // a medição falhou; tenta de novo sozinha depois
  size_partial?: boolean;   // houve pasta ilegível: o tamanho é um mínimo ("≥")
}
export interface WorktreeCommit { sha: string; subject: string; at: number }
export interface WorktreeRepo { repo: string; worktrees: WorktreeStatus[] }

export type WorktreeState = 'gone' | 'session' | 'dirty' | 'merged' | 'detached' | 'active';

/** Um estado só por worktree, na ordem do que mais importa antes de apagar. */
export function worktreeState(w: WorktreeStatus): WorktreeState {
  if (!w.exists) return 'gone';
  if (w.sessions.length) return 'session';
  if (w.dirty) return 'dirty';
  if (w.merged) return 'merged';
  if (!w.branch) return 'detached';
  return 'active';
}

export const WORKTREE_STALE_DAYS = 14;

/** Dias desde o último commit; sem commit lido, desde a criação. */
export function worktreeAgeDays(w: WorktreeStatus, now = Date.now() / 1000): number | null {
  const at = w.last_commit?.at ?? w.created_at;
  return at ? Math.max(0, Math.floor((now - at) / 86400)) : null;
}

/** Pronta para apagar: mesclada, sem alteração e sem sessão. Arquivo ignorado não segura (cópia
 *  do CLAUDE.local.md, cache); a confirmação lista o que ele perde. */
export function worktreeReady(w: WorktreeStatus): boolean {
  return w.merged && !w.dirty && !w.sessions.length && !w.degraded;
}

/** Soma do disco de várias: com alguma pendente, falha ou parcial, o total é só um mínimo. */
export function worktreesSizeTotal(ws: WorktreeStatus[]): { bytes: number; partial: boolean } {
  return {
    bytes: ws.reduce((a, w) => a + (w.size ?? 0), 0),
    partial: ws.some((w) => w.size_partial || w.size_error || w.size_pending),
  };
}

/** Worktree que o Claude cria para um subagente: nome gerado, agrupada à parte na lista. */
export function worktreeIsAgent(w: WorktreeStatus): boolean {
  return /[\\/]agent-[0-9a-f]{8,}$/.test(w.path.replace(/[\\/]+$/, ''));
}

/** Nome para mostrar: a branch quando a pasta tem nome gerado e a branch diz algo. */
export function worktreeTitle(w: WorktreeStatus): string {
  const base = w.path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || w.path;
  if (worktreeIsAgent(w) && w.branch && !w.branch.startsWith('worktree-agent-')) return w.branch;
  return base;
}

export function createWorktreeForServer(server: Server, body: {
  repo: string; branch: string; name: string; new_branch?: boolean; base?: string | null; fetch?: boolean;
}): Promise<{ path: string }> {
  return apiFetchForServer(server, '/api/worktrees/create', { method: 'POST', body: JSON.stringify(body) }, FOLDER_ACTION_MS);
}

/** O lote de mescladas: o que entra na confirmação (com o que cada uma perde) e o que fica de fora
 *  porque tem sessão aberta ou não foi lida direito. */
export function mergedWorktreeBatch(r: WorktreeRepo): { deletable: WorktreeStatus[]; blocked: WorktreeStatus[] } {
  const merged = r.worktrees.filter((w) => w.merged);
  return {
    deletable: merged.filter((w) => !w.sessions.length && !w.degraded),
    blocked: merged.filter((w) => w.sessions.length || w.degraded),
  };
}

export async function getWorktreesForServer(server: Server, signal?: AbortSignal): Promise<WorktreeRepo[]> {
  const r = await apiFetchForServer<{ repos: WorktreeRepo[] }>(server, '/api/worktrees',
    { signal: comTeto(signal, FOLDER_READ_MS) }, FOLDER_READ_MS);
  return r.repos;
}
export function getWorktreeForServer(server: Server, path: string, signal?: AbortSignal): Promise<WorktreeStatus> {
  return apiFetchForServer(server, `/api/worktrees/detail?${new URLSearchParams({ path })}`,
    { signal: comTeto(signal, FOLDER_READ_MS) }, FOLDER_READ_MS);
}
export async function fetchWorktreesForServer(server: Server, repo: string): Promise<void> {
  await apiFetchForServer(server, '/api/worktrees/fetch', { method: 'POST', body: JSON.stringify({ repo }) }, FOLDER_ACTION_MS);
}
export function deleteWorktreeForServer(server: Server, body: { repo: string; path: string; confirm: boolean; delete_branch: boolean }):
  Promise<{ removed: string; branch_deleted: boolean; moved: number }> {
  return apiFetchForServer(server, '/api/worktrees/delete', { method: 'POST', body: JSON.stringify(body) }, FOLDER_ACTION_MS);
}
/** Sem `confirmed`, só as mescladas que não perdem nada. Com `confirmed`, as que a tela mostrou na
 *  confirmação; perde arquivos só a que a tela mostrou perdendo, nunca uma que sujou depois. */
export async function deleteMergedWorktreesForServer(server: Server, repo: string,
  confirmed?: WorktreeStatus[]): Promise<string[]> {
  const opts = confirmed && {
    paths: confirmed.map((w) => w.path), confirm: true,
    lossy: confirmed.filter((w) => w.dirty || w.ignored.length).map((w) => w.path),
  };
  const r = await apiFetchForServer<{ removed: string[] }>(server, '/api/worktrees/delete-merged',
    { method: 'POST', body: JSON.stringify({ repo, ...opts }) }, FOLDER_ACTION_MS);
  return r.removed;
}
/** Nome curto do chip: a pasta da worktree onde o agente está. */
export function worktreeLabel(s: SessionInfo): string | null {
  const p = s.worktree_path ?? (s.worktree ? s.cwd : null);
  return p ? basename(p) : null;
}

// Importação Claude → Codex da conta padrão (a mesma do "Reconciliar agora" em Harnesses).
export function getCodexIntegrationForServer(server: Server, signal?: AbortSignal): Promise<CodexIntegracaoEstado> {
  return apiFetchForServer(server, '/api/harness/codex/integracao', { signal: comTeto(signal, 8000) });
}
export function startCodexIntegrationForServer(server: Server): Promise<CodexIntegracaoEstado> {
  return apiFetchForServer(server, '/api/harness/codex/integracao', { method: 'POST' });
}

// Cria a pasta da conta Claude no servidor. NÃO loga — o OAuth é interativo e roda dentro da
// primeira sessão aberta nela (o backend devolve a conta com active=false justamente por isso).
// alvo não-nulo (outra máquina) sai por apiFetchForServer: sem self-heal de 401, porque um 401
// DAQUELE servidor não pode apagar a credencial ativa (contrato de api.ts:129-131).
export async function criarConta(alvo: Server | null, nome: string): Promise<ConfigDirInfo> {
  const init = { method: 'POST', body: JSON.stringify({ nome }) };
  return alvo
    ? apiFetchForServer<ConfigDirInfo>(alvo, '/api/claude-configs', init)
    : apiFetch<ConfigDirInfo>('/api/claude-configs', init);
}

/** Bloco que o app gravou no config do Kimi e que ficou órfão — o provedor nativo dele dá 404. */
export async function apagarProvedorKimi(alvo: Server | null, nome: string): Promise<void> {
  const rota = `/api/credenciais/kimi/${encodeURIComponent(nome)}`;
  const init = { method: 'DELETE' };
  if (alvo) {
    await apiFetchForServer<void>(alvo, rota, init);
    return;
  }
  await apiFetch(rota, init);
}

// Apaga a conta e os transcripts dela no servidor. Recusa 409 se houver sessão viva usando-a.
export async function apagarConta(alvo: Server | null, nome: string): Promise<void> {
  const init = { method: 'DELETE' };
  if (alvo) {
    await apiFetchForServer<void>(alvo, `/api/claude-configs/${encodeURIComponent(nome)}`, init);
    return;
  }
  await apiFetch(`/api/claude-configs/${encodeURIComponent(nome)}`, init);
}

// Sai da conta (claude auth logout) mantendo a pasta. Recusa 409 se houver sessão viva usando-a.
export async function sairConta(alvo: Server | null, nome: string): Promise<void> {
  const caminho = `/api/claude-configs/${encodeURIComponent(nome)}/logout`;
  const init = { method: 'POST' };
  if (alvo) await apiFetchForServer<void>(alvo, caminho, init);
  else await apiFetch(caminho, init);
}

// Web Push: chave VAPID publica deste servidor (applicationServerKey). Vazia = push desligado la.
export async function getVapidKey(s: Server): Promise<string> {
  const res = await fetch(`${baseOf(s)}/api/push/vapid`, {
    headers: { Authorization: `Bearer ${s.token}` },
  });
  if (!res.ok) throw new Error(`vapid ${res.status}`);
  return ((await res.json() as { key?: string }).key ?? '') as string;
}

// Shape do PushSubscription.toJSON() do navegador — local pra nao puxar a lib DOM inteira no core.
export interface PushSubscriptionJSON {
  endpoint?: string;
  expirationTime?: number | null;
  keys?: Record<string, string>;
}

// Registra a inscricao push do celular NESTE servidor, com label + id locais (pra notif e deep-link)
// e o idioma escolhido na tela Geral — o backend renderiza a notificacao no idioma da inscricao
// (app/push.py); inscricao antiga sem o campo cai em pt.
export async function subscribePush(s: Server, subscription: PushSubscriptionJSON): Promise<void> {
  const res = await fetch(`${baseOf(s)}/api/push/subscribe`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ subscription, label: s.label, serverId: s.id, locale: localeAtual() }),
  });
  if (!res.ok) throw new Error(`subscribe ${res.status}`);
}

// Preferencias de push (feature #5): sessoes silenciadas + janela de quiet hours global — do servidor
// ATIVO (mesma convencao das ops por-sessao, que sempre miram o server selecionado no momento).
export function getPushSettings(): Promise<{ muted: string[]; quiet_hours: { start: string; end: string } | null }> {
  return apiFetch('/api/push/settings');
}

export function getPushSettingsForServer(s: Server): Promise<{ muted: string[]; quiet_hours: { start: string; end: string } | null }> {
  return apiFetchForServer(s, '/api/push/settings');
}

export function setSessionMute(session: string, muted: boolean): Promise<{ ok: boolean }> {
  return apiFetch('/api/push/mute', { method: 'POST', body: JSON.stringify({ session, muted }) });
}

export function setQuietHours(start: string | null, end: string | null): Promise<{ ok: boolean }> {
  return apiFetch('/api/push/quiet-hours', { method: 'POST', body: JSON.stringify({ start, end }) });
}

export function setQuietHoursForServer(s: Server, start: string | null, end: string | null): Promise<{ ok: boolean }> {
  return apiFetchForServer(s, '/api/push/quiet-hours', { method: 'POST', body: JSON.stringify({ start, end }) });
}

export async function deleteSession(name: string): Promise<{ ok: boolean; warning: unknown | null }> {
  return apiFetch<{ ok: boolean; warning: unknown | null }>(`/api/sessions/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// Renomeia a sessao do tmux. Devolve o nome final (sanitizado pelo backend).
export async function renameSession(name: string, newName: string): Promise<{ ok: boolean; name: string }> {
  return apiFetch<{ ok: boolean; name: string }>(`/api/sessions/${encodeURIComponent(name)}/rename`, {
    method: 'POST',
    body: JSON.stringify({ new: newName }),
  });
}

// Relança uma sessão "sem id" com `claude --resume <uuid>` -> passa a rastreá-la, continuando a
// conversa. sessionId ausente = deixa o backend escolher (caso seguro) ou devolver candidatos (ambíguo).
export function resumeSession(name: string, sessionId?: string): Promise<ResumeResult> {
  return apiFetch<ResumeResult>(`/api/sessions/${encodeURIComponent(name)}/resume`, {
    method: 'POST',
    body: JSON.stringify({ session_id: sessionId ?? null }),
  });
}

// Encadeamento de sessao (feature #12): arma o vinculo 'then' — quando `name` terminar o turno,
// `text` e enviado pra `target` (mesmo backend/servidor; ver app.chain no backend). Um hop so.
export function setThenLink(name: string, target: string, text: string): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/then`, {
    method: 'PUT',
    body: JSON.stringify({ target, text }),
  });
}

export function clearThenLink(name: string): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/then`, {
    method: 'DELETE',
  });
}

// Abre o cwd da sessao no editor da MAQUINA do backend (so-desktop). Binario fixo (CP_EDITOR).
export function openEditor(name: string): Promise<{ ok: boolean }> {
  return apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/open-editor`, {
    method: 'POST',
  });
}

// Cancelamento NÃO é falha. O Chat aborta o /history em voo quando a carga perde a validade (troca
// de sessão, /clear, volta do background, unmount) — o rejeito que vem daí não pode virar pílula de
// erro na tela. `TimeoutError` (o teto de 45s abaixo) fica de FORA de propósito: aquilo é falha de
// verdade e o usuário precisa ver.
export function isAbortError(e: unknown): boolean {
  return e instanceof Error && e.name === 'AbortError';
}

// Estourou o TETO de tempo (AbortSignal.timeout), que NÃO é o mesmo que cancelamento por quem
// chamou (isAbortError, acima): quem cancela sabe que cancelou; um teto estourado é falha e vale
// tentar de novo — o segundo pedido sai numa conexão nova.
export function isTimeoutError(e: unknown): boolean {
  return e instanceof Error && e.name === 'TimeoutError';
}

// Histórico da sessão ATIVA. Com `limit` é a MESMA rota do tail dos cards do board
// (getHistoryTailForServer): o backend faz tail-read, parseando só o fim do jsonl em vez do
// arquivo inteiro. O Chat abre com a cauda e busca o resto (sem limit) em segundo plano.
// `signal` cancela de verdade: sem ele o fetch do histórico COMPLETO (medido: 1596 eventos num
// jsonl de 136MB) seguia baixando depois de já ter sido descartado — banda e parse à toa no
// celular, e vários em paralelo quando o usuário pula de sessão em sessão.
export async function getHistory(name: string, limit?: number, signal?: AbortSignal,
                           timeoutMs = 45_000, server?: Server): Promise<ChatEvent[]> {
  // Teto largo (transcript grande em link lento existe), mas TETO: o resume do iOS chamava isto
  // sem timeout e um socket pendurado deixava o fetch em voo por minutos, sobrescrevendo estado
  // novo com foto velha quando enfim resolvia.
  // `timeoutMs` existe porque a PRIMEIRA carga do Chat quer um teto curto: 45s de skeleton parado
  // (medido no iPhone, 17/08/2026 — o pedido nem chegou no servidor) e a espera inteira sem nada na
  // tela. Ela pede 10s e tenta de novo; o resto do histórico, em segundo plano, mantém os 45s.
  const cap = AbortSignal.timeout(timeoutMs);
  // `!== undefined` e não truthy: limit=0 é um pedido explícito de zero, não um pedido do arquivo inteiro.
  const q = limit !== undefined ? `?limit=${limit}` : '';
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/history${q}`, {
    signal: signal ? AbortSignal.any([signal, cap]) : cap,
  }, server);
  if (server) { if (!res.ok) await throwForServer(res, server); } else await ensureOk(res);
  return res.json() as Promise<ChatEvent[]>;
}

/** A cauda do histórico, mas SÓ se mudou desde a última vez.
 *
 *  `etag` é o validador que veio no `ETag` da resposta anterior (guardado junto com os eventos).
 *  Igual ao do servidor -> `'igual'`, ~200 bytes e nenhum corpo: o que está na tela continua sendo
 *  a verdade. Diferente (ou sem etag) -> a cauda inteira, com o validador novo pra guardar.
 *  Medido em 06/09/2026 na `pr-junior`: a cauda são 313 KB, pagos a cada entrada na sessão.
 *
 *  `etag: null` no retorno = servidor sem validador (transcript que não dá pra medir) — o chamador
 *  guarda os eventos mesmo assim, só não terá o que perguntar na próxima. */
export async function getHistoryDesde(
  name: string, limit: number, etag: string | null, signal?: AbortSignal, timeoutMs = 45_000,
  server?: Server,
): Promise<{ eventos: ChatEvent[]; etag: string | null } | 'igual'> {
  const cap = AbortSignal.timeout(timeoutMs);
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/history?limit=${limit}`, {
    signal: signal ? AbortSignal.any([signal, cap]) : cap,
    headers: etag ? { 'If-None-Match': etag } : {},
  }, server);
  if (res.status === 304) return 'igual';
  if (server) { if (!res.ok) await throwForServer(res, server); } else await ensureOk(res);
  return { eventos: (await res.json()) as ChatEvent[], etag: res.headers.get('ETag') };
}

export function getCommands(name: string, server?: Server | null): Promise<CommandInfo[]> {
  return sessionFetch<CommandInfo[]>(server, `/api/sessions/${encodeURIComponent(name)}/commands`);
}

// Workflows: lista de runs + detalhe (fases + agentes) — lidos dos arquivos do run no disco.
export function getWorkflows(name: string, server?: Server | null): Promise<WorkflowSummary[]> {
  return sessionFetch<WorkflowSummary[]>(server, `/api/sessions/${encodeURIComponent(name)}/workflows`);
}

export function getWorkflow(name: string, runId: string, server?: Server | null): Promise<WorkflowDetail> {
  return sessionFetch<WorkflowDetail>(server, `/api/sessions/${encodeURIComponent(name)}/workflows/${encodeURIComponent(runId)}`);
}

export function getWorkflowAgent(name: string, runId: string, agentId: string, server?: Server | null): Promise<WorkflowAgentDetail> {
  return sessionFetch<WorkflowAgentDetail>(server, `/api/sessions/${encodeURIComponent(name)}/workflows/${encodeURIComponent(runId)}/agents/${encodeURIComponent(agentId)}`);
}

// Subagentes soltos (tool Agent) da sessão: o transcript PRÓPRIO de cada um, que o jsonl do pai
// não carrega. É o que permite ver as ferramentas que ele está chamando enquanto roda.
export function getSubagents(name: string, server?: Server | null): Promise<SubagentRun[]> {
  return sessionFetch<SubagentRun[]>(server, `/api/sessions/${encodeURIComponent(name)}/subagents`);
}
export function getSubagent(name: string, agentId: string, events = 0, server?: Server | null): Promise<SubagentRun> {
  const q = events ? `?events=${events}` : '';
  return sessionFetch<SubagentRun>(server, `/api/sessions/${encodeURIComponent(name)}/subagents/${encodeURIComponent(agentId)}${q}`);
}

// Raízes liberadas do scanner (chips no topo do FolderScanner).
export function getRoots(): Promise<FsRoot[]> {
  return apiFetch<FsRoot[]>('/api/fs/roots');
}

/**
 * Lista os subdiretórios imediatos de `path` (default = `root`) dentro da raiz.
 * Rejeições de fronteira do backend (403 raiz não liberada, 400 caminho inválido,
 * 404 ausente) viram um FsScanResult com `error` tipado: a UI tem UM caminho de
 * renderização (lê `result.error`), em vez de misturar throws com campos. Apenas 401
 * borbulha (problema de auth, não de varredura).
 */
export async function scanDir(root: string, path?: string, server?: Server): Promise<FsScanResult> {
  const qs = new URLSearchParams({ root });
  if (path) qs.set('path', path);
  const url = `/api/fs/scan?${qs.toString()}`;
  try {
    return await (server ? apiFetchForServer<FsScanResult>(server, url) : apiFetch<FsScanResult>(url));
  } catch (e) {
    if (!(e instanceof Error)) throw e;
    // `.status`, nao parseInt(e.message): ensureOk (api.ts) parava de embutir o status no TEXTO
    // da mensagem, entao ler o numero de la quebraria toda vez que o detail do backend comecasse
    // com digito (ex: "404 arquivos encontrados"). O status ja vem anotado no proprio erro.
    const status = (e as Error & { status?: number }).status ?? NaN;
    if (status === 401) throw e;
    return { entries: [], error: SCAN_ERRORS[status] ?? 'unknown' };
  }
}

const SCAN_ERRORS: Record<number, FsScanError> = {
  400: 'invalid_path',
  403: 'root_not_allowed',
  404: 'not_found',
};

// Mesma tradução de recusa do scanDir, mas sem status HTTP (rede, prazo, cancelamento) o erro
// borbulha: a criação precisa distinguir "pasta recusada" de "máquina não respondeu".
export async function scanDirForServer(server: Server, root: string, path?: string,
  signal?: AbortSignal): Promise<FsScanResult> {
  const qs = new URLSearchParams({ root });
  if (path) qs.set('path', path);
  try {
    return await apiFetchForServer<FsScanResult>(server, `/api/fs/scan?${qs.toString()}`,
      { signal: comTeto(signal, 8000) });
  } catch (e) {
    const status = (e as { status?: unknown } | null)?.status;
    if (typeof status !== 'number' || status === 401) throw e;
    return { entries: [], error: SCAN_ERRORS[status] ?? 'unknown' };
  }
}

// Cria a subpasta `name` em `path` (default = raiz), sob a mesma allowlist do scan.
export function makeDir(root: string, path: string | null, name: string, server?: Server): Promise<FsEntry> {
  const init = { method: 'POST', body: JSON.stringify({ root, path, name }) };
  // Prazo largo: estourar depois de o servidor criar mostraria falha de uma pasta que existe.
  return server ? apiFetchForServer<FsEntry>(server, '/api/fs/mkdir', init, 20_000) : apiFetch<FsEntry>('/api/fs/mkdir', init);
}

// ── Arquivo: conversas mortas (transcripts sem sessão tmux viva) ──────────────
// Navegação pasta-primeiro: nível 1 = pastas (agregado barato), nível 2 = conversas da pasta.
export interface ArchiveFolder {
  project: string;
  cwd: string | null;
  count: number;
  mtime: number;
}

export interface ArchiveEntry {
  codex_account?: string | null;
  codex_home?: string | null;
  project: string;
  cwd: string | null;
  session_id: string;
  mtime: number;
  preview: string;
  ultima: string;   // ultima msg da conversa — e o que identifica qual sessao e essa
  live: boolean;
  config_dir: string | null;   // conta dona do transcript (null = a do backend)
  conta: string;               // rotulo dela, pra mostrar na lista
  provider: Provider;          // cada agente guarda transcript num lugar proprio
}

export function getArchive(): Promise<ArchiveFolder[]> {
  return apiFetch<ArchiveFolder[]>('/api/archive');
}

export function getArchiveRecentForServer(server: Server, cap = 40, signal?: AbortSignal): Promise<ArchiveEntry[]> {
  return apiFetchForServer(server, `/api/archive/recent?cap=${cap}`, { signal: comTeto(signal, 8000) });
}

export function getArchiveFolder(project: string): Promise<ArchiveEntry[]> {
  return apiFetch<ArchiveEntry[]>(`/api/archive/${encodeURIComponent(project)}`);
}

// "Retomar conversa": sobe uma sessao tmux NOVA no cwd original com `claude --resume <uuid>`,
// continuando esta conversa morta. Devolve a SessionInfo da sessao nova (o front navega pro chat dela).
// engine: o pane original morreu, entao nao ha /proc pra descobrir que motor rodava -- quem retoma
// escolhe de novo (ou nenhum -> volta na conta Anthropic, igual hoje). Body vazio quando omitido:
// o backend aceita `ResumeArchivedBody = ResumeArchivedBody()` como default, entao chamadores antigos
// continuam funcionando sem mandar nada.
export function resumeArchivedConversation(
  project: string,
  sessionId: string,
  engine?: string | null,
  configDir?: string | null,
  provider?: string,
  codexAccount?: string | null,
  server?: Server | null,
): Promise<SessionInfo> {
  const request = server ? <T>(path: string, init?: RequestInit) => apiFetchForServer<T>(server, path, init, SESSION_BIRTH_MS) : apiFetch;
  return request<SessionInfo>(
    `/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sessionId)}/resume`,
    { method: 'POST', body: JSON.stringify({
      engine: engine ?? null, config_dir: configDir ?? null, provider: provider ?? 'claude',
      ...(provider === 'codex' && codexAccount ? { codex_account: codexAccount } : {}) }) },
  );
}

// Conversas retomaveis de UM cwd, na conta pedida — a lista do modal de sessao nova. Pasta sem
// conversa devolve [], nao erro.
export function getArchivePorCwd(cwd: string, configDir?: string | null,
                                 provider?: string, codexAccount?: string | null,
                                 server?: Server | null, signal?: AbortSignal): Promise<ArchiveEntry[]> {
  const q = new URLSearchParams({ cwd });
  if (configDir) q.set('config_dir', configDir);
  if (provider && provider !== 'claude') q.set('provider', provider);
  if (provider === 'codex' && codexAccount) q.set('codex_account', codexAccount);
  const init = { signal: comTeto(signal, 8000) };
  return server ? apiFetchForServer(server, `/api/archive-por-cwd?${q}`, init) : apiFetch(`/api/archive-por-cwd?${q}`, init);
}

// `tail` = so as N ultimas mensagens, lidas pelo fim do arquivo (previa). Sem ele, a conversa
// inteira, como sempre.
export function getArchiveHistory(project: string, sid: string, tail?: number,
                                  configDir?: string | null,
                                  provider?: string, codexAccount?: string | null,
                                  server?: Server | null, signal?: AbortSignal): Promise<ChatEvent[]> {
  const q = new URLSearchParams();
  if (tail) q.set('tail', String(tail));
  if (configDir) q.set('config_dir', configDir);
  if (provider && provider !== 'claude') q.set('provider', provider);
  if (provider === 'codex' && codexAccount) q.set('codex_account', codexAccount);
  const path = `/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sid)}/history${q.size ? `?${q}` : ''}`;
  const init = { signal: comTeto(signal, 8000) };
  return server ? apiFetchForServer(server, path, init) : apiFetch(path, init);
}

// URL de imagem colada no terminal, versão arquivo (mesmo ?token das outras URLs de <img>).
export function archiveImageUrl(project: string, sid: string, id: string, idx: number): string {
  const t = apiEnv().getToken() ?? '';
  return `${apiEnv().getBaseUrl()}/api/archive/${encodeURIComponent(project)}/${encodeURIComponent(sid)}/transcript-image/${encodeURIComponent(id)}/${idx}?token=${encodeURIComponent(t)}`;
}

// ── Busca de conteudo cross-session (feature #10): grep (rg) em todos os transcripts do servidor ──
export interface SearchHit {
  project: string;
  session_id: string;
  session_name: string | null;  // nome tmux se a sessao esta viva -> abre o chat; null = arquivo
  cwd: string | null;
  line: string;                 // trecho legivel (texto da msg) ja capado no backend
  mtime: number;
  live: boolean;
  role?: 'user' | 'assistant' | null;  // quem escreveu a mensagem casada
  event_id?: string | null;            // id do evento no histórico: abre a conversa no ponto
  ts?: number | null;                  // quando a mensagem foi escrita
}

// A mensagem do trecho e as vizinhas (suas e do assistente), pra ler sem sair da busca.
export async function getSearchContextForServer(
  s: Server, project: string, sessionId: string, eventId: string,
): Promise<ChatEvent[]> {
  const q = new URLSearchParams({ project, session_id: sessionId, event_id: eventId });
  return apiFetchForServer(s, `/api/search/context?${q}`);
}

// Busca em UM servidor (baseUrl+token explicitos), sem mexer no ativo — a UI faz fan-out por servidor
// (mesmo padrao de fetchSessionsForServer) e junta os resultados. Timeout 4s: server morto falha rapido.
// RAG lexical "onde falei sobre X": o backend busca trechos (rg) e um claude -p efemero responde
// apontando as sessoes. Timeout largo (90s): busca + chamada de modelo.
export async function askHistoryForServer(
  s: Server,
  question: string,
): Promise<{ answer: string; hits: SearchHit[] }> {
  const res = await fetch(`${baseOf(s)}/api/ask-history`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ question }),
    signal: AbortSignal.timeout(90000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ answer: string; hits: SearchHit[] }>;
}

export async function searchTranscriptsForServer(s: Server, q: string): Promise<SearchHit[]> {
  // 8s: com o índice FTS a busca responde em milissegundos; esperar mais só prende a tela num
  // servidor inalcançável. O `rg` de antes do índice ficar pronto pode estourar — vira aviso de falha.
  const res = await fetch(`${baseOf(s)}/api/search?q=${encodeURIComponent(q)}`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(8000),
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<SearchHit[]>;
}

// Tira da fila durável uma entrada que o backend desistiu de entregar (a bolha "não chegou").
// `entryId` é o id CRU da fila — a bolha do chat é `queued-<id>`.
export async function descartarDaFila(name: string, entryId: string, server?: Server | null): Promise<void> {
  await sessionFetch<{ ok: boolean }>(server,
    `/api/sessions/${encodeURIComponent(name)}/queue/${encodeURIComponent(entryId)}`,
    { method: 'DELETE' },
  );
}

export async function sendInput(name: string, text: string, server?: Server | null): Promise<void> {
  await sessionFetch<{ ok: boolean }>(server, `/api/sessions/${encodeURIComponent(name)}/input`, {
    method: 'POST',
    body: JSON.stringify({ text }),
  });
}

// Kimi promove a fila por ctrl-s; Codex aceita texto imediato ou promove a fila por turn/steer.
// promoted=true: o backend já baixou a fila durável — o front tira as bolhas "queued-" na hora,
// porque o user_msg real só é gravado no wire no FIM do turno (medido: ~34s depois do ctrl-s).
export async function steerSession(
  name: string,
  text?: string, server?: Server | null,
): Promise<{ ok: boolean; promoted?: boolean; confirmed?: number; queued_ids?: string[] }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/steer`, {
    method: 'POST',
    body: text === undefined ? undefined : JSON.stringify({ text }),
  });
}

// Pareamento ("trabalhando juntas"): o backend grava o vínculo simétrico e injeta o prompt de
// pareamento nas DUAS sessões — daí em diante elas se falam via hangar-send por iniciativa própria.
// warning: falha PARCIAL de aviso (algum membro sem o prompt do grupo) — o backend reporta de
// propósito; descartar isso virava "sucesso" mudo com membro que não sabe que está no grupo.
export interface PairResult { ok: boolean; warning: string | EnvelopeErro | null }
export async function pairSession(
  name: string,
  peers: string[],
  task = '',
  replaceTask = false,
  server?: Server,
): Promise<PairResult> {
  const rota = `/api/sessions/${encodeURIComponent(name)}/pair`;
  const init = { method: 'POST', body: JSON.stringify({ peers, task, replace_task: replaceTask }) };
  // A rota espera o backend do par remoto e a entrega do aviso a cada membro: 8s dava falso "fora do ar".
  return server ? apiFetchForServer<PairResult>(server, rota, init, 30_000) : apiFetch<PairResult>(rota, init);
}

export function suggestGroupTask(sessions: string[]): Promise<{ task: string }> {
  return apiFetch<{ task: string }>('/api/pair/task-suggestion', {
    method: 'POST',
    body: JSON.stringify({ sessions }),
  });
}

export async function unpairSession(name: string, server?: Server): Promise<PairResult> {
  const rota = `/api/sessions/${encodeURIComponent(name)}/pair`;
  const init = { method: 'DELETE' };
  return server ? apiFetchForServer<PairResult>(server, rota, init, 30_000) : apiFetch<PairResult>(rota, init);
}

// Contrato compartilhado do par: markdown que as duas sessões editam via fs; o app só exibe.
export interface PairContract { peers: string[]; path: string; content: string }
export function getPairContract(name: string, server?: Server): Promise<PairContract> {
  const path = `/api/sessions/${encodeURIComponent(name)}/pair/contract`;
  return server ? apiFetchForServer<PairContract>(server, path) : apiFetch<PairContract>(path);
}

// Fan-out de um prompt pra N sessoes DO SERVIDOR ATIVO (feature #9). Mira sempre 1 servidor por
// chamada — selecao cross-server manda 1 chamada por servidor (selectServer antes, igual ao resto
// do app). O backend roda a MESMA sequencia do /input por nome (fila duravel + confirmacao).
export interface BroadcastResult { ok: boolean; error: string | EnvelopeErro | null }
export async function broadcast(names: string[], text: string, server?: Server | null): Promise<Record<string, BroadcastResult>> {
  const res = await sessionFetch<{ results: Record<string, BroadcastResult> }>(server, '/api/broadcast', {
    method: 'POST',
    body: JSON.stringify({ names, text }),
  });
  return res.results;
}

/**
 * Envia os bytes crus de um arquivo (imagem, video, pdf, ...) pra sessao (sem multipart). O backend
 * salva e devolve o path; o app depois manda a legenda + path pelo /input. O filename vai no header
 * X-Filename (percent-encoded) so pra extensao; o nome final e gerado pelo servidor. 401 -> self-heal.
 */
// Lista os anexos JA enviados pra sessao (galeria). Sem timeout curto de propósito: roda sob
// interação do usuário (abrir a sheet), não em poll — falhar rápido aqui só viraria erro à toa.
export function listUploads(name: string, server?: Server | null): Promise<{ files: UploadFile[] }> {
  return sessionFetch<{ files: UploadFile[] }>(server, `/api/sessions/${encodeURIComponent(name)}/uploads`);
}

// ── Configuração do servidor ────────────────────────────────────────────────
// O segredo (chave da Groq) volta MASCARADO — dá pra conferir qual chave está lá, não pra copiar.
export interface CampoConfig {
  erro?: string;
  valor: string | number | boolean | null;
  definido: boolean;
  origem: 'app' | 'env';
}

// Lista ordenada de serviços de transcrição (`transcription_providers`). Chega em
// `campos.transcription_providers.valor` como lista; `CampoConfig.valor` não é alargado porque os
// leitores de campo escalar continuam lendo string — quem quer a lista usa este parser.
export type TranscriptionProviderKind = 'openai' | 'elevenlabs' | 'whisper_cpp';
export interface TranscriptionProviderConfig {
  id: string; kind: TranscriptionProviderKind; name: string; base_url: string; api_key: string; model: string;
  executable_path?: string; model_path?: string; language?: string; converter_path?: string;
}
export interface TranscriptionProviderStatus {
  id: string; name: string; kind: string; waiting_until: number | null; reason: string | null;
  state?: 'not_started' | 'starting' | 'ready' | 'failed' | null; error?: string | null;
}

export function parseTranscriptionProviders(v: unknown): TranscriptionProviderConfig[] {
  if (!Array.isArray(v)) return [];
  return v
    .filter((p): p is Record<string, unknown> => !!p && typeof p === 'object'
      && typeof p.id === 'string' && (p.kind === 'openai' || p.kind === 'elevenlabs' || p.kind === 'whisper_cpp'))
    .map((p) => ({
      id: p.id as string, kind: p.kind as TranscriptionProviderKind,
      name: String(p.name ?? ''), base_url: String(p.base_url ?? ''),
      api_key: String(p.api_key ?? ''), model: String(p.model ?? ''),
      ...(p.kind === 'whisper_cpp' ? {
        executable_path: String(p.executable_path ?? ''), model_path: String(p.model_path ?? ''),
        language: String(p.language || 'pt'), converter_path: String(p.converter_path ?? ''),
      } : {}),
    }));
}

// Mesmo nome que o backend deriva quando o item não tem nome: o item recém-adicionado ainda não
// passou pelo servidor e precisa de rótulo.
export function transcriptionProviderLabel(
  p: Pick<TranscriptionProviderConfig, 'kind' | 'name' | 'base_url' | 'model'>,
): string {
  if (p.name.trim()) return p.name.trim();
  if (p.kind === 'elevenlabs') return 'ElevenLabs';
  if (p.kind === 'whisper_cpp') return 'whisper.cpp';
  // Como o `urlparse(...).hostname or base` do backend: sem porta, e o texto cru se não for URL.
  const base = p.base_url.trim() || 'https://api.groq.com/openai/v1';
  let host = base;
  try { host = new URL(base).hostname || base; } catch { /* endpoint sendo digitado */ }
  return `${host} · ${p.model.trim() || 'whisper-large-v3'}`;
}

// Ordem da lista = ordem de tentativa. Troca com o vizinho; na ponta, a lista volta igual.
export function moveTranscriptionProvider<T>(list: readonly T[], i: number, d: -1 | 1): T[] {
  const n = [...list];
  const j = i + d;
  if (i < 0 || i >= n.length || j < 0 || j >= n.length) return n;
  [n[i], n[j]] = [n[j], n[i]];
  return n;
}

// Chave apagada volta à máscara guardada: o servidor lê a máscara como "mantém a chave".
export function editTranscriptionProviderKey(
  item: TranscriptionProviderConfig, typed: string, mask: string | undefined,
): TranscriptionProviderConfig {
  return { ...item, api_key: typed || mask || '' };
}

// O servidor só troca a máscara pela chave guardada com o mesmo tipo e endpoint do item salvo:
// a chave nunca vai para outro serviço.
export function transcriptionProviderKeepsKey(
  item: Pick<TranscriptionProviderConfig, 'kind' | 'base_url'>,
  saved: Pick<TranscriptionProviderConfig, 'kind' | 'base_url'> | undefined,
): boolean {
  return !!saved && item.kind === saved.kind
    && (item.kind === 'elevenlabs' || item.base_url.trim() === saved.base_url);
}

// Tipo ou endpoint trocado num item salvo: a máscara sai, e o "falta a chave" segura o Salvar em
// vez do 400 do servidor. De volta ao tipo e endpoint salvos, a máscara volta a valer.
export function editTranscriptionProviderTarget(
  item: TranscriptionProviderConfig,
  change: Partial<Pick<TranscriptionProviderConfig, 'kind' | 'base_url'>>,
  saved: TranscriptionProviderConfig | undefined,
): TranscriptionProviderConfig {
  const next = { ...item, ...change };
  const mask = saved?.api_key;
  if (!mask) return next;
  const keeps = transcriptionProviderKeepsKey(next, saved);
  if (!keeps && next.api_key === mask) return { ...next, api_key: '' };
  if (keeps && !next.api_key) return { ...next, api_key: mask };
  return next;
}

// Só o ElevenLabs exige chave; servidores compatíveis podem autenticar de outra forma.
export function transcriptionProvidersMissingKey(list: readonly Pick<TranscriptionProviderConfig, 'api_key' | 'kind'>[]): boolean {
  return list.some((p) => p.kind === 'elevenlabs' && !p.api_key);
}

export function getTranscriptionProvidersStatus(
  server?: Server | null,
): Promise<{ providers: TranscriptionProviderStatus[] }> {
  return server
    ? apiFetchForServer(server, '/api/transcription/providers/status')
    : apiFetch('/api/transcription/providers/status', { signal: AbortSignal.timeout(8000) });
}

export function testTranscriptionProvider(id: string, audio: Blob, filename: string, server?: Server | null): Promise<{ text: string; provider: string; aviso?: string }> {
  const path = `/api/transcription/providers/${encodeURIComponent(id)}/test`;
  const options = { method: 'POST', body: audio, headers: { 'Content-Type': 'application/octet-stream', 'X-Filename': encodeURIComponent(filename) } };
  return server ? apiFetchForServer(server, path, options, 300_000)
    : apiFetch(path, { ...options, signal: AbortSignal.timeout(300_000) });
}

/**
 * Uma variável do `.env` mostrada em Avançado, só leitura.
 *
 * `valor` é `null` quando `segredo` — o backend nunca o devolve, nem mascarado, e `definida` é a
 * única coisa que a tela sabe sobre ele. `descricao` e `alerta` são CÓDIGOS que a tela traduz
 * (padrão do repo); código desconhecido não vira texto nenhum, e a linha mostra só o nome cru —
 * nunca o identificador da mensagem.
 */
export interface VariavelEnv {
  nome: string;
  valor: string | number | boolean | null;
  definida: boolean;
  segredo: boolean;
  descricao: string | null;
  alerta: string | null;
}
export interface ConfigServidor {
  campos: Record<string, CampoConfig>;
  // `terminal_panel` (Task 6, Step 8) e o unico booleano aqui -- `pty` e POSIX-only.
  somente_leitura: Record<string, string | number | boolean>;
  /**
   * IRMÃ do `somente_leitura`, nunca dentro dele: aquele é um mapa chave -> valor simples,
   * desenhado linha a linha. Opcional porque é ACRÉSCIMO — o app nativo continua lendo o que lia,
   * e um backend mais antigo responde sem a chave.
   */
  variaveis_env?: VariavelEnv[];
}

export function getConfig(): Promise<ConfigServidor> {
  // Prazo igual ao do caminho *ForServer (apiFetchForServer): o servidor ativo atras de VPN nao
  // recusa conexao — sem teto, abrir Configuracoes prendia a folha em "Carregando..." pra sempre.
  // Quem precisa de outro prazo (ou de nenhum) passa o proprio signal no init.
  return apiFetch('/api/config', { signal: AbortSignal.timeout(8000) });
}

/**
 * Estado do botão Atualizar. Sem teto de tempo curto: o pré-voo forka `git` e paga o disco frio.
 *
 * `procurar` faz o servidor ir à REDE (`git fetch`) antes de comparar. É o que separa "me diz o
 * que tu já sabe" de "vai olhar se saiu versão nova" — sem ele, o botão respondia "Tudo em dia"
 * com a foto do último fetch automático, que roda a cada 30min. Só para clique: o polling da tela
 * bate neste endpoint a cada 2s.
 */
export function getAtualizacao(procurar = false): Promise<Atualizacao> {
  return apiFetch(`/api/atualizacao${procurar ? '?procurar=1' : ''}`,
                  { signal: AbortSignal.timeout(procurar ? 120000 : 20000) });
}

/** Tela temporária "Migração para Rust": quem atende cada área, versões e consumo. */
export function getMigrationStatus(s: Server | null = null): Promise<MigrationStatus> {
  return s ? apiFetchForServer(s, '/api/migration/status', {}, 10000)
           : apiFetch('/api/migration/status', { signal: AbortSignal.timeout(10000) });
}

/** Lança a atualização. Devolve na hora — ela roda fora do processo do backend, que vai reiniciar. */
export function iniciarAtualizacao(): Promise<{ ok: boolean; pid: number }> {
  return apiFetch('/api/atualizacao/iniciar', { method: 'POST' });
}

/** Reinicia o servidor sem atualizar nada (disco já à frente do processo). 409 fora do systemd. */
export function reiniciarServidor(): Promise<{ ok: boolean; pid: number }> {
  return apiFetch('/api/atualizacao/reiniciar', { method: 'POST' });
}

/** Estado da atualização/reinício no servidor que a tela está editando. */
export function getAtualizacaoEm(s: Server | null): Promise<Atualizacao> {
  return s ? apiFetchForServer(s, '/api/atualizacao') : getAtualizacao();
}

/** O mesmo reinício, no servidor que a tela está editando (que pode não ser o ativo). */
export function reiniciarServidorEm(s: Server | null): Promise<{ ok: boolean; pid: number }> {
  return s ? apiFetchForServer(s, '/api/atualizacao/reiniciar', { method: 'POST' }) : reiniciarServidor();
}

/** A mesma atualização, no servidor que a tela está editando (que pode não ser o ativo). */
export function iniciarAtualizacaoEm(s: Server | null): Promise<{ ok: boolean; pid: number }> {
  return s ? apiFetchForServer(s, '/api/atualizacao/iniciar', { method: 'POST' }) : iniciarAtualizacao();
}

/**
 * Resumo do pensamento em português, curto. Chamado quando a pessoa ABRE o bloco — nunca no
 * carregamento da conversa, porque a maioria dos pensamentos ninguém abre.
 *
 * O backend NUNCA falha aqui: sem provedor ou com o provedor fora do ar ele devolve o texto
 * original. Quem chama trata rejeição de rede só voltando ao que já estava na tela.
 */
export function pensamentoEmPt(textos: string[]): Promise<{ textos: string[] }> {
  return apiFetch('/api/pensamento/pt', {
    method: 'POST', body: JSON.stringify({ textos }), signal: AbortSignal.timeout(30000),
  });
}

export function getConfigForServer(s: Server, prazoMs = 8000): Promise<ConfigServidor> {
  return apiFetchForServer(s, '/api/config', undefined, prazoMs);
}

// `somente_leitura` é opcional: backend mais antigo responde só com `campos`.
export type RespostaPatchConfig = Pick<ConfigServidor, 'campos'> & Partial<Pick<ConfigServidor, 'somente_leitura'>>;

export function patchConfig(mudancas: Record<string, unknown>): Promise<RespostaPatchConfig> {
  // POST, nao PATCH: o proxy na frente do backend barra PATCH (era o unico do app).
  // Mesmo teto do GET: servidor vivo demora demais pra rejeitar; sem isso o Salvar travava.
  return apiFetch('/api/config', { method: 'POST', body: JSON.stringify(mudancas), signal: AbortSignal.timeout(8000) });
}

export function patchConfigForServer(s: Server, mudancas: Record<string, unknown>): Promise<RespostaPatchConfig> {
  return apiFetchForServer(s, '/api/config', { method: 'POST', body: JSON.stringify(mudancas) });
}

// Detalhe do plano em execução (Task/Step, markdown cru). 404 = sem plano ativo, NÃO é erro — o
// chamador (PlanPanel) trata null como "nada pra mostrar", não como falha. Por isso um fetch cru
// em vez de apiFetch/apiFetchForServer: as duas lançam pra qualquer !ok, inclusive 404.
export async function getPlan(name: string, server?: Server | null): Promise<PlanDetail | null> {
  const res = await fetch(`${server ? baseOf(server) : apiEnv().getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/plan`, {
    headers: { 'Content-Type': 'application/json', ...(server ? { Authorization: `Bearer ${server.token}` } : authHeaders()) },
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<PlanDetail>;
}

// Mesmo /plan, mas de UM servidor explícito — a tela Orq mostra a execução de qualquer máquina da
// malha, e a sessão executora dela pode não estar no servidor ativo. `fetch` cru pelo mesmo motivo
// do getPlan acima: 404 aqui é "sem plano ativo", não falha.
export async function getPlanForServer(s: Server, name: string): Promise<PlanDetail | null> {
  const res = await fetch(`${baseOf(s)}/api/sessions/${encodeURIComponent(name)}/plan`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(8000),
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<PlanDetail>;
}

// Um plano do repo, pro seletor. Inclui os NÃO começados (0/N) e os completos, que a eleição
// automática descarta — são justamente os que o usuário precisa poder escolher na mão.
export interface PlanListItem {
  stem: string;      // nome do arquivo sem .md — é o que o pin grava
  name: string;      // rótulo já sem o prefixo de data
  done: number;
  total: number;
  complete: boolean;
}

export function getPlans(name: string, server?: Server): Promise<{ plans: PlanListItem[]; pinned: string | null }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/plans`;
  return server ? apiFetchForServer(server, path, undefined, SESSION_FILE_TIMEOUT_MS) : apiFetch(path);
}

// stem = null solta o pin e devolve o painel pra eleição automática.
export function setPlanPin(name: string, stem: string | null, server?: Server): Promise<{ pinned: string | null }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/plan-pin`;
  const init = { method: 'POST', body: JSON.stringify({ stem }) };
  return server ? apiFetchForServer(server, path, init, SESSION_FILE_TIMEOUT_MS) : apiFetch(path, init);
}

// Marca/desmarca um step no .md do plano. Quem marca no fluxo normal é o agente — isto é pro caso
// dele esquecer, que é justamente o que deixa o plano preso em 14/16 pra sempre.
export function setPlanStep(name: string, stem: string, idx: number, done: boolean, server?: Server | null):
    Promise<{ done: number | null; total: number | null; complete: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/plan-step`, {
    method: 'POST', body: JSON.stringify({ stem, idx, done }),
  });
}

// Encerra o plano: move o .md (e o .html irmão) pra docs/superpowers/plans/feitos/.
export function archivePlan(name: string, stem: string, server?: Server | null): Promise<{ moved: string[] }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/plan-archive`, {
    method: 'POST', body: JSON.stringify({ stem }),
  });
}

// ── Motores de modelo ───────────────────────────────────────────────────────
export interface Motor {
  label?: string;
  base_url: string;
  model: string;
  subagent_model?: string;
  context_window?: number;
  vision?: boolean | null;
  // Capacidades do harness. Sempre positivas ("true = ligado"); o backend traduz pras env vars
  // negativas (DISABLE_*) em engines.env_de.
  tool_search?: boolean;
  bundled_skills?: boolean;
  experimental_betas?: boolean;
  prompt_caching?: boolean;
  adaptive_thinking?: boolean;
  gateway_model_discovery?: boolean;
  fine_grained_tool_streaming?: boolean;
  // Manda a chave também no header `x-api-key` (além do `Authorization: Bearer`). Provedor que só
  // lê o primeiro devolve `401 Missing API key` sem isto — ver o snippet mAuthHeader na tela.
  auth_via_api_key?: boolean;
  auto_compact_window?: number;
  max_output_tokens?: number;
  // Sempre mascarada (sk-k••••••••1234). A chave inteira nunca volta do servidor.
  api_key: string;
  api_key_definida: boolean;
  // Só em motor do CLIProxyAPI local: as contas ChatGPT do proxy, ou o motivo de não as ter lido.
  cliproxy_accounts?: CliProxyAccount[];
  cliproxy_error?: string;
}
export interface CliProxyAccount { account: string; credential_id: string; email: string; label: string; prefix?: string | null }
export interface ModeloProvedor {
  id: string;
  context_length: number | null;
  vision: boolean | null;
}

export interface EnginesResponse {
  motores: Record<string, Motor>;
  // true quando engines.json existe mas não pôde ser lido (hand-edit quebrado, etc): a tela
  // precisa distinguir isto de "nenhum motor configurado" — as duas batem em `motores: {}`.
  arquivo_corrompido: boolean;
  arquivo_caminho: string;
}

export function getEngines(): Promise<EnginesResponse> {
  return apiFetch('/api/engines');
}

export function getProviders(): Promise<Record<string, { disponivel: boolean; motivo: string | null; default?: boolean }>> {
  return apiFetch('/api/providers', { signal: AbortSignal.timeout(30_000) });
}

export function getProvidersForServer(s: Server, signal?: AbortSignal): Promise<Record<string, { disponivel: boolean; motivo: string | null; default?: boolean }>> {
  // A sonda inclui leitura de login, que pode iniciar o CLI.
  return apiFetchForServer(s, '/api/providers', { signal: comTeto(signal, 30_000) }, 30_000);
}

export function getEnginesForServer(s: Server): Promise<EnginesResponse> {
  return apiFetchForServer(s, '/api/engines');
}

export function putEngine(nome: string, dados: Record<string, unknown>): Promise<{ motores: Record<string, Motor> }> {
  return apiFetch(`/api/engines/${encodeURIComponent(nome)}`, {
    method: 'PUT',
    body: JSON.stringify(dados),
  });
}

export function putEngineForServer(s: Server, nome: string, dados: Record<string, unknown>): Promise<{ motores: Record<string, Motor> }> {
  return apiFetchForServer(s, `/api/engines/${encodeURIComponent(nome)}`, {
    method: 'PUT',
    body: JSON.stringify(dados),
  });
}

export function deleteEngine(nome: string): Promise<{ ok: boolean }> {
  return apiFetch(`/api/engines/${encodeURIComponent(nome)}`, { method: 'DELETE' });
}

export function deleteEngineForServer(s: Server, nome: string): Promise<{ ok: boolean }> {
  return apiFetchForServer(s, `/api/engines/${encodeURIComponent(nome)}`, { method: 'DELETE' });
}

// Modelos que a key pode usar, direto do provedor. Também é o "Testar": erro aqui traz a mensagem
// do provedor (401, host errado), em vez de deixar o usuário sem pista.
// `nome` OU `base_url`+`api_key` — nunca os dois (o servidor rejeita com 400, pra key salva nunca
// viajar pra um endereço que o cliente digitou).
type EngineModelosBody = { nome: string } | { base_url: string; api_key: string };
// `gateway`: proxy conhecido que atende no endereço (hoje só "cliproxyapi"); null quando não sabe.
export type EngineModelosResposta = { modelos: ModeloProvedor[]; gateway?: string | null };

export function engineModelos(corpo: EngineModelosBody): Promise<EngineModelosResposta> {
  return apiFetch('/api/engines/modelos', { method: 'POST', body: JSON.stringify(corpo) });
}

export function engineModelosForServer(s: Server, corpo: EngineModelosBody): Promise<EngineModelosResposta> {
  return apiFetchForServer(s, '/api/engines/modelos', { method: 'POST', body: JSON.stringify(corpo) });
}

// CLIProxyAPI local da máquina do servidor. A chave não vem: quem grava manda
// `use_cliproxy_key: true` no PUT e o servidor a lê do config dele.
// `found=false,error=null` = não instalado; `found=true,error` = achou e não respondeu.
export interface CliproxyDeteccao {
  found: boolean;
  base_url: string | null;
  models: ModeloProvedor[];
  error: string | null;
  // Contas ChatGPT ainda sem nome no proxy: a tela pede a senha de gerenciamento só quando > 0.
  unnamed_accounts?: number | null;
  management_key_set?: boolean;
  naming_error?: string | null;
}

export type CliproxyNomeacao = Required<Pick<CliproxyDeteccao, 'unnamed_accounts' | 'management_key_set' | 'naming_error'>>;

export function putCliproxyManagementKey(management_key: string): Promise<CliproxyNomeacao> {
  return apiFetch('/api/engines/cliproxy/management-key', { method: 'PUT', body: JSON.stringify({ management_key }) });
}

export function putCliproxyManagementKeyForServer(s: Server, management_key: string): Promise<CliproxyNomeacao> {
  return apiFetchForServer(s, '/api/engines/cliproxy/management-key', { method: 'PUT', body: JSON.stringify({ management_key }) });
}

export function engineCliproxy(): Promise<CliproxyDeteccao> {
  return apiFetch('/api/engines/cliproxy');
}

export function engineCliproxyForServer(s: Server): Promise<CliproxyDeteccao> {
  return apiFetchForServer(s, '/api/engines/cliproxy');
}

/**
 * Sobe um anexo da sessão. `onProgresso` recebe 0..100 conforme os bytes saem.
 *
 * XMLHttpRequest, e não `fetch`: só ele reporta progresso de UPLOAD (`fetch` só entrega o corpo da
 * resposta, o que já chegou). Sem isso, o anel em volta do tile teria que ser inventado — animação
 * que não mede nada é pior que nenhuma, porque some da tela junto com um arquivo que ainda está
 * subindo. Cabeçalhos, teto e tratamento de erro são os mesmos do resto do arquivo.
 */
export function uploadFile(
  name: string,
  file: File,
  onProgresso?: (pct: number) => void,
  server?: Server | null,
  // Ditado: o `.webm` do Chrome é só áudio, e sem isto o servidor o trataria como vídeo (quadros
  // e uma transcrição a mais).
  opts?: { audioOnly?: boolean },
): Promise<{ path: string; frames?: string[]; transcript?: string }> {
  const base = server ? baseOf(server) : apiEnv().getBaseUrl();
  // Diário à mão: sair do `apiFetchRes` significa sair do registro, e o comentário dele avisa
  // exatamente isso. Upload é AÇÃO, então entra dando certo ou não — o mesmo id de pedido dos dois
  // lados, que é o que deixa seguir a cadeia depois.
  const req = novoReq();
  const rota = 'POST /api/sessions/:name/upload';
  const t0 = Date.now();
  const anotar = (nivel: 'ok' | 'aviso' | 'erro', codigo: string, motivo = '') =>
    registrarDiag({ evento: 'acao', nivel, codigo, ms: Date.now() - t0, req,
                    detalhe: [rota, motivo].filter(Boolean).join(' — ') }, server?.baseUrl ?? base);
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', `${base}/api/sessions/${encodeURIComponent(name)}/upload${opts?.audioOnly ? '?audio_only=1' : ''}`);
    for (const [k, v] of Object.entries(fileAuthHeader(server ?? undefined))) xhr.setRequestHeader(k, String(v));
    xhr.setRequestHeader('X-Hangar-Req', req);
    xhr.setRequestHeader('Content-Type', file.type || 'application/octet-stream');
    xhr.setRequestHeader('X-Filename', encodeURIComponent(file.name || 'arquivo'));
    // Mesmo teto do uploadFileForServer: sem ele, uma foto grande num link ruim deixava o composer
    // presto em "enviando…" pra sempre.
    xhr.timeout = 180_000;
    xhr.upload.onprogress = (e) => {
      // `lengthComputable` é falso em algumas pontes (proxy que recodifica): aí não há fração pra
      // mostrar, e quem chama decide o que fazer com a ausência.
      if (e.lengthComputable && e.total > 0) onProgresso?.(Math.round((e.loaded / e.total) * 100));
    };
    xhr.onload = () => {
      if (xhr.status === 401 && ehCredencialAtiva(server)) {
        anotar('erro', '401');
        // No core quem sabe derrubar o servidor ativo e recarregar é o ambiente injetado.
        apiEnv().onUnauthorized();
        reject(Object.assign(new Error(m.sessao_expirada()), { status: 401 }));
        return;
      }
      if (xhr.status < 200 || xhr.status >= 300) {
        // Mesma leitura de detalhe do `lerErro`, sobre o texto cru que o XHR entrega.
        let msg = xhr.responseText || xhr.statusText || `falha ${xhr.status} sem detalhe do servidor`;
        try {
          const j = JSON.parse(xhr.responseText);
          if (typeof j?.detail === 'string') msg = j.detail;
          else if (typeof j?.detail?.code === 'string') {
            msg = mensagemDeErro(j.detail.code, j.detail.params ?? {}) ?? j.detail.msg ?? j.detail.code;
          }
        } catch { /* corpo não-JSON: fica o texto cru */ }
        anotar(xhr.status >= 500 ? 'erro' : 'aviso', String(xhr.status));
        reject(Object.assign(new Error(msg), { status: xhr.status }));
        return;
      }
      try {
        const corpo = JSON.parse(xhr.responseText);
        anotar('ok', String(xhr.status));
        resolve(corpo);
      } catch (e) {
        anotar('erro', String(xhr.status), 'resposta ilegivel');
        reject(e instanceof Error ? e : new Error(String(e)));
      }
    };
    xhr.onerror = () => {
      // Sem status: nunca houve resposta. É o mesmo caso do `api.sem_rede` do apiFetchRes.
      registrarDiag({ evento: 'api.sem_rede', nivel: 'erro', ms: Date.now() - t0, req, detalhe: rota }, server?.baseUrl ?? base);
      reject(new Error(m.composer_falha_envio()));
    };
    xhr.ontimeout = () => {
      anotar('erro', 'timeout');
      reject(new Error(m.composer_falha_envio()));
    };
    xhr.send(file);
  });
}

/**
 * Envia os bytes de um áudio para a sessão: o backend salva na pasta de anexos e transcreve num
 * round-trip, devolvendo `{ path, text }`. O ditado do composer não usa mais este caminho (sobe com
 * `uploadFile` e transcreve com `transcribeUploaded`); fica para quem manda áudio sem precisar do
 * caminho antes. `limpar: true` pede `?limpar=1` e devolve também `raw` e `aviso`.
 */
export async function transcribeFile(
  name: string,
  file: File,
  opts?: OpcoesTranscribe,
  server?: Server | null,
): Promise<TranscribeResult> {
  const base = server ? baseOf(server) : apiEnv().getBaseUrl();
  const qs = queryTranscribe(opts);
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/transcribe${qs}`, {
    method: 'POST',
    headers: {
      ...fileAuthHeader(server ?? undefined),
      'Content-Type': file.type || 'application/octet-stream',
      'X-Filename': encodeURIComponent(file.name || 'audio.webm'),
    },
    body: file,
    // Teto de 5 MINUTOS, não de 2: o backend pode gastar 120s na Whisper MAIS 120s na limpeza do
    // briefing (narrar._TRAVAS_POR_ESTILO), e o teto antigo de 120s abortava a requisição com o
    // trabalho ainda em curso — a pessoa perdia o ditado inteiro por causa do relógio do navegador.
    // Continua havendo teto: sem ele, rede caída deixa o composer preso em "transcrevendo…" pra
    // sempre. Quem estourar aqui ainda tem o áudio guardado pra tentar de novo (Composer).
    signal: AbortSignal.timeout(300_000),
  });
  await ensureOk(res, server);
  return res.json() as Promise<TranscribeResult>;
}

/** `transcribeUploadedForServer` pelo servidor da sessão aberta no chat (ou o ativo). */
export async function transcribeUploaded(
  name: string,
  arquivo: string,
  opts?: OpcoesTranscribe,
  server?: Server | null,
): Promise<TranscribeResult> {
  const base = server ? baseOf(server) : apiEnv().getBaseUrl();
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/transcribe${queryArquivo(opts, arquivo)}`, {
    method: 'POST',
    headers: fileAuthHeader(server ?? undefined),
    signal: AbortSignal.timeout(300_000),
  });
  await ensureOk(res, server);
  return res.json() as Promise<TranscribeResult>;
}

/**
 * Aplica outro estilo ao texto CRU de um ditado já transcrito. Não reenvia o áudio: a Whisper já
 * rodou e devolveu o cru em `raw`, então trocar de estilo custa só a limpeza (~1-16s conforme o
 * provedor, contra isso MAIS a transcrição inteira). Sem `name` porque a limpeza não lê nada da
 * sessão. `aviso` não-nulo = a limpeza desistiu e voltou o cru, mesmo contrato do transcribeFile.
 */
export async function relimparDitado(
  texto: string,
  estilo: string,
  server?: Server | null,
): Promise<{ text: string; aviso?: string | null; estilo_aplicado?: string }> {
  // Na máquina que transcreveu: a limpeza usa a chave e o estilo configurados nela.
  return sessionFetch<{ text: string; aviso?: string | null; estilo_aplicado?: string }>(server, '/api/ditado/relimpar', {
    method: 'POST',
    body: JSON.stringify({ texto, estilo }),
    // Mesmo motivo do teto do transcribeFile, sem a parcela da Whisper: o briefing pode gastar 120s
    // no servidor (narrar._TRAVAS_POR_ESTILO) e abortar antes disso perderia a limpeza já em curso.
    signal: AbortSignal.timeout(180_000),
  });
}

export async function selectOption(name: string, option: number, server?: Server | null): Promise<void> {
  await sessionFetch<{ ok: boolean }>(server, `/api/sessions/${encodeURIComponent(name)}/select`, {
    method: 'POST',
    body: JSON.stringify({ option }),
  });
}

/** Múltipla escolha: envia o que já está marcado. Marcar (selectOption) e enviar são ações
 *  diferentes ali — ver terminal_input.submeter_multipla. */
export async function submitSelected(name: string, server?: Server | null): Promise<void> {
  await sessionFetch<{ ok: boolean }>(server, `/api/sessions/${encodeURIComponent(name)}/select/submit`, {
    method: 'POST',
  });
}

// ── Git pela sessao (cwd da sessao tmux): listar/trocar branch + status/pull ──
export interface BranchInfo {
  current: string | null;
  branches: string[];
  remotes?: string[];  // remotas sem local correspondente (nome curto); trocar pra uma faz o DWIM do switch
  dirty?: boolean;     // working tree suja -> o front avisa antes de trocar (switch carrega mudancas)
}

export function getBranches(name: string, server?: Server | null): Promise<BranchInfo> {
  return sessionFetch<BranchInfo>(server, `/api/sessions/${encodeURIComponent(name)}/branches`);
}

export function checkoutBranch(name: string, branch: string, server?: Server | null): Promise<{ current: string; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/checkout`, {
    method: 'POST',
    body: JSON.stringify({ branch }),
  });
}

export type GitAction = 'status' | 'pull' | 'fetch' | 'stash' | 'stash-pop' | 'log'
  | 'revert-abort' | 'cherry-pick-abort';

export function gitAction(name: string, action: GitAction, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git`, {
    method: 'POST',
    body: JSON.stringify({ action }),
  });
}

export interface ChangedFile {
  path: string;
  code: string;      // 2 chars XY do git porcelain: ' M', 'M ', '??', 'A '...
  staged: boolean;
  // numstat do arquivo vs HEAD. null em untracked e binario: `git diff HEAD` nao os enxerga.
  added: number | null;
  removed: number | null;
}

// `sequencer`: revert/cherry-pick em andamento (conflito ainda nao resolvido/abortado), lido do
// DISCO (CHERRY_PICK_HEAD/REVERT_HEAD) — nao de memoria de sessao. E o que permite o botao de
// abort sobreviver a um reload/reabertura da sheet enquanto o repo continua em conflito.
export function getChangedFiles(name: string, server?: Server | null): Promise<{ files: ChangedFile[]; sequencer: 'revert' | 'cherry-pick' | null }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/files`);
}

export function getFileDiff(name: string, path: string, server?: Server | null): Promise<{ path: string; diff: string; truncated: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/diff`, {
    method: 'POST',
    body: JSON.stringify({ path }),
  });
}

// Arvore de arquivos do repo da sessao (filetree.py): lista, le e busca de arquivos.
export function listFiles(name: string, path?: string, soModificados = true, server?: Server): Promise<TreeListing> {
  const q = new URLSearchParams({ so_modificados: String(soModificados) });
  if (path) q.set('path', path);
  const rota = `/api/sessions/${encodeURIComponent(name)}/files/list?${q}`;
  return server ? apiFetchForServer(server, rota, undefined, SESSION_FILE_TIMEOUT_MS) : apiFetch(rota);
}

export function readFile(name: string, path: string, server?: Server): Promise<FileContent> {
  const q = new URLSearchParams({ path });
  const rota = `/api/sessions/${encodeURIComponent(name)}/files/read?${q}`;
  return server ? apiFetchForServer(server, rota, undefined, SESSION_FILE_TIMEOUT_MS) : apiFetch(rota);
}

// Arquivo CITADO na conversa (fora da raiz da sessao), como texto editavel: mesma resposta do
// readFile — inclusive o digest, que e o que liga o botao de salvar no visor.
export function readCitedFile(name: string, path: string, server?: Server | null): Promise<FileContent> {
  const q = new URLSearchParams({ path });
  // O teto não é enfeite: o caminho citado pode estar num mount de rede ou num dispositivo lento,
  // e sem ele a Promise nunca assenta — o visor fica com o esqueleto girando para sempre, sem
  // erro e sem pista. Era o que o `fetch` cru do `abrirExterno` já garantia antes.
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/file/text?${q}`,
    { signal: AbortSignal.timeout(30_000) });
}

export function writeCitedFile(name: string, path: string, text: string, digest: string | null, server?: Server | null): Promise<{ path: string; size: number; digest: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/file/text`, {
    method: 'POST',
    body: JSON.stringify({ path, text, digest }),
    signal: AbortSignal.timeout(30_000),
  });
}

// Visão "citados": quais caminhos citados existem (relativo resolvido) e quais não.
export function resolverCitados(name: string, caminhos: string[], server?: Server | null): Promise<{ ok: Record<string, { relativo: string | null; real: string }>; faltam: string[] }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/files/resolver`, { method: 'POST', body: JSON.stringify({ caminhos }) });
}

// Busca por conteúdo varre o repo inteiro: o prazo padrão de outro servidor (8s) cortaria repo grande.
export function searchFiles(name: string, q: string, mode: 'names' | 'contents', server?: Server): Promise<SearchResult> {
  const qs = new URLSearchParams({ q, mode });
  const rota = `/api/sessions/${encodeURIComponent(name)}/files/search?${qs}`;
  return server ? apiFetchForServer(server, rota, undefined, 30_000) : apiFetch(rota);
}

// Diff de UM arquivo (git_ops.path_diff), soma desde a base da branch ou so o nao-commitado.
export function pathDiff(name: string, path: string, escopo: 'branch' | 'nao_commitado', server?: Server): Promise<PathDiff> {
  const rota = `/api/sessions/${encodeURIComponent(name)}/git/path-diff`;
  const init = { method: 'POST', body: JSON.stringify({ path, escopo }) };
  return server ? apiFetchForServer(server, rota, init, SESSION_FILE_TIMEOUT_MS) : apiFetch(rota, init);
}

export function getCommitFiles(name: string, sha: string, server?: Server | null): Promise<{ files: ChangedFile[] }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/files`);
}

export function getCommitFileDiff(name: string, sha: string, path: string, server?: Server | null): Promise<{ path: string; diff: string }> {
  const q = new URLSearchParams({ path });
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff?${q}`);
}

// Diff unificado do commit INTEIRO (todos os arquivos) — a "Show changes as unified diff" do Tortoise.
// `truncated`: o backend capa em 200KB (_DIFF_MAX em git_ops.py) — precisa chegar na UI, senao um
// diff cortado parece completo e uma decisao (ex. reset --hard) seria tomada em cima de metade dele.
export function getCommitDiff(name: string, sha: string, server?: Server | null): Promise<{ sha: string; diff: string; truncated: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff-full`);
}

export function gitRevert(name: string, sha: string, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/revert`, {
    method: 'POST', body: JSON.stringify({ sha }),
  });
}

export function gitCherryPick(name: string, sha: string, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/cherry-pick`, {
    method: 'POST', body: JSON.stringify({ sha }),
  });
}

export type GitResetMode = 'soft' | 'mixed' | 'hard';

export function gitReset(name: string, sha: string, mode: GitResetMode, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/reset`, {
    method: 'POST', body: JSON.stringify({ sha, mode }),
  });
}

export function gitCreateBranch(name: string, opts: { name: string; sha?: string; switch_after?: boolean }, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/branch`, {
    method: 'POST', body: JSON.stringify(opts),
  });
}

export function gitCreateTag(name: string, opts: { name: string; sha?: string; message?: string }, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/tag`, {
    method: 'POST', body: JSON.stringify(opts),
  });
}

// Commit vs o DISCO agora — o "Compare with working tree" do Tortoise. Mesmo teto/`truncated` do
// getCommitDiff (git_ops.py:_cap aplica aos dois).
export function getCommitDiffVsWorktree(name: string, sha: string, server?: Server | null): Promise<{ sha: string; diff: string; truncated: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/diff-worktree`);
}

// Branches (locais e remotas) que contêm o commit.
export function getCommitBranches(name: string, sha: string, server?: Server | null): Promise<{ local: string[]; remote: string[] }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/commit/${encodeURIComponent(sha)}/branches`);
}

// Um commit da view de log. Campos superset (parents/refs) pro detalhe-de-commit e o grafo (fase 2).
export interface GitCommit {
  hash: string;       // hash completo (âncora do grafo + lookup de detalhe)
  short: string;      // hash curto pra exibir
  parents: string[];  // hashes dos parents (vazio no root; 2+ num merge)
  refs: string;       // decoração %D (branches/tags), sem os parênteses; '' se nenhuma
  author: string;
  ts: number;         // author date, unix epoch (ordenação estável)
  rel: string;        // data relativa pronta ("2 hours ago")
  subject: string;
  body: string;       // corpo da mensagem (%b), sem o assunto; '' quando o commit nao tem corpo
  // true = ainda não está no upstream (o "unpublished" do VS Code). Sem upstream configurado, o
  // backend devolve false em todos: não dá pra afirmar que algo falta enviar sem ter com o que comparar.
  local?: boolean;
  col?: number;       // coluna (lane) do commit no grafo — preenchida por assign_lanes no backend
  edges?: { to_col: number; curved: boolean }[];  // arestas descendo pros parents (merge = curva)
  passthrough?: number[];  // colunas de outras lanes que cruzam esta linha sem dot (vertical cheia)
}

// `n` = quantos commits pedir (o "carregar mais" da coluna dobra a cada clique).
// `ahead`/`behind` vêm junto porque a lista é o lugar onde "falta enviar" precisa aparecer;
// `null` nos dois = branch sem upstream, e aí não há o que comparar.
export function getGitLog(name: string, q?: string, n?: number, server?: Server | null):
  Promise<{ commits: GitCommit[]; ahead: number | null; behind: number | null }> {
  const p = new URLSearchParams();
  if (q) p.set('q', q);
  if (n) p.set('n', String(n));
  const qs = p.toString() ? `?${p}` : '';
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/log${qs}`);
}

export function discardFile(name: string, path: string, server?: Server): Promise<{ ok: boolean; path: string }> {
  const rota = `/api/sessions/${encodeURIComponent(name)}/git/discard`;
  const init = { method: 'POST', body: JSON.stringify({ path }) };
  return server ? apiFetchForServer(server, rota, init, SESSION_FILE_TIMEOUT_MS) : apiFetch(rota, init);
}

export function commitFiles(name: string, message: string, paths: string[],
                            opts?: { amend?: boolean; newBranch?: string }, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/commit`, {
    method: 'POST',
    body: JSON.stringify({ message, paths, amend: opts?.amend ?? false, new_branch: opts?.newBranch ?? null }),
  });
}

// Mensagem completa do HEAD (pra pré-preencher o amend). 409 se o repo não tem commit.
export function getLastCommitMessage(name: string, server?: Server | null): Promise<{ message: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/last-message`);
}

export function gitPush(name: string, server?: Server | null): Promise<{ ok: boolean; output: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/git/push`, { method: 'POST' });
}

// Envia respostas do stepper AskUserQuestion para o backend.
// `fallback` = o backend não conseguiu dirigir o seletor da TUI, mandou Escape e entregou a
// resposta como TEXTO. É sucesso (a resposta chegou), mas o Escape aparece no transcript como
// "user declined"/"Request interrupted" — em vermelho. Sem propagar este campo, quem respondeu vê
// só o vermelho e conclui que perdeu a resposta; era o que acontecia até 27/08/2026.
export interface SessionPlanPreview { name: string; path: string; markdown?: string }

export function getSessionPlanPreview(name: string, content = true, server?: Server | null): Promise<SessionPlanPreview | null> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/plan-preview?content=${content}`);
}

export async function answerQuestions(name: string, answers: AnswerItem[], requestId?: string | number, server?: Server): Promise<{ ok: boolean; fallback?: boolean }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/answer`;
  const init = {
    method: 'POST', body: JSON.stringify({ answers, ...(requestId !== undefined ? { request_id: requestId } : {}) }),
  };
  if (!server) return apiFetch(path, init);
  const res = await apiFetchRes(path, init, server);
  if (!res.ok) await throwForServer(res, server);
  return res.json() as Promise<{ ok: boolean; fallback?: boolean }>;
}

export async function skipQuestion(name: string, requestId: string, server?: Server): Promise<{ ok: boolean }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/question/skip`;
  const init = {
    method: 'POST', body: JSON.stringify({ request_id: requestId }),
  };
  if (!server) return apiFetch(path, init);
  const res = await apiFetchRes(path, init, server);
  if (!res.ok) await throwForServer(res, server);
  return res.json() as Promise<{ ok: boolean }>;
}

// clear=true tambem limpa o input do terminal (2o Esc no backend). So passar quando havia msg pendente.
export async function interrupt(name: string, clear = false, server?: Server): Promise<void> {
  const q = clear ? '?clear=true' : '';
  const path = `/api/sessions/${encodeURIComponent(name)}/interrupt${q}`;
  const init = {
    method: 'POST',
    body: JSON.stringify({}),
  };
  await (server ? apiFetchForServer<{ ok: boolean }>(server, path, init)
                : apiFetch<{ ok: boolean }>(path, init));
}

/** Clique num botão que um mod desenhou na faixa ou num painel. `copied` e `opened` são o que o mod
 *  copiou ou mandou abrir, para quem clicou fazer no próprio aparelho. Recusa vem como erro `erro_mod_*`. */
export async function pressPluginButton(
  name: string, site: string, button: PluginControl, server?: Server,
): Promise<{ ok: boolean; copied?: string; opened?: string }> {
  return retryWithoutMod((withMod) => postPluginPress(name, withMod ? { site, ...button } : { site, key: button.key }, server));
}

function postPluginPress(name: string, body: Record<string, string>, server?: Server): Promise<{ ok: boolean; copied?: string; opened?: string }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/plugin/press`;
  const init = { method: 'POST', body: JSON.stringify(body) };
  return server ? apiFetchForServer<{ ok: boolean; copied?: string; opened?: string }>(server, path, init, 8000, true)
                : apiFetch<{ ok: boolean; copied?: string; opened?: string }>(path, init);
}

/** Servidor de antes de o pedido levar o mod recusa o campo desconhecido com 422: repete uma vez sem ele, e
 *  o servidor acha o mod pela `key`, como antes. */
async function retryWithoutMod<T>(send: (withMod: boolean) => Promise<T>): Promise<T> {
  try {
    return await send(true);
  } catch (err) {
    if ((err as { status?: unknown } | null)?.status !== 422) throw err;
    return send(false);
  }
}

/** Rota de mod que só fala de um painel (`{ site }`). */
function postPluginPane(action: 'close' | 'show', name: string, site: string, server?: Server): Promise<{ ok: boolean }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/plugin/${action}`;
  const init = { method: 'POST', body: JSON.stringify({ site }) };
  return server ? apiFetchForServer<{ ok: boolean }>(server, path, init, 8000, true) : apiFetch<{ ok: boolean }>(path, init);
}

/** Fecha o painel de mod `site`, como o ✕ do cabeçalho dele (`plugin/close`). */
export async function closePluginPane(name: string, site: string, server?: Server): Promise<{ ok: boolean }> {
  try {
    return await postPluginPane('close', name, site, server);
  } catch (err) {
    // Servidor de antes da rota `close`: o ✕ ia pelo `press` com a `key` reservada.
    if (!isMissingRoute(err)) throw err;
    return postPluginPress(name, { site, key: '__close__' }, server);
  }
}

/** Traz um painel de mod para a frente (`plugin/show`). Servidor sem a rota responde 404 ou 405 (`isMissingRoute`). */
export async function showPluginPane(name: string, site: string, server?: Server): Promise<{ ok: boolean }> {
  return postPluginPane('show', name, site, server);
}

export type PluginInputKind = 'change' | 'submit';

/** Digitação num `Input` de mod (`plugin/input`): só a sessão sem terminal aceita; com terminal vem
 *  `erro_mod_sem_digitacao`. Sempre com prazo de 8 s, o mesmo do `apiFetchForServer` (que o aplica quando há
 *  servidor): o campo manda um pedido por vez, e um pedido pendurado prenderia toda a digitação nele. */
export async function inputPluginField(
  name: string, site: string, field: PluginControl, kind: PluginInputKind, value: string, server?: Server,
): Promise<{ ok: boolean }> {
  const path = `/api/sessions/${encodeURIComponent(name)}/plugin/input`;
  return retryWithoutMod((withMod) => {
    const init = { method: 'POST', body: JSON.stringify(withMod ? { site, plugin: field.plugin, key: field.key, kind, value } : { site, key: field.key, kind, value }) };
    return server ? apiFetchForServer<{ ok: boolean }>(server, path, init, 8000, true)
                  : apiFetch<{ ok: boolean }>(path, { ...init, signal: AbortSignal.timeout(8000) });
  });
}

// Pergunta lateral (/btw do Claude Code): o backend dirige o overlay da TUI e devolve a resposta.
// Demora o que a resposta demorar (ate 120s no backend) — quem chama mostra espera.
export interface PerguntaLateral {
  question: string;
  answer: string;
  fonte: 'buffer' | 'pane';   // 'pane' = lida da tela, pode estar cortada
  ts: number;
  salvo?: boolean;            // false = respondeu, mas o histórico não foi gravado
}

export async function perguntaLateral(name: string, question: string, server?: Server | null): Promise<PerguntaLateral> {
  return sessionFetch<PerguntaLateral>(server, `/api/sessions/${encodeURIComponent(name)}/btw`, {
    method: 'POST',
    body: JSON.stringify({ question }),
  });
}

export async function historicoLateral(name: string, server?: Server | null): Promise<PerguntaLateral[]> {
  return sessionFetch<PerguntaLateral[]>(server, `/api/sessions/${encodeURIComponent(name)}/btw`);
}

export interface EtapaFerramenta { t: number | null; message: string }

/** Saída parcial de um Bash ainda rodando; `null` quando o comando não está (mais) em execução. */
export async function getBashOutput(name: string, command: string, server?: Server | null): Promise<string | null> {
  const r = await sessionFetch<{ text: string | null }>(server, `/api/sessions/${encodeURIComponent(name)}/bash-output`, {
    method: 'POST',
    body: JSON.stringify({ command }),
  });
  return r.text;
}

export async function getToolProgress(name: string, toolUseId: string, server?: Server | null): Promise<EtapaFerramenta[]> {
  return sessionFetch<EtapaFerramenta[]>(server,
    `/api/sessions/${encodeURIComponent(name)}/tool-progress/${encodeURIComponent(toolUseId)}`);
}

// Espelho do pane (overlays so-TUI): le o pane cru e manda teclas de navegacao (allowlist no backend).
export type NavKey =
  | 'Up' | 'Down' | 'Left' | 'Right'
  | 'Enter' | 'Escape' | 'Tab' | 'BTab'
  | 'PageUp' | 'PageDown' | 'Space';

// `lines` = quanto scrollback trazer acima da tela visível (o espelho pede mais ao rolar pro topo).
// `scrollback` na resposta = quantas linhas o tmux REALMENTE tem; vale 0 num TUI de tela alternada
// (Claude Code), onde pedir mais nunca traz nada e subir é papel do PageUp do próprio TUI.
export async function getPane(name: string, lines?: number, server?: Server | null): Promise<{ text: string; scrollback: number }> {
  const qs = lines ? `?lines=${lines}` : '';
  const res = await sessionFetch<{ text: string; scrollback?: number }>(server,
    `/api/sessions/${encodeURIComponent(name)}/pane${qs}`);
  return { text: res.text, scrollback: res.scrollback ?? 0 };
}

export async function sendKey(name: string, key: NavKey, server?: Server | null): Promise<void> {
  await sessionFetch<{ ok: boolean }>(server, `/api/sessions/${encodeURIComponent(name)}/keys`, {
    method: 'POST',
    body: JSON.stringify({ key }),
  });
}

// Terminal interativo (desktop): texto digitado (literal) e/ou tecla nomeada (allowlist no backend).
export async function sendTermInput(name: string, payload: { text?: string; key?: string }, server?: Server | null): Promise<void> {
  await sessionFetch<{ ok: boolean }>(server, `/api/sessions/${encodeURIComponent(name)}/term-input`, {
    method: 'POST',
    body: JSON.stringify(payload),
  });
}

// Cria (ou reata) a sessao de shell escondida do app, no cwd da sessao do agente, no servidor `srv`
// (o DONO da sessao — nunca o ativo: a sessao vive numa maquina, e o POST tem que chegar nela).
// Devolve o nome tmux real ("term-<nome>") — e ele, nao `name`, que a aba do shell usa pra conectar.
// 409 = ja existe uma sessao com esse nome que nao e o nosso shell; a mensagem do backend explica o
// motivo, e quem chama deve mostra-la (nao engolir). Mesmo molde do apiFetchForServer: sem self-heal
// de 401 (a credencial ativa e de OUTRA maquina — nao se limpa o ativo por erro de um remoto).
export function openShell(srv: Server, name: string): Promise<{ ok: true; shell: string }> {
  return apiFetchForServer<{ ok: true; shell: string }>(
    srv, `/api/sessions/${encodeURIComponent(name)}/shell`, { method: 'POST' });
}

// Abre um emulador de terminal NATIVO (janela do SO) anexado a sessao `name` no servidor `srv` (o
// dono — mesma regra do openShell). 503 = sem emulador no PATH, ou o emulador morreu logo apos
// abrir — o `detail` do erro e texto pra humano, mostrar direto.
export function openNativeTerminal(srv: Server, name: string): Promise<{ ok: true }> {
  return apiFetchForServer<{ ok: true }>(
    srv, `/api/sessions/${encodeURIComponent(name)}/open-terminal`, { method: 'POST' });
}

export interface ModelEffortBody {
  model?: string; // keyword de uma linha do picker: 'default' | 'opus' | 'fable' | 'sonnet' | …
  effort?: string; // low | medium | high | xhigh | max | ultracode
  scope: 'session' | 'default';
}

export interface ModelOption {
  id: string;              // keyword do picker (conta) ou id do provedor (motor)
  name?: string;           // rotulo exibido (so no picker da conta)
  desc?: string;           // descricao da linha do picker
  active?: boolean;
  context_length?: number | null;  // so no motor
  vision?: boolean | null;         // so no motor
}

export interface ModelOptionsResponse {
  kind: 'claude' | 'engine';
  engine: string | null;
  effort?: string | null;
  models: ModelOption[];
}

/**
 * Modelos que ESTA sessao pode escolher. Nada e chumbado no front de proposito: numa sessao da
 * conta a lista sai do proprio picker do Claude Code (ela muda com a conta e com a versao — o
 * Fable entrou e a lista fixa daqui nao soube), e numa sessao de MOTOR sai do /v1/models do
 * provedor (o picker ali so lista os 4 aliases, todos o mesmo modelo).
 * 409 = sessao ocupada/menu aberto: nao da pra ler o picker agora.
 */
// ── Cache curto dos catálogos dos seletores (modelo/esforço/permissão) ───────────────────────
// A lista muda raramente (versão do CLI, config do provedor), mas o GET demora o bastante pro
// popover abrir em "Carregando…" a CADA toque. TTL 60s + dedupe de em-voo (dois consumidores
// dividem UM GET) + invalidação nos set*. O prefetch ao trocar de sessão (Composer) aquece a
// chave antes do primeiro toque — a abertura fica instantânea.
const _catCache = new Map<string, { at: number; data: unknown; recusa?: boolean }>();
const _catEmVoo = new Map<string, Promise<unknown>>();
const _catEpoca = new Map<string, number>();   // por sessão: o set* incrementa -> GET em voo não grava dado pré-troca
const _CAT_TTL = 60_000;

function _catalogo<T>(key: string, fetcher: () => Promise<T>): Promise<T> {
  const hit = _catCache.get(key);
  if (hit && Date.now() - hit.at < _CAT_TTL) {
    return hit.recusa ? Promise.reject(hit.data) : Promise.resolve(hit.data as T);
  }
  const emVoo = _catEmVoo.get(key);
  if (emVoo) return emVoo as Promise<T>;
  const sessao = key.slice(key.lastIndexOf('|') + 1);
  const epoca = _catEpoca.get(sessao) ?? 0;
  const p = fetcher()
    .then((data) => {
      // Se um set* invalidou a sessão enquanto este GET voava, o dado é PRÉ-troca: não cacheia.
      if ((_catEpoca.get(sessao) ?? 0) === epoca) {
        _catCache.set(key, { at: Date.now(), data });
        if (_catCache.size > 100) {   // mesmo sweep do _tailCache: o TTL é lógico, a Map não pode só crescer
          for (const [k, v] of _catCache) if (Date.now() - v.at >= _CAT_TTL) _catCache.delete(k);
        }
      }
      return data;
    })
    .catch((e: unknown) => {
      // "Esta rota só existe pra Claude" é resposta FINAL pra esta sessão, não falha transitória:
      // cacheia a recusa pelo mesmo TTL. Sem isto cada montagem do Composer numa sessão Pi/Kimi
      // repetia o GET e o 400 (medido: 516 numa semana).
      if ((e as { code?: string })?.code === 'erro_rota_so_claude') {
        _catCache.set(key, { at: Date.now(), data: e, recusa: true });
      }
      throw e;
    })
    .finally(() => { _catEmVoo.delete(key); });
  _catEmVoo.set(key, p);
  return p;
}

// Troca aplicada (modelo/effort/permissão): a LISTA não muda, mas o default/atual pode — 60s de
// cache velho num rótulo errado é pior que um GET a mais na próxima abertura.
function _invalidarCatalogo(name: string): void {
  _catEpoca.set(name, (_catEpoca.get(name) ?? 0) + 1);
  for (const k of _catCache.keys()) if (k.endsWith(`|${name}`)) _catCache.delete(k);
}

export function getModelOptions(name: string, server?: Server | null): Promise<ModelOptionsResponse> {
  return _catalogo(`model|${server?.id ?? ''}|${name}`, () => sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/model/options`));
}

/**
 * Troca o modelo de uma sessao de MOTOR (digita `/model <id>`). O backend repoe o default global
 * que esse comando grava de lambuja — a troca vale so nesta sessao.
 */
export function setEngineModel(
  name: string,
  body: { model: string; effort?: string | null }, server?: Server | null,
): Promise<{ ok: boolean; model: string; result: string | null; effort_error?: string }> {
  _invalidarCatalogo(name);
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/engine/model`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/**
 * Applies a model/effort switch by driving Claude Code's interactive `/model` picker.
 * scope 'session' presses `s` (current session only); 'default' presses Enter (saved default).
 * Unlike the old full-arg `/model <arg>` command, scope 'session' does NOT change the user's
 * default for new sessions.
 */
export interface ModelEffortResposta {
  ok: boolean;
  scope?: string;
  /** Nome do nivel quando o Claude abriu "Change effort level?" no terminal: NAO pegou ainda —
   *  quem aceita e o usuario, na conversa. Tratar isto como sucesso pinta um valor que nao vale. */
  pending_confirm?: string | null;
  result?: string | null;
}

export async function setModelEffort(
  name: string,
  body: ModelEffortBody, server?: Server | null,
): Promise<ModelEffortResposta> {
  _invalidarCatalogo(name);
  return sessionFetch<ModelEffortResposta>(server,
    `/api/sessions/${encodeURIComponent(name)}/model-effort`,
    { method: 'POST', body: JSON.stringify(body) },
  );
}

// provider: quem RESPONDEU de fato (pode ter virado "local" pelo fallback do backend quando falta
// a chave da ElevenLabs mas ha comando local configurado) — nao necessariamente o que foi pedido.
export interface TtsResposta { url: string; chars: number; cached: boolean; provider: string }

// `confirm: true` repete o pedido depois que o usuario aceitou o aviso de custo (409 do backend).
// `instruction`: fase 2 (narracao guiada) — a instrucao que ja tratou este `text` via /api/tts/narrar
// (ou "" quando foi lido como esta). So entra na chave do cache do backend, nunca dispara a Groq
// aqui: quando isto chega, o texto ja esta pronto pra virar audio.
export async function sintetizarTts(
  body: { text: string; voice?: string; provider?: string; confirm?: boolean; instruction?: string },
): Promise<TtsResposta> {
  return apiFetch<TtsResposta>('/api/tts', { method: 'POST', body: JSON.stringify(body) });
}

export interface TtsVoz { id: string; nome: string }

export async function listarVozesTts(): Promise<TtsVoz[]> {
  const r = await apiFetch<{ voices: TtsVoz[] }>('/api/tts/voices');
  return r.voices;
}

export async function saldoTts(): Promise<{ usados: number | null; limite: number | null }> {
  return apiFetch('/api/tts/saldo');
}

// Fase 2 (narracao guiada): pede pra Groq tratar o texto falavel ANTES de virar audio. `used_groq`
// diz se a chamada de fato saiu (false quando a instrucao e "ler como esta" — o backend nao gasta
// token nesse caminho, e devolve o texto de volta como veio).
export interface NarrarResposta { text: string; chars_sent: number; used_groq: boolean }

export async function narrarSelecao(
  body: { text: string; code_blocks: string[]; instruction: string },
): Promise<NarrarResposta> {
  return apiFetch<NarrarResposta>('/api/tts/narrar', { method: 'POST', body: JSON.stringify(body) });
}

/**
 * Opens an SSE stream for the given session.
 * In production (same-origin), the auth cookie is sent automatically.
 * In dev, appends ?token= as fallback.
 */
// `lastEventId`: posição de retomada do transcript ("<stem-do-jsonl>:<offset>"), enviada como QUERY
// PARAM de propósito. O header `Last-Event-ID` só é reenviado quando o MESMO objeto EventSource se
// reconecta sozinho — e este app nunca deixa isso acontecer: o `onerror` fecha o es e o `connectSSE`
// cria um novo (para o auto-retry nativo não virar uma 2ª máquina de retry em paralelo), e o
// watchdog de 25s faz o mesmo. Objeto novo nasce sem memória de id, então sem este param a retomada
// exata jamais dispararia no uso real, e toda queda voltaria a custar o backfill cego de 200 linhas.
export function openEventStream(name: string, lastEventId?: string | null, req = novoReq(), server?: Server | null): EventSourceLike {
  if (server) return openEventStreamForServer(server, name, req, lastEventId);
  const base = apiEnv().getBaseUrl();
  const token = apiEnv().getToken();
  const path = `/api/sessions/${encodeURIComponent(name)}/events`;

  // Use ?token param only in dev (different origin) or when no cookie is set
  const o = apiEnv().origin;
  const isSameOrigin = !!o && (!base || base === o);
  const params = new URLSearchParams();
  if (!isSameOrigin) params.set('token', token ?? '');
  if (lastEventId) params.set('last_event_id', lastEventId);
  if (req) params.set('diag_req', req);
  // O app junta a diferença da vista dos mods (`plugin_ui_delta`, `applyPluginUiDelta`).
  params.set('ui_delta', '1');
  const qs = params.toString();
  const url = `${base}${path}${qs ? `?${qs}` : ''}`;

  return apiEnv().createEventSource(url, { withCredentials: isSameOrigin });
}

export interface SyncSetup {
  enabled: boolean;
  registered: boolean;
  user: string | null;
}

export interface SyncSetupBody {
  user: string;
  salt: string;
  auth_hash: string;
  enc_blob: { iv: string; data: string };
}

export function getSyncSetupForServer(server: Server, signal?: AbortSignal): Promise<SyncSetup> {
  return apiFetchForServer(server, '/api/sync/setup', { signal: comTeto(signal, 8000) });
}

export function setupSyncForServer(server: Server, body?: SyncSetupBody): Promise<SyncSetup> {
  return apiFetchForServer(server, '/api/sync/setup', { method: 'POST', body: JSON.stringify(body ?? {}) });
}

export function disableSyncForServer(server: Server): Promise<SyncSetup> {
  return apiFetchForServer(server, '/api/sync/setup/disable', { method: 'POST' });
}

export type Me = { role: 'owner' | 'guest'; name: string | null };

export function getMeForServer(server: Server, signal?: AbortSignal): Promise<Me> {
  return apiFetchForServer(server, '/api/me', { signal: comTeto(signal, 8000) });
}

export function createGuestForServer(
  server: Server, body: { name: string; root: string; sees_owner: boolean; owner_sees: boolean },
): Promise<{ id: string; token: string }> {
  return apiFetchForServer(server, '/api/guests', { method: 'POST', body: JSON.stringify(body) });
}

export function updateGuestForServer(
  server: Server, id: string, body: { root: string; sees_owner: boolean; owner_sees: boolean },
): Promise<{ id: string }> {
  return apiFetchForServer(server, `/api/guests/${encodeURIComponent(id)}`, { method: 'POST', body: JSON.stringify(body) });
}

export function deleteGuestForServer(server: Server, id: string): Promise<{ ok: true }> {
  return apiFetchForServer(server, `/api/guests/${encodeURIComponent(id)}/delete`, { method: 'POST' });
}

// EventSource da LISTA de UM servidor (baseUrl/token explícitos). ?token cross-origin (EventSource
// não manda header e cross-origin não leva cookie); withCredentials same-origin. Por-servidor:
// cada um tem o seu, falha isolada.
export function openSessionsStream(s: Server, req = novoReq()): EventSourceLike {
  const o = apiEnv().origin;
  const base = baseOf(s);
  const isSameOrigin = !!o && (!base || base === o);
  const params = new URLSearchParams();
  if (!isSameOrigin) params.set('token', s.token);
  if (req) params.set('diag_req', req);
  const qs = params.toString();
  const url = `${base}/api/sessions/events${qs ? `?${qs}` : ''}`;
  return apiEnv().createEventSource(url, { withCredentials: isSameOrigin });
}

// EventSource de UMA sessão de um servidor ESPECÍFICO (baseUrl/token explícitos, sem tocar no
// ativo) — usado pela grade de comparação (feature #11), que pode misturar sessões de servidores
// diferentes no mesmo relance. Mesma convenção de openSessionsStream (?token cross-origin,
// withCredentials same-origin).
export function openEventStreamForServer(s: Server, name: string, req = novoReq(), lastEventId?: string | null): EventSourceLike {
  const path = `/api/sessions/${encodeURIComponent(name)}/events`;
  const o = apiEnv().origin;
  const base = baseOf(s);
  const isSameOrigin = !!o && (!base || base === o);
  const params = new URLSearchParams();
  if (!isSameOrigin) params.set('token', s.token);
  if (lastEventId) params.set('last_event_id', lastEventId);
  if (req) params.set('diag_req', req);
  // O app junta a diferença da vista dos mods (`plugin_ui_delta`, `applyPluginUiDelta`).
  params.set('ui_delta', '1');
  const qs = params.toString();
  const url = `${base}${path}${qs ? `?${qs}` : ''}`;
  return apiEnv().createEventSource(url, { withCredentials: isSameOrigin });
}

// ── Preview: expõe um projeto local (porta) via `tailscale serve` da máquina do backend, pra ver
// num iframe. Global por máquina (slot único), não por sessão.
export interface PreviewState {
  active: boolean;
  port: number | null;
  url: string | null;
}

// ── Diário de uso (lib/diag.ts + backend/app/diag.py) ────────────────────────────────────────
export interface LinhaDiag {
  ts: string;
  evento: string;
  nivel?: 'ok' | 'aviso' | 'erro';
  origem?: 'tela' | 'servidor';
  tela?: string;
  sessao?: string;
  provider?: string;
  codigo?: string;
  detalhe?: string;
  ms?: number;
  so?: string;
  navegador?: string;
  versao?: string;
  vista?: string;
  tela_px?: string;
  cli?: string;
}

export interface ResumoDiag {
  dias: number;
  bytes: number;
  arquivos: string[];
  dias_guardados: number;
  /** As linhas mais recentes, mais novas primeiro — a prova de que está gravando. */
  ultimas: LinhaDiag[];
}

export function getDiagResumo(): Promise<ResumoDiag> {
  return apiFetch('/api/diag');
}

/** Baixa o diário como arquivo. Sai da máquina só aqui, e por ação de quem usa. */
export async function baixarDiag(): Promise<Blob> {
  const res = await fetch(`${apiEnv().getBaseUrl()}/api/diag/arquivo`, { headers: authHeaders() });
  if (!res.ok) throw Object.assign(new Error(await errorDetail(res)), { status: res.status });
  return res.blob();
}

export function getPreview(server?: Server | null): Promise<PreviewState> {
  return sessionFetch<PreviewState>(server, '/api/preview');
}

export function startPreview(port: number, server?: Server | null): Promise<{ url: string; port: number }> {
  return sessionFetch(server, '/api/preview', { method: 'POST', body: JSON.stringify({ port }) });
}

export function stopPreview(server?: Server | null): Promise<PreviewState> {
  return sessionFetch(server, '/api/preview', { method: 'DELETE' });
}

/** URL aberta no navegador embutido da sessão (app desktop). `null` = a sessão não tem navegador. */
export function getNavegadorDaSessao(name: string, server?: Server | null): Promise<{ url: string | null }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/navegador`);
}

export function getRunners(name: string, server?: Server | null): Promise<RunnersResponse> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/runners`);
}

export function startRun(name: string, command: string, server?: Server | null): Promise<RunInfo> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/run`, {
    method: 'POST',
    body: JSON.stringify({ command }),
  });
}

export function stopRun(name: string, server?: Server | null): Promise<{ ok: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/run/stop`, { method: 'POST' });
}

/** Grava a lista INTEIRA de comandos personalizados do projeto (add/editar/remover são a mesma
 * operação). Devolve a lista como o servidor a leu. */
export function setCustomRunners(
  name: string, commands: { label: string; command: string }[], server?: Server | null,
): Promise<Runner[]> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/runners/custom`, {
    method: 'POST',
    body: JSON.stringify({ commands }),
  });
}

export function getRunPane(name: string, server?: Server | null): Promise<{ pane: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/run/pane`);
}

/** Pergunta que o programa do terminal está fazendo (`screen` = fim da tela, pra dar contexto). */
export interface ShortcutQuestion { text: string; default: string; screen: string[] }

/** Terminal escondido de um atalho "shell": uma aba no painel de terminal da sessão. O pane fica
 * depois que o comando sai (`alive: false`), com a saída e o código na tela até alguém fechar. */
export interface ShortcutTerminal {
  id: string;
  label: string;
  alive: boolean;
  exit_code: number | null;
  created?: number;
  ask?: boolean;
  key?: string;
  question?: ShortcutQuestion | null;
}

/** Linha do evento `shortcut_terminals`: dono vazio + `key` = No Hangar; `origin` = quem abriu. */
export interface LiveShortcutTerminal extends ShortcutTerminal {
  owner: string;
  key: string;
  origin: string;
}

export interface ShortcutShellResult {
  ok: boolean;
  terminal?: ShortcutTerminal;
  reused?: boolean;
  focused?: boolean;
}

/** Servidor com versão anterior aos atalhos No Hangar: ignoraria `runs_in` e abriria uma cópia da sessão. */
export class OutdatedServerError extends Error {}

/** Atalho "shell" da fileira, no cwd da sessão. No servidor POSIX cada execução ganha um
 * terminal próprio (`terminal` na resposta). Se o comando sai com erro nos primeiros 2 s, volta
 * 422 com o código e o fim da saída — o terminal continua listado pra ver a saída inteira. */
export async function runShortcutShell(name: string, command: string, label?: string, pasta?: string,
                                       opts: { key?: string; hangar?: boolean; home?: boolean; ask?: boolean } = {}, server?: Server | null): Promise<ShortcutShellResult> {
  const r = await sessionFetch<ShortcutShellResult>(server, `/api/sessions/${encodeURIComponent(name)}/shortcut-shell`, {
    method: 'POST',
    body: JSON.stringify({ command, ...(label ? { label } : {}), ...(pasta ? { pasta } : {}),
      ...(opts.key ? { key: opts.key } : {}),
      ...(opts.hangar ? { runs_in: 'hangar', home: opts.home !== false } : {}),
      ask: opts.ask !== false }),
  });
  if (opts.hangar && r.reused === undefined) throw new OutdatedServerError('outdated');
  return r;
}

/** Executa o código mostrado na conversa no servidor da sessão, em terminal próprio. */
export function runCodeCommand(srv: Server, name: string, command: string, language: string | undefined, key: string): Promise<ShortcutShellResult> {
  return apiFetchForServer<ShortcutShellResult>(srv, `/api/sessions/${encodeURIComponent(name)}/run-code`, {
    method: 'POST', body: JSON.stringify({ command, key, ...(language ? { language } : {}) }),
  });
}

/** Atalhos do projeto da sessão (guardados na máquina do servidor, por repositório). */
export function getProjectShortcuts(name: string, server?: Server | null): Promise<ProjectShortcuts> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/project-shortcuts`);
}

/** Grava a lista INTEIRA do projeto; lista vazia apaga. Devolve como o servidor gravou. */
export function putProjectShortcuts(name: string, items: ProjectShortcut[], server?: Server | null): Promise<ProjectShortcuts> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/project-shortcuts`, {
    method: 'PUT',
    body: JSON.stringify({ items }),
  });
}

/** Terminais de atalho da sessão no servidor dono dela (`srv`), do mais antigo pro mais novo. */
export async function listShortcutTerminals(srv: Server, name: string): Promise<ShortcutTerminal[]> {
  const r = await apiFetchForServer<{ terminals: ShortcutTerminal[] }>(
    srv, `/api/sessions/${encodeURIComponent(name)}/shortcut-terminals`);
  return r.terminals ?? [];
}

/** Arquivo de exportação dos atalhos: a lista gravada, com cada credencial trocada por
 * `⟦SEGREDO:<nome>⟧` no backend. `removed` = quantas saíram (não vai pro arquivo). */
export interface ShortcutScript { path: string; content: string; executable: boolean }
export interface ShortcutExport {
  version: number;
  shortcuts: Shortcut[];
  scripts?: ShortcutScript[];
  warnings?: string[];
  removed: number;
}

export function exportShortcuts(
  srv?: Server | null,
  options: { ids?: string[]; includeScripts?: boolean } = {},
): Promise<ShortcutExport> {
  const query = new URLSearchParams();
  if (options.ids) for (const id of options.ids.length ? options.ids : ['']) query.append('ids', id);
  if (options.includeScripts !== undefined) query.set('include_scripts', String(options.includeScripts));
  const encoded = query.toString();
  const path = '/api/shortcuts/export' + (encoded ? `?${encoded}` : '');
  return srv ? apiFetchForServer<ShortcutExport>(srv, path) : apiFetch<ShortcutExport>(path);
}

export interface ShortcutImportResult {
  added: number;
  replaced: number;
  placeholders: { id: string; label: string; names: string[] }[];
  files?: { path: string; status: 'create' | 'replace' | 'same'; content: string }[];
  warnings?: string[];
}

/** Importação dos atalhos: sem `apply` só confere e conta; com `apply` preenche os `secrets`
 * ({id: {nome: valor}}) e junta por id à lista atual. Arquivo inválido volta 400, nada muda. */
export function importShortcuts(
  body: { data: unknown; apply?: boolean; secrets?: Record<string, Record<string, string>> },
  srv?: Server | null,
): Promise<ShortcutImportResult> {
  const path = '/api/shortcuts/import';
  const init = { method: 'POST', body: JSON.stringify(body) };
  return srv ? apiFetchForServer<ShortcutImportResult>(srv, path, init) : apiFetch<ShortcutImportResult>(path, init);
}

/** Fecha o terminal de atalho: o backend derruba o processo e a sessão tmux escondida. */
export function closeShortcutTerminal(srv: Server, name: string, id: string): Promise<{ ok: true }> {
  return apiFetchForServer<{ ok: true }>(
    srv, `/api/sessions/${encodeURIComponent(name)}/shortcut-terminals/${encodeURIComponent(id)}/close`,
    { method: 'POST' });
}

const hangarPath = (id: string, action: string) => `/api/hangar-terminals/${encodeURIComponent(id)}/${action}`;

export function closeHangarTerminal(srv: Server, id: string): Promise<{ ok: true }> {
  return apiFetchForServer(srv, hangarPath(id, 'close'), { method: 'POST' });
}
export function restartHangarTerminal(srv: Server, id: string): Promise<ShortcutShellResult> {
  return apiFetchForServer(srv, hangarPath(id, 'restart'), { method: 'POST' });
}
/** Digita a resposta e Enter. Vazio = só Enter (aceita o padrão). */
export function answerHangarTerminal(srv: Server, id: string, text: string): Promise<{ ok: true }> {
  return apiFetchForServer(srv, hangarPath(id, 'answer'), { method: 'POST', body: JSON.stringify({ text }) });
}
export function answerShortcutTerminal(srv: Server, name: string, id: string, text: string): Promise<{ ok: true }> {
  return apiFetchForServer(srv,
    `/api/sessions/${encodeURIComponent(name)}/shortcut-terminals/${encodeURIComponent(id)}/answer`,
    { method: 'POST', body: JSON.stringify({ text }) });
}
export function focusHangarTerminal(srv: Server, id: string): Promise<{ focused: boolean }> {
  return apiFetchForServer(srv, hangarPath(id, 'focus'), { method: 'POST' });
}

// Limites de uso da conta Codex (Task B) — so sessoes Codex; o back devolve 400 pra Claude.
export function getLimits(name: string, server?: Server | null): Promise<SessionLimits> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/limits`);
}

// Modelo + reasoning effort do Codex (Task C) — so sessoes Codex; o back devolve 400 pra Claude.
export function getCodexModels(name: string, server?: Server | null): Promise<CodexModelsResponse> {
  // O catálogo pode ser estável; a escolha atual também muda pelo terminal.
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/models`);
}

export function setCodexMode(name: string, mode: 'default' | 'plan', server?: Server | null): Promise<CodexModelsResponse['current']> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/codex/mode`, {
    method: 'POST', body: JSON.stringify({ mode }),
  });
}

export function implementCodexPlan(name: string, server?: Server | null): Promise<void> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/codex/plan/implement`, {
    method: 'POST',
  });
}

// Atualiza as configurações nativas compartilhadas pelo chat e pelo terminal.
export function setCodexModel(name: string, model: string, effort?: string | null, server?: Server | null): Promise<void> {
  _invalidarCatalogo(name);
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/model`, {
    method: 'POST',
    body: JSON.stringify({ model, effort: effort ?? undefined }),
  });
}

// ── Modo de permissao de uma sessao Codex (`/permissions` da TUI) ─────────────────────────────
// Sem cache de catalogo, ao contrario do modelo: a lista carrega QUAL modo esta ativo, e ele muda
// pelo terminal tambem. Cada abertura da pilula pergunta de novo.
export interface CodexPermissionMode {
  numero: number;
  nome: string;
  desc: string;
  cursor: boolean;
  atual: boolean;
}

export function getCodexPermissions(
  name: string, server?: Server | null,
): Promise<{ modes: CodexPermissionMode[]; current: string | null }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/codex-permissions`);
}

export function setCodexPermission(name: string, mode: string, server?: Server | null): Promise<{ current: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/codex-permissions`, {
    method: 'POST',
    body: JSON.stringify({ mode }),
  });
}

// ── Modelo + nivel de raciocinio de uma sessao Pi ─────────────────────────────────────────────
// 409 = extensao hangar-state.ts ausente/desatualizada no Pi (o backend manda a instrucao no detail).

export function getPiModels(name: string, server?: Server | null): Promise<PiModelsResponse> {
  return _catalogo(`pi|${server?.id ?? ''}|${name}`, () => sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/pi/models`));
}

// Aplica na sessao viva (digita /cp-model e/ou /cp-think). A resposta e o READ-BACK: o Pi clampa o
// nivel pro que o modelo suporta, entao quem manda no rotulo e o que voltou, nao o que foi pedido.
export function setPiModel(
  name: string,
  body: { provider?: string; model?: string; effort?: string | null }, server?: Server | null,
): Promise<{ ok: boolean; current: PiModelsResponse['current']; thinking: string | null; levels: string[] }> {
  _invalidarCatalogo(name);
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/pi/model`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

// ── Modelo de uma sessao Kimi ────────────────────────────────────────────────────────────────
// Catalogo do ~/.kimi-code/config.toml; a troca dirige o picker do /model (busca pelo alias +
// Alt+S, so a sessao) e a resposta e o READ-BACK da linha "Switched to …" — quem manda no rotulo
// e o `current` que voltou, nao o que foi pedido. 409 = sessao trabalhando/terminal aberto/sem
// confirmacao; 422 = alias fora do catalogo.

export interface KimiModel {
  alias: string;           // "provider/id" — o que a busca do picker casa e o POST manda
  provider: string;
  id: string;
  name: string;            // display_name (repete entre providers: K3 existe nos dois)
  context_length?: number | null;
  efforts?: string[];
  default_effort?: string | null;
}

export function getKimiModels(name: string, server?: Server | null): Promise<{ models: KimiModel[]; default: string | null }> {
  return _catalogo(`kimi|${server?.id ?? ''}|${name}`, () => sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/kimi/models`));
}

export function setKimiModel(
  name: string,
  body: { model?: string; effort?: string }, server?: Server | null,
): Promise<{ ok: boolean; current: { alias: string; name: string } | null; effort: string | null; result: string | null }> {
  _invalidarCatalogo(name);
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/kimi/model`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

// ── Modo de permissão do Claude (Task 5) ────────────────────────────────────────
// Leitura pelo rodapé (⏸/⏵⏵) e troca via BTab. 409 = sessão não é claude, terminal
// aberto, menu aberto no pane, ou alvo fora do ciclo / teto de 6 teclas. Sessão trabalhando
// NÃO recusa: BTab troca o modo no meio do turno, como no terminal.
// GET devolve o ciclo vivo (4 ou 5) + o atual; POST devolve o que FICOU.

export function writeFile(name: string, path: string, text: string, digest: string | null, server?: Server): Promise<{ path: string; size: number; digest: string }> {
  const rota = `/api/sessions/${encodeURIComponent(name)}/files/write`;
  const init = { method: 'POST', body: JSON.stringify({ path, text, digest }) };
  return server ? apiFetchForServer(server, rota, init, 30_000) : apiFetch(rota, init);
}

export function getPermissionModes(name: string, sondar = false, server?: Server | null): Promise<{ current: string; modes: string[]; sondavel: boolean; restaurado?: boolean; previous_non_plan: string }> {
  // Fora do cache de catálogo (revisão): o `current` muda FORA do app — shift+tab no terminal da
  // sessão — e a pill lê pelo poll do Composer; cacheado, o modo aparecia errado por até 60s.
  // A sonda (sondar=1) segue ação viva, como sempre foi.
  const qs = sondar ? "?sondar=1" : "";
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/permission-modes${qs}`);
}

/** Troca a sessão Claude entre terminal e sem terminal, na mesma conversa. Só ociosa (409 com o motivo). */
export function setModoExecucao(name: string, terminal: boolean, server?: Server | null): Promise<{ ok: boolean; terminal: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/modo-execucao`, {
    method: 'POST',
    body: JSON.stringify({ terminal }),
  });
}

/** Recicla o processo da sessão Claude sem terminal na mesma conversa (relê MCP/hooks/settings). Só ociosa (409 com o motivo). */
export function recarregarSessao(name: string, server?: Server | null): Promise<{ ok: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/recarregar`, { method: 'POST' });
}

/** Conta Claude para onde a conversa pode ir: `pct` é a janela de cota mais cheia (null = sem leitura);
 *  `low` pede confirmação com aviso, `full` não aceita a conversa. */
export interface AccountTarget { path: string; label: string; pct: number | null; low: boolean; full: boolean }

/** Contas de destino da sessão, sem a atual e com a de mais folga primeiro. */
export function listAccountTargets(name: string, server?: Server | null): Promise<AccountTarget[]> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/conta`);
}

/** Continua a mesma conversa noutra conta Claude (`path` de `listAccountTargets`). Só ociosa (409 com o motivo). */
export function setSessionAccount(name: string, configDir: string, server?: Server | null): Promise<{ ok: boolean; config_dir: string }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/conta`, {
    method: 'POST',
    body: JSON.stringify({ config_dir: configDir }),
  });
}

export function setPermissionMode(name: string, mode: string, server?: Server | null): Promise<{ mode: string; current: string; previous_non_plan: string }> {
  _invalidarCatalogo(name);
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/permission-mode`, {
    method: 'POST',
    body: JSON.stringify({ mode }),
  });
}

// ── Loop runner (Task 9+): obter, criar, parar e resolver loops autonomos por sessao ───────────

// Obtem o estado atual de um loop (ou null se nao existe) e sugestoes de próximas ações.
export async function getLoopForServer(s: Server, name: string): Promise<{ loop: LoopState | null; suggestions: string[] }> {
  const res = await fetch(`${baseOf(s)}/api/sessions/${encodeURIComponent(name)}/loop`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    signal: AbortSignal.timeout(8000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState | null; suggestions: string[] }>;
}

// Cria um novo loop com goal, check_cmd opcional, max_iters e require_branch.
export async function createLoopForServer(s: Server, name: string, body: { goal: string; check_cmd?: string | null; max_iters?: number; require_branch?: boolean }): Promise<{ loop: LoopState }> {
  const res = await fetch(`${baseOf(s)}/api/sessions/${encodeURIComponent(name)}/loop`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState }>;
}

// Para um loop em execucao (muda status para 'stopped').
// Refina o objetivo do loop via claude -p efemero no backend (boas praticas embutidas no prompt).
// Timeout proprio de 60s: o claude -p leva segundos e nao e mutacao — abortar e seguro.
export async function refineLoopForServer(
  s: Server,
  name: string,
  goal: string,
  check_cmd: string | null,
): Promise<{ goal: string }> {
  const res = await fetch(`${baseOf(s)}/api/sessions/${encodeURIComponent(name)}/loop/refine`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ goal, check_cmd }),
    signal: AbortSignal.timeout(60000),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ goal: string }>;
}

export async function stopLoopForServer(s: Server, name: string): Promise<{ loop: LoopState }> {
  const res = await fetch(`${baseOf(s)}/api/sessions/${encodeURIComponent(name)}/loop`, {
    method: 'DELETE',
    headers: { Authorization: `Bearer ${s.token}` },
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState }>;
}

// Resolve um loop no estado 'done_claimed' (accept=true) ou 'stopped' (accept=false).
export async function resolveLoopForServer(s: Server, name: string, accept: boolean): Promise<{ loop: LoopState }> {
  const res = await fetch(`${baseOf(s)}/api/sessions/${encodeURIComponent(name)}/loop/resolve`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
    body: JSON.stringify({ accept }),
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<{ loop: LoopState }>;
}

// Com `stream=1` o backend manda uma linha por etapa e o resultado na última. Hangar antigo
// ignora o parâmetro e devolve JSON puro: aí só o resultado, sem as etapas.
async function readConfigSyncStream<T>(res: Response, onProgress: (p: ConfigSyncProgress) => void,
  onPlain: () => void): Promise<T> {
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  if (!res.headers.get('content-type')?.includes('ndjson') || !res.body) {
    onPlain();
    return res.json() as Promise<T>;
  }
  const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
  // Só a linha `error` é falha certa; corte, prazo ou linha ilegível no meio deixam o
  // resultado em aberto (o servidor segue aplicando), e a mensagem diz para conferir a máquina.
  let rest = '';
  let last: { type: string; result?: T; status?: number; detail?: unknown } | null = null;
  const take = (line: string) => {
    if (!line.trim()) return;
    const ev = JSON.parse(line);
    if (ev.type === 'progress') onProgress(ev as ConfigSyncProgress);
    else last = ev;
  };
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) {
        take(rest);
        break;
      }
      rest += value;
      const lines = rest.split('\n');
      rest = lines.pop() ?? '';
      for (const line of lines) {
        take(line);
        if (last) break;
      }
      if (last) break;
    }
  } catch (e) {
    // Cancelamento de quem chamou não é corte de stream; os demais guardam a causa.
    if (isAbortError(e)) throw e;
    throw new Error(m.shared_config_stream_cut(), { cause: e });
  } finally {
    void reader.cancel().catch(() => {});
  }
  const end = last as { type: string; result?: T; status?: number; detail?: unknown } | null;
  if (end?.type === 'done') return end.result as T;
  if (end?.type === 'error') {
    const detail = formataErro(end.detail) ?? String(end.detail);
    throw Object.assign(new Error(`${end.status}: ${detail}`), { status: end.status });
  }
  throw new Error(m.shared_config_stream_cut());
}

// Configuração compartilhada. O manifesto soma o disco inteiro de skills da máquina, e a
// aplicação instala plugins no destino: os prazos são de minutos, não os 8s de uma leitura.
export async function getConfigSyncManifestForServer(s: Server, signal?: AbortSignal,
  onProgress?: (p: ConfigSyncProgress) => void, onPlain?: () => void): Promise<ConfigSyncManifest> {
  if (!onProgress) return apiFetchForServer(s, '/api/config-sync/manifest', { signal: comTeto(signal, 60_000) }, 60_000);
  const res = await apiFetchRes('/api/config-sync/manifest?stream=1', { signal: comTeto(signal, 180_000) }, s);
  return readConfigSyncStream(res, onProgress, onPlain ?? (() => {}));
}

export async function getConfigSyncBundleForServer(s: Server, items: readonly ConfigSyncItem[], signal?: AbortSignal,
  keys?: Partial<Record<ConfigSyncItem, string[]>>): Promise<Blob> {
  const escolha = keys && Object.keys(keys).length ? `&keys=${encodeURIComponent(JSON.stringify(keys))}` : '';
  const res = await apiFetchRes(`/api/config-sync/bundle?items=${encodeURIComponent(items.join(','))}${escolha}`,
    { signal: comTeto(signal, 180_000) }, s);
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  return res.blob();
}

// Lote inteiro numa chamada ao provedor: o prazo é o de uma tradução longa, não o de uma leitura.
export function translateConfigSyncTextsForServer(s: Server, texts: string[], lang: 'pt' | 'en', signal?: AbortSignal): Promise<{ texts: string[]; error: string }> {
  return apiFetchForServer(s, '/api/config-sync/translate', {
    method: 'POST',
    body: JSON.stringify({ texts, lang }),
    headers: { 'Content-Type': 'application/json' },
    signal: comTeto(signal, 300_000),
  }, 300_000);
}

export async function applyConfigSyncForServer(s: Server, items: readonly ConfigSyncItem[], bundle: Blob, signal?: AbortSignal,
  onProgress?: (p: ConfigSyncProgress) => void, onPlain?: () => void): Promise<ConfigSyncReport> {
  const res = await apiFetchRes(`/api/config-sync/apply?items=${encodeURIComponent(items.join(','))}${onProgress ? '&stream=1' : ''}`, {
    method: 'POST',
    body: bundle,
    headers: { 'Content-Type': 'application/gzip' },
    signal: comTeto(signal, 600_000),
  }, s);
  if (onProgress) return readConfigSyncStream(res, onProgress, onPlain ?? (() => {}));
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  return res.json() as Promise<ConfigSyncReport>;
}

// Compartilhar sessão (dono). Todas no servidor ATIVO: a Sidebar usa `withServer`.
export async function createShare(name: string, local = false, server?: Server | null): Promise<ShareCreated> {
  const res = await apiFetchRes(`/api/sessions/${encodeURIComponent(name)}/share${local ? '?local=true' : ''}`, { method: 'POST' }, server ?? undefined);
  if (res.status === 409) {
    const corpo = (await res.clone().json().catch(() => null)) as { detail?: EnvelopeErro } | null;
    const d = corpo?.detail;
    if (d?.code === 'erro_compartilhar_pre_requisito') {
      throw new SharePrerequisiteError(
        Array.isArray(d.params?.missing) ? d.params.missing : [],
        typeof d.params?.fix === 'string' ? d.params.fix : '',
        formataErro(d) ?? m.erro_compartilhar_pre_requisito(),
        tailscaleEnableUrl(d.params?.enable_url),
      );
    }
  }
  await ensureOk(res, server);
  return res.json() as Promise<ShareCreated>;
}

// Só consulta: não liga o Funnel. Sem tailscale responde 409, e quem confere segue esperando.
export async function sharePrereqs(server?: Server | null): Promise<SharePrereqs> {
  const r = await sessionFetch<SharePrereqs>(server, '/api/share/prereqs');
  return { ...r, enable_url: tailscaleEnableUrl(r.enable_url) };
}

export function listShares(name: string, server?: Server | null): Promise<{ shares: ShareInfo[] }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/share`);
}

export function revokeShare(name: string, id: string, server?: Server | null): Promise<{ ok: boolean }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/share/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export function revokeAllShares(name: string, server?: Server | null): Promise<{ ok: boolean; revoked: number }> {
  return sessionFetch(server, `/api/sessions/${encodeURIComponent(name)}/share`, { method: 'DELETE' });
}

// Stream da lista de um convite caiu: 410 aqui é o dono ter encerrado, e o gancho do `apiFetchRes`
// já avisou (401 vale o mesmo que 410). 503 devolve false (indisponível por instantes, não encerrado). `probe`: a queda acabou
// de esfriar o servidor, e esta pergunta não pode esperar o prazo.
export async function checkInviteForServer(s: Server): Promise<boolean> {
  const res = await apiFetchRes('/api/sessions', { signal: AbortSignal.timeout(4000) }, s, true);
  return res.status === 410 || res.status === 401;
}

// ── Páginas de servidor do app (Configurações): variantes com o servidor explícito ────────────
// O menu de Configurações escolhe a máquina; ler do servidor "ativo" global trocaria de máquina calado.

export function getOrqPoliticaForServer(s: Server, signal?: AbortSignal): Promise<import('./orquestracao').OrqPolitica> {
  return apiFetchForServer(s, '/api/orquestracao/politica', { signal: comTeto(signal, 10_000) }, 10_000);
}

export function putOrqContaForServer(
  s: Server,
  conta: string,
  body: { provider: string; apelido?: string; modelos?: string[]; trocar?: boolean; ligada?: boolean; mtime: number },
): Promise<{ ok: boolean; mtime: number }> {
  return apiFetchForServer(s, `/api/orquestracao/politica/${encodeURIComponent(conta)}`,
    { method: 'PUT', body: JSON.stringify(body) }, 10_000);
}

export function getDiagSummaryForServer(s: Server, signal?: AbortSignal): Promise<ResumoDiag> {
  return apiFetchForServer(s, '/api/diag', { signal: comTeto(signal, 20_000) }, 20_000);
}

/** O diário inteiro (NDJSON) como texto: no celular ele sai pela folha de compartilhar. */
export async function getDiagFileForServer(s: Server): Promise<string> {
  const res = await apiFetchRes('/api/diag/arquivo', { signal: AbortSignal.timeout(60_000) }, s);
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  return res.text();
}

/** Um item da saúde de um harness (`/api/harness`): `codigo`/`params` viram texto pelas chaves `harness_*`. */
export interface HarnessItem {
  id: string;
  ok: boolean | null;
  codigo: string;
  params?: Record<string, string>;
  /** Id do conserto que o servidor sabe fazer; `sync:` = sincronizar. */
  conserto: string | null;
  info?: boolean;
}
export interface HarnessCli { id: string; nome: string; instalado: boolean; versao: string | null; itens: HarnessItem[] }

/** Estado da instalação de um CLI, que vive no servidor e é lido por polling. */
export interface HarnessInstall {
  fase: string;
  harness?: string | null;
  etapa?: string | null;
  passo?: number;
  total?: number;
  log?: string[];
  avisos?: string[];
  ok?: boolean | null;
  erro?: string | null;
  comandos?: Record<string, string>;
  manual?: Record<string, string>;
}

// `--version` de cinco CLIs em série no servidor: o prazo é de leitura lenta.
export function getHarnessesForServer(s: Server, signal?: AbortSignal): Promise<HarnessCli[]> {
  return apiFetchForServer(s, '/api/harness', { signal: comTeto(signal, 30_000) }, 30_000);
}

export function repairHarnessForServer(s: Server, id: string): Promise<{ feito: string; harnesses: HarnessCli[] }> {
  return apiFetchForServer(s, `/api/harness/conserto/${id.split('/').map(encodeURIComponent).join('/')}`,
    { method: 'POST' }, 120_000);
}

export function getHarnessInstallForServer(s: Server, signal?: AbortSignal): Promise<HarnessInstall> {
  return apiFetchForServer(s, '/api/harness/instalar', { signal: comTeto(signal, 15_000) }, 15_000);
}

export function startHarnessInstallForServer(s: Server, cli: string): Promise<HarnessInstall> {
  return apiFetchForServer(s, `/api/harness/instalar/${encodeURIComponent(cli)}`, { method: 'POST' }, 30_000);
}
