// Ditado por sessão, fora do Composer: a transcrição pertence à SESSÃO, e trocar de sessão desmonta
// o Composer no meio dela. Quem está montado assina (Composer, ou o Chat sem Composer); sem
// ninguém, o texto vai para o FIM do rascunho guardado da sessão.
import { SvelteMap } from 'svelte/reactivity';
import * as m from '../paraglide/messages';
import { transcribeUploaded, uploadFile, type MotivoFim, type TranscribeResult } from '@hangar/core';
import type { Server } from './auth';
import { sessionsStore } from './sessionsStore.svelte';

export type DictationOpts = {
  /** Mic: pede limpeza do texto e abre a barra de versões. Áudio anexado não. */
  ditado: boolean;
  avisoTeto?: boolean;
  autoEnvio?: boolean;
  motivo?: MotivoFim | null;
  estilo?: string;
};

export type DictationIdentity = { serverId: string; name: string; jsonl: string | null; epoca: number };

export type DictationEntry = {
  id: number;
  identity: DictationIdentity;
  server: Server | undefined;
  status: 'inflight' | 'ready' | 'failed';
  opts: DictationOpts;
  /** Blob da gravação: player e "de novo" sem rede enquanto a aba vive. */
  file?: File;
  /** Nome do áudio na pasta da sessão no servidor: player da barra e transcrição pelo nome. */
  arquivo?: string;
  /** Caminho absoluto devolvido pelo upload: só ele alcança o áudio depois de um `/clear`. */
  path?: string;
  result?: TranscribeResult;
  error?: string;
  /** Já escrito no rascunho guardado: quem montar depois só mostra o aviso. */
  stored?: boolean;
};

/** Por que o `start` não começou; `started` = começou. */
export type DictationStart = 'started' | 'empty' | 'inflight' | 'undelivered' | 'onlyOnDevice';

export interface DictationReceiver {
  deliver(e: DictationEntry): void | Promise<void>;
  restored?(e: DictationEntry): void;
  /** `false` = está montado mas ainda não pode receber; o resultado espera o `redeliver`. */
  accepts?(e: DictationEntry): boolean;
}

export function dictationKey(serverId: string, name: string): string {
  return `${serverId}::${name}`;
}
// Chaves do aparelho levam o servidor: a sessão "hangar" de duas máquinas não divide rascunho.
export const draftStorageKey = (serverId: string, name: string) => `cp-draft:${serverId}::${name}`;
export const dictationBarKey = (serverId: string, name: string) => `cp-ditado:${serverId}::${name}`;
const pendingKey = (serverId: string, name: string) => `cp-ditado-pendente:${serverId}::${name}`;

/** Lê a chave nova; sem ela, move a antiga (só pelo nome) para a nova, uma vez. */
export function readMigrating(nova: string, velha: string): string | null {
  try {
    const v = localStorage.getItem(nova);
    if (v !== null) return v;
    const antiga = localStorage.getItem(velha);
    if (antiga === null) return null;
    localStorage.setItem(nova, antiga);
    localStorage.removeItem(velha);
    return antiga;
  } catch {
    return null;
  }
}

/** Rascunho do campo. Valor antigo, só texto, vale como transcript desconhecido. */
export function parseStoredDraft(cru: string | null): { text: string; jsonl: string | null } {
  if (!cru) return { text: '', jsonl: null };
  try {
    const d = JSON.parse(cru);
    if (d && typeof d === 'object' && typeof d.text === 'string') {
      return { text: d.text, jsonl: typeof d.jsonl === 'string' ? d.jsonl : null };
    }
  } catch { /* texto cru de versão anterior */ }
  return { text: cru, jsonl: null };
}

const entries = new SvelteMap<string, DictationEntry>();
const receivers = new Map<string, DictationReceiver[]>();
// Pendente guardado no aparelho, relido depois de recarregar. Cache por chave; `versao` acorda quem
// lê pelo `get` quando o guardado muda.
const persisted = new Map<string, DictationEntry | null>();
let versao = $state(0);
let nextId = 0;
const VERSOES = ['limpar', 'prosa', 'briefing'];
const baseName = (p?: string) => p?.split(/[\\/]/).pop() || undefined;
const recriada = (i: DictationIdentity) => sessionsStore.epoca(i.serverId, i.name) !== i.epoca;
/** Falha cuja gravação nunca chegou ao servidor: o Blob na memória é a única cópia. */
const onlyOnDevice = (e?: DictationEntry) =>
  e?.status === 'failed' && !!e.file && !e.path && !e.arquivo;
