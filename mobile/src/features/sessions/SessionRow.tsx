import { memo, useMemo, type ReactNode } from 'react';
import { ActionSheetIOS, Platform, Pressable, Text, View } from 'react-native';
import { MenuView, type NativeActionEvent } from '@react-native-menu/menu';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Haptics from 'expo-haptics';
import { cwdParts, isOrq, loopBadge, pairExternals, providerName, relativeTime, rotuloEstado, sessionContextPct, tagHex, untrackedReason, worktreeLabel, type AggSession, type PairExternal } from '@hangar/core';
import { Chip, type Tone } from '../../ui/Chip';
import { SessionAvatar } from './SessionAvatar';
import { HeadlessSignal } from './HeadlessSignal';
import { Icon } from '../../ui/Icon';
import { MACHINE_ICON_NAME } from '../tags/tagVisuals';
import { SessionPlate } from './SessionPlate';
import { superficie } from '../../theme/superficie';
import { SPACE } from '../../theme/scale';
import { useWorktreeStatus } from '../worktrees/worktreeStatus';
import { groupBadge } from '../pair/groupPairs';
import { GroupGlyph } from '../pair/GroupGlyph';
import * as m from '../../paraglide/messages';

// LOOP_TONE_COLOR do core é CSS var (`var(--accent)`) — não serve em RN; o tom vira `Chip tone`.
const TOM_DO_LOOP: Record<'ok' | 'warn' | 'attention' | 'muted', Tone> = {
  ok: 'success',
  warn: 'warning',
  attention: 'error',
  muted: 'neutral',
};

const ACCOUNT_COLORS = ['#80bbc3', '#d3a781', '#97c69d', '#cd99b9', '#9ba9d9', '#ccbf87'];

// Servidor sem o evento `tags` (versão antiga): o nome sai da `conta`, e a cor por hash vale também
// para a etiqueta sem cor. O cabeçalho inclui motor e reservas; aqui só a conta de assinatura.
function accountChip(account: string | null | undefined) {
  const harness = ['claude', 'codex'].find(h => account?.startsWith(`${h}:`));
  if (!account || !harness) return null;
  const base = account.slice(harness.length + 1).replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? '';
  const name = base === `.${harness}` ? '' : base.replace(new RegExp(`^\\.${harness}-`), '');
  const label = name.replace(new RegExp(`^${harness}-(?=.)`), '') || m.conta_padrao();
  let hash = 0;
  for (const char of base) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return { label, name: name || m.conta_padrao(), color: ACCOUNT_COLORS[hash % ACCOUNT_COLORS.length], harness };
}

// Ações de par externo da linha: a lista decide o que cada uma faz (confirmação, servidor, folha).
export type PairAction =
  | { kind: 'invite' }
  | { kind: 'accept' }
  | { kind: 'leave' }
  | { kind: 'undo'; pair: PairExternal }
  | { kind: 'release'; pair: PairExternal }
  | { kind: 'group' };

interface Props {
  session: AggSession;
  mostrarServidor: boolean; // false quando a lista já está agrupada por servidor
  compact?: boolean;
  selected?: boolean;
  // recebem a sessão: a lista passa a mesma função a todas as linhas, e o memo segura o render
  onPress: (s: AggSession) => void;
  onGit?: (s: AggSession) => void;
  onExcluir?: (s: AggSession) => void;
  onRenomear?: (s: AggSession) => void;
  onResume?: (s: AggSession) => void;
  // Ausente em servidor de convite: o backend recusa worktrees ao convidado, e o chip é só texto.
  onWorktree?: (s: AggSession, path: string) => void;
  // Ausente em servidor de convite: lá a pessoa é convidada, não dona do par.
  onPair?: (s: AggSession, action: PairAction) => void;
  // Ocultar (true) ou mostrar (false) na lista; ausente onde a pessoa não é dona da sessão.
  onHidden?: (s: AggSession, hide: boolean) => void;
  // Marca "Oculta" na linha, para quem mostra as ocultas no meio das outras (seletor).
  hiddenBadge?: boolean;
  // Selo do grupo entre máquinas pelo modelo global (`groupBadges` da lista), em valores soltos para o memo; ausente =
  // grupo de uma máquina só.
  crossBadgeText?: string | null;
  crossBadgeMachines?: number;
}

