import { Text, View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { ComposerBox } from '../../chat/ComposerField';
import { QuietPill } from '../create/QuietPill';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `sem-projeto`: o toque em "Sem projeto" acende a pílula; a pasta vira "Sem projeto" e o botão sai.
const CYCLE = 6;
const PRESS = 1.8;
const PICKED = 2.2;
const STEPS = [PRESS, PICKED];
// Movimento reduzido: o botão à vista, antes do toque.
const STILL = 0;

const noop = () => {};

export function NoProjectAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const picked = t >= PICKED;
  const folder = picked ? m.native_new_chat_no_project() : m.news_tags_sample_session();
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={styles.pills}>
      <QuietPill icon="Monitor" label={m.native_news_login_browser_home()} aria={m.native_new_chat_machine()} onPress={noop} />
      <QuietPill icon="Folder" label={folder} aria={m.native_new_chat_folder()} onPress={noop} />
      {picked ? null : <QuietPill plain icon="House" label={m.native_new_chat_no_project()} aria={m.native_new_chat_no_project_hint()}
        tone={t >= PRESS ? 'accent' : undefined} onPress={noop} />}
    </View>
    <ComposerBox focused={false}>
      <Text style={styles.placeholder} numberOfLines={1}>{m.composer_mensagem_para({ nome: 'Claude' })}</Text>
    </ComposerBox>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    gap: theme.base.space[1.5], padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  pills: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', alignItems: 'center', gap: theme.base.space[0.5] },
  placeholder: { padding: theme.base.space[3], fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
}));
