import { useMemo, useState, type Dispatch, type SetStateAction } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  createIntegration,
  formatCommandArgs,
  isValidIntegrationUrl,
  parseCommandArgs,
  parseEnvVars,
  parseHeaders,
  parseLines,
  parseScopes,
  updateIntegration,
  type CreateIntegrationInput,
  type IntegrationAuthConfig,
  type IntegrationPreset,
  type IntegrationSecretsPatch,
  type IntegrationTransportInput,
  type IntegrationView,
  type ParseEnvResult,
  type ParseHeadersResult,
  type PermissionPolicy,
  type Server,
  type UpdateIntegrationInput,
} from '@hangar/core';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { Segmented } from '../config/Segmented';
import { useSettingsColors } from '../config/colors';
import { IntegrationPolicyRows, SetupGuideBlock } from './IntegrationRows';
import { integrationErrorText } from './errors';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

type TransportKind = 'remote_http' | 'local_stdio';
type AuthKind = IntegrationAuthConfig['kind'];

const SLUG_RE = /^[a-z][a-z0-9-]{0,63}$/;

function suggestPresetSlug(baseId: string, existingIds: string[]): string {
  const used = new Set(existingIds);
  if (!used.has(baseId)) return baseId;
  for (let i = 2; i < 1000; i++) {
    const candidate = `${baseId}-${i}`;
    if (!used.has(candidate)) return candidate;
  }
  return baseId;
}

function headerErrorText(res: Extract<ParseHeadersResult, { ok: false }>): string {
  if (res.reason === 'invalid_name' || res.reason === 'reserved_name') {
    return m.integrations_validation_header_name({ name: res.name ?? '' });
  }
  if (res.reason === 'invalid_value' && res.name) {
    return `${m.integrations_field_headers()}: ${res.name}`;
  }
  return m.integrations_field_headers();
}

function envErrorText(res: Extract<ParseEnvResult, { ok: false }>): string {
  return res.key ? `${m.integrations_field_env()}: ${res.key}` : m.integrations_field_env();
}

const sameList = (a: readonly string[], b: readonly string[]) =>
  a.length === b.length && a.every((v, i) => v === b[i]);

function defaultAuthForPreset(preset?: IntegrationPreset | null): AuthKind {
  if (!preset) return 'oauth_dcr';
  return preset.auth_kind === 'oauth_manual_client' ? 'oauth_manual_client' : 'oauth_dcr';
}

