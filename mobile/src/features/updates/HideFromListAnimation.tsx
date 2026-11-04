import { useMemo } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, FadeOut, LinearTransition, SlideOutLeft, ZoomIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AggSession } from '@hangar/core';
import { SessionRow } from '../sessions/SessionRow';
import { SessionSectionCard, SessionSectionHeader } from '../sessions/SessionSection';
import { buildSessionSections } from '../sessions/sessionSections';
import { Glass } from '../../ui/Glass';
import { SettingsList } from '../../ui/SettingsList';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `ocultar-da-lista`: o toque longo abre o menu da primeira sessão, Ocultar da lista é escolhido, a linha
// sai e a seção "Ocultas (1)" aparece no fim; depois ela abre com a sessão dentro.
const CYCLE = 6;
const MENU = 0.9;
const PRESS = 1.8;
const HIDE = 2.3;
const EXPAND = 3.9;
const STEPS = [MENU, PRESS, HIDE, EXPAND];
// Movimento reduzido: a sessão já dentro de "Ocultas (1)", aberta.
const STILL = 4.5;

const noop = () => {};
const NO_GROUPS = new Set<string>();

function sampleRows(): AggSession[] {
  const base = { serverId: 'news', serverLabel: '', serverColor: '', provider: 'claude' as const, tracked: true, state: 'idle' as const, branch: 'main' };
  const now = Date.now() / 1000;
  return [
    { ...base, name: m.news_tags_sample_session(), last_activity: now - 240 },
    { ...base, name: m.news_rename_sample_name(), last_activity: now - 900 },
    { ...base, name: m.news_tags_sample_session_other(), last_activity: now - 3 * 3600 },
  ];
}

export function HideFromListAnimation({ width }: { width: number }) {
  const { theme, rt } = useUnistyles();
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const rows = useMemo(sampleRows, []);
  // A lista de verdade ordena por nome: a primeira linha é a que se oculta.
  const target = rows[0].name;
  const hidden = t >= HIDE;
  const expanded = t >= EXPAND;
  const pressed = t >= PRESS;
  const sections = useMemo(
    () => buildSessionSections(hidden ? rows.map((r) => (r.name === target ? { ...r, hidden_from_list: true } : r)) : rows,
      expanded ? 'expanded' : 'collapsed', 'none', '', NO_GROUPS),
    [rows, target, hidden, expanded],
  );
  const items = useMemo(() => [
    { id: 'rename', label: m.sessao_renomear() },
    { id: 'hide', label: m.session_hide_from_list(), selected: pressed },
    { id: 'delete', label: m.sessao_excluir_curto() },
  ], [pressed]);
  const menu = t >= MENU && !hidden;
  const enter = reduced ? undefined : FadeIn.duration(240);
  const fade = reduced ? undefined : FadeOut.duration(200);
  const layout = reduced ? undefined : LinearTransition.duration(260);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {sections.map((section) => <Animated.View key={section.id} layout={layout} exiting={fade}>
      {section.label ? <Animated.View entering={enter}>
        <SessionSectionHeader section={section} onToggle={noop} />
      </Animated.View> : null}
      {section.data.map((s, index) => {
        const open = menu && s.name === target;
        return <Animated.View key={`${section.id}::${s.name}`} entering={enter} layout={layout}
          exiting={reduced ? undefined : SlideOutLeft.duration(320)} style={open && styles.raised}>
          {/* Fundo de linha apertada enquanto o menu do toque longo está aberto. */}
          <View style={open && { backgroundColor: superficie(theme, 0.6) }}>
            <SessionSectionCard index={index} total={section.data.length}>
              <SessionRow session={s} compact mostrarServidor={false} onPress={noop} />
            </SessionSectionCard>
          </View>
          {open ? <Animated.View entering={reduced ? undefined : ZoomIn.duration(180)} exiting={reduced ? undefined : FadeOut.duration(140)}
            style={[styles.menu, { boxShadow: `0 8px 28px rgba(0,0,0,${rt.themeName === 'dark' ? 0.4 : 0.16})` }]}>
            {/* Tinta quase sólida, como o painel dos menus do app: a linha de trás não atravessa. */}
            <View style={[StyleSheet.absoluteFill, styles.menuRadius, { backgroundColor: `rgba(${theme.tokens.glass.panelRgb.join(',')},0.97)` }]} />
            <Glass variant="modal" style={styles.menuPanel}>
              <SettingsList items={items} onPick={noop} searchable={false} />
            </Glass>
          </Animated.View> : null}
        </Animated.View>;
      })}
    </Animated.View>)}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura reservada para o quadro mais alto (seção aberta): o cartão não pula entre os passos.
  stage: {
    minHeight: 232, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  raised: { zIndex: 2 },
  menu: { position: 'absolute', top: '100%', right: theme.base.space[3], width: '62%', borderRadius: theme.base.radius.lg },
  menuRadius: { borderRadius: theme.base.radius.lg },
  menuPanel: { borderRadius: theme.base.radius.lg, padding: theme.base.space[1] },
}));
