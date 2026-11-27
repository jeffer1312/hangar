import { useState } from 'react';
import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `contagens-anonimas`: a linha da máquina passa de não respondida a ligada.
const CYCLE = 6;
const ON = 1.6;
const STEPS = [ON];
// Movimento reduzido: quadro parado na escolha feita.
const STILL = 3;

export function TelemetryConsentAnimation({ width }: { width: number }) {
  const on = useScriptStep(STEPS, CYCLE, STILL, useReducedMotion()) >= ON;
  // "Não respondido" quebra o título em duas linhas: a altura maior fica, e o cartão não pula.
  const [minHeight, setMinHeight] = useState(0);
  return <View style={{ width, minHeight }} onLayout={(e) => { const h = e.nativeEvent.layout.height; setMinHeight((old) => Math.max(old, h)); }} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <MachineGroup>
      <MachineRow title={m.telemetry_setting()} description={m.telemetry_setting_hint()}
        value={on ? m.auto_report_on() : m.auto_report_value_unanswered()} checked={on || 'mixed'} />
      <MachineRow title={m.telemetry_what_is_sent()} action />
    </MachineGroup>
  </View>;
}