const jsonlNaLista = (serverId: string, name: string) =>
  sessionsStore.sessionsForServer(serverId).find((s) => s.name === name)?.jsonl ?? null;

// Nome solto enquanto o transcript é o de quem gravou: é o único que o convidado pode mandar.
// Depois de `/clear` o áudio ficou na pasta do transcript anterior, que só o caminho absoluto alcança.
function alvoGuardado(e: DictationEntry, agora: string | null): string | undefined {
  const limpou = agora !== null && e.identity.jsonl !== null && agora !== e.identity.jsonl;
  return (limpou && e.path) || e.arquivo || e.path;
}

function persist(e: DictationEntry) {
  const { serverId, name, jsonl } = e.identity;
  try {
    localStorage.setItem(pendingKey(serverId, name), JSON.stringify({
      arquivo: e.arquivo, path: e.path, error: e.error ?? null, jsonl,
      opts: { ditado: e.opts.ditado, estilo: e.opts.estilo, avisoTeto: e.opts.avisoTeto },
    }));
  } catch { /* sem storage: a falha vale só enquanto a aba vive */ }
  persisted.delete(dictationKey(serverId, name));
  versao++;
}

function forget(serverId: string, name: string) {
  try { localStorage.removeItem(pendingKey(serverId, name)); } catch { /* sem storage */ }
  persisted.delete(dictationKey(serverId, name));
  versao++;
}

function readPersisted(serverId: string, name: string): DictationEntry | null {
  const key = dictationKey(serverId, name);
  if (persisted.has(key)) return persisted.get(key)!;
  let e: DictationEntry | null = null;
  try {
    const d = JSON.parse(localStorage.getItem(pendingKey(serverId, name)) ?? 'null');
    if (d && (typeof d.path === 'string' || typeof d.arquivo === 'string')) {
      e = {
        id: ++nextId, status: 'failed', server: undefined,
        opts: { ditado: d.opts?.ditado === true, autoEnvio: false, avisoTeto: d.opts?.avisoTeto === true,
          estilo: typeof d.opts?.estilo === 'string' ? d.opts.estilo : undefined },
        arquivo: typeof d.arquivo === 'string' ? d.arquivo : undefined,
        path: typeof d.path === 'string' ? d.path : undefined,
        // Sem erro guardado: a PWA morreu com a transcrição no ar.
        error: typeof d.error === 'string' ? d.error : m.composer_ditado_interrompido(),
        identity: { serverId, name, jsonl: typeof d.jsonl === 'string' ? d.jsonl : null,
          epoca: sessionsStore.epoca(serverId, name) },
      };
    }
  } catch { e = null; }
  persisted.set(key, e);
  return e;
}

function begin(key: string, entry: DictationEntry, jsonlAgora: string | null = null) {
  entries.set(key, entry);
  // A época de recriação só anda com a lista de sessões assinada; no celular ninguém a segura com
  // o Chat aberto, então o ditado segura enquanto está no ar.
  sessionsStore.retain();
  void run(key, entry, jsonlAgora);
}

