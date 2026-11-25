import { useMemo, useState } from 'react';
import {
  createIntegration,
  updateIntegration,
  type CreateIntegrationInput,
  type IntegrationAuthConfig,
  type IntegrationPreset,
  type IntegrationSecretsPatch,
  type IntegrationTransportInput,
  type IntegrationView,
  type PermissionPolicy,
  type Server,
  type UpdateIntegrationInput,
} from '@hangar/core';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { Segmented } from '../config/Segmented';
import { IntegrationPolicyRows, SetupGuideBlock } from './IntegrationRows';
import { integrationErrorText } from './errors';
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

function isValidHttpUrl(raw: string): boolean {
  try {
    const u = new URL(raw.trim());
    return (u.protocol === 'https:' || u.protocol === 'http:') && !!u.hostname && !u.username && !u.password;
  } catch {
    return false;
  }
}

function parseCommandArgs(raw: string): string[] {
  const trimmed = raw.trim();
  if (!trimmed) return [];
  if (trimmed.includes('\n')) {
    return trimmed.split('\n').map((s) => s.trim()).filter(Boolean);
  }
  const out: string[] = [];
  let cur = '';
  let quote: '"' | "'" | null = null;
  for (const ch of trimmed) {
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (/\s/.test(ch)) {
      if (cur) { out.push(cur); cur = ''; }
    } else {
      cur += ch;
    }
  }
  if (cur) out.push(cur);
  return out;
}

function parseHeaders(raw: string): { names: string[]; map: Record<string, string> } {
  const names: string[] = [];
  const map: Record<string, string> = {};
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const sep = trimmed.indexOf(':');
    if (sep <= 0) continue;
    const name = trimmed.slice(0, sep).trim().toLowerCase();
    const val = trimmed.slice(sep + 1).trim();
    if (!name || !val) continue;
    if (!names.includes(name)) names.push(name);
    map[name] = val;
  }
  return { names, map };
}

function parseEnvVars(raw: string): { keys: string[]; map: Record<string, string> } {
  const keys: string[] = [];
  const map: Record<string, string> = {};
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const sep = trimmed.indexOf('=');
    if (sep <= 0) continue;
    const key = trimmed.slice(0, sep).trim();
    const val = trimmed.slice(sep + 1);
    if (!key) continue;
    if (!keys.includes(key)) keys.push(key);
    map[key] = val;
  }
  return { keys, map };
}

function parseLines(raw: string): string[] {
  return raw.split('\n').map((s) => s.trim()).filter(Boolean);
}

function parseScopes(raw: string): string[] {
  return raw.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
}

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
  const [id, setId] = useState(initialSlug);
  const [name, setName] = useState(editing?.name ?? preset?.name ?? '');
  const [transportKind, setTransportKind] = useState<TransportKind>(
    editing?.transport.kind ?? 'remote_http',
  );
  const [url, setUrl] = useState(
    editing?.transport.kind === 'remote_http' ? editing.transport.url : preset?.url ?? '',
  );
  const [headersText, setHeadersText] = useState('');
  const [commandText, setCommandText] = useState(
    editing?.transport.kind === 'local_stdio' ? editing.transport.command.join('\n') : '',
  );
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

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

  const validate = (): string | null => {
    if (!editing && !SLUG_RE.test(id.trim())) return m.integrations_validation_id();
    if (!name.trim()) return m.integrations_validation_name();
    if (transportKind === 'remote_http') {
      if (!isValidHttpUrl(url)) return m.integrations_validation_url();
    } else if (parseCommandArgs(commandText).length === 0) {
      return m.integrations_validation_command();
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

    setBusy(true);
    setError('');
    try {
      const parsedHeaders = parseHeaders(headersText);
      const parsedEnv = parseEnvVars(envText);
      const existingHeaderNames = editing?.transport.kind === 'remote_http' ? editing.transport.header_names : [];
      const existingEnvKeys = editing?.transport.kind === 'local_stdio' ? editing.transport.env_keys : [];

      const transport: IntegrationTransportInput = transportKind === 'remote_http'
        ? {
          kind: 'remote_http',
          url: url.trim(),
          header_names: Array.from(new Set([...existingHeaderNames, ...parsedHeaders.names])),
        }
        : {
          kind: 'local_stdio',
          command: parseCommandArgs(commandText),
          cwd: cwd.trim() || null,
          env_keys: Array.from(new Set([...existingEnvKeys, ...parsedEnv.keys])),
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
      if (transportKind === 'remote_http' && parsedHeaders.names.length > 0) {
        secrets.headers = parsedHeaders.map;
      }
      if (transportKind === 'local_stdio' && parsedEnv.keys.length > 0) {
        secrets.env = parsedEnv.map;
      }
      const hasSecrets = Object.keys(secrets).length > 0;
      const projects = parseLines(projectsText);

      if (editing) {
        const patch: UpdateIntegrationInput = {
          name: name.trim(),
          transport,
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
      {editing && ((transportKind === 'remote_http' && editing.transport.kind === 'remote_http' && editing.transport.header_names.length > 0)
        || (transportKind === 'local_stdio' && editing.transport.kind === 'local_stdio' && editing.transport.env_keys.length > 0)) ? (
          <MachineHelp>{m.integrations_secret_kept_hint()}</MachineHelp>
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
