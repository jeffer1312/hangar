import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert } from 'react-native';
import * as Linking from 'expo-linking';
import {
  deleteIntegration,
  listIntegrationTools,
  openIntegrationOAuthOnDesktop,
  registrarDiag,
  resolveMobilePublicBaseUrl,
  startIntegrationOAuth,
  testIntegration,
  updateIntegration,
  type IntegrationToolSummary,
  type IntegrationView,
  type OAuthStartResult,
  type PermissionPolicy,
  type Server,
} from '@hangar/core';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import {
  IntegrationPolicyRows,
  ToolSummaryRow,
  integrationProjectsLabel,
  integrationStatusLabel,
  integrationStatusTone,
  integrationTransportSummary,
} from './IntegrationRows';
import { integrationErrorText } from './errors';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

/** Folha de detalhes, OAuth (mobile vs handoff desktop), ferramentas, política e remoção. */
export function IntegrationDetailSheet({
  server,
  integration,
  autoStartOAuth = false,
  onClose,
  onChanged,
  onEdit,
}: {
  server: Server;
  integration: IntegrationView;
  autoStartOAuth?: boolean;
  onClose: () => void;
  onChanged: (updated?: IntegrationView) => void;
  onEdit: (integration: IntegrationView) => void;
}) {
  const [localIntegration, setLocalIntegration] = useState(integration);
  const [tools, setTools] = useState<IntegrationToolSummary[]>([]);
  const [toolsLoading, setToolsLoading] = useState(true);
  const [toolsError, setToolsError] = useState('');
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [oauthOpening, setOauthOpening] = useState(false);
  const [oauthStarted, setOauthStarted] = useState(false);
  const [handoff, setHandoff] = useState<OAuthStartResult | null>(null);
  const [desktopOpened, setDesktopOpened] = useState(false);
  const [testSummary, setTestSummary] = useState('');
  const [error, setError] = useState('');
  const alive = useRef(true);
  const autoStartedFor = useRef<string | null>(null);
  const toolsVersionRef = useRef(0);
  const view = localIntegration.id === integration.id ? localIntegration : integration;
  const currentTargetRef = useRef({ serverId: server.id, baseUrl: server.baseUrl, id: integration.id });
  currentTargetRef.current = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };

  useEffect(() => {
    setLocalIntegration(integration);
  }, [integration]);

  useEffect(() => {
    if (view.status === 'ready') {
      setOauthStarted(false);
      setHandoff(null);
      setDesktopOpened(false);
    }
  }, [view.status]);

  const isSameTarget = (snap: { serverId: string; baseUrl: string; id: string }) => {
    const cur = currentTargetRef.current;
    return alive.current && cur.serverId === snap.serverId && cur.baseUrl === snap.baseUrl && cur.id === snap.id;
  };

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  const refreshTools = useCallback(async () => {
    const version = ++toolsVersionRef.current;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };
    setToolsLoading(true);
    setToolsError('');
    try {
      const list = await listIntegrationTools(server, integration.id);
      if (isSameTarget(snap) && version === toolsVersionRef.current) {
        setTools(list);
        setToolsError('');
      }
    } catch (e) {
      const msg = integrationErrorText(e);
      if (isSameTarget(snap) && version === toolsVersionRef.current) {
        setToolsError(msg || m.integrations_load_error());
      }
      registrarDiag({
        evento: 'integrations.tools_refresh_failed',
        nivel: 'aviso',
        tela: 'integrations',
        codigo: typeof (e as { code?: unknown } | null)?.code === 'string'
          ? (e as { code: string }).code
          : e instanceof Error ? e.name : 'erro',
        detalhe: `${integration.id}: ${msg}`.slice(0, 300),
      }, server.baseUrl);
    } finally {
      if (isSameTarget(snap) && version === toolsVersionRef.current) {
        setToolsLoading(false);
      }
    }
  }, [server, integration.id]);

  useEffect(() => {
    toolsVersionRef.current += 1;
    setTools([]);
    setToolsLoading(true);
    setToolsError('');
    setError('');
    setTestSummary('');
    setOauthStarted(false);
    setHandoff(null);
    setDesktopOpened(false);
    void refreshTools();
    return () => { toolsVersionRef.current += 1; };
  }, [refreshTools]);

  const supportsOAuth = view.transport.kind === 'remote_http'
    && (view.auth.kind === 'oauth_dcr' || view.auth.kind === 'oauth_manual_client');

  const startOAuth = useCallback(async () => {
    if (busy || !supportsOAuth) return;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: view.id };
    setBusy(true);
    setOauthOpening(true);
    setOauthStarted(false);
    setHandoff(null);
    setDesktopOpened(false);
    setError('');
    try {
      const result = await startIntegrationOAuth(server, view.id, {
        origin_client: 'mobile',
        public_base_url: resolveMobilePublicBaseUrl(server),
      });
      if (!isSameTarget(snap)) return;
      if (result.mode === 'open_url') {
        const parsed = new URL(result.authorize_url);
        if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
          throw new Error(m.integrations_validation_url());
        }
        await Linking.openURL(parsed.href);
        registrarDiag({
          evento: 'integrations.oauth_browser_opened',
          nivel: 'ok',
          tela: 'integrations',
          detalhe: view.id,
        }, server.baseUrl);
        if (isSameTarget(snap) && view.status !== 'ready') setOauthStarted(true);
      } else {
        setHandoff(result);
      }
    } catch (e) {
      if (isSameTarget(snap)) {
        setError(integrationErrorText(e));
        haptic.error();
      }
    } finally {
      if (isSameTarget(snap)) {
        setOauthOpening(false);
        setBusy(false);
      }
    }
  }, [busy, supportsOAuth, server, view.id, view.status]);

  useEffect(() => {
    if (!autoStartOAuth || !supportsOAuth) return;
    if (autoStartedFor.current === view.id) return;
    autoStartedFor.current = view.id;
    void startOAuth();
  }, [autoStartOAuth, supportsOAuth, view.id, startOAuth]);

  const openOnDesktop = async () => {
    if (busy || !handoff) return;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: view.id };
    setBusy(true);
    setError('');
    try {
      await openIntegrationOAuthOnDesktop(server, view.id, handoff.authorize_url);
      if (!isSameTarget(snap)) return;
      setDesktopOpened(true);
      toast.ok(m.integrations_oauth_desktop_opened());
    } catch (e) {
      if (isSameTarget(snap)) {
        setError(integrationErrorText(e));
        haptic.error();
      }
    } finally {
      if (isSameTarget(snap)) setBusy(false);
    }
  };

  const runTest = async () => {
    if (busy) return;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: view.id };
    setBusy(true);
    setTesting(true);
    setError('');
    setTestSummary('');
    try {
      const res = await testIntegration(server, view.id);
      if (!isSameTarget(snap)) return;
      if (res.status === 'ready') {
        // Invalida leituras de cache mais antigas para não sobrescrever as ferramentas deste teste.
        toolsVersionRef.current += 1;
        setTools(res.tools);
        setToolsLoading(false);
        setToolsError('');
        setOauthStarted(false);
        setHandoff(null);
        setDesktopOpened(false);
        const msg = m.integrations_test_ok({ n: res.tools.length });
        setTestSummary(msg);
        toast.ok(msg);
      } else {
        const translated = res.status === 'auth_required'
          ? m.integrations_err_auth_required()
          : m.integrations_err_auth_expired();
        setError(res.reason ? `${translated} (${res.reason})` : translated);
        haptic.error();
      }
      onChanged();
    } catch (e) {
      if (isSameTarget(snap)) {
        setError(integrationErrorText(e));
        haptic.error();
      }
    } finally {
      if (isSameTarget(snap)) {
        setTesting(false);
        setBusy(false);
      }
    }
  };

  const patch = async (change: Parameters<typeof updateIntegration>[2], reloadTools = false) => {
    if (busy) return;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: view.id };
    setBusy(true);
    setError('');
    try {
      const updated = await updateIntegration(server, view.id, change);
      if (!isSameTarget(snap)) return;
      // Aplica o estado retornado antes de reabilitar as chaves para evitar PATCH repetido com valor velho.
      setLocalIntegration(updated);
      if (change.policy !== undefined) {
        // Descarta permissões antigas até a leitura pós-PATCH confirmar o novo acesso de cada ferramenta.
        toolsVersionRef.current += 1;
        setTools([]);
      }
      if (reloadTools) await refreshTools();
      toast.ok(m.integrations_saved());
      onChanged(updated);
    } catch (e) {
      if (isSameTarget(snap)) {
        setError(integrationErrorText(e));
        haptic.error();
      }
    } finally {
      if (isSameTarget(snap)) setBusy(false);
    }
  };

  const changePolicy = (policy: PermissionPolicy) => {
    if (policy === view.policy) return;
    void patch({ policy }, true);
  };

  const allowToolAlways = (toolName: string) => {
    void patch({ add_allowed_tools: [toolName] }, true);
  };

  const revokeToolAlways = (toolName: string) => {
    void patch({ remove_allowed_tools: [toolName] }, true);
  };

  const confirmRemove = () => {
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: view.id };
    Alert.alert(m.integrations_remove_confirm({ name: view.name }), undefined, [
      { text: m.integrations_cancel(), style: 'cancel' },
      {
        text: m.integrations_remove(),
        style: 'destructive',
        onPress: () => {
          setBusy(true);
          setError('');
          void deleteIntegration(server, view.id)
            .then(() => {
              if (!isSameTarget(snap)) return;
              toast.ok(m.integrations_removed());
              onChanged();
              onClose();
            })
            .catch((e: unknown) => {
              if (isSameTarget(snap)) {
                setError(integrationErrorText(e));
                haptic.error();
              }
            })
            .finally(() => {
              if (isSameTarget(snap)) setBusy(false);
            });
        },
      },
    ]);
  };

  const oauthActionTitle = view.status === 'auth_required'
    ? m.integrations_authorize()
    : m.integrations_reconnect();

  return (
    <MachineSheet title={view.name} onClose={onClose}>
      <MachineGroup>
        <MachineRow
          title={view.id}
          description={integrationTransportSummary(view)}
          value={integrationStatusLabel(view.status)}
          valueTone={integrationStatusTone(view.status)}
        />
        <MachineRow
          title={m.integrations_enabled()}
          value={view.enabled ? m.integrations_yes() : m.integrations_no()}
          checked={view.enabled}
          disabled={busy}
          onPress={() => {
            haptic.selection();
            void patch({ enabled: !view.enabled });
          }}
        />
        <MachineRow
          title={m.integrations_field_pinned()}
          description={m.integrations_field_pinned_hint()}
          value={view.pinned ? m.integrations_yes() : m.integrations_no()}
          checked={view.pinned}
          disabled={busy}
          onPress={() => {
            haptic.selection();
            void patch({ pinned: !view.pinned });
          }}
        />
        <MachineRow
          title={m.integrations_field_projects()}
          value={integrationProjectsLabel(view.projects)}
          disabled={busy}
          onPress={() => onEdit(view)}
        />
      </MachineGroup>

      {view.last_error ? <Notice text={view.last_error} error /> : null}
      {view.preset?.startsWith('google-') && view.status === 'auth_expired'
        ? <InfoNotice text={m.integrations_google_testing_hint()} />
        : null}

      <MachineGroup>
        {supportsOAuth ? (
          <MachineRow
            title={oauthOpening ? m.integrations_oauth_opening() : oauthActionTitle}
            action
            disabled={busy || !view.enabled}
            onPress={() => void startOAuth()}
          />
        ) : null}
        <MachineRow
          title={testing ? m.integrations_testing() : m.integrations_test()}
          action
          disabled={busy || !view.enabled}
          onPress={() => void runTest()}
        />
      </MachineGroup>

      {oauthStarted && view.status !== 'ready' ? <InfoNotice text={m.integrations_oauth_started()} /> : null}
      {handoff ? (
        <>
          <InfoNotice text={handoff.reason ? `${m.integrations_oauth_handoff_title()}: ${handoff.reason}` : m.integrations_oauth_handoff_title()} />
          <MachineGroup>
            <MachineRow
              title={m.integrations_oauth_open_desktop()}
              action
              disabled={busy}
              onPress={() => void openOnDesktop()}
            />
          </MachineGroup>
        </>
      ) : null}
      {desktopOpened ? <InfoNotice text={m.integrations_oauth_desktop_opened()} /> : null}
      {testSummary ? <InfoNotice text={testSummary} /> : null}
      <Notice text={error} error />

      <MachineHelp>{m.integrations_field_policy()}</MachineHelp>
      <MachineGroup>
        <IntegrationPolicyRows value={view.policy} onChange={changePolicy} disabled={busy} />
      </MachineGroup>

      <MachineHelp>{m.integrations_tools_heading()}</MachineHelp>
      {toolsLoading && tools.length === 0 ? (
        <InfoNotice text={m.integrations_loading()} />
      ) : toolsError && tools.length === 0 ? (
        <>
          <Notice text={toolsError} error />
          <MachineGroup>
            <MachineRow
              title={m.integrations_retry()}
              action
              disabled={busy}
              onPress={() => void refreshTools()}
            />
          </MachineGroup>
        </>
      ) : tools.length === 0 ? (
        <InfoNotice text={m.integrations_tools_empty()} />
      ) : (
        <MachineGroup>
          {tools.map((tool) => (
            <ToolSummaryRow
              key={tool.name}
              tool={tool}
              busy={busy}
              onAllowAlways={allowToolAlways}
              onRevokeAlways={revokeToolAlways}
            />
          ))}
        </MachineGroup>
      )}

      <MachineGroup>
        <MachineRow title={m.integrations_edit()} action disabled={busy} onPress={() => onEdit(view)} />
        <MachineRow title={m.integrations_remove()} danger disabled={busy} onPress={confirmRemove} />
      </MachineGroup>
    </MachineSheet>
  );
}
