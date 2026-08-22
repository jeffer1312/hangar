import { useEffect, useState, useCallback } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { createSession, getEngines, getSessions, listClaudeConfigs, modelOptions } from '@hangar/core';
import { basename, providerName } from '@hangar/core';
import type { ConfigDirInfo, Provider, ModelOption } from '@hangar/core';
import { MenuView } from '@react-native-menu/menu';
import { useServers } from '../../stores/servers';
import { CwdPicker } from './CwdPicker';
import { ProviderPicker } from './ProviderPicker';
import * as m from '../../paraglide/messages';

const PROVIDERS: Provider[] = ['claude', 'codex', 'pi', 'kimi'];
const NIVEIS: Record<string, string[]> = {
  claude: ['low', 'medium', 'high', 'xhigh', 'max'],
  pi: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
};
const MODOS_PERMISSAO = ['acceptEdits', 'auto', 'bypassPermissions', 'manual', 'dontAsk', 'plan'];

function uniqueName(base: string, taken: Set<string>): string {
  const clean = base.replace(/[^A-Za-z0-9_-]/g, '-').replace(/^-+|-+$/g, '') || 'sessao';
  if (!taken.has(clean)) return clean;
  let i = 2;
  while (taken.has(`${clean}-${i}`)) i++;
  return `${clean}-${i}`;
}

function valorModelo(mm: ModelOption): string {
  return mm.provider ? `${mm.provider}/${mm.id}` : mm.id;
}

