import { useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getComputerControlEnabled, getJevAuxCliproxy, listJevAuxModels, patchConfigForServer, sameJevAuxUrl, type JevAuxCliproxy } from '@hangar/core';
import { BlockHead, Box, ConfigRow, Disclosure, Muted, RowShell, ScopeChips, ServerConfigPage, useInputStyle } from '../../src/features/config/ServerConfigParts';
import { AnchoredPanel } from '../../src/ui/AnchoredPanel';
import { Icon } from '../../src/ui/Icon';
import { Pill } from '../../src/features/config/PageHeader';
import { Segmented } from '../../src/features/config/Segmented';
import { useSettingsColors } from '../../src/features/config/colors';
import { FIELDS, textOf, useServerConfig, type ServerConfig } from '../../src/features/config/serverConfig';
import * as m from '../../src/paraglide/messages';

const message = (e: unknown) => (e instanceof Error && e.message ? e.message : m.erro_desconhecido());

export default function Jev() {
  const cfg = useServerConfig();
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const failed = theme.tokens.status.error;
  const server = cfg.server;
  const [advanced, setAdvanced] = useState(false);
  const [cliproxy, setCliproxy] = useState<JevAuxCliproxy | null>(null);
  const [applying, setApplying] = useState(false);
  // `url`: o endereço gravado; editar o campo depois tira a mensagem da tela.
  const [cliproxyMsg, setCliproxyMsg] = useState<{ ok: boolean; text: string; url: string } | null>(null);
  const [models, setModels] = useState<string[]>([]);
  // Endereço e chave da última busca: lista e erro só valem enquanto os campos forem esses.
  const [fetchedFor, setFetchedFor] = useState('');
  const [fetching, setFetching] = useState(false);
  const [modelsError, setModelsError] = useState('');
  const [computer, setComputer] = useState<{ enabled: boolean } | { error: string } | null>(null);
  const [cliproxyError, setCliproxyError] = useState('');
  // Troca de servidor no meio de uma chamada: a resposta do anterior não pinta esta tela.
  const gen = useRef(0);

  useEffect(() => {
    const g = ++gen.current;
    const live = () => g === gen.current;
    setCliproxy(null);
    setCliproxyError('');
    setCliproxyMsg(null);
    setApplying(false);
    setFetching(false);
    setComputer(null);
    setModels([]);
    setModelsError('');
    if (!server) return;
    // 404 é servidor sem o atalho (versão anterior): só aí o atalho some calado.
    getJevAuxCliproxy(server)
      .then((r) => { if (live()) setCliproxy(r); })
      .catch((e: unknown) => { if (live() && (e as { status?: number }).status !== 404) setCliproxyError(message(e)); });
    getComputerControlEnabled(server)
      .then((r) => { if (live()) setComputer({ enabled: !!r.enabled }); })
      .catch((e: unknown) => { if (live()) setComputer({ error: message(e) }); });
    return () => { gen.current++; };
  }, [server?.id, server?.baseUrl, server?.token]);

  const auxSupported = 'jev_aux_base_url' in cfg.fields;
  const url = textOf(cfg.current('jev_aux_base_url')).trim();
  // Servidor que não publica a escolha (mais antigo) fica só com o auxiliar por API, como antes.
  const modeSupported = 'jev_aux_mode' in cfg.fields;
  const savedMode = textOf(cfg.current('jev_aux_mode')).trim();
  // Sem escolha, a falta de um endereço salvo (sem ele nenhum modelo por API é chamado). O salvo, não o rascunho:
  // apagar o campo para redigitar não esconde a seção.
  const savedUrl = textOf(cfg.fields.jev_aux_base_url?.valor).trim();
  const bySession = modeSupported && (savedMode === 'session' || (savedMode !== 'api' && !savedUrl));
  const model = textOf(cfg.current('jev_aux_modelo')).trim();
  const fetchKey = `${url}\n${textOf(cfg.draft.jev_aux_api_key).trim()}`;
  const fetchValid = fetchedFor === fetchKey;
  const shownMsg = cliproxyMsg && cliproxyMsg.url === url ? cliproxyMsg : null;

  const fetchModels = () => {
    if (!server || fetching || !url) return;
    const key = textOf(cfg.draft.jev_aux_api_key).trim();
    const g = gen.current;
    setFetching(true);
    setModelsError('');
    setFetchedFor(fetchKey);
    const saved = !key && sameJevAuxUrl(url, cfg.fields.jev_aux_base_url?.valor);
    listJevAuxModels(server, { base_url: url, api_key: key || null, use_saved_key: saved })
      .then((r) => { if (g !== gen.current) return; setModels(r.models); if (!r.models.length) setModelsError(m.browser_jev_aux_models_none()); })
      .catch((e: unknown) => { if (g !== gen.current) return; setModels([]); setModelsError(message(e)); })
      .finally(() => { if (g === gen.current) setFetching(false); });
  };

  // Grava na hora, fora do rascunho: a chave do CLIProxyAPI só existe no servidor.
  const applyCliproxy = () => {
    if (!server || !cliproxy || applying) return;
    const g = gen.current;
    setApplying(true);
    setCliproxyMsg(null);
    const target = cliproxy.preset_url;
    patchConfigForServer(server, { jev_aux_cliproxy: true, jev_aux_base_url: target })
      .then(() => {
        if (g !== gen.current) return;
        cfg.unstage('jev_aux_base_url');
        cfg.unstage('jev_aux_api_key');
        cfg.reload();
        setCliproxyMsg({ ok: true, text: m.browser_jev_aux_cliproxy_done(), url: target });
        // A região viva só existe no Android: no iOS o leitor de tela precisa do anúncio.
        AccessibilityInfo.announceForAccessibility(m.browser_jev_aux_cliproxy_done());
      })
      .catch((e: unknown) => {
        if (g !== gen.current) return;
        setCliproxyMsg({ ok: false, text: message(e), url });
        AccessibilityInfo.announceForAccessibility(message(e));
      })
      .finally(() => { if (g === gen.current) setApplying(false); });
  };


  return (
    // Releitura que falha depois do atalho troca a página pelo erro: o "gravado" fica embaixo dele.
    <ServerConfigPage title={m.jev_title()} cfg={cfg} extra={cfg.load.status !== 'ready' && cliproxyMsg ? (
      <Text accessibilityLiveRegion="polite" style={[styles.note, { color: cliproxyMsg.ok ? c.muted : failed }]}>{cliproxyMsg.text}</Text>
    ) : null}>
      <Muted>{m.jev_intro()}</Muted>

      <BlockHead title={m.browser_jev_decisions_title()} help={m.browser_jev_decisions_help()} />
      <Box>
        <ConfigRow cfg={cfg} k="jev_api_key" />
      </Box>
      <Disclosure open={advanced} label={m.jev_advanced()} onChange={setAdvanced} />
      {advanced ? (
        <Box>
          <ConfigRow cfg={cfg} k="jev_endpoint" />
          <ConfigRow cfg={cfg} k="jev_model" />
        </Box>
      ) : null}

      <BlockHead title={m.browser_jev_aux_title()} help={m.browser_jev_aux_help()} />
      {modeSupported ? (
        <Box>
          <RowShell icon="Bot" title={m.browser_jev_aux_mode()} help={bySession ? m.browser_jev_aux_mode_session_help() : ''}>
            <Segmented label={m.browser_jev_aux_mode()} value={bySession ? 'session' : 'api'}
              options={[{ v: 'session', label: m.browser_jev_aux_mode_session() }, { v: 'api', label: m.browser_jev_aux_mode_api() }]}
              onChange={(v) => cfg.stage('jev_aux_mode', v)} />
          </RowShell>
        </Box>
      ) : null}
      {!auxSupported ? <Muted>{m.browser_jev_aux_unsupported()}</Muted> : bySession ? null : (
        <>
          {cliproxy ? (
            <View style={styles.block}>
              <Muted>{!cliproxy.running ? m.browser_jev_aux_cliproxy_stopped({ url: cliproxy.preset_url })
                : cliproxy.has_keys ? m.browser_jev_aux_cliproxy_ready({ url: cliproxy.preset_url })
                : m.browser_jev_aux_cliproxy_no_key()}</Muted>
              <View style={styles.row}>
                <Pill label={m.browser_jev_aux_cliproxy_use()} onPress={applyCliproxy}
                  disabled={applying || !cliproxy.running || !cliproxy.has_keys} />
              </View>
              {shownMsg ? (
                <Text accessibilityLiveRegion="polite" style={[styles.note, { color: shownMsg.ok ? c.muted : failed }]}>{shownMsg.text}</Text>
              ) : null}
            </View>
          ) : cliproxyError ? (
            <Text accessibilityRole="alert" style={[styles.note, { color: failed }]}>{m.browser_jev_aux_cliproxy_error({ e: cliproxyError })}</Text>
          ) : null}
          <Box>
            <ConfigRow cfg={cfg} k="jev_aux_base_url" />
            <ConfigRow cfg={cfg} k="jev_aux_api_key" />
            <ModelField cfg={cfg} models={fetchValid ? models : []}
              onOpen={() => { if (url && !fetching && (!fetchValid || modelsError)) fetchModels(); }}
              notice={!url ? m.browser_jev_aux_models_need_url()
                : fetching ? m.browser_jev_aux_models_fetching()
                : !fetchValid ? m.browser_jev_aux_models_stale()
                : modelsError || null}
              failed={fetchValid && !!modelsError && modelsError !== m.browser_jev_aux_models_none() && !fetching} />
          </Box>
          <Box>
            <ConfigRow cfg={cfg} k="jev_aux_esforco" />
          </Box>
        </>
      )}

      <BlockHead title={m.browser_jev_where_title()} />
      <Box>
        <ConfigRow cfg={cfg} k="jev_padrao" />
      </Box>
      <Muted>{m.browser_jev_where_orq()}</Muted>
      <Muted>{computer === null ? m.comum_carregando()
        : 'error' in computer ? m.browser_jev_where_cu_error({ e: computer.error })
        : computer.enabled ? m.browser_jev_where_cu_on() : m.browser_jev_where_cu_off()}</Muted>
    </ServerConfigPage>
  );
}