async function run(key: string, entry: DictationEntry, jsonlAgora: string | null) {
  const { identity, server, opts } = entry;
  const { serverId, name } = identity;
  const q = { limpar: opts.ditado, estilo: opts.estilo };
  let atual = entry;
  const vivo = () => entries.get(key) === atual;
  const falhar = (campos: Partial<DictationEntry>) => {
    if (!vivo()) return;
    atual = { ...atual, status: 'failed', ...campos };
    entries.set(key, atual);
    if (atual.path || atual.arquivo) persist(atual);
    else forget(serverId, name);
  };
  const descartar = () => {
    if (!vivo()) return;
    forget(serverId, name);
    atual = { ...atual, status: 'failed', file: undefined, arquivo: undefined, path: undefined,
      error: m.composer_ditado_sessao_recriada() };
    entries.set(key, atual);
  };
  try {
    let alvo = atual.path || atual.arquivo
      ? alvoGuardado(atual, jsonlAgora ?? jsonlNaLista(serverId, name)) : undefined;
    if (!alvo) {
      // Sobe ANTES de transcrever: o caminho fica guardado mesmo que a resposta nunca volte.
      const { path } = await uploadFile(name, atual.file!, undefined, server, { audioOnly: true });
      if (!vivo()) return;
      atual = { ...atual, path, arquivo: baseName(path) };
      entries.set(key, atual);
      persist(atual);
      alvo = atual.arquivo!;
    }
    const result = await transcribeUploaded(name, alvo, q, server);
    if (!vivo()) return;
    if (recriada(identity)) { descartar(); return; }
    if (!result.text.trim()) { falhar({ error: m.composer_transcricao_vazia() }); return; }
    atual = { ...atual, status: 'ready', result };
    entries.set(key, atual);
    deliver(key);
  } catch (err) {
    if (!vivo()) return;
    console.error(m.composer_transcricao_falhou(), err);
    if (recriada(identity)) { descartar(); return; }
    const status = (err as { status?: number } | null)?.status;
    // 503 = nenhum serviço de transcrição configurado: diz onde resolver, não só "falhou".
    const code = (err as { code?: string } | null)?.code;
    falhar({ error: status === 503 && code === 'transcription_not_configured' ? m.composer_transcription_not_configured()
      : err instanceof Error ? err.message : m.composer_falha_transcricao() });
  } finally {
    sessionsStore.release();
  }
}

function deliver(key: string) {
  const e = entries.get(key);
  if (!e || e.status !== 'ready') return;
  // Resultado na memória da sessão morta não entra na recriada com o mesmo nome, mas avisa.
  if (recriada(e.identity)) {
    forget(e.identity.serverId, e.identity.name);
    entries.set(key, { ...e, status: 'failed', result: undefined, stored: undefined, file: undefined,
      arquivo: undefined, path: undefined, error: m.composer_ditado_sessao_recriada() });
    return;
  }
  const montados = receivers.get(key) ?? [];
  const r = montados.findLast((x) => x.accepts?.(e) ?? true);
  if (r) {
    entries.delete(key);
    forget(e.identity.serverId, e.identity.name);
    if (e.stored) r.restored?.(e);
    else void (async () => r.deliver(e))().catch((err) => receiverFailed(key, e, err));
    return;
  }
  if (montados.length || e.stored) return;   // tela montada, ainda não pronta: espera o redeliver
  if (storeInDraft(e)) {
    forget(e.identity.serverId, e.identity.name);
    entries.set(key, { ...e, stored: true });
  }
}

// O campo quebrou no meio da inserção: o texto vai para o rascunho e fica escrito no aviso, que
// sobrevive a recarregar. Nunca some calado.
function receiverFailed(key: string, e: DictationEntry, err: unknown) {
  console.error('dictation: receiver failed', err);
  storeInDraft(e);
  if (entries.has(key)) return;   // outro ditado já começou: não pisa nele
  const falha: DictationEntry = { ...e, status: 'failed', result: undefined, stored: undefined,
    error: m.composer_ditado_nao_entrou({
      erro: err instanceof Error ? err.message : String(err), text: e.result!.text.trim() }) };
  entries.set(key, falha);
  if (falha.path || falha.arquivo) persist(falha);
}

