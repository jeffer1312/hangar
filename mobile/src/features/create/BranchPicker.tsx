import { useEffect, useState } from 'react';
import { Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  basename, defaultBase, getFolderBranchesForServer, getWorktreesForServer, pickFolderRoot, sanitizeSessionName, worktreeAgeDays,
  worktreeIsAgent, worktreeTitle, WORKTREE_STALE_DAYS, type FolderBranches, type Server, type WorktreeChoice, type WorktreeRepo, type WorktreeStatus,
} from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

type Props = { server: Server; cwd: string; sessionName: string; value: WorktreeChoice | null;
               onChange: (v: WorktreeChoice | null) => void;
               /** Abrir a sessão noutra pasta (uma worktree que já existe, ou de volta à principal); `keepOpen` mantém o painel para o aviso. */
               onPickFolder: (path: string, keepOpen: boolean) => void };
type Worktrees = { repo: string; list: WorktreeStatus[] } | null;

// Pasta fora de repositório não é falha: só não há o que escolher (mesma regra do web, `isNotRepo`).
const isNotRepo = (e: unknown) => {
  const status = (e as { status?: number } | null)?.status;
  return status === 404 || (status === 409 && e instanceof Error && e.message.includes('not a git repository'));
};
const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
const inside = (root: string, path: string) => pickFolderRoot([{ path: root }], path) !== null;
// Mesma pasta com outra grafia (barra final, `\`, letra da unidade): uma contém a outra.
export const samePath = (a: string, b: string) => inside(a, b) && inside(b, a);

function worktreeHint(w: WorktreeStatus, now: number): string {
  if (w.sessions[0]) return m.worktree_menu_em_uso({ nome: w.sessions[0] });
  const days = worktreeAgeDays(w, now);
  if (days != null && days > WORKTREE_STALE_DAYS) return m.worktree_parada_dias({ n: days });
  return days ? m.worktree_dias_atras({ n: days }) : '';
}