/** Folha para criar (preset do catálogo ou customizada) ou editar uma integração existente. */
export function IntegrationEditorSheet({
  server,
  preset = null,
  editing = null,
  existingIds,
  onClose,
  onSaved,
}: {
  server: Server;
  preset?: IntegrationPreset | null;
  editing?: IntegrationView | null;
  existingIds: string[];
  onClose: () => void;
  onSaved: (integration: IntegrationView, opts: { isNew: boolean }) => void;
}) {
  const initialSlug = useMemo(
    () => (editing ? editing.id : preset ? suggestPresetSlug(preset.id, existingIds) : ''),
    [editing, preset, existingIds],
  );
  const [originalCommand] = useState<string[] | null>(() => (
    editing?.transport.kind === 'local_stdio' ? editing.transport.command : null
  ));
  const [initialCommandText] = useState(() => (
    originalCommand ? formatCommandArgs(originalCommand) : ''
  ));
  const [id, setId] = useState(initialSlug);
  const [name, setName] = useState(editing?.name ?? preset?.name ?? '');
  const [transportKind, setTransportKind] = useState<TransportKind>(
    editing?.transport.kind ?? 'remote_http',
  );
  const [url, setUrl] = useState(
    editing?.transport.kind === 'remote_http' ? editing.transport.url : preset?.url ?? '',
  );
  const [headersText, setHeadersText] = useState('');
  const [commandText, setCommandText] = useState(initialCommandText);
  const [cwd, setCwd] = useState(
    editing?.transport.kind === 'local_stdio' ? editing.transport.cwd ?? '' : '',
  );
  const [envText, setEnvText] = useState('');

  const [authKind, setAuthKind] = useState<AuthKind>(
    editing?.auth.kind ?? defaultAuthForPreset(preset),
  );
  const [clientId, setClientId] = useState(
    editing?.auth.kind === 'oauth_manual_client' ? editing.auth.client_id : '',
  );
  const [clientSecret, setClientSecret] = useState('');
  const [scopesText, setScopesText] = useState(
    editing?.auth.kind === 'oauth_manual_client'
      ? editing.auth.scopes.join(' ')
      : preset?.default_scopes.join(' ') ?? '',
  );
  const [bearerToken, setBearerToken] = useState('');

  const [policy, setPolicy] = useState<PermissionPolicy>(editing?.policy ?? 'read_free_ask_write');
  const [pinned, setPinned] = useState<boolean>(editing?.pinned ?? false);
  const [projectsText, setProjectsText] = useState((editing?.projects ?? []).join('\n'));
  const [removedHeaders, setRemovedHeaders] = useState<string[]>([]);
  const [removedEnvKeys, setRemovedEnvKeys] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const existingHeaderNames = useMemo(
    () => (editing?.transport.kind === 'remote_http' ? editing.transport.header_names : []),
    [editing],
  );
  const existingEnvKeys = useMemo(
    () => (editing?.transport.kind === 'local_stdio' ? editing.transport.env_keys : []),
    [editing],
  );

  const toggleRemoved = (setter: Dispatch<SetStateAction<string[]>>, key: string) => {
    haptic.selection();
    setter((cur) => (cur.includes(key) ? cur.filter((item) => item !== key) : [...cur, key]));
  };

  const isPreset = !!preset;
  const urlEditable = isPreset ? preset.url_editable : true;
  const authChoices: AuthKind[] = isPreset
    ? preset.auth_kind === 'oauth_dcr_or_bearer'
      ? ['oauth_dcr', 'bearer']
      : [defaultAuthForPreset(preset)]
    : transportKind === 'local_stdio'
      ? ['none']
      : ['oauth_dcr', 'oauth_manual_client', 'bearer', 'none'];

  const effectiveAuth: AuthKind = authChoices.includes(authKind) ? authKind : authChoices[0];
  const isOAuth = effectiveAuth === 'oauth_dcr' || effectiveAuth === 'oauth_manual_client';

  const authLabel = (kind: AuthKind): string => {
    switch (kind) {
      case 'oauth_dcr': return m.integrations_auth_oauth_dcr();
      case 'oauth_manual_client': return m.integrations_auth_oauth_manual();
      case 'bearer': return m.integrations_auth_bearer();
      case 'none': return m.integrations_auth_none();
    }
  };

  const keptHeaders = existingHeaderNames.filter((h) => !removedHeaders.includes(h));
  const keptEnvKeys = existingEnvKeys.filter((k) => !removedEnvKeys.includes(k));
  const commandUnchanged = !!originalCommand && commandText.trim() === initialCommandText.trim();

  const validate = (): string | null => {
    if (!editing && !SLUG_RE.test(id.trim())) return m.integrations_validation_id();
    if (!name.trim()) return m.integrations_validation_name();
    if (transportKind === 'remote_http') {
      const urlCheck = isValidIntegrationUrl(url, {
        hasAuthOrHeaders: effectiveAuth !== 'none' || keptHeaders.length > 0,
      });
      if (!urlCheck.valid) {
        return urlCheck.insecure ? m.integrations_validation_url_insecure() : m.integrations_validation_url();
      }
      const headersResult = parseHeaders(headersText);
      if (!headersResult.ok) return headerErrorText(headersResult);
      if (headersResult.names.length > 0) {
        const withNewHeaders = isValidIntegrationUrl(url, { hasAuthOrHeaders: true });
        if (!withNewHeaders.valid) {
          return withNewHeaders.insecure ? m.integrations_validation_url_insecure() : m.integrations_validation_url();
        }
      }
    } else {
      if (commandUnchanged) {
        if (originalCommand.length === 0) return m.integrations_validation_command();
      } else {
        const parsedCmd = parseCommandArgs(commandText);
        if (!parsedCmd.ok) return m.integrations_validation_command_quotes();
        if (parsedCmd.args.length === 0) return m.integrations_validation_command();
      }
      const envResult = parseEnvVars(envText);
      if (!envResult.ok) return envErrorText(envResult);
    }
    if (effectiveAuth === 'oauth_manual_client' && !clientId.trim()) {
      return m.integrations_validation_client_id();
    }
    return null;
  };

  const submit = async () => {
    if (busy) return;
    const invalid = validate();
    if (invalid) {
      setError(invalid);
      haptic.error();
      return;
    }

    const parsedHeaders = transportKind === 'remote_http'
      ? parseHeaders(headersText)
      : { ok: true as const, names: [], map: {} };
    if (!parsedHeaders.ok) {
      setError(headerErrorText(parsedHeaders));
      haptic.error();
      return;
    }
    const parsedEnv = transportKind === 'local_stdio'
      ? parseEnvVars(envText)
      : { ok: true as const, keys: [], map: {} };
    if (!parsedEnv.ok) {
      setError(envErrorText(parsedEnv));
      haptic.error();
      return;
    }

    let nextCommand: string[] = [];
    if (transportKind === 'local_stdio') {
      if (commandUnchanged && originalCommand) {
        // Preserva o vetor salvo quando só cwd ou env mudaram sem editar o comando.
        nextCommand = originalCommand;
      } else {
        const parsedCmd = parseCommandArgs(commandText);
        if (!parsedCmd.ok) {
          setError(m.integrations_validation_command_quotes());
          haptic.error();
          return;
        }
        nextCommand = parsedCmd.args;
      }
    }

    setBusy(true);
    setError('');
    try {
      const transport: IntegrationTransportInput = transportKind === 'remote_http'
        ? {
          kind: 'remote_http',
          url: url.trim(),
          header_names: Array.from(new Set([...keptHeaders, ...parsedHeaders.names])),
        }
        : {
          kind: 'local_stdio',
          command: nextCommand,
          cwd: cwd.trim() || null,
          env_keys: Array.from(new Set([...keptEnvKeys, ...parsedEnv.keys])),
        };

      const auth: IntegrationAuthConfig = effectiveAuth === 'oauth_manual_client'
        ? { kind: 'oauth_manual_client', client_id: clientId.trim(), scopes: parseScopes(scopesText) }
        : effectiveAuth === 'oauth_dcr'
          ? { kind: 'oauth_dcr' }
          : effectiveAuth === 'bearer'
            ? { kind: 'bearer' }
            : { kind: 'none' };

      const secrets: IntegrationSecretsPatch = {};
      if (effectiveAuth === 'bearer' && bearerToken.trim()) {
        secrets.bearer_token = bearerToken.trim();
      }
      if (effectiveAuth === 'oauth_manual_client' && clientSecret.trim()) {
        secrets.oauth_client_secret = clientSecret.trim();
      }
      if (transportKind === 'remote_http') {
        const headersPatch: Record<string, string | null> = {};
        for (const removed of removedHeaders) headersPatch[removed] = null;
        for (const [k, v] of Object.entries(parsedHeaders.map)) headersPatch[k] = v;
        if (Object.keys(headersPatch).length > 0) secrets.headers = headersPatch;
      }
      if (transportKind === 'local_stdio') {
        const envPatch: Record<string, string | null> = {};
        for (const removed of removedEnvKeys) envPatch[removed] = null;
        for (const [k, v] of Object.entries(parsedEnv.map)) envPatch[k] = v;
        if (Object.keys(envPatch).length > 0) secrets.env = envPatch;
      }
      const hasSecrets = Object.keys(secrets).length > 0;
      const projects = parseLines(projectsText);

      if (editing) {
        const transportChanged = editing.transport.kind !== transport.kind
          || (editing.transport.kind === 'remote_http' && transport.kind === 'remote_http'
            && (transport.url !== editing.transport.url
              || !sameList(transport.header_names ?? [], editing.transport.header_names)))
          || (editing.transport.kind === 'local_stdio' && transport.kind === 'local_stdio'
            && (!sameList(transport.command, editing.transport.command)
              || (transport.cwd ?? null) !== (editing.transport.cwd ?? null)
              || !sameList(transport.env_keys ?? [], editing.transport.env_keys)));

        const patch: UpdateIntegrationInput = {
          name: name.trim(),
          // Omitir transporte inalterado evita invalidar sessões ou aprovações no servidor.
          ...(transportChanged ? { transport } : {}),
          auth,
          policy,
          pinned,
          projects,
          ...(hasSecrets ? { secrets } : {}),
        };
        const updated = await updateIntegration(server, editing.id, patch);
        toast.ok(m.integrations_saved());
        onSaved(updated, { isNew: false });
        onClose();
      } else {
        const body: CreateIntegrationInput = {
          id: id.trim(),
          name: name.trim(),
          preset: preset?.id ?? null,
          transport,
          auth,
          policy,
          pinned,
          projects,
          ...(hasSecrets ? { secrets } : {}),
        };
        const created = await createIntegration(server, body);
        toast.ok(m.integrations_created());
        onSaved(created, { isNew: true });
        onClose();
      }
    } catch (e) {
      setError(integrationErrorText(e));
      haptic.error();
    } finally {
      setBusy(false);
    }
  };

  const title = editing ? editing.name : preset ? preset.name : m.integrations_add_custom();

  return (
    <MachineSheet title={title} onClose={onClose}>
      {preset?.setup_guide ? (
        <>
          <MachineHelp>{m.integrations_setup_guide_title()}</MachineHelp>
          <SetupGuideBlock guide={preset.setup_guide} showGoogleTestingHint={preset.id.startsWith('google-')} />
        </>
      ) : null}

      <MachineGroup>
        {!editing ? (
          <MachineField
            label={m.integrations_field_id()}
            value={id}
            onChangeText={(t) => setId(t.toLowerCase())}
            editable={!busy}
            placeholder={preset?.id ?? 'gmail-pessoal'}
          />
        ) : null}
        <MachineField
          label={m.integrations_field_name()}
          value={name}
          onChangeText={setName}
          editable={!busy}
          autoCapitalize="words"
          placeholder={m.integrations_field_name_placeholder()}
        />
      </MachineGroup>
      {!editing ? <MachineHelp>{m.integrations_field_id_hint()}</MachineHelp> : null}

      {!isPreset && !editing?.preset ? (
        <>
          <MachineHelp>{m.integrations_field_transport()}</MachineHelp>
          <Segmented<TransportKind>
            label={m.integrations_field_transport()}
            value={transportKind}
            disabled={busy}
            minHeight={44}
            onChange={(next) => {
              haptic.selection();
              setTransportKind(next);
            }}
            options={[
              { v: 'remote_http', label: m.integrations_transport_remote() },
              { v: 'local_stdio', label: m.integrations_transport_local() },
            ]}
          />
        </>
      ) : null}

      {transportKind === 'remote_http' ? (
        <MachineGroup>
          <MachineField
            label={m.integrations_field_url()}
            value={url}
            onChangeText={setUrl}
            editable={!busy && urlEditable}
            keyboardType="url"
            placeholder="https://..."
          />
          <MachineField
            label={m.integrations_field_headers()}
            value={headersText}
            onChangeText={setHeadersText}
            editable={!busy}
            stacked
            multiline
            placeholder="X-Workspace: meu-id"
          />
        </MachineGroup>
      ) : (
        <MachineGroup>
          <MachineField
            label={m.integrations_field_command()}
            value={commandText}
            onChangeText={setCommandText}
            editable={!busy}
            stacked
            multiline
            placeholder="npx -y @modelcontextprotocol/server-filesystem"
          />
          <MachineField
            label={m.integrations_field_cwd()}
            value={cwd}
            onChangeText={setCwd}
            editable={!busy}
            placeholder={m.integrations_optional()}
          />
          <MachineField
            label={m.integrations_field_env()}
            value={envText}
            onChangeText={setEnvText}
            editable={!busy}
            stacked
            multiline
            placeholder="API_TOKEN=..."
          />
        </MachineGroup>
      )}
      {transportKind === 'local_stdio' ? (
        <MachineHelp>{`${m.integrations_field_command_hint()} · ${m.integrations_field_env_hint()}`}</MachineHelp>
      ) : null}
      {editing && transportKind === 'remote_http' && existingHeaderNames.length > 0 ? (
        <MachineGroup>
          {existingHeaderNames.map((headerName) => (
            <ExistingSecretRow
              key={headerName}
              name={headerName}
              marked={removedHeaders.includes(headerName)}
              disabled={busy}
              onToggle={() => toggleRemoved(setRemovedHeaders, headerName)}
            />
          ))}
        </MachineGroup>
      ) : null}
      {editing && transportKind === 'local_stdio' && existingEnvKeys.length > 0 ? (
        <MachineGroup>
          {existingEnvKeys.map((envKey) => (
            <ExistingSecretRow
              key={envKey}
              name={envKey}
              marked={removedEnvKeys.includes(envKey)}
              disabled={busy}
              onToggle={() => toggleRemoved(setRemovedEnvKeys, envKey)}
            />
          ))}
        </MachineGroup>
      ) : null}

      <MachineHelp>{m.integrations_field_auth()}</MachineHelp>
      <MachineGroup>
        {authChoices.map((kind) => (
          <MachineRow
            key={kind}
            title={authLabel(kind)}
            selected={effectiveAuth === kind}
            disabled={busy || authChoices.length === 1}
            onPress={() => {
              if (authChoices.length > 1) {
                haptic.selection();
                setAuthKind(kind);
              }
            }}
          />
        ))}
      </MachineGroup>

      {effectiveAuth === 'oauth_manual_client' ? (
        <MachineGroup>
          <MachineField
            label={m.integrations_field_client_id()}
            value={clientId}
            onChangeText={setClientId}
            editable={!busy}
          />
          <MachineField
            label={m.integrations_field_client_secret()}
            value={clientSecret}
            onChangeText={setClientSecret}
            editable={!busy}
            secureTextEntry
            autoComplete="off"
            textContentType="none"
            placeholder={editing ? m.integrations_secret_kept_hint() : m.integrations_optional()}
          />
          <MachineField
            label={m.integrations_field_scopes()}
            value={scopesText}
            onChangeText={setScopesText}
            editable={!busy}
            placeholder={m.integrations_optional()}
          />
        </MachineGroup>
      ) : null}

      {effectiveAuth === 'bearer' ? (
        <MachineGroup>
          <MachineField
            label={m.integrations_field_bearer()}
            value={bearerToken}
            onChangeText={setBearerToken}
            editable={!busy}
            secureTextEntry
            autoComplete="off"
            textContentType="none"
            placeholder={editing ? m.integrations_secret_kept_hint() : undefined}
          />
        </MachineGroup>
      ) : null}

      <MachineHelp>{m.integrations_field_policy()}</MachineHelp>
      <MachineGroup>
        <IntegrationPolicyRows value={policy} onChange={setPolicy} disabled={busy} />
      </MachineGroup>

      <MachineGroup>
        <MachineRow
          title={m.integrations_field_pinned()}
          description={m.integrations_field_pinned_hint()}
          value={pinned ? m.integrations_yes() : m.integrations_no()}
          checked={pinned}
          disabled={busy}
          onPress={() => {
            haptic.selection();
            setPinned((v) => !v);
          }}
        />
        <MachineField
          label={m.integrations_field_projects()}
          value={projectsText}
          onChangeText={setProjectsText}
          editable={!busy}
          stacked
          multiline
          placeholder={m.integrations_projects_all()}
        />
      </MachineGroup>
      <MachineHelp>{m.integrations_field_projects_hint()}</MachineHelp>

      <Notice text={error} error />

      <MachineGroup>
        <MachineRow
          title={busy ? m.integrations_saving() : editing ? m.integrations_save() : isOAuth ? m.integrations_connect() : m.integrations_save()}
          action
          disabled={busy}
          onPress={() => void submit()}
        />
        <MachineRow title={m.integrations_cancel()} disabled={busy} onPress={onClose} />
      </MachineGroup>
    </MachineSheet>
  );
}

