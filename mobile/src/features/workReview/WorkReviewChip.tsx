import { memo } from 'react';
import { Pressable } from 'react-native';
import type { WorkReviewBadge } from '@hangar/core';
import { Chip } from '../../ui/Chip';
import { BADGE_CHIP_TONE } from './tones';

// O chip mede uns 20 pt: a folga completa os 44 do toque sem empurrar a linha.
const HIT_SLOP = { top: 12, bottom: 12, left: 6, right: 6 };

/** Chip da revisão na linha da sessão; com `onPress` abre a revisão (no celular, a folha). */
export const WorkReviewChip = memo(function WorkReviewChip({ badge, onPress }: { badge: WorkReviewBadge; onPress?: () => void }) {
  const chip = <Chip tone={BADGE_CHIP_TONE[badge.tone]} icon="ClipboardCheck">{badge.text}</Chip>;
  if (!onPress) return chip;
  return (
    <Pressable onPress={onPress} hitSlop={HIT_SLOP} accessibilityRole="button" accessibilityLabel={badge.text}>
      {chip}
    </Pressable>
  );
});