export function BranchPicker({ server, cwd, sessionName, value, onChange, onPickFolder }: Props) {
  const { theme } = useUnistyles();
  const [info, setInfo] = useState<FolderBranches | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState('');
  // O painel desmonta ao fechar: a escolha volta de `value` para não zerar a cada abertura.
  const [fresh, setFresh] = useState(!!value);
  const [name, setName] = useState(value?.branch ?? '');
  const [base, setBase] = useState(value?.base ?? '');
  const [baseOpen, setBaseOpen] = useState(false);
  const [picking, setPicking] = useState(false);
  const [agentsOpen, setAgentsOpen] = useState(false);
  const [wts, setWts] = useState<Worktrees>(null);
  const [wtsLoading, setWtsLoading] = useState(true);
  const [wtsFailed, setWtsFailed] = useState('');
  const [wtsTry, setWtsTry] = useState(0);

  useEffect(() => {
    let vivo = true;
    // A leitura anterior fica na tela enquanto relê: escolher uma worktree ocupada troca a pasta e o aviso não some.
    setFailed('');
    setLoading(true);
    getFolderBranchesForServer(server, cwd)
      .then((r) => { if (vivo) { setInfo(r); setBase((b) => b || defaultBase(r)); } })
      .catch((e: unknown) => {
        if (!vivo) return;
        setInfo(null);
        if (!isNotRepo(e)) setFailed(m.native_create_checkout_failed({ reason: reason(e) }));
      })
      .finally(() => { if (vivo) setLoading(false); });
    return () => { vivo = false; };
  }, [server, cwd]);

  // Só o repositório da pasta e sem medir o disco: o menu não mostra espaço. A lista anterior fica
  // na tela enquanto relê, para o aviso de ocupada não piscar ao escolher uma worktree.
  useEffect(() => {
    const controller = new AbortController();
    setWts((cur) => (cur && (inside(cur.repo, cwd) || cur.list.some((w) => inside(w.path, cwd))) ? cur : null));
    setWtsFailed('');
    setWtsLoading(true);
    getWorktreesForServer(server, controller.signal, { repo: cwd, sizes: false })
      .then((repos: WorktreeRepo[]) => {
        if (controller.signal.aborted) return;
        const r = repos.find((x) => inside(x.repo, cwd) || x.worktrees.some((w) => inside(w.path, cwd)));
        setWts(r ? { repo: r.repo, list: r.worktrees.filter((w) => w.exists) } : null);
      })
      .catch((e: unknown) => { if (!controller.signal.aborted) setWtsFailed(m.native_create_checkout_failed({ reason: reason(e) })); })
      .finally(() => { if (!controller.signal.aborted) setWtsLoading(false); });
    return () => controller.abort();
  }, [server, cwd, wtsTry]);

  const typed = name.trim();
  // Nome de uma branch que já existe abre a worktree nela, sem criar outra; a base não vale para ela.
  // Branch aberta em outra worktree (ou na pasta principal) não abre outra: o git recusa.
  const openElsewhere = !!wts?.list.some((w) => w.branch === typed || w.main_branch === typed);
  const known = !!info && !!typed && typed !== info.current && !openElsewhere && [...info.branches, ...info.remotes].includes(typed);

  useEffect(() => {
    if (!info) return;   // sem a leitura não dá para saber se o nome já existe
    // Nome em branco: o store preenche com o nome final da sessão ao criar.
    if (!fresh) onChange(null);
    else if (known) onChange({ branch: typed });
    else onChange({ branch: typed, new_branch: true, base: base || null });
  }, [info, fresh, known, typed, base, onChange]);

  if (loading && !info) return <Text style={styles.help} accessibilityLiveRegion="polite">{m.native_create_checkout_loading()}</Text>;
  if (failed) return <Text style={styles.error} accessibilityRole="alert">{failed}</Text>;
  if (!info) return null;   // pasta sem git: nada a escolher

  // A pasta escolhida já é uma worktree: é ela a "que já existe".
  const chosen = wts?.list.find((w) => samePath(w.path, cwd)) ?? null;
  const busy = !fresh && chosen?.sessions[0];
  const notice = busy ? <Text style={styles.warn} accessibilityRole="alert">{m.worktree_menu_ocupada({ nome: busy })}</Text> : null;

  const option = (key: string, title: string, hint: string, on: boolean, onPress: () => void, trailing?: 'chevron') => (
    <Pressable key={key} onPress={onPress} accessibilityRole="button" accessibilityLabel={title} accessibilityHint={hint || undefined}
      accessibilityState={{ selected: on }} style={[styles.option, on && styles.on]}>
      <View style={styles.optionText}>
        <Text style={styles.optionTitle} numberOfLines={1}>{title}</Text>
        {hint ? <Text style={styles.help} numberOfLines={2}>{hint}</Text> : null}
      </View>
      {on ? <Icon name="Check" size={16} color={theme.tokens.accent.base} /> : null}
      {trailing ? <Icon name="ChevronRight" size={14} color={theme.tokens.text.muted} /> : null}
    </Pressable>
  );

  if (picking) {
    const now = Date.now() / 1000;
    const row = (w: WorktreeStatus) => {
      const hint = worktreeHint(w, now);
      return option(`wt:${w.path}`, worktreeTitle(w), hint, !fresh && !!chosen && samePath(chosen.path, w.path), () => {
        const occupied = !!w.sessions.length;
        setFresh(false);
        setName('');
        // Ocupada, o painel fica aberto para o aviso de que outra sessão já trabalha nela.
        if (!occupied) setPicking(false);
        onPickFolder(w.path, occupied);
      });
    };
    const list = wts?.list ?? [];
    const agents = list.filter(worktreeIsAgent);
    const mine = list.filter((w) => !worktreeIsAgent(w)).sort((a, b) => Number(!!b.sessions.length) - Number(!!a.sessions.length));
    // A falha da releitura aparece mesmo com a lista anterior na tela: ela pode estar velha.
    const failure = wtsFailed ? (
      <View style={styles.box}>
        <Text style={styles.error} accessibilityRole="alert">{wtsFailed}</Text>
        <Pressable onPress={() => setWtsTry((n) => n + 1)} accessibilityRole="button" accessibilityLabel={m.native_retry()} style={styles.option}>
          <Text style={styles.link}>{m.native_retry()}</Text>
        </Pressable>
      </View>
    ) : null;
    const body = wtsFailed && !wts ? null : wtsLoading && !wts ? <Text style={styles.help} accessibilityLiveRegion="polite">{m.native_create_checkout_loading()}</Text>
      : !list.length ? <Text style={styles.help}>{m.worktree_menu_nenhuma()}</Text>
      : (
        <>
          {mine.map(row)}
          {agents.length ? (
            <Pressable onPress={() => setAgentsOpen((o) => !o)} accessibilityRole="button" accessibilityLabel={m.worktree_subagentes()}
              accessibilityState={{ expanded: agentsOpen }} style={styles.option}>
              <Text style={[styles.help, styles.grow]} numberOfLines={1}>{m.worktree_subagentes()}</Text>
              <Text style={styles.help}>{agents.length}</Text>
              <Icon name={agentsOpen ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
            </Pressable>
          ) : null}
          {agentsOpen ? agents.map(row) : null}
        </>
      );
    return (
      <ScrollView contentContainerStyle={styles.box} keyboardShouldPersistTaps="handled">
        <Pressable onPress={() => setPicking(false)} accessibilityRole="button" accessibilityLabel={m.comum_voltar()} style={styles.option}>
          <Icon name="ChevronLeft" size={14} color={theme.tokens.text.muted} />
          <Text style={[styles.optionTitle, styles.grow]} numberOfLines={1}>{m.worktree_menu_existentes()}</Text>
        </Pressable>
        {failure}
        {body}
        {notice}
      </ScrollView>
    );
  }

  const list = wts?.list ?? [];
  const inUse = list.filter((w) => w.sessions.length).length;
  const summary = wtsLoading && !wts ? m.native_create_checkout_loading()
    : wtsFailed ? wtsFailed
    : chosen && !fresh ? worktreeTitle(chosen)
    : !list.length ? m.worktree_menu_nenhuma()
    : (list.length === 1 ? m.worktree_menu_uma() : m.worktree_menu_n({ n: list.length })) + (inUse ? ` · ${m.worktree_menu_n_em_uso({ n: inUse })}` : '');
  // Dentro de uma worktree, "sua pasta" é a principal do repositório.
  const home = chosen && wts ? wts.repo : cwd;
  const homeBranch = (chosen ? chosen.main_branch : info.current) ?? m.native_create_checkout_current();
  const bases = [info.current, ...info.branches.filter((b) => b !== info.current)].filter((b): b is string => !!b);
  const remoteOnly = known && info.remotes.includes(typed);
  const parent = info.worktree_parent?.replace(/[\\/]+$/, '');
  const sep = parent && parent.includes('\\') && !parent.includes('/') ? '\\' : '/';
  // O nome da pasta é o da branch pela mesma regra do servidor.
  const stem = typed ? sanitizeSessionName(typed) || 'worktree' : m.worktree_menu_nome_sessao();

  return (
    <ScrollView contentContainerStyle={styles.box} keyboardShouldPersistTaps="handled">
      <Text style={styles.title} accessibilityRole="header">{m.worktree_menu_titulo()}</Text>
      {option('here', m.worktree_menu_pasta(), m.worktree_menu_pasta_dica({ pasta: basename(home), branch: homeBranch }), !fresh && !chosen, () => {
        setFresh(false);
        if (chosen && wts) onPickFolder(wts.repo, false);
      })}
      {option('new', m.worktree_menu_nova(), m.worktree_menu_nova_dica(), fresh, () => setFresh(true))}
      {fresh ? (
        <View style={styles.fields}>
          <View style={styles.line}>
            <Text style={styles.fieldLabel}>{m.worktree_menu_parte_de()}</Text>
            <Pressable onPress={() => setBaseOpen((o) => !o)} disabled={known} accessibilityRole="button"
              accessibilityLabel={`${m.worktree_menu_parte_de()} ${base}`} accessibilityState={{ disabled: known, expanded: baseOpen }}
              style={[styles.baseBtn, known && styles.dim]}>
              <Text style={[styles.optionTitle, styles.grow]} numberOfLines={1}>{base}</Text>
              <Icon name={baseOpen && !known ? 'ChevronUp' : 'ChevronDown'} size={14} color={theme.tokens.text.muted} />
            </Pressable>
          </View>
          {baseOpen && !known ? bases.map((b) => option(`base:${b}`, b, '', base === b, () => { setBase(b); setBaseOpen(false); })) : null}
          <View style={styles.line}>
            <Text style={styles.fieldLabel}>{m.worktree_menu_branch()}</Text>
            <TextInput accessibilityLabel={m.worktree_menu_branch()} placeholder={sessionName} value={name}
              onChangeText={setName} autoCapitalize="none" autoCorrect={false} style={[styles.input, styles.grow]}
              placeholderTextColor={theme.tokens.text.muted} />
          </View>
          {known ? (
            <Text style={styles.warn}>
              {m.worktree_menu_ja_existe({ branch: typed })}{remoteOnly ? ` (${m.worktree_nova_so_remoto()})` : ''}
            </Text>
          ) : typed && typed === info.current ? (
            <Text style={styles.warn}>{m.worktree_menu_ja_aberta({ branch: typed, pasta: basename(cwd) })}</Text>
          ) : null}
          {parent ? (
            <View style={styles.line}>
              <Text style={styles.fieldLabel}>{m.worktree_menu_pasta_destino()}</Text>
              <Text style={[styles.path, styles.grow]}>{`${parent}${sep}${stem}`}</Text>
            </View>
          ) : null}
        </View>
      ) : null}
      {option('existing', m.worktree_menu_existente(), summary, !fresh && !!chosen, () => setPicking(true), 'chevron')}
      {notice}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  box: { gap: theme.base.space[2] },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary, paddingHorizontal: theme.base.space[3] },
  option: {
    minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[1.5], borderRadius: theme.base.radius.md,
  },
  on: { backgroundColor: theme.tokens.accent.dim },
  optionText: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  optionTitle: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  grow: { flex: 1, minWidth: 0 },
  fields: {
    gap: theme.base.space[2], padding: theme.base.space[3], marginHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  line: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  fieldLabel: { minWidth: 64, color: theme.tokens.text.secondary, fontSize: theme.base.text.xs },
  baseBtn: {
    flex: 1, minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  dim: { opacity: 0.5 },
  input: {
    minHeight: 44, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, fontSize: theme.base.text.sm,
    color: theme.tokens.text.primary, backgroundColor: superficie(theme), borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  path: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  help: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm },
  link: { color: theme.tokens.accent.base, fontSize: theme.base.text.sm },
  warn: { color: theme.tokens.status.warning, fontSize: theme.base.text.xs },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
}));
