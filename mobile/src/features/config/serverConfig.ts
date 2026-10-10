import { useCallback, useEffect, useRef, useState } from 'react';
import {
  getConfigForServer, parseTranscriptionProviders, patchConfigForServer, transcriptionProvidersMissingKey,
  type CampoConfig, type Server, type VariavelEnv,
} from '@hangar/core';
import type { IconName } from '../../ui/Icon';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

/** Porta de `desktop-native/src/app/server_config.rs`: tipos de campo, a tabela FIELDS e o rascunho. */
export type Kind =
  | { t: 'toggle' }
  | { t: 'number'; suffix: () => string }
  | { t: 'text' }
  // Entra mas não sai: o servidor devolve só a máscara, e o campo nunca a recebe.
  | { t: 'secret' }
  | { t: 'choice'; values: readonly string[]; empty: () => string };

export interface Field { key: string; label: () => string; help: () => string; icon: IconName; kind: Kind }

const TOGGLE: Kind = { t: 'toggle' };
const TEXT: Kind = { t: 'text' };
const SECRET: Kind = { t: 'secret' };

const LIST: Field[] = [
  { key: 'upload_retention_days', label: m.native_server_keep_attachments, help: m.native_server_keep_attachments_help, icon: 'Paperclip',
    kind: { t: 'number', suffix: m.native_server_days } },
  { key: 'notify_finished', label: m.native_server_notify_finished, help: m.native_server_notify_finished_help, icon: 'CircleCheck', kind: TOGGLE },
  { key: 'finish_min_seconds', label: m.native_server_short_turn, help: m.native_server_short_turn_help, icon: 'Clock',
    kind: { t: 'number', suffix: m.native_server_seconds } },
  { key: 'notify_dead', label: m.native_server_notify_dead, help: m.native_server_notify_dead_help, icon: 'CircleX', kind: TOGGLE },
  { key: 'stall_seconds', label: m.native_server_stall, help: m.native_server_stall_help, icon: 'TriangleAlert',
    kind: { t: 'number', suffix: m.native_server_seconds } },
  { key: 'automations', label: m.native_server_automations, help: m.native_server_automations_help, icon: 'Zap', kind: TOGGLE },
  { key: 'mostrar_pensamento', label: m.native_server_thinking, help: m.native_server_thinking_help, icon: 'Eye', kind: TOGGLE },
  { key: 'traduzir_pensamento', label: m.native_server_translate_thinking, help: m.native_server_translate_thinking_help, icon: 'Languages', kind: TOGGLE },
  { key: 'editor', label: m.native_server_editor, help: m.native_server_editor_help, icon: 'SquarePen', kind: TEXT },
  { key: 'jev_api_key', label: m.native_server_jev_key, help: m.native_server_jev_key_help, icon: 'Key', kind: SECRET },
  { key: 'jev_padrao', label: m.native_server_jev_default, help: m.native_server_jev_default_help, icon: 'Rocket', kind: TOGGLE },
  { key: 'jev_endpoint', label: m.native_server_jev_endpoint, help: m.native_server_jev_endpoint_help, icon: 'Globe', kind: TEXT },
  { key: 'jev_model', label: m.native_server_jev_model, help: m.native_server_jev_model_help, icon: 'Bot', kind: TEXT },
  { key: 'jev_texto_base_url', label: m.native_server_jev_text_endpoint, help: m.native_server_jev_text_endpoint_help, icon: 'Globe', kind: TEXT },
  { key: 'jev_texto_api_key', label: m.native_server_jev_text_key, help: m.native_server_jev_text_key_help, icon: 'Key', kind: SECRET },
  { key: 'jev_texto_modelo', label: m.native_server_jev_text_model, help: m.native_server_jev_text_model_help, icon: 'Bot', kind: TEXT },
  { key: 'jev_texto_cmd', label: m.native_server_jev_cmd, help: m.native_server_jev_cmd_help, icon: 'SquareTerminal', kind: TEXT },
  { key: 'groq_api_key', label: m.native_voice_groq, help: m.native_voice_groq_help, icon: 'Key', kind: SECRET },
  { key: 'transcription_base_url', label: m.native_voice_transcription_endpoint, help: m.native_voice_transcription_endpoint_help, icon: 'Globe', kind: TEXT },
  { key: 'transcription_model', label: m.native_voice_transcription_model, help: m.native_voice_transcription_model_help, icon: 'Bot', kind: TEXT },
  { key: 'ditado_vocabulario', label: m.native_voice_vocabulary, help: m.native_voice_vocabulary_help, icon: 'BookOpen', kind: TEXT },
  { key: 'llm_base_url', label: m.native_voice_llm_endpoint, help: m.native_voice_llm_endpoint_help, icon: 'Globe', kind: TEXT },
  { key: 'llm_api_key', label: m.native_voice_llm_key, help: m.native_voice_llm_key_help, icon: 'Key', kind: SECRET },
  { key: 'llm_model', label: m.native_voice_llm_model, help: m.native_voice_llm_model_help, icon: 'Bot', kind: TEXT },
  { key: 'llm_reasoning_effort', label: m.native_voice_llm_effort, help: m.native_voice_llm_effort_help, icon: 'SlidersHorizontal',
    kind: { t: 'choice', values: ['', 'none', 'low', 'medium', 'high'], empty: m.native_voice_llm_effort_default } },
  { key: 'llm_briefing_base_url', label: m.native_voice_briefing_endpoint, help: m.native_voice_briefing_endpoint_help, icon: 'Globe', kind: TEXT },
  { key: 'llm_briefing_api_key', label: m.native_voice_briefing_key, help: m.native_voice_briefing_key_help, icon: 'Key', kind: SECRET },
  { key: 'llm_briefing_model', label: m.native_voice_briefing_model, help: m.native_voice_briefing_model_help, icon: 'Bot', kind: TEXT },
  { key: 'elevenlabs_api_key', label: m.native_voice_elevenlabs_key, help: m.native_voice_elevenlabs_key_help, icon: 'Key', kind: SECRET },
  { key: 'tts_local_cmd', label: m.native_voice_local_cmd, help: m.native_voice_local_cmd_help, icon: 'SquareTerminal', kind: TEXT },
  { key: 'tts_max_chars', label: m.native_voice_max_chars, help: m.native_voice_max_chars_help, icon: 'Hash', kind: { t: 'number', suffix: m.native_voice_chars } },
];