// `id` obrigatório: só com campos opcionais o TS recusa os itens sem `attributes` (tipo fraco).
const isDisabled = (a: { id: string; attributes?: { disabled?: boolean; destructive?: boolean } }) => !!a.attributes?.disabled;

// O iOS já vibra ao abrir o menu de contexto; o PopupMenu do Android não.
const vibrarAoAbrir =
  Platform.OS === 'android' ? () => void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {}) : undefined;

export const SessionRow = memo(function SessionRow({ session: s, mostrarServidor, compact = false, selected, onPress, onGit, onExcluir, onRenomear, onResume, onWorktree, onPair, onHidden, hiddenBadge, crossBadgeText, crossBadgeMachines }: Props) {
  const { theme } = useUnistyles();
  // O orquestrador não se renomeia nem se fecha: o backend recusa, então a linha nem oferece.
  const orq = isOrq(s);
  const fora = s.outside ?? null;
  const pares = useMemo(() => (fora ? [] : pairExternals(s)), [s, fora]);
  const parAtivo = !!onPair && !orq && !fora;
  const wtPath = s.worktree_path ?? (s.worktree ? s.cwd ?? null : null);
  const wtNome = worktreeLabel(s);
  // Abrir a worktree mora no toque longo; convidado não tem (o backend recusa).
  const wtAcao = onWorktree && !fora && !s.worktree_gone && wtNome && wtPath ? wtPath : null;
  // Vários pares: o dono sozinho não diz qual desfazer.
  const donoDo = (p: PairExternal) => (pares.length > 1 ? `${p.owner} · ${p.session}` : p.owner);
  // Esperando você, a oculta voltaria na rodada seguinte da lista: ocultar fica desabilitado, com o motivo no título.
  const waiting = s.state === 'awaiting_input' || (s.pending_questions ?? 0) > 0;
  // Sem o arrasto, o menu é o único caminho visível pro Git: o orquestrador fica só com ele.
  // A sessão de fora é de outra pessoa: nem renomear, nem fechar, só sair do par.
  const acoesMenu = useMemo(
    () => fora ? (onPair ? [{ id: 'leave', title: m.pair_leave_outside({ dono: fora.owner }), attributes: { destructive: true } }] : []) : [
      ...(orq || !onRenomear ? [] : [{ id: 'rename', title: m.sessao_renomear(), image: Platform.select({ ios: 'pencil', android: 'ic_menu_edit' }) }]),
      ...(onGit && s.cwd && !s.serverInvite ? [{ id: 'git', title: m.sessao_git(), image: Platform.select({ ios: 'arrow.triangle.branch' }) }] : []),
      ...(wtAcao ? [{ id: 'worktree', title: m.sessao_worktree_abrir(), image: Platform.select({ ios: 'folder' }) }] : []),
      ...(parAtivo ? [
        { id: 'pair-invite', title: m.native_par_convidar(), image: Platform.select({ ios: 'link' }) },
        { id: 'pair-accept', title: m.native_par_menu(), image: Platform.select({ ios: 'link.badge.plus' }) },
        ...pares.map((p, i) => ({ id: `undo:${i}`, title: m.pair_undo_own({ dono: donoDo(p) }) })),
      ] : []),
      ...(!onHidden ? [] : s.hidden_from_list
        ? [{ id: 'show', title: m.session_show_in_list(), image: Platform.select({ ios: 'eye' }) }]
        : [{
          id: 'hide',
          title: waiting ? `${m.session_hide_from_list()} · ${m.session_hide_waiting()}` : m.session_hide_from_list(),
          image: Platform.select({ ios: 'eye.slash' }),
          ...(waiting ? { attributes: { disabled: true } } : {}),
        }]),
      ...(orq || !onExcluir ? [] : [{
        id: s.serverInvite ? 'stop-following' : 'delete',
        title: s.serverInvite ? m.convite_parar() : m.sessao_excluir_curto(),
        image: Platform.select({ ios: 'trash', android: 'ic_menu_delete' }),
        attributes: { destructive: true },
      }]),
    ],
    [s.cwd, s.serverInvite, s.hidden_from_list, waiting, orq, fora, parAtivo, pares, wtAcao, onPair, onRenomear, onGit, onExcluir, onHidden],
  );
  const acao = (nome: string) => {
    if (nome === 'rename') onRenomear?.(s);
    else if (nome === 'hide' && !waiting) onHidden?.(s, true);
    else if (nome === 'show') onHidden?.(s, false);
    else if (nome === 'git') onGit?.(s);
    else if (nome === 'worktree' && wtAcao) onWorktree?.(s, wtAcao);
    else if (nome === 'delete' || nome === 'stop-following') onExcluir?.(s);
    else if (nome === 'leave') onPair?.(s, { kind: 'leave' });
    else if (nome === 'pair-invite') onPair?.(s, { kind: 'invite' });
    else if (nome === 'pair-accept') onPair?.(s, { kind: 'accept' });
    else if (nome === 'group') onPair?.(s, { kind: 'group' });
    else if (nome.startsWith('undo:')) {
      const pair = pares[Number(nome.slice(5))];
      if (pair) onPair?.(s, { kind: 'undo', pair });
    }
  };
  // Renomear/Git/Excluir moram no toque longo, gesto que o leitor de tela não alcança. Aqui elas viram ações do rotor/menu de acessibilidade da própria linha.
  const badge = groupBadge(s, crossBadgeMachines ? { text: crossBadgeText ?? null, machines: crossBadgeMachines } : undefined);
  const badgeText = badge ? (badge.text ?? m.grupo_selo_maquinas({ n: badge.machines })) : null;
  // O selo abre o painel do grupo; no leitor de tela vira ação da linha.
  const groupTitle = badge && onPair && !fora ? m.par_grupo_titulo({ n: (s.pair_peers?.length ?? 0) + 1 }) : null;
  // O desabilitado não vira ação do leitor de tela: o rótulo da linha já diz que ela espera você.
  const acoesA11y = useMemo(
    () => [...acoesMenu.filter((a) => !isDisabled(a)).map((a) => ({ name: a.id, label: a.title })), ...(groupTitle ? [{ name: 'group', label: groupTitle }] : [])],
    [acoesMenu, groupTitle],
  );
  const pendentes = pares.filter((p) => p.pending);
  const untracked = s.tracked === false;
  const cwd = cwdParts(s.cwd);
  const loop = loopBadge(s.loop_status, s.loop_iter, s.loop_max);
  const pendingQuestions = s.pending_questions ?? 0;
  const reply = s.state === 'idle' ? s.last_reply : null;
  const sub = s.question ?? (s.state === 'working' ? s.label : reply) ?? null;
  const engineLabel = !orq && s.provider === 'claude' ? s.engine : null;
  const providerLabel = orq ? null : [providerName(s.provider), engineLabel].filter(Boolean).join(', ');
  const legacy = accountChip(s.conta);
  const tag = s.accountTag;
  const account = tag
    ? { label: tag.name, name: tag.name, color: tagHex(tag.color) ?? legacy?.color ?? theme.tokens.text.secondary, icon: tag.icon }
    : legacy && { ...legacy, icon: null };
  const accountProvider = legacy?.harness ?? s.provider ?? '';
  const ctxPct = useMemo(() => sessionContextPct(s), [s]);
  const plate = mostrarServidor || !!account;
  const juntada = useWorktreeStatus(s.serverId, wtPath)?.merged === true;
  // Na worktree a branch sai do texto e ganha o ícone de ramo; a pasta some quando repete o nome
  // da sessão ou da worktree.
  // Sem branch (HEAD destacado), o nome da worktree ocupa o lugar dela.
  const branchWt = wtNome && !s.worktree_gone ? `${s.branch ?? wtNome}${juntada ? ' ✓' : ''}` : null;
  const meta = [
    s.cwd && cwd.base !== s.name && cwd.base !== wtNome ? cwd.base : null,
    branchWt ? null : s.branch,
  ].filter((p): p is string => !!p);
  // Zero não desenha: a ausência de seta é "está em dia", como no PWA.
  const gitAhead = s.git_ahead || 0;
  const gitBehind = s.git_behind || 0;
  const gitAdded = s.git_added || 0;
  const gitRemoved = s.git_removed || 0;
  const temGit = gitAhead > 0 || gitBehind > 0 || gitAdded > 0 || gitRemoved > 0;
  const muted = theme.tokens.text.muted;

  // iOS: o menu de contexto (MenuView) engolia o toque simples da linha dentro da gaveta; a folha de
  // ações nativa abre pelo próprio toque longo e não disputa o toque. Android segue com o PopupMenu.
  const abrirAcoesIOS = () => {
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
    const destructive = acoesMenu.findIndex((a) => a.id === 'delete' || a.id === 'stop-following');
    ActionSheetIOS.showActionSheetWithOptions(
      {
        title: s.name,
        options: [...acoesMenu.map((a) => a.title), m.comum_cancelar()],
        cancelButtonIndex: acoesMenu.length,
        destructiveButtonIndex: destructive >= 0 ? destructive : undefined,
        disabledButtonIndices: acoesMenu.flatMap((a, i) => (isDisabled(a) ? [i] : [])),
      },
      (i) => { if (i < acoesMenu.length) acao(acoesMenu[i].id); },
    );
  };
  const comMenu = (linha: ReactNode) =>
    acoesMenu.length === 0 || Platform.OS === 'ios' ? linha : (
      <MenuView
        shouldOpenOnLongPress
        actions={acoesMenu}
        onPressAction={({ nativeEvent }: NativeActionEvent) => acao(nativeEvent.event)}
        onOpenMenu={vibrarAoAbrir}
      >
        {linha}
      </MenuView>
    );

  return comMenu(
    <Pressable
      onPress={() => onPress(s)}
      // no-op de propósito: soltar o dedo depois do toque longo que abriu o menu não abre a conversa
      onLongPress={acoesMenu.length === 0 ? undefined : Platform.OS === 'ios' ? abrirAcoesIOS : () => {}}
      // kimi sem id é estado NORMAL pré-1º prompt, e codex sem thread ainda está no startup:
      // a tela da conversa espera o vínculo sem herdar transcript de outra sessão
      disabled={selected || (!!onResume && untracked && s.provider !== 'kimi' && s.provider !== 'codex')}
      style={({ pressed }) => [styles.row, compact && styles.compact, fora && styles.fora,
        (selected || pressed) && { backgroundColor: superficie(theme, selected ? 0.8 : 0.6) }]}
      accessibilityState={selected === undefined ? undefined : { selected, disabled: selected }}
      accessibilityRole="button"
      // rótulo composto: um label explícito no pai faz o RN descartar o texto dos filhos, e o
      // estado e a pergunta sumiriam do leitor de tela.
      // Máquina e projeto entram mesmo quando a tela os esconde (lista agrupada, pasta = nome):
      // é o que distingue duas conversas de mesmo nome no leitor de tela.
      accessibilityLabel={`${s.name}${hiddenBadge && s.hidden_from_list ? `, ${m.session_hidden_badge()}` : ''}${s.headless === true ? `, ${m.criar_modo_exec_headless()}` : ''}${fora ? `, ${m.pair_from_owner({ dono: fora.owner })}${s.guest_kind === 'share' ? '' : `, ${m.pair_level_view()}`}` : ''}, ${rotuloEstado(s.state)}${s.limited ? `, ${s.limit_reset ? m.sessao_limite_volta({ n: s.limit_reset }) : m.sessao_limite()}` : ''}${pendingQuestions > 0 ? `, ${m.ask_perguntas()}: ${pendingQuestions}` : ''}${sub ? `, ${sub}` : ''}${badgeText ? `, ${badgeText}` : ''}${providerLabel ? `, ${providerLabel}` : ''}${account ? `, ${m.sessao_conta({ n: account.name })}` : ''}${ctxPct !== null ? `, ${m.tags_context_ring({ pct: Math.round(ctxPct) })}` : ''}, ${s.serverLabel}${s.cwd ? `, ${cwd.base}` : ''}${s.branch ? `, ${s.branch}` : ''}${gitAhead || gitBehind ? `, ${m.git_sync_titulo({ ahead: gitAhead, behind: gitBehind })}` : ''}${selected ? `, ${m.switcher_atual()}` : ''}`}
      accessibilityActions={acoesA11y}
      onAccessibilityAction={({ nativeEvent }) => {
        acao(nativeEvent.actionName);
      }}
    >
      {fora ? <View style={[styles.faixa, { backgroundColor: theme.tokens.accent.base }]} /> : null}
      <View style={styles.lead}>
        <SessionAvatar session={s} ctxPct={ctxPct} />
      </View>
      <View style={styles.col}>
        <View style={styles.linha}>
          <Text style={[styles.nome, { color: theme.tokens.text.primary }]} numberOfLines={1}>{s.name}</Text>
          {s.headless === true ? <HeadlessSignal /> : null}
          {pendingQuestions > 0 ? <Chip tone="warning">{`? ${pendingQuestions}`}</Chip> : null}
          {orq ? <Chip>{m.orq_row_badge()}</Chip> : null}
          {hiddenBadge && s.hidden_from_list ? <Chip icon="EyeOff">{m.session_hidden_badge()}</Chip> : null}
          {untracked ? <Chip tone="warning">{m.sessao_sem_id()}</Chip> : null}
          {fora ? <Chip tone="accent" icon={s.guest_kind === 'share' ? 'Link' : 'Eye'}>{m.pair_from_owner({ dono: fora.owner })}</Chip> : null}
          <Text style={[styles.tempo, { color: muted }]} numberOfLines={1}>{relativeTime(s.last_activity)}</Text>
        </View>
        {sub ? (
          <Text
            style={[styles.sub, s.question ? { color: theme.tokens.pill.input.fg }
              : s.state === 'working' ? { color: theme.tokens.text.secondary, fontStyle: 'italic' } : { color: muted }]}
            numberOfLines={1}
          >
            {reply && !s.question ? '◆ ' : ''}
            {sub}
          </Text>
        ) : null}
        {meta.length || engineLabel || plate || branchWt || temGit || badgeText ? (
          <View style={styles.metaRow}>
            {engineLabel ? (
              <View style={styles.engine}>
                <Icon name="Settings" size={12} color={theme.tokens.accent.base} />
                <Text style={[styles.meta, styles.engineText, { color: theme.tokens.accent.base }]} numberOfLines={1}>{engineLabel}</Text>
              </View>
            ) : null}
            {meta.length ? <Text style={[styles.meta, styles.metaRest, { color: muted }]} numberOfLines={1}>{engineLabel ? ' · ' : ''}{meta.join(' · ')}</Text> : null}
            {badgeText ? (
              <Pressable
                onPress={groupTitle ? () => onPair?.(s, { kind: 'group' }) : undefined}
                disabled={!groupTitle}
                hitSlop={{ top: 8, bottom: 13, left: 6, right: 6 }}
                style={[styles.badge, (meta.length > 0 || !!engineLabel) && styles.badgeAfter, { borderColor: theme.tokens.border.subtle }]}
                accessibilityElementsHidden
                importantForAccessibility="no-hide-descendants"
              >
                <GroupGlyph size={12} color={muted} />
                <Text style={[styles.meta, styles.metaRest, { color: muted }]} numberOfLines={1}>{badgeText}</Text>
              </Pressable>
            ) : null}
            {branchWt ? <>
              {meta.length || engineLabel || badgeText ? <Text style={[styles.meta, styles.metaSeparator, { color: muted }]}>{' · '}</Text> : null}
              <Icon name="GitBranch" size={SPACE[3]} color={muted} />
              <Text style={[styles.meta, styles.metaRest, styles.branchWt, { color: muted }]} numberOfLines={1}>{branchWt}</Text>
            </> : null}
            {temGit ? (
              <View style={styles.git}>
                {gitAhead > 0 ? <View style={styles.gitPar}>
                  <Icon name="ArrowUp" size={11} color={theme.tokens.accent.base} />
                  <Text style={[styles.meta, styles.gitSync, { color: theme.tokens.accent.base }]}>{gitAhead}</Text>
                </View> : null}
                {gitBehind > 0 ? <View style={styles.gitPar}>
                  <Icon name="ArrowDown" size={11} color={theme.tokens.status.warning} />
                  <Text style={[styles.meta, styles.gitSync, { color: theme.tokens.status.warning }]}>{gitBehind}</Text>
                </View> : null}
                {gitAdded > 0 ? <Text style={[styles.meta, { color: theme.tokens.status.success }]}>{`+${gitAdded}`}</Text> : null}
                {gitRemoved > 0 ? <Text style={[styles.meta, { color: theme.tokens.status.error }]}>{`−${gitRemoved}`}</Text> : null}
              </View>
            ) : null}
            {/* Placa: [máquina, só fora da lista agrupada] [conta]; fecha a linha, e os metadados cedem antes dela. */}
            {plate ? (
              <SessionPlate
                machine={mostrarServidor ? { label: s.serverLabel, color: s.serverColor, icon: s.machineTag ? MACHINE_ICON_NAME[s.machineTag.icon] : 'Monitor' } : null}
                account={account ?? null}
                provider={accountProvider}
              />
            ) : null}
          </View>
        ) : null}
        {s.worktree_gone ? (
          <Text style={[styles.meta, { color: muted }]} numberOfLines={1}>{m.worktree_apagada()}</Text>
        ) : null}
        {pendentes.length ? (
          <View style={styles.chips}>
            <Chip tone="warning">{m.pair_full_pending()}</Chip>
            {onPair ? pendentes.map((p) => (
              <Pressable key={`${p.alias}::${p.session}`} onPress={() => onPair(s, { kind: 'release', pair: p })} hitSlop={8}
                accessibilityRole="button" accessibilityLabel={m.pair_release({ dono: p.owner })} style={styles.release}>
                <Text style={[styles.metaTxt, { color: theme.tokens.accent.base, fontWeight: theme.base.weight.semibold }]}>{m.pair_release({ dono: p.owner })}</Text>
              </Pressable>
            )) : null}
          </View>
        ) : null}
        {s.limited || loop ? (
          <View style={styles.chips}>
            {s.limited ? <Chip tone="warning" icon="Hourglass">{s.limit_reset ?? ''}</Chip> : null}
            {loop ? <Chip tone={TOM_DO_LOOP[loop.tone]}>{loop.label}</Chip> : null}
          </View>
        ) : null}
        {onResume && untracked && s.provider !== 'kimi' && s.provider !== 'pi' && s.provider !== 'omp' ? (
          <Pressable onPress={() => onResume(s)} style={styles.resume} accessibilityRole="button" accessibilityLabel={m.sessao_retomar()}>
            <Icon name="RotateCw" size={12} color={theme.tokens.text.secondary} />
            <Text style={[styles.metaTxt, { color: theme.tokens.text.primary }]}>{m.sessao_retomar()}</Text>
          </Pressable>
        ) : untracked ? (
          <Text style={[styles.metaTxt, { color: muted }]}>{untrackedReason(s.provider)}</Text>
        ) : null}
      </View>
      {selected ? <Icon name="Check" size={18} color={theme.tokens.accent.base} /> : null}
    </Pressable>,
  );
});