// pequeno wrapper pra MenuView — renderiza botão com valor atual e abre menu nativo
function MenuSelect({
  value,
  options,
  onChange,
  placeholder,
}: {
  value: string;
  options: { value: string; label: string; hint?: string }[];
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const actions = options.map((o) => ({
    id: o.value,
    title: o.label,
    subtitle: o.hint ?? undefined,
    state: (o.value === value ? 'on' : 'off') as 'on' | 'off',
  }));
  const label = options.find((o) => o.value === value)?.label ?? placeholder ?? '—';
  return (
    <MenuView actions={actions} onPressAction={({ nativeEvent }) => onChange(nativeEvent.event)}>
      <Pressable style={styles.selectBtn}>
        <Text style={styles.selectTxt} numberOfLines={1}>
          {label}
        </Text>
        <Text style={styles.selectChevron}>›</Text>
      </Pressable>
    </MenuView>
  );
}

export function CreateSessionSheet({ onClose }: { onClose?: () => void }) {
  const router = useRouter();
  const active = useServers((s) => s.active());
  const serverId = active?.id ?? useServers.getState().servers[0]?.id ?? '';

  const [picked, setPicked] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [checking, setChecking] = useState(false);
  const [takenNames, setTakenNames] = useState<Set<string>>(new Set());
  const [hasSameFolder, setHasSameFolder] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const [provider, setProvider] = useState<Provider>('claude');

  const [configs, setConfigs] = useState<ConfigDirInfo[]>([]);
  const [selectedConfig, setSelectedConfig] = useState<string | null>(null);
  const [motores, setMotores] = useState<Record<string, { label?: string; model?: string }>>({});
  const [engine, setEngine] = useState('');
  const [modelos, setModelos] = useState<ModelOption[]>([]);
  const [modelo, setModelo] = useState('');
  const [esforco, setEsforco] = useState('');
  const [permissao, setPermissao] = useState('');
  const [listaReduzida, setListaReduzida] = useState(false);
  const [erroModelos, setErroModelos] = useState('');
  const [manualOpen, setManualOpen] = useState(false);
  const [manualPath, setManualPath] = useState('');

  // carrega configs + motores uma vez (e quando provider volta a claude)
  useEffect(() => {
    let alive = true;
    void listClaudeConfigs()
      .then((cs) => {
        if (!alive) return;
        setConfigs(cs);
        const sel = cs.find((c) => c.active)?.path ?? cs[0]?.path ?? null;
        setSelectedConfig(sel);
      })
      .catch(() => {
        if (alive) setConfigs([]);
      });
    void getEngines()
      .then((r) => {
        if (alive) setMotores(r.motores as any);
      })
      .catch(() => {
        if (alive) setMotores({});
      });
    return () => {
      alive = false;
    };
  }, []);

  // modelos quando provider/config/engine mudam
  useEffect(() => {
    // reset incondicional — igual à PWA (CreateSessionSheet.svelte:141), evita vazar modelo/esforço pro Codex
    setModelo('');
    setEsforco('');
    if (provider !== 'claude' && provider !== 'pi' && provider !== 'kimi') {
      setModelos([]);
      setListaReduzida(false);
      setErroModelos('');
      return;
    }
    let alive = true;
    setErroModelos('');
    void modelOptions(provider, engine || null, selectedConfig)
      .then((r) => {
        if (!alive) return;
        setModelos(r.models);
        setListaReduzida(r.reduced);
      })
      .catch((e) => {
        if (!alive) return;
        setModelos([]);
        setErroModelos(e instanceof Error ? e.message : m.criar_modelos_erro());
      });
    return () => {
      alive = false;
    };
  }, [provider, engine, selectedConfig]);

  const handlePick = useCallback(async (p: string) => {
    setPicked(p);
    setError('');
    setChecking(true);
    try {
      const sessions = await getSessions();
      const taken = new Set(sessions.map((s) => s.name));
      setTakenNames(taken);
      setHasSameFolder(sessions.some((s) => (s as any).cwd === p));
      setName(uniqueName(basename(p), taken));
    } catch {
      setTakenNames(new Set());
      setHasSameFolder(false);
      setName(basename(p));
    } finally {
      setChecking(false);
    }
  }, []);

  const handleManual = () => {
    const p = manualPath.trim();
    if (p) void handlePick(p);
  };

  const canCreate = !!picked && !!name.trim() && !loading;

  const handleCreate = async () => {
    if (!picked || !name.trim()) return;
    setLoading(true);
    setError('');
    try {
      const s = await createSession(
        name.trim(),
        picked,
        provider === 'claude' ? selectedConfig : null,
        provider,
        provider === 'claude' ? engine || null : null,
        (provider === 'claude' || provider === 'pi' || provider === 'kimi') ? (modelo || null) : null,
        (provider === 'claude' || provider === 'pi') ? (esforco || null) : null,
        provider === 'claude' ? permissao || null : null,
      );
      // sucesso → abre chat da nova sessão — não chamar onClose (router.back) que desfaz o replace
      if (serverId) {
        router.replace((`/s/${serverId}/${s.name}` as never) as never);
      } else {
        router.replace('/' as never);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : m.criar_sessao_erro());
    } finally {
      setLoading(false);
    }
  };

  // sem pasta escolhida → picker + opção manual
  if (!picked) {
    return (
      <View style={styles.root}>
        <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
          <Text style={styles.title}>{m.sessao_nova()}</Text>
          <View style={styles.pickerWrap}>
            <CwdPicker onPick={handlePick} selected={picked} />
          </View>
          <View style={styles.advanced}>
            <Pressable onPress={() => setManualOpen((v) => !v)} style={styles.advToggle}>
              <Text style={styles.advTxt}>{m.criar_avancado()}</Text>
              <Text style={[styles.chev, manualOpen && styles.chevOpen]}>›</Text>
            </Pressable>
            {manualOpen ? (
              <View style={styles.manualForm}>
                <TextInput
                  style={styles.input}
                  value={manualPath}
                  onChangeText={setManualPath}
                  placeholder={m.criar_caminho_placeholder()}
                  placeholderTextColor="#8d8489"
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                <Pressable onPress={handleManual} disabled={!manualPath.trim()} style={[styles.manualGo, !manualPath.trim() && styles.manualGoDis]}>
                  <Text style={styles.manualGoTxt}>{m.criar_usar()}</Text>
                </Pressable>
              </View>
            ) : null}
          </View>
        </ScrollView>
      </View>
    );
  }

  // com pasta → formulário
  return (
    <View style={styles.root}>
      <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
        <View style={styles.picked}>
          <Text style={styles.pickedName}>{basename(picked)}</Text>
          <Text style={styles.pickedPath} numberOfLines={1}>
            {picked}
          </Text>
        </View>

        {checking ? (
          <View style={styles.rowCenter}>
            <ActivityIndicator />
            <Text style={styles.hint}>{m.criar_verificando()}</Text>
          </View>
        ) : (
          <>
            {hasSameFolder ? <Text style={styles.hint}>{m.criar_ja_existe()}</Text> : null}
            <View style={styles.field}>
              <Text style={styles.label}>{m.comum_nome()}</Text>
              <TextInput
                style={styles.input}
                value={name}
                onChangeText={setName}
                placeholder={m.criar_nome_placeholder()}
                autoCapitalize="none"
                autoCorrect={false}
                placeholderTextColor="#8d8489"
              />
            </View>

            <View style={styles.field}>
              <Text style={styles.label}>{m.comum_provider()}</Text>
              <ProviderPicker value={provider} onChange={(p) => setProvider(p)} />
            </View>

            {provider === 'claude' && configs.length > 1 ? (
              <View style={styles.field}>
                <Text style={styles.label}>{m.comum_conta_claude()}</Text>
                <MenuSelect
                  value={selectedConfig ?? ''}
                  options={configs.map((c) => ({
                    value: c.path,
                    label: c.label,
                    hint: c.active ? (m.switcher_atual() as string) ?? undefined : undefined,
                  }))}
                  onChange={(v) => setSelectedConfig(v)}
                />
              </View>
            ) : null}

            {provider === 'claude' && Object.keys(motores).length ? (
              <View style={styles.field}>
                <Text style={styles.label}>{m.comum_motor()}</Text>
                <MenuSelect
                  value={engine}
                  options={[{ value: '', label: m.criar_claude_sua_conta() }, ...Object.entries(motores).map(([k, v]) => ({ value: k, label: (v as any).label ?? k, hint: (v as any).model }))]}
                  onChange={(v) => setEngine(v)}
                />
              </View>
            ) : null}

            {(provider === 'claude' || provider === 'pi' || provider === 'kimi') && (
              <View style={styles.field}>
                <Text style={styles.label}>{m.composer_modelo()}</Text>
                <MenuSelect
                  value={modelo}
                  options={[{ value: '', label: m.criar_padrao() }, ...modelos.map((md) => ({ value: valorModelo(md), label: md.name ?? md.id, hint: [md.provider, (md as any).context ?? ((md as any).context_length ? `${Math.round(((md as any).context_length) / 1000)}K` : null), ((md as any).vision ?? (md as any).images) ? '👁' : null].filter(Boolean).join(' · ') }))]}
                  onChange={(v) => setModelo(v)}
                />
                {listaReduzida ? <Text style={styles.hintSm}>{m.criar_lista_reduzida()}</Text> : null}
                {erroModelos ? <Text style={styles.hintSm}>{m.criar_abre_padrao({ erro: erroModelos } as any)}</Text> : null}
              </View>
            )}

            {(provider === 'claude' || provider === 'pi') && (
              <View style={styles.field}>
                <Text style={styles.label}>{provider === 'pi' ? (m.criar_raciocinio()) : (m.composer_esforco())}</Text>
                <MenuSelect
                  value={esforco}
                  options={[{ value: '', label: m.criar_padrao() }, ...NIVEIS[provider].map((n) => ({ value: n, label: n }))]}
                  onChange={(v) => setEsforco(v)}
                />
              </View>
            )}

            {provider === 'claude' && (
              <View style={styles.field}>
                <Text style={styles.label}>{m.criar_permissao()}</Text>
                <MenuSelect
                  value={permissao}
                  options={[{ value: '', label: m.criar_permissao_padrao() }, ...MODOS_PERMISSAO.map((n) => ({ value: n, label: n }))]}
                  onChange={(v) => setPermissao(v)}
                />
              </View>
            )}

            {error ? (
              <Text style={styles.error} accessibilityRole="alert">
                {error}
              </Text>
            ) : null}

            <Pressable onPress={handleCreate} disabled={!canCreate} style={[styles.primary, !canCreate && styles.primaryDis]}>
              <Text style={styles.primaryTxt}>{loading ? m.criar_criando() : m.sessao_nova()}</Text>
            </Pressable>

            <Pressable onPress={() => setPicked(null)} style={styles.ghost}>
              <Text style={styles.ghostTxt}>{m.criar_outra_pasta()}</Text>
            </Pressable>
          </>
        )}
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1, backgroundColor: theme.tokens.bg.base },
  scroll: { padding: theme.base.space[4], gap: theme.base.space[4], paddingBottom: 32 },
  title: { fontSize: 20, fontWeight: '600', color: theme.tokens.text.primary, marginBottom: 4 },
  pickerWrap: { minHeight: 380, flex: 1 },
  advanced: { borderTopWidth: 1, borderTopColor: theme.tokens.border.subtle, paddingTop: theme.base.space[3], gap: theme.base.space[2] },
  advToggle: { height: 44, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: 4 },
  advTxt: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  chev: { color: theme.tokens.text.muted, fontSize: 18 },
  chevOpen: { transform: [{ rotate: '90deg' }] } as any,
  manualForm: { flexDirection: 'row', gap: theme.base.space[2] },
  input: {
    flex: 1,
    height: 44,
    backgroundColor: theme.tokens.bg.surface,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    color: theme.tokens.text.primary,
    fontSize: 16,
    paddingHorizontal: theme.base.space[3],
  },
  manualGo: {
    height: 44,
    paddingHorizontal: theme.base.space[4],
    borderRadius: theme.base.radius.md,
    backgroundColor: theme.tokens.accent.dim,
    justifyContent: 'center',
  },
  manualGoDis: { opacity: 0.5 },
  manualGoTxt: { color: theme.tokens.text.primary, fontWeight: '600', fontSize: theme.base.text.sm },
  picked: {
    padding: theme.base.space[3],
    backgroundColor: theme.tokens.bg.surface,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    borderRadius: theme.base.radius.md,
    gap: 2,
  },
  pickedName: { fontSize: theme.base.text.lg, fontWeight: '600', color: theme.tokens.text.primary },
  pickedPath: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  rowCenter: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  hint: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  hintSm: { fontSize: 12, color: theme.tokens.text.muted, marginTop: 4 },
  field: { gap: theme.base.space[2] },
  label: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: '500' },
  selectBtn: {
    height: 44,
    backgroundColor: theme.tokens.bg.surface,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: theme.base.space[3],
  },
  selectTxt: { color: theme.tokens.text.primary, fontSize: 16, flex: 1 },
  selectChevron: { color: theme.tokens.text.muted, fontSize: 18, marginLeft: 8 },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
  primary: {
    height: 50,
    backgroundColor: theme.tokens.accent.base,
    borderRadius: theme.base.radius.md,
    justifyContent: 'center',
    alignItems: 'center',
  },
  primaryDis: { opacity: 0.5 },
  primaryTxt: { color: '#fff', fontWeight: '600', fontSize: theme.base.text.base },
  ghost: { height: 44, justifyContent: 'center', alignItems: 'center' },
  ghostTxt: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm },
}));
