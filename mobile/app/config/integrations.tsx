import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AppState, RefreshControl } from 'react-native';
import { Stack } from 'expo-router';
import {
  fold,
  listCatalog,
  listIntegrations,
  registrarDiag,
  type IntegrationPreset,
  type IntegrationView,
} from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { MachineGroup, MachineHelp, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { PluginsField } from '../../src/features/plugins/parts';
import { CatalogPresetRow, ConfiguredIntegrationRow } from '../../src/features/integrations/IntegrationRows';
import { IntegrationDetailSheet } from '../../src/features/integrations/IntegrationSheets';
import { IntegrationEditorSheet } from '../../src/features/integrations/IntegrationEditorSheet';
import { integrationErrorText } from '../../src/features/integrations/errors';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

type EditorDraft =
  | { mode: 'custom' }
  | { mode: 'preset'; preset: IntegrationPreset }
  | { mode: 'edit'; integration: IntegrationView };

type EditorState = EditorDraft & {
  serverKey: string;
  instance: number;
};

type SelectionState = {
  serverKey: string;
  id: string;
  /** Usado apenas durante a transição de criação antes da lista refletir o item novo. */
  pendingNew?: IntegrationView;
  autoStartOAuth: boolean;
};

const serverScopeKey = (srv: { id: string; baseUrl: string; token?: string | null } | null | undefined) =>
  srv ? `${srv.id}::${srv.baseUrl}::${srv.token ?? ''}` : null;

export default function IntegrationsScreen() {
  const server = useServers((s) => s.active());
  const serverKey = serverScopeKey(server);
  const [integrations, setIntegrations] = useState<IntegrationView[] | null>(null);
  const [presets, setPresets] = useState<IntegrationPreset[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [selection, setSelection] = useState<SelectionState | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);

  const generation = useRef(0);
  const hasLoadedRef = useRef(false);
  const manualLoadingRef = useRef(false);
  const editorSeqRef = useRef(0);
  const activeEditorInstanceRef = useRef<number | null>(null);
  const serverRef = useRef(server);
  serverRef.current = server;
  const serverKeyRef = useRef(serverKey);
  serverKeyRef.current = serverKey;

  const openEditor = useCallback((draft: EditorDraft) => {
    const key = serverKeyRef.current;
    if (!key) return;
    const instance = ++editorSeqRef.current;
    activeEditorInstanceRef.current = instance;
    setEditor({ ...draft, serverKey: key, instance });
  }, []);

  const closeEditor = useCallback((instance?: number) => {
    if (instance !== undefined && activeEditorInstanceRef.current !== instance) return;
    activeEditorInstanceRef.current = null;
    setEditor(null);
  }, []);

  const load = useCallback((opts?: { silent?: boolean }) => {
    const silent = opts?.silent ?? false;
    const now = ++generation.current;
    const current = serverRef.current;
    const currentKey = serverKeyRef.current;
    if (!current || !currentKey) return;
    if (!silent) {
      manualLoadingRef.current = true;
      setLoading(true);
      setError('');
    }
    void Promise.all([listIntegrations(current), listCatalog(current)])
      .then(([list, catalog]) => {
        if (now !== generation.current || serverKeyRef.current !== currentKey) return;
        hasLoadedRef.current = true;
        setError('');
        setIntegrations(list);
        setPresets(catalog);
        setSelection((cur) => {
          if (!cur || cur.serverKey !== currentKey) return cur;
          if (list.some((item) => item.id === cur.id)) {
            return cur.pendingNew ? { ...cur, pendingNew: undefined } : cur;
          }
          // Recarga válida confirmou que o item não existe mais neste servidor.
          return null;
        });
      })
      .catch((e: unknown) => {
        if (now !== generation.current || serverKeyRef.current !== currentKey) return;
        if (silent) {
          const msg = integrationErrorText(e) || m.integrations_load_error();
          // Atualização automática em segundo plano não derruba nem polui a lista já carregada.
          registrarDiag({
            evento: 'integrations.refresh_failed',
            nivel: 'aviso',
            tela: 'integrations',
            codigo: typeof (e as { code?: unknown } | null)?.code === 'string'
              ? (e as { code: string }).code
              : e instanceof Error ? e.name : 'erro',
            detalhe: msg.slice(0, 300),
          }, current.baseUrl);
          console.warn('integrations: silent refresh failed', e);
          return;
        }
        setError(integrationErrorText(e) || m.integrations_load_error());
      })
      .finally(() => {
        // Se uma carga silenciosa assumiu uma carga manual em andamento, ela encerra o indicador ao concluir.
        if (now === generation.current && serverKeyRef.current === currentKey && (!silent || manualLoadingRef.current)) {
          manualLoadingRef.current = false;
          setLoading(false);
        }
      });
  }, []);

  useEffect(() => {
    hasLoadedRef.current = false;
    manualLoadingRef.current = false;
    activeEditorInstanceRef.current = null;
    setLoading(false);
    setIntegrations(null);
    setPresets([]);
    setError('');
    setSelection(null);
    setEditor(null);
    load();
    return () => {
      generation.current++;
      manualLoadingRef.current = false;
      activeEditorInstanceRef.current = null;
    };
  }, [load, serverKey]);

  // Ao voltar do navegador após concluir o OAuth, atualiza o estado das integrações.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') load({ silent: hasLoadedRef.current });
    });
    return () => sub.remove();
  }, [load]);

  const q = fold(query.trim());
  const configuredShown = useMemo(() => {
    if (!integrations) return [];
    if (!q) return integrations;
    return integrations.filter((i) => fold(`${i.name} ${i.id} ${i.preset ?? ''}`).includes(q));
  }, [integrations, q]);

  const presetsShown = useMemo(() => {
    if (!q) return presets;
    return presets.filter((p) => fold(`${p.name} ${p.id} ${p.description}`).includes(q));
  }, [presets, q]);

  const countByPreset = useMemo(() => {
    const map = new Map<string, number>();
    for (const item of integrations ?? []) {
      if (item.preset) map.set(item.preset, (map.get(item.preset) ?? 0) + 1);
    }
    return map;
  }, [integrations]);

  const existingIds = useMemo(() => (integrations ?? []).map((i) => i.id), [integrations]);

  const activeSelection = selection && serverKey && selection.serverKey === serverKey ? selection : null;
  const activeEditor = editor && serverKey && editor.serverKey === serverKey ? editor : null;

  const selected = useMemo(() => {
    if (!activeSelection) return null;
    return integrations?.find((i) => i.id === activeSelection.id) ?? activeSelection.pendingNew ?? null;
  }, [activeSelection, integrations]);

  const sameServerScope = (ownerKey: string) => serverKeyRef.current === ownerKey;

  return (
    <Pagina
      refreshControl={
        server ? <RefreshControl refreshing={loading && integrations !== null} onRefresh={() => load()} /> : undefined
      }
    >
      <Stack.Screen options={{ title: m.integrations_title() }} />
      <MachineHelp>{m.integrations_subtitle()}</MachineHelp>

      {!server ? (
        <InfoNotice text={m.integrations_load_error()} />
      ) : !integrations && loading ? (
        <InfoNotice text={m.integrations_loading()} />
      ) : !integrations && error ? (
        <>
          <Notice text={error} error />
          <MachineGroup>
            <MachineRow title={m.integrations_retry()} action onPress={() => load()} />
          </MachineGroup>
        </>
      ) : integrations ? (
        <>
          {error ? <Notice text={error} error /> : null}
          <PluginsField
            icon="Search"
            value={query}
            onChangeText={setQuery}
            placeholder={m.native_settings_search()}
            returnKeyType="search"
          />

          <MachineHelp>{m.integrations_configured_heading()}</MachineHelp>
          {integrations.length === 0 ? (
            <InfoNotice text={m.integrations_empty()} />
          ) : configuredShown.length === 0 ? (
            <InfoNotice text={m.native_settings_search_none()} />
          ) : (
            <MachineGroup>
              {configuredShown.map((item) => (
                <ConfiguredIntegrationRow
                  key={item.id}
                  integration={item}
                  onPress={(picked) => {
                    if (!serverKey) return;
                    setSelection({ serverKey, id: picked.id, autoStartOAuth: false });
                  }}
                />
              ))}
            </MachineGroup>
          )}

          <MachineGroup>
            <MachineRow
              title={m.integrations_add_custom()}
              action
              onPress={() => openEditor({ mode: 'custom' })}
            />
            <MachineRow
              title={m.integrations_refresh()}
              disabled={loading}
              onPress={() => load()}
            />
          </MachineGroup>

          <MachineHelp>{m.integrations_catalog_heading()}</MachineHelp>
          <InfoNotice text={m.integrations_catalog_hint()} />
          {presetsShown.length === 0 ? (
            <InfoNotice text={m.native_settings_search_none()} />
          ) : (
            <MachineGroup>
              {presetsShown.map((preset) => (
                <CatalogPresetRow
                  key={preset.id}
                  preset={preset}
                  connectedCount={countByPreset.get(preset.id) ?? 0}
                  onPress={(picked) => openEditor({ mode: 'preset', preset: picked })}
                />
              ))}
            </MachineGroup>
          )}
        </>
      ) : null}

      {server && serverKey && activeSelection && selected ? (
        <IntegrationDetailSheet
          server={server}
          integration={selected}
          autoStartOAuth={activeSelection.autoStartOAuth}
          onClose={() => {
            if (!sameServerScope(activeSelection.serverKey)) return;
            setSelection(null);
          }}
          onChanged={(updated) => {
            if (!sameServerScope(activeSelection.serverKey)) return;
            if (updated) {
              setIntegrations((cur) => (cur ? cur.map((item) => (item.id === updated.id ? updated : item)) : cur));
            }
            load();
          }}
          onEdit={(item) => {
            if (!sameServerScope(activeSelection.serverKey)) return;
            setSelection(null);
            openEditor({ mode: 'edit', integration: item });
          }}
        />
      ) : null}

      {server && activeEditor ? (
        <IntegrationEditorSheet
          key={activeEditor.instance}
          server={server}
          preset={
            activeEditor.mode === 'preset'
              ? activeEditor.preset
              : activeEditor.mode === 'edit'
                ? (presets.find((p) => p.id === activeEditor.integration.preset) ?? null)
                : null
          }
          editing={activeEditor.mode === 'edit' ? activeEditor.integration : null}
          existingIds={existingIds}
          onClose={() => {
            if (!sameServerScope(activeEditor.serverKey)) return;
            closeEditor(activeEditor.instance);
          }}
          onSaved={(saved, { isNew }) => {
            // Descarta gravação que concluiu depois da troca de escopo (máquina, URL ou token).
            if (!sameServerScope(activeEditor.serverKey)) return;
            setIntegrations((cur) => {
              if (!cur) return [saved];
              const idx = cur.findIndex((x) => x.id === saved.id);
              if (idx >= 0) return cur.map((x) => (x.id === saved.id ? saved : x));
              return [...cur, saved];
            });
            // Só abre detalhe/OAuth se esta gravação ainda pertencer à instância ativa do editor.
            if (activeEditorInstanceRef.current === activeEditor.instance) {
              const isOAuth = saved.auth.kind === 'oauth_dcr' || saved.auth.kind === 'oauth_manual_client';
              if (isNew && isOAuth) {
                setSelection({
                  serverKey: activeEditor.serverKey,
                  id: saved.id,
                  pendingNew: saved,
                  autoStartOAuth: true,
                });
              }
            }
            load();
          }}
        />
      ) : null}
    </Pagina>
  );
}