/**
 * Campo Modelo que abre a lista do provedor com busca. Digitar muda o valor (vale um modelo fora da
 * lista) e filtra por trecho; tocar numa linha escolhe e fecha.
 */
function ModelField({ cfg, models, onOpen, notice, failed }: {
  cfg: ServerConfig; models: string[]; onOpen: () => void; notice: string | null; failed: boolean;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const input = useInputStyle();
  const anchor = useRef<View>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const k = 'jev_aux_modelo';
  const field = FIELDS[k];
  const value = textOf(cfg.current(k));
  const off = !(k in cfg.fields);
  const q = query.trim().toLowerCase();
  const list = q ? models.filter((id) => id.toLowerCase().includes(q)) : models;
  const show = () => { if (off) return; setQuery(''); setOpen(true); onOpen(); };
  const close = () => setOpen(false);

  return (
    <RowShell icon={field.icon} title={field.label()} chips={<ScopeChips cfg={cfg} k={k} />} help={field.help()}>
      <Pressable ref={anchor} onPress={show} disabled={off} accessibilityRole="combobox" accessibilityLabel={field.label()}
        accessibilityValue={{ text: value }} accessibilityState={{ expanded: open, disabled: off }}
        style={[input.style, styles.trigger]}>
        <Text style={[styles.triggerText, { color: value ? c.text : c.faint }]} numberOfLines={1}>{value || '—'}</Text>
        <Icon name="ChevronDown" size={14} color={c.muted} />
      </Pressable>
      <AnchoredPanel open={open} anchor={anchor.current} onClose={close} label={field.label()} closeLabel={m.native_close()}
        radius={12} padding={4} maxHeightRatio={1}>
        <View style={styles.panel}>
          <TextInput {...input} autoFocus value={value} accessibilityLabel={field.label()} autoCapitalize="none" autoCorrect={false}
            returnKeyType="done" onSubmitEditing={close}
            onChangeText={(t) => { cfg.stage(k, t); setQuery(t); }} />
          {notice ? (
            <Text accessibilityRole={failed ? 'alert' : undefined} style={[styles.note, { color: failed ? theme.tokens.status.error : c.muted }]}>{notice}</Text>
          ) : list.length === 0 ? (
            <Text style={[styles.note, { color: c.muted }]}>{q ? m.browser_jev_aux_models_no_match() : m.browser_jev_aux_models_none()}</Text>
          ) : (
            <ScrollView style={styles.models} keyboardShouldPersistTaps="handled">
              {list.map((id) => (
                <Pressable key={id} accessibilityRole="button" accessibilityLabel={id} accessibilityState={{ selected: id === value }}
                  onPress={() => { cfg.stage(k, id); close(); }}
                  style={({ pressed }) => [styles.model, { borderTopColor: c.border },
                    id === value && { backgroundColor: c.accentDim }, pressed && { backgroundColor: c.hover }]}>
                  <Text style={[styles.modelText, { color: id === value ? c.accentText : c.text }]} numberOfLines={1}>{id}</Text>
                </Pressable>
              ))}
            </ScrollView>
          )}
        </View>
      </AnchoredPanel>
    </RowShell>
  );
}

const styles = StyleSheet.create((theme) => ({
  trigger: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  triggerText: { flex: 1, fontSize: theme.base.text.sm },
  panel: { gap: theme.base.space[2], padding: theme.base.space[1] },
  models: { maxHeight: 360 },
  block: { gap: theme.base.space[2] },
  row: { flexDirection: 'row' },
  note: { fontSize: theme.base.text.xs, lineHeight: 18, paddingHorizontal: theme.base.space[1] },
  // 44 pt: alvo de toque mínimo.
  model: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[3], borderTopWidth: StyleSheet.hairlineWidth },
  modelText: { fontSize: theme.base.text.xs },
}));