function storeInDraft(e: DictationEntry): boolean {
  const { serverId, name, jsonl: origem } = e.identity;
  const r = e.result!;
  const texto = r.text.trim();
  // `/clear` troca o transcript sem trocar a sessão: o rascunho vai com o transcript de AGORA,
  // senão o Chat o descartaria ao abrir.
  const jsonl = jsonlNaLista(serverId, name) ?? origem;
  try {
    const chave = draftStorageKey(serverId, name);
    const salvo = parseStoredDraft(readMigrating(chave, `cp-draft:${name}`));
    // Rascunho de outro transcript: quem decide é o Chat ao abrir. O texto espera na memória.
    if (salvo.text && salvo.jsonl !== null && salvo.jsonl !== jsonl) return false;
    const before = salvo.text.trim() ? `${salvo.text.trimEnd()} ` : '';
    localStorage.setItem(chave, JSON.stringify({ text: before + texto, jsonl }));
    const barra = dictationBarKey(serverId, name);
    if (e.opts.ditado && e.arquivo) {
      const atual = VERSOES.includes(r.estilo_aplicado ?? '') ? r.estilo_aplicado! : 'cru';
      const raw = r.raw?.trim() || texto;
      localStorage.setItem(barra, JSON.stringify({
        arquivo: e.arquivo, before, after: '', raw, atual, cache: { cru: raw, [atual]: texto }, jsonl,
      }));
    } else {
      // Áudio anexado não tem barra; a velha, com o `after` de antes, apagaria texto ao trocar de versão.
      localStorage.removeItem(barra);
    }
    return true;
  } catch (err) {
    // Sem armazenamento: o resultado fica na memória e entra no campo quando a conversa abrir.
    console.warn('dictation: draft not stored', err);
    return false;
  }
}

export const dictations = {
  get(serverId: string, name: string): DictationEntry | undefined {
    const e = entries.get(dictationKey(serverId, name));
    if (e) return e;
    void versao;
    const p = readPersisted(serverId, name);
    // Pela época, não pelo transcript: depois de `/clear` a falha continua valendo e o "de novo"
    // usa o caminho absoluto; só a sessão recriada com o mesmo nome não a herda.
    return p && !recriada(p.identity) ? p : undefined;
  },
  start(input: {
    serverId: string; name: string; jsonl: string | null; server: Server | undefined;
    file?: File; arquivo?: string; opts: DictationOpts;
  }): DictationStart {
    const key = dictationKey(input.serverId, input.name);
    const atual = entries.get(key);
    if (!input.file && !input.arquivo) return 'empty';
    if (atual?.status === 'inflight') return 'inflight';
    // Substituir perderia o que só existe aqui: resultado ainda não entregue ou gravação que não subiu.
    if (atual?.status === 'ready' && !atual.stored) return 'undelivered';
    if (onlyOnDevice(atual)) return 'onlyOnDevice';
    const { serverId, name, jsonl, server, file, arquivo, opts } = input;
    const entry: DictationEntry = {
      id: ++nextId, status: 'inflight', server, file, arquivo, opts,
      identity: { serverId, name, jsonl, epoca: sessionsStore.epoca(serverId, name) },
    };
    if (arquivo) persist(entry);
    begin(key, entry, jsonl);
    return 'started';
  },
  /** `jsonl`: transcript de agora da sessão aberta, que decide entre o nome solto e o caminho. */
  retry(serverId: string, name: string, server?: Server, jsonl: string | null = null): boolean {
    const e = dictations.get(serverId, name);
    if (e?.status !== 'failed' || (!e.path && !e.arquivo && !e.file)) return false;
    begin(dictationKey(serverId, name),
      { ...e, server: e.server ?? server, id: ++nextId, status: 'inflight', error: undefined, result: undefined },
      jsonl);
    return true;
  },
  clear(serverId: string, name: string) {
    const key = dictationKey(serverId, name);
    if (entries.get(key)?.status === 'inflight') return;
    entries.delete(key);
    forget(serverId, name);
  },
  receive(serverId: string, name: string, r: DictationReceiver): () => void {
    const key = dictationKey(serverId, name);
    receivers.set(key, [...(receivers.get(key) ?? []), r]);
    // Depois da montagem: quem registra ainda está montando o campo.
    if (entries.get(key)?.status === 'ready') queueMicrotask(() => deliver(key));
    return () => {
      const resto = (receivers.get(key) ?? []).filter((x) => x !== r);
      if (resto.length) receivers.set(key, resto);
      else receivers.delete(key);
    };
  },
  redeliver(serverId: string, name: string) {
    deliver(dictationKey(serverId, name));
  },
  _resetForTests() {
    entries.clear();
    receivers.clear();
    persisted.clear();
  },
};
