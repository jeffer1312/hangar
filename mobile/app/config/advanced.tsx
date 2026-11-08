import { useState } from 'react';
import { Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { VariavelEnv } from '@hangar/core';
import { BlockHead, Box, Chip, ConfigRow, ScopeChips, ServerConfigPage, useInputStyle } from '../../src/features/config/ServerConfigParts';
import { Pill } from '../../src/features/config/PageHeader';
import { textOf, useServerConfig, type ServerConfig } from '../../src/features/config/serverConfig';
import { useSettingsColors } from '../../src/features/config/colors';
import { Icon } from '../../src/ui/Icon';
import * as m from '../../src/paraglide/messages';

const KEYS = ['automations', 'mostrar_pensamento', 'traduzir_pensamento', 'compactar_parada', 'compactar_parada_min_mil', 'editor'];
// Do bloco só leitura, o que Servidores já mostra não se repete aqui.
const READ_IN_MACHINES = ['port', 'lan_bind_ip', 'server_id', 'public_url', 'terminal_origem_ok'];
const READ_LABELS: Record<string, () => string> = {
  terminal_panel: m.native_server_read_terminal_panel,
  traducao_pensamento: m.native_server_read_traducao_pensamento,
  versao: m.native_server_read_versao,
};
// Códigos de descrição do `.env`; código fora daqui não vira texto nenhum e a linha fica só com o nome.
const ENV_TEXT: Record<string, () => string> = {
  terminal: m.config_server_env_terminal, pricing_offline: m.config_server_env_pricing_offline,
  claude_config_dirs: m.config_server_env_claude_config_dirs, engines_file: m.config_server_env_engines_file,
  codex_sync_enabled: m.config_server_env_codex_sync_enabled, auto_resume: m.config_server_env_auto_resume,
  omp_plugin_sync: m.config_server_env_omp_plugin_sync, omp_claude_context: m.config_server_env_omp_claude_context,
  lan_bind_ip: m.config_server_env_lan_bind_ip, port: m.config_server_env_port, auth_token: m.config_server_env_auth_token,
  projects_dir: m.config_server_env_projects_dir, reload: m.config_server_env_reload, diag_term_input: m.config_server_env_diag_term_input,
  front_port: m.config_server_env_front_port, public_url: m.config_server_env_public_url, server_id: m.config_server_env_server_id,
  vapid_public: m.config_server_env_vapid_public, vapid_private: m.config_server_env_vapid_private, vapid_subject: m.config_server_env_vapid_subject,
  stall_poll_seconds: m.config_server_env_stall_poll_seconds, omp_plugin_sync_interval: m.config_server_env_omp_plugin_sync_interval,
  sync: m.config_server_env_sync, sync_bootstrap: m.config_server_env_sync_bootstrap, sync_data: m.config_server_env_sync_data,
  sync_session_secret: m.config_server_env_sync_session_secret, sync_rate_max: m.config_server_env_sync_rate_max,
  sync_rate_window: m.config_server_env_sync_rate_window, forwarded_allow_ips: m.config_server_env_forwarded_allow_ips,
  deploy_secret: m.config_server_env_deploy_secret, update_branch: m.config_server_env_update_branch,
};
const ENV_ALERTS: Record<string, () => string> = { codex_sync_desligado: m.config_server_env_alerta_codex_sync };

/** Valor só leitura: sim/não, "—" quando vazio. */
const shown = (v: unknown) => (typeof v === 'boolean' ? (v ? m.native_server_yes() : m.native_server_no()) : textOf(v) || '—');
/** O segredo do `.env` não vem do servidor: a linha diz só se está definido. */
const envValue = (v: VariavelEnv) => (v.segredo ? (v.definida ? m.native_server_env_set() : m.native_server_env_unset()) : shown(v.valor));

export default function Advanced() {
  const cfg = useServerConfig();
  return (
    <ServerConfigPage title={m.native_settings_page_advanced()} cfg={cfg}>
      <Box>{KEYS.map((k) => <ConfigRow key={k} cfg={cfg} k={k} />)}</Box>
      <Roots cfg={cfg} />
      <ReadOnly cfg={cfg} />
      <Env cfg={cfg} />
    </ServerConfigPage>
  );
}

/** Pastas mapeadas (`scan_roots`, string "a,b" do `CP_SCAN_ROOTS`): a tela edita como lista. */
function Roots({ cfg }: { cfg: ServerConfig }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const input = useInputStyle();
  const [typed, setTyped] = useState('');
  const roots = textOf(cfg.current('scan_roots')).split(',').map((p) => p.trim()).filter(Boolean);
  const off = !('scan_roots' in cfg.fields);
  // Só quem adicionou limpa o campo: repetido não mexe e quem digitou não perde o texto.
  const add = () => {
    const path = typed.trim();
    if (!path || roots.includes(path)) return;
    if (cfg.stage('scan_roots', [...roots, path].join(','))) setTyped('');
  };
  return (
    <>
      <BlockHead title={m.native_server_roots()} help={m.native_server_roots_help()} right={<View style={styles.chips}><ScopeChips cfg={cfg} k="scan_roots" /></View>} />
      <Box>
        {roots.length === 0 ? (
          <Text style={[styles.line, styles.help, { borderTopColor: c.border, color: c.muted }]}>{m.native_server_roots_empty()}</Text>
        ) : roots.map((p) => (
          <View key={p} style={[styles.line, styles.rootRow, { borderTopColor: c.border }]}>
            <Icon name="Folder" size={16} color={c.muted} />
            <Text style={[styles.flex, styles.mono, { color: c.text, fontFamily: theme.base.fontMono }]} numberOfLines={1} ellipsizeMode="middle">{p}</Text>
            <Pill icon="X" label={m.native_server_remove()}
              onPress={() => cfg.stage('scan_roots', roots.filter((r) => r !== p).join(','))} />
          </View>
        ))}
        <View style={[styles.line, styles.rootRow, { borderTopColor: c.border }]}>
          <TextInput {...input} style={[input.style, styles.flex]} accessibilityLabel={m.native_server_root_new()} editable={!off}
            placeholder={m.native_server_root_placeholder()} autoCapitalize="none" autoCorrect={false}
            value={typed} onChangeText={setTyped} onSubmitEditing={add} returnKeyType="done" />
          {typed.trim() ? <Pill label={m.native_server_root_add()} onPress={add} /> : null}
        </View>
      </Box>
    </>
  );
}

/** O que só a máquina decide: o app lê e não grava. */
function ReadOnly({ cfg }: { cfg: ServerConfig }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const rows = Object.entries(cfg.read).filter(([k]) => !READ_IN_MACHINES.includes(k));
  if (!rows.length) return null;
  return (
    <>
      <BlockHead title={m.native_server_machine_only()} help={m.native_server_machine_only_help()} />
      <Box>
        {rows.map(([k, v]) => (
          <View key={k} style={[styles.line, styles.pair, { borderTopColor: c.border }]}>
            <Text style={[styles.flex, styles.help, { color: c.text }]}>{READ_LABELS[k]?.() ?? k}</Text>
            <Text selectable style={[styles.value, styles.mono, { color: c.muted, fontFamily: theme.base.fontMono }]}>{shown(v)}</Text>
          </View>
        ))}
      </Box>
    </>
  );
}

/** Variáveis do `.env`: nome, valor (segredo só diz se está definido), descrição e alerta traduzidos por código. */
function Env({ cfg }: { cfg: ServerConfig }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  if (!cfg.env.length) return null;
  return (
    <>
      <BlockHead title={m.native_server_env()} help={m.native_server_env_help()} right={<Chip text={m.native_server_env_scope()} />} />
      <Box>
        {cfg.env.map((v) => {
          const description = v.descricao ? ENV_TEXT[v.descricao]?.() : undefined;
          const alert = v.alerta ? ENV_ALERTS[v.alerta]?.() : undefined;
          return (
            <View key={v.nome} style={[styles.line, styles.envRow, { borderTopColor: c.border }]}>
              <View style={styles.pair}>
                <Text selectable style={[styles.mono, { color: c.text, fontFamily: theme.base.fontMono }]}>{v.nome}</Text>
                <Text selectable style={[styles.value, styles.mono, { color: c.muted, fontFamily: theme.base.fontMono }, !v.definida && styles.italic]}>
                  {envValue(v)}
                </Text>
              </View>
              {description ? <Text style={[styles.small, { color: c.muted }]}>{description}</Text> : null}
              {alert ? <Text style={[styles.small, { color: theme.tokens.status.warning }]}>{alert}</Text> : null}
            </View>
          );
        })}
      </Box>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  flex: { flex: 1, minWidth: 0 },
  chips: { flexDirection: 'row', gap: theme.base.space[1.5] },
  line: { borderTopWidth: 1, paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3] },
  rootRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  pair: { flexDirection: 'row', alignItems: 'flex-start', justifyContent: 'space-between', gap: theme.base.space[4] },
  envRow: { gap: theme.base.space[1] },
  help: { fontSize: theme.base.text.sm },
  mono: { fontSize: theme.base.text.xs },
  value: { flexShrink: 1, maxWidth: '60%', textAlign: 'right' },
  small: { fontSize: theme.base.text.xs, lineHeight: 17 },
  italic: { fontStyle: 'italic' },
}));
