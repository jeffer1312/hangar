import { View } from 'react-native';
import { stateColors, type State } from '@hangar/core';

export function StateDot({ state, size = 8 }: { state: State; size?: number }) {
  return <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: stateColors[state] }} />;
}
