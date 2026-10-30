import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { SectionCard } from '../config/SectionCard';
import { SettingsRow } from '../config/SettingsRow';
import { MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `relatorio-automatico`: o interruptor é ligado, a atualização da máquina falha, o relatório sai sozinho e
// o código aparece na linha dela.
const CYCLE = 6.5;
const ON = 0.7;
const FAILED = 2;
const SENDING = 2.9;
const SENT = 4;
const STEPS = [ON, FAILED, SENDING, SENT];
// Movimento reduzido: quadro parado no resultado.
const STILL = 5;
// Formato real de código de relatório, sem ser o de ninguém.
const SAMPLE_CODE = 'k7m2q4xa';

function statusAt(t: number): string {
  if (t < FAILED) return m.update_all_running({ texto: m.news_auto_report_sample_step(), passo: '2', total: '3' });
  const failed = m.update_all_failed({ erro: m.news_auto_report_sample_error() });
  if (t < SENDING) return failed;
  return `${failed} · ${t < SENT ? m.update_report_pending() : m.update_report_sent({ code: SAMPLE_CODE })}`;
}

export function AutoReportAnimation({ width }: { width: number }) {
  const t = useScriptStep(STEPS, CYCLE, STILL, useReducedMotion());
  const on = t >= ON;
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <SectionCard>
      <MachineRow title={m.auto_report_setting()} value={on ? m.auto_report_on() : m.auto_report_off()} checked={on} />
    </SectionCard>
    <SectionCard>
      {/* Altura reservada para o texto mais longo: o cartão não pula quando o relatório entra. */}
      <View style={styles.status}>
        <SettingsRow title={m.news_auto_report_sample_machine()}>
          <Notice text={statusAt(t)} error={t >= FAILED} />
        </SettingsRow>
      </View>
    </SectionCard>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { gap: theme.base.space[2] },
  status: { minHeight: 104 },
}));
