import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type {
  IntegrationPreset,
  IntegrationStatus,
  IntegrationToolSummary,
  IntegrationView,
  IntegrationSetupGuide,
  PermissionPolicy,
  ToolAccess,
} from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { InfoNotice } from '../config/InfoNotice';
import { useSettingsColors } from '../config/colors';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

export function integrationStatusLabel(status: IntegrationStatus): string {
  switch (status) {
    case 'ready': return m.integrations_status_ready();
    case 'auth_required': return m.integrations_status_auth_required();
    case 'auth_expired': return m.integrations_status_auth_expired();
    case 'disabled': return m.integrations_status_disabled();
    case 'error': return m.integrations_status_error();
  }
}

export function integrationStatusTone(status: IntegrationStatus): 'success' | 'error' | undefined {
  if (status === 'ready') return 'success';
  if (status === 'auth_expired' || status === 'error') return 'error';
  return undefined;
}

export function integrationPolicyLabel(policy: PermissionPolicy, short = false): string {
  switch (policy) {
    case 'read_free_ask_write':
      return short ? m.integrations_policy_read_free_ask_write_short() : m.integrations_policy_read_free_ask_write();
    case 'allow_all':
      return m.integrations_policy_allow_all();
    case 'read_only':
      return m.integrations_policy_read_only();
  }
}

export function integrationProjectsLabel(projects: string[]): string {
  return projects.length === 0 ? m.integrations_projects_all() : m.integrations_projects_count({ n: projects.length });
}

export function presetAuthLabel(preset: IntegrationPreset): string {
  switch (preset.auth_kind) {
    case 'oauth_dcr': return m.integrations_auth_oauth_dcr();
    case 'oauth_manual_client': return m.integrations_auth_oauth_manual();
    case 'oauth_dcr_or_bearer': return `${m.integrations_auth_oauth_dcr()} · ${m.integrations_auth_bearer()}`;
  }
}

export function toolAccessLabel(access: ToolAccess): string {
  switch (access) {
    case 'read': return m.integrations_tool_access_read();
    case 'allowed_always': return m.integrations_tool_access_allowed_always();
    case 'ask_write': return m.integrations_tool_access_ask_write();
    case 'blocked': return m.integrations_tool_access_blocked();
  }
}

export function integrationTransportSummary(integration: IntegrationView): string {
  return integration.transport.kind === 'remote_http'
    ? integration.transport.url
    : integration.transport.command.join(' ');
}

export function IntegrationPolicyRows({ value, onChange, disabled }: {
  value: PermissionPolicy;
  onChange: (p: PermissionPolicy) => void;
  disabled?: boolean;
}) {
  const options: PermissionPolicy[] = ['read_free_ask_write', 'allow_all', 'read_only'];
  return (
    <>
      {options.map((opt) => (
        <MachineRow
          key={opt}
          title={integrationPolicyLabel(opt)}
          selected={value === opt}
          disabled={disabled}
          onPress={() => {
            if (value !== opt) {
              haptic.selection();
              onChange(opt);
            }
          }}
        />
      ))}
    </>
  );
}

export function ConfiguredIntegrationRow({ integration, onPress }: {
  integration: IntegrationView;
  onPress: (item: IntegrationView) => void;
}) {
  const summary = [
    integration.id,
    integrationPolicyLabel(integration.policy, true),
    integrationProjectsLabel(integration.projects),
    integration.last_error,
  ].filter(Boolean).join(' · ');

  return (
    <MachineRow
      title={integration.name}
      description={summary}
      value={integrationStatusLabel(integration.status)}
      valueTone={integrationStatusTone(integration.status)}
      onPress={() => onPress(integration)}
    />
  );
}

export function CatalogPresetRow({ preset, connectedCount, onPress }: {
  preset: IntegrationPreset;
  connectedCount: number;
  onPress: (preset: IntegrationPreset) => void;
}) {
  return (
    <MachineRow
      title={preset.name}
      description={`${presetAuthLabel(preset)} · ${preset.description}`}
      value={connectedCount > 0 ? m.integrations_add_account() : m.integrations_badge_official()}
      onPress={() => onPress(preset)}
    />
  );
}

export function ToolSummaryRow({ tool, policy, busy, onAllowAlways, onRevokeAlways }: {
  tool: IntegrationToolSummary;
  policy?: PermissionPolicy;
  busy?: boolean;
  onAllowAlways?: (name: string) => void;
  onRevokeAlways?: (name: string) => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const accessTone = tool.effective_access === 'blocked'
    ? theme.tokens.status.error
    : tool.effective_access === 'read' || tool.effective_access === 'allowed_always'
      ? theme.tokens.status.success
      : theme.tokens.status.warning;

  const meta = [
    tool.read_only_hint === true ? m.integrations_tool_read_only() : m.integrations_tool_write(),
    toolAccessLabel(tool.effective_access),
  ].join(' · ');

  return (
    <View style={[styles.toolRow, { borderTopColor: c.border }]}>
      <View style={styles.toolInfo}>
        <Text style={[styles.toolName, { color: c.text }]}>{tool.name}</Text>
        <Text style={[styles.toolMeta, { color: accessTone }]}>{meta}</Text>
        {tool.description ? <Text style={[styles.toolDesc, { color: c.muted }]}>{tool.description}</Text> : null}
      </View>
      {tool.effective_access === 'ask_write' && onAllowAlways ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${m.integrations_tool_allow_always()} ${tool.name}`}
          accessibilityState={{ disabled: busy }}
          disabled={busy}
          onPress={() => onAllowAlways(tool.name)}
          style={({ pressed }) => [
            styles.allowBtn,
            {
              borderColor: c.borderStrong,
              backgroundColor: pressed ? c.hover : c.accentDim,
              opacity: busy ? 0.5 : 1,
            },
          ]}
        >
          <Text style={[styles.allowText, { color: c.accentText }]}>{m.integrations_tool_allow_always()}</Text>
        </Pressable>
      ) : tool.effective_access === 'allowed_always' && policy === 'read_free_ask_write' && onRevokeAlways ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${m.integrations_remove()} ${tool.name}`}
          accessibilityState={{ disabled: busy }}
          disabled={busy}
          onPress={() => onRevokeAlways(tool.name)}
          style={({ pressed }) => [
            styles.allowBtn,
            {
              borderColor: c.borderStrong,
              backgroundColor: pressed ? c.hover : 'transparent',
              opacity: busy ? 0.5 : 1,
            },
          ]}
        >
          <Text style={[styles.allowText, { color: theme.tokens.status.error }]}>{m.integrations_remove()}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

export function SetupGuideBlock({ guide, showGoogleTestingHint }: {
  guide: IntegrationSetupGuide;
  showGoogleTestingHint: boolean;
}) {
  return (
    <>
      <MachineGroup>
        <MachineRow title={guide.title} description={guide.docs_hint ?? undefined} />
        {guide.steps.map((step, idx) => (
          <MachineRow key={idx} title={`${idx + 1}. ${step}`} />
        ))}
      </MachineGroup>
      {showGoogleTestingHint ? <InfoNotice text={m.integrations_google_testing_hint()} /> : null}
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  toolRow: {
    minHeight: 56,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    paddingHorizontal: theme.base.space[4],
    paddingVertical: theme.base.space[3],
    borderTopWidth: 1,
  },
  toolInfo: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  toolName: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  toolMeta: { fontSize: theme.base.text.xs },
  toolDesc: { fontSize: theme.base.text.xs, lineHeight: 18 },
  allowBtn: {
    minHeight: 44,
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  allowText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
}));