export const FIELDS: Record<string, Field> = Object.fromEntries(LIST.map((f) => [f.key, f]));

/** Veredito com o "por quê?" que expande. */
export const VERDICTS: Record<string, [() => string, () => string]> = {
  automations: [m.native_server_recommended_on, m.native_server_automations_why],
  llm_reasoning_effort: [m.native_voice_llm_effort_verdict, m.native_voice_llm_effort_why],
  tts_local_cmd: [m.native_voice_local_cmd_verdict, m.native_voice_local_cmd_why],
};

export const textOf = (v: unknown): string => (v === null || v === undefined ? '' : String(v));

type Load = { status: 'loading' } | { status: 'error'; error: string } | { status: 'ready' };
type Draft = Record<string, unknown>;

export const PROVIDERS = 'transcription_providers';

const message = (e: unknown) => (e instanceof Error && e.message ? e.message : m.erro_desconhecido());

/**
 * Um rascunho por página, gravado pelo Salvar do rodapé num só `POST /api/config` (como o Rust).
 * Só entra no rascunho chave que a última leitura boa trouxe: servidor antigo sem o campo não recebe
 * gravação às cegas. Falha no Salvar mantém o rascunho e mostra o erro; "Desfazer" volta ao servidor.
 */
export function useServerConfig() {
  const server = useServers((s) => s.active());
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [fields, setFields] = useState<Record<string, CampoConfig>>({});
  const [read, setRead] = useState<Record<string, string | number | boolean>>({});
  const [env, setEnv] = useState<VariavelEnv[]>([]);
  const [draft, setDraft] = useState<Draft>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [saved, setSaved] = useState(false);
  // Troca de servidor: resposta atrasada do anterior não pode cair nesta tela.
  const gen = useRef(0);
  const savedTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const reload = useCallback((target: Server | null) => {
    const g = ++gen.current;
    setFields({});
    setRead({});
    setEnv([]);
    setSaveError('');
    setSaved(false);
    if (!target) return;
    setLoad({ status: 'loading' });
    getConfigForServer(target)
      .then((config) => {
        if (g !== gen.current) return;
        if (!config || typeof config.campos !== 'object' || config.campos === null) throw new Error(m.native_invalid_response());
        setFields(config.campos);
        setRead(config.somente_leitura ?? {});
        setEnv(config.variaveis_env ?? []);
        setLoad({ status: 'ready' });
      })
      .catch((e: unknown) => { if (g === gen.current) setLoad({ status: 'error', error: message(e) }); });
  }, []);

  useEffect(() => {
    setDraft({});
    setSaving(false);
    reload(server);
    return () => { gen.current++; clearTimeout(savedTimer.current); };
  }, [reload, server?.id, server?.baseUrl, server?.token]);

  const current = (key: string): unknown => (key in draft ? draft[key] : fields[key]?.valor ?? null);
  const removing = (key: string) => key in draft && draft[key] === null;
  const editedInApp = (key: string) => fields[key]?.origem === 'app';
  const filled = (key: string) => textOf(current(key)).trim() !== '';
  /** Máscara do segredo guardado (`gsk_••••1234`), se há um. */
  const secretMask = (key: string) => (fields[key]?.definido ? textOf(fields[key].valor) : null);
  const keySet = (key: string) => secretMask(key) !== null && !removing(key);

  const stage = (key: string, value: unknown) => {
    if (!(key in fields)) return false;
    setDraft((d) => ({ ...d, [key]: value }));
    return true;
  };
  const unstage = (key: string) => setDraft((d) => { const { [key]: _, ...rest } = d; return rest; });
  const discard = () => { setDraft({}); setSaveError(''); };
  // Serviço de transcrição sem chave faz o servidor recusar o Salvar inteiro: o Salvar espera a chave.
  const saveBlocked = PROVIDERS in draft && transcriptionProvidersMissingKey(parseTranscriptionProviders(draft[PROVIDERS]));

  const save = () => {
    if (!server || saving || saveBlocked || Object.keys(draft).length === 0) return;
    const sent = { ...draft };
    const g = gen.current;
    setSaving(true);
    setSaveError('');
    setSaved(false);
    patchConfigForServer(server, sent)
      .then((r) => {
        if (g !== gen.current) return;
        if (!r || typeof r.campos !== 'object' || r.campos === null) throw new Error(m.native_invalid_response());
        setFields(r.campos);
        if (r.somente_leitura) setRead(r.somente_leitura);
        // Sai só o que ainda é o enviado: o que foi mexido durante o salvar vai no próximo.
        setDraft((d) => Object.fromEntries(Object.entries(d).filter(([k, v]) => !(k in sent && sent[k] === v))));
        setSaved(true);
        clearTimeout(savedTimer.current);
        savedTimer.current = setTimeout(() => setSaved(false), 2500);
      })
      // Erro de validação do backend chega como veio ("stall_seconds: …").
      .catch((e: unknown) => { if (g === gen.current) setSaveError(message(e)); })
      .finally(() => { if (g === gen.current) setSaving(false); });
  };

  return {
    server, load, fields, read, env, draft, saving, saveError, saved, saveBlocked,
    dirty: Object.keys(draft).length > 0,
    reload: () => reload(server),
    current, removing, editedInApp, filled, secretMask, keySet, stage, unstage, discard, save,
  };
}

export type ServerConfig = ReturnType<typeof useServerConfig>;
