import { useState } from 'react';
import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { placementReasonText, serverColor } from '@hangar/core';
import { MachineMenu, type MachineEntry } from '../create/MachineMenu';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `maquina-sugerida`: no menu de máquinas da conversa nova, a marca passa para a sugerida, com o motivo.
const CYCLE = 6;
const SWITCH = 1.6;
const STEPS = [SWITCH];
// Movimento reduzido: quadro parado na sugerida, com o motivo à vista.
const STILL = 3;

const noop = () => {};

export function PlacementSuggestAnimation({ width }: { width: number }) {
  const moved = useScriptStep(STEPS, CYCLE, STILL, useReducedMotion()) >= SWITCH;
  // A linha do motivo aumenta o menu: a altura maior fica, e o cartão não pula.
  const [minHeight, setMinHeight] = useState(0);
  const machine = (id: string, label: string): MachineEntry =>
    ({ id, label, color: serverColor(id), icon: 'Laptop', accounts: undefined, demo: false, offline: false });
  const busy = machine('busy', m.native_news_placement_busy());
  const free = machine('free', m.native_news_placement_free());
  // O mesmo texto da sugestão real, montado dos códigos que o servidor devolve.
  const text = placementReasonText({ server: free.label, reason: { code: 'least_loaded', local_out: 'working' }, clone: null, warnings: [] });
  return <View style={{ width, minHeight }} onLayout={(e) => { const h = e.nativeEvent.layout.height; setMinHeight((old) => Math.max(old, h)); }}
    pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <MachineMenu machines={[busy, free]} active={moved ? free.id : busy.id} onPick={noop} suggested={moved ? { serverId: free.id, text } : null} />
  </View>;
}
