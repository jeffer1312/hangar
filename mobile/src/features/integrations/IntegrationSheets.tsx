import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert } from 'react-native';
import * as Linking from 'expo-linking';
import {
  deleteIntegration,
  listIntegrationTools,
  openIntegrationOAuthOnDesktop,
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
  onChanged: () => void;
  onEdit: (integration: IntegrationView) => void;
}) {
  const [tools, setTools] = useState<IntegrationToolSummary[]>([]);
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
  const currentTargetRef = useRef({ serverId: server.id, baseUrl: server.baseUrl, id: integration.id });
  currentTargetRef.current = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };

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
    try {
      const list = await listIntegrationTools(server, integration.id);
      if (isSameTarget(snap) && version === toolsVersionRef.current) setTools(list);
    } catch {
      /* Cache vazio ou servidor sem suporte mantém lista vazia até o primeiro Testar. */
    }
  }, [server, integration.id]);

  useEffect(() => {
    toolsVersionRef.current += 1;
    setTools([]);
    setError('');
    setTestSummary('');
    setOauthStarted(false);
    setHandoff(null);
    setDesktopOpened(false);
    void refreshTools();
    return () => { toolsVersionRef.current += 1; };
  }, [refreshTools]);

  const supportsOAuth = integration.transport.kind === 'remote_http'
    && (integration.auth.kind === 'oauth_dcr' || integration.auth.kind === 'oauth_manual_client');

  const startOAuth = useCallback(async () => {
    if (busy || !supportsOAuth) return;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };
    setBusy(true);
    setOauthOpening(true);
    setOauthStarted(false);
    setHandoff(null);
    setDesktopOpened(false);
    setError('');
    try {
      const result = await startIntegrationOAuth(server, integration.id, {
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
        if (isSameTarget(snap)) setOauthStarted(true);
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
  }, [busy, supportsOAuth, server, integration.id]);

  useEffect(() => {
    if (!autoStartOAuth || !supportsOAuth) return;
    if (autoStartedFor.current === integration.id) return;
    autoStartedFor.current = integration.id;
    void startOAuth();
  }, [autoStartOAuth, supportsOAuth, integration.id, startOAuth]);

  const openOnDesktop = async () => {
    if (busy || !handoff) return;
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };
    setBusy(true);
    setError('');
    try {
      await openIntegrationOAuthOnDesktop(server, integration.id, handoff.authorize_url);
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
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };
    setBusy(true);
    setTesting(true);
    setError('');
    setTestSummary('');
    try {
      const res = await testIntegration(server, integration.id);
      if (!isSameTarget(snap)) return;
      if (res.status === 'ready') {
        // Invalida leituras de cache mais antigas para não sobrescrever as ferramentas deste teste.
        toolsVersionRef.current += 1;
        setTools(res.tools);
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
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };
    setBusy(true);
    setError('');
    try {
      await updateIntegration(server, integration.id, change);
      if (!isSameTarget(snap)) return;
      if (reloadTools) await refreshTools();
      toast.ok(m.integrations_saved());
      onChanged();
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
    if (policy === integration.policy) return;
    void patch({ policy }, true);
  };

  const allowToolAlways = (toolName: string) => {
    void patch({ add_allowed_tools: [toolName] }, true);
  };

  const confirmRemove = () => {
    const snap = { serverId: server.id, baseUrl: server.baseUrl, id: integration.id };
    Alert.alert(m.integrations_remove_confirm({ name: integration.name }), undefined, [
      { text: m.integrations_cancel(), style: 'cancel' },
      {
        text: m.integrations_remove(),
        style: 'destructive',
        onPress: () => {
          setBusy(true);
          setError('');
          void deleteIntegration(server, integration.id)
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

  const oauthActionTitle = integration.status === 'auth_required'
    ? m.integrations_authorize()
    : m.integrations_reconnect();

  return (
    <MachineSheet title={integration.name} onClose={onClose}>
      <MachineGroup>
        <MachineRow
          title={integration.id}
          description={integrationTransportSummary(integration)}
          value={integrationStatusLabel(integration.status)}
          valueTone={integrationStatusTone(integration.status)}
        />
        <MachineRow
          title={m.integrations_enabled()}
          value={integration.enabled ? m.integrations_yes() : m.integrations_no()}
          checked={integration.enabled}
          disabled={busy}
          onPress={() => {
            haptic.selection();
            void patch({ enabled: !integration.enabled });
          }}
        />
        <MachineRow
          title={m.integrations_field_pinned()}
          description={m.integrations_field_pinned_hint()}
          value={integration.pinned ? m.integrations_yes() : m.integrations_no()}
          checked={integration.pinned}
          disabled={busy}
          onPress={() => {
            haptic.selection();
            void patch({ pinned: !integration.pinned });
          }}
        />
        <MachineRow
          title={m.integrations_field_projects()}
          value={integrationProjectsLabel(integration.projects)}
          disabled={busy}
          onPress={() => onEdit(integration)}
        />
      </MachineGroup>

      {integration.last_error ? <Notice text={integration.last_error} error /> : null}
      {integration.preset?.startsWith('google-') && integration.status === 'auth_expired'
        ? <InfoNotice text={m.integrations_google_testing_hint()} />
        : null}

      <MachineGroup>
        {supportsOAuth ? (
          <MachineRow
            title={oauthOpening ? m.integrations_oauth_opening() : oauthActionTitle}
            action
            disabled={busy || !integration.enabled}
            onPress={() => void startOAuth()}
          />
        ) : null}
        <MachineRow
          title={testing ? m.integrations_testing() : m.integrations_test()}
          action
          disabled={busy || !integration.enabled}
          onPress={() => void runTest()}
        />
      </MachineGroup>

      {oauthStarted ? <InfoNotice text={m.integrations_oauth_started()} /> : null}
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
        <IntegrationPolicyRows value={integration.policy} onChange={changePolicy} disabled={busy} />
      </MachineGroup>

      <MachineHelp>{m.integrations_tools_heading()}</MachineHelp>
      {tools.length === 0 ? (
        <InfoNotice text={m.integrations_tools_empty()} />
      ) : (
        <MachineGroup>
          {tools.map((tool) => (
            <ToolSummaryRow
              key={tool.name}
              tool={tool}
              busy={busy}
              onAllowAlways={allowToolAlways}
            />
          ))}
        </MachineGroup>
      )}

      <MachineGroup>
        <MachineRow title={m.integrations_edit()} action disabled={busy} onPress={() => onEdit(integration)} />
        <MachineRow title={m.integrations_remove()} danger disabled={busy} onPress={confirmRemove} />
      </MachineGroup>
    </MachineSheet>
  );
}
