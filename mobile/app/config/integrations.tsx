import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AppState, RefreshControl } from 'react-native';
import { Stack } from 'expo-router';
import {
  fold,
  listCatalog,
  listIntegrations,
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

type EditorState =
  | { mode: 'custom' }
  | { mode: 'preset'; preset: IntegrationPreset }
  | { mode: 'edit'; integration: IntegrationView };

export default function IntegrationsScreen() {
  const server = useServers((s) => s.active());
  const [integrations, setIntegrations] = useState<IntegrationView[] | null>(null);
  const [presets, setPresets] = useState<IntegrationPreset[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [fallbackSelected, setFallbackSelected] = useState<IntegrationView | null>(null);
  const [autoStartOAuth, setAutoStartOAuth] = useState(false);
  const [editor, setEditor] = useState<EditorState | null>(null);

  const generation = useRef(0);
  const serverRef = useRef(server);
  serverRef.current = server;

  const load = useCallback(() => {
    const now = ++generation.current;
    const current = serverRef.current;
    if (!current) return;
    setLoading(true);
    setError('');
    void Promise.all([listIntegrations(current), listCatalog(current)])
      .then(([list, catalog]) => {
        if (now !== generation.current) return;
        setIntegrations(list);
        setPresets(catalog);
      })
      .catch((e: unknown) => {
        if (now === generation.current) setError(integrationErrorText(e) || m.integrations_load_error());
      })
      .finally(() => {
        if (now === generation.current) setLoading(false);
      });
  }, []);

  useEffect(() => {
    setIntegrations(null);
    setPresets([]);
    setError('');
    setSelectedId(null);
    setFallbackSelected(null);
    setAutoStartOAuth(false);
    setEditor(null);
    load();
    return () => { generation.current++; };
  }, [load, server?.id, server?.baseUrl, server?.token]);

  // Ao voltar do navegador após concluir o OAuth, atualiza o estado das integrações.
  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (state === 'active') load();
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

  const selected = useMemo(() => {
    if (!selectedId) return null;
    return integrations?.find((i) => i.id === selectedId)
      ?? (fallbackSelected?.id === selectedId ? fallbackSelected : null);
  }, [selectedId, integrations, fallbackSelected]);

  return (
    <Pagina
      refreshControl={
        server ? <RefreshControl refreshing={loading && integrations !== null} onRefresh={load} /> : undefined
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
            <MachineRow title={m.integrations_retry()} action onPress={load} />
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
                    setAutoStartOAuth(false);
                    setFallbackSelected(picked);
                    setSelectedId(picked.id);
                  }}
                />
              ))}
            </MachineGroup>
          )}

          <MachineGroup>
            <MachineRow
              title={m.integrations_add_custom()}
              action
              onPress={() => setEditor({ mode: 'custom' })}
            />
            <MachineRow
              title={m.integrations_refresh()}
              disabled={loading}
              onPress={load}
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
                  onPress={(picked) => setEditor({ mode: 'preset', preset: picked })}
                />
              ))}
            </MachineGroup>
          )}
        </>
      ) : null}

      {server && selected ? (
        <IntegrationDetailSheet
          server={server}
          integration={selected}
          autoStartOAuth={autoStartOAuth}
          onClose={() => {
            setSelectedId(null);
            setFallbackSelected(null);
            setAutoStartOAuth(false);
          }}
          onChanged={load}
          onEdit={(item) => {
            setSelectedId(null);
            setFallbackSelected(null);
            setAutoStartOAuth(false);
            setEditor({ mode: 'edit', integration: item });
          }}
        />
      ) : null}

      {server && editor ? (
        <IntegrationEditorSheet
          server={server}
          preset={
            editor.mode === 'preset'
              ? editor.preset
              : editor.mode === 'edit'
                ? (presets.find((p) => p.id === editor.integration.preset) ?? null)
                : null
          }
          editing={editor.mode === 'edit' ? editor.integration : null}
          existingIds={existingIds}
          onClose={() => setEditor(null)}
          onSaved={(saved, { isNew }) => {
            setIntegrations((cur) => {
              if (!cur) return [saved];
              const idx = cur.findIndex((x) => x.id === saved.id);
              if (idx >= 0) return cur.map((x) => (x.id === saved.id ? saved : x));
              return [...cur, saved];
            });
            const isOAuth = saved.auth.kind === 'oauth_dcr' || saved.auth.kind === 'oauth_manual_client';
            if (isNew && isOAuth) {
              setFallbackSelected(saved);
              setAutoStartOAuth(true);
              setSelectedId(saved.id);
            }
            load();
          }}
        />
      ) : null}
    </Pagina>
  );
}