const styles = StyleSheet.create((theme) => ({
  row: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3], minHeight: 56, paddingVertical: theme.base.space[2], paddingHorizontal: theme.base.space[3] },
  compact: { paddingVertical: theme.base.space[1.5], minHeight: 44 },
  // O avatar acompanha o topo da linha para não saltar quando aparece uma pergunta.
  lead: { width: 36, alignItems: 'center', paddingTop: 1 },
  col: { flex: 1, gap: theme.base.space[0.5], minWidth: 0 },
  linha: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], minWidth: 0 },
  nome: { fontSize: theme.base.text.base, lineHeight: 22, fontWeight: theme.base.weight.semibold, flexShrink: 1 },
  tempo: { marginLeft: 'auto', paddingLeft: theme.base.space[1], flexShrink: 0, fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  sub: { fontSize: theme.base.text.sm, lineHeight: 20 },
  metaRow: { flexDirection: 'row', alignItems: 'center', minWidth: 0 },
  engine: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minWidth: 0, flexShrink: 0 },
  engineText: { fontWeight: theme.base.weight.semibold, flexShrink: 0 },
  metaRest: { flexShrink: 1 },
  metaSeparator: { flexShrink: 0 },
  badgeAfter: { marginLeft: theme.base.space[1] },
  badge: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5], minWidth: 0, flexShrink: 1, paddingHorizontal: theme.base.space[1], borderWidth: 1, borderRadius: theme.base.radius.full },
  meta: { fontSize: theme.base.text.xs, lineHeight: 18 },
  metaTxt: { fontSize: theme.base.text.xs },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[1], marginTop: theme.base.space[1] },
  branchWt: { paddingLeft: theme.base.space[0.5] },
  // O git pede ação (enviar, trazer): não encolhe, quem cede é o texto antes dele.
  git: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], flexShrink: 0, paddingLeft: theme.base.space[2] },
  gitPar: { flexDirection: 'row', alignItems: 'center', gap: 1 },
  gitSync: { fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] },
  // Recuada e com a faixa do grupo, como a linha de fora no app de PC.
  fora: { marginLeft: theme.base.space[2] },
  faixa: { position: 'absolute', left: 0, top: 6, bottom: 6, width: 2, borderRadius: 1 },
  release: { minHeight: 24, justifyContent: 'center' },
  resume: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], marginTop: theme.base.space[1], minHeight: 32, alignSelf: 'flex-start' },
}));