function ExistingSecretRow({
  name,
  marked,
  disabled,
  onToggle,
}: {
  name: string;
  marked: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const actionLabel = marked ? m.native_server_undo() : m.integrations_remove();
  const actionTone = marked ? c.text : theme.tokens.status.error;
  return (
    <View style={[styles.secretRow, { borderTopColor: c.border }]}>
      <View style={styles.secretInfo}>
        <Text style={[styles.secretName, { color: marked ? c.muted : c.text }]}>{name}</Text>
        <Text style={[styles.secretHint, { color: marked ? theme.tokens.status.error : c.muted }]}>
          {marked ? m.native_server_remove_on_save() : m.integrations_secret_kept_hint()}
        </Text>
      </View>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`${actionLabel} ${name}`}
        accessibilityState={{ disabled }}
        disabled={disabled}
        onPress={onToggle}
        style={({ pressed }) => [
          styles.secretAction,
          {
            borderColor: c.borderStrong,
            backgroundColor: pressed ? c.hover : 'transparent',
            opacity: disabled ? 0.5 : 1,
          },
        ]}
      >
        <Icon name={marked ? 'RotateCcw' : 'Trash2'} size={14} color={actionTone} />
        <Text style={[styles.secretActionText, { color: actionTone }]}>{actionLabel}</Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  secretRow: {
    minHeight: 56,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    paddingHorizontal: theme.base.space[4],
    paddingVertical: theme.base.space[3],
    borderTopWidth: 1,
  },
  secretInfo: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  secretName: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  secretHint: { fontSize: theme.base.text.xs, lineHeight: 18 },
  secretAction: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1.5],
    paddingHorizontal: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.sm,
  },
  secretActionText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
}));
