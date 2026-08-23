import { Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { LOOP_GUIDE, type LoopGuideMessageKey } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  goal: string;
  checkCmd: string;
  maxIters: number;
  requireBranch: boolean;
  suggestions: string[];
  creating: boolean;
  createErr: string;
  refining: boolean;
  refineErr: string;
  prevGoal: string | null;
  guideOpen: boolean;
  guideOpenSections: Set<number>;
  onGoalChange: (value: string) => void;
  onCheckCmdChange: (value: string) => void;
  onMaxItersChange: (value: number) => void;
  onRequireBranchChange: (value: boolean) => void;
  onRefine: () => void;
  onUndoRefine: () => void;
  onStart: () => void;
  onToggleGuide: () => void;
  onToggleGuideSection: (index: number) => void;
}

const GUIDE_MESSAGES: Record<LoopGuideMessageKey, () => string> = {
  loop_objetivo_titulo: m.loop_objetivo_titulo,
  loop_objetivo_corpo: m.loop_objetivo_corpo,
  loop_check_titulo: m.loop_check_titulo,
  loop_check_corpo: m.loop_check_corpo,
  loop_iteracoes_titulo: m.loop_iteracoes_titulo,
  loop_iteracoes_corpo: m.loop_iteracoes_corpo,
  loop_sinais_titulo: m.loop_sinais_titulo,
  loop_sinais_corpo: m.loop_sinais_corpo,
  loop_dica_titulo: m.loop_dica_titulo,
  loop_dica_corpo: m.loop_dica_corpo,
};

export function LoopForm({
  goal,
  checkCmd,
  maxIters,
  requireBranch,
  suggestions,
  creating,
  createErr,
  refining,
  refineErr,
  prevGoal,
  guideOpen,
  guideOpenSections,
  onGoalChange,
  onCheckCmdChange,
  onMaxItersChange,
  onRequireBranchChange,
  onRefine,
  onUndoRefine,
  onStart,
  onToggleGuide,
  onToggleGuideSection,
}: Props) {
  const { theme } = useUnistyles();
  const goalTrimmed = goal.trim();

  return (
    <ScrollView contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.loop_titulo()}</Text>

      <View style={styles.field}>
        <View style={styles.fieldHead}>
          <Text style={[styles.label, { color: theme.tokens.text.secondary }]}>{m.loop_objetivo()}</Text>
          <View style={styles.fieldActions}>
            {prevGoal !== null ? (
              <Pressable onPress={onUndoRefine} style={styles.smallAction} accessibilityRole="button">
                <Text style={[styles.smallActionText, { color: theme.tokens.text.muted }]}>{m.loop_desfazer()}</Text>
              </Pressable>
            ) : null}
            <Pressable
              onPress={onRefine}
              disabled={refining || !goalTrimmed}
              style={[styles.refineButton, { borderColor: theme.tokens.border.default }, (refining || !goalTrimmed) && styles.disabled]}
              accessibilityRole="button"
              accessibilityLabel={m.loop_reescreve_objetivo()}
            >
              <Text style={[styles.refineText, { color: theme.tokens.text.secondary }]}>{refining ? m.loop_melhorando() : m.loop_melhorar()}</Text>
            </Pressable>
          </View>
        </View>
        <TextInput
          style={[styles.goalInput, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.default, color: theme.tokens.text.primary }]}
          value={goal}
          onChangeText={onGoalChange}
          placeholder={m.loop_objetivo_placeholder()}
          placeholderTextColor={theme.tokens.text.muted}
          multiline
          numberOfLines={4}
          textAlignVertical="top"
          editable={!refining}
          accessibilityLabel={m.loop_objetivo()}
        />
        {refineErr ? (
          <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>
            {refineErr}
          </Text>
        ) : null}
      </View>

      <View style={styles.field}>
        <Text style={[styles.label, { color: theme.tokens.text.secondary }]}>{m.loop_check_label()}</Text>
        <TextInput
          style={[styles.input, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.default, color: theme.tokens.text.primary }]}
          value={checkCmd}
          onChangeText={onCheckCmdChange}
          placeholder={m.loop_check_placeholder()}
          placeholderTextColor={theme.tokens.text.muted}
          autoCapitalize="none"
          autoCorrect={false}
          accessibilityLabel={m.loop_check_label()}
        />
        {suggestions.length ? (
          <View style={styles.chips}>
            {suggestions.map((suggestion) => (
              <Pressable
                key={suggestion}
                onPress={() => onCheckCmdChange(suggestion)}
                style={[styles.chip, { backgroundColor: theme.tokens.bg.surface, borderColor: checkCmd === suggestion ? theme.tokens.accent.base : theme.tokens.border.default }]}
                accessibilityRole="button"
                accessibilityState={{ selected: checkCmd === suggestion }}
              >
                <Text style={[styles.chipText, { color: theme.tokens.text.secondary }]}>{suggestion}</Text>
              </Pressable>
            ))}
          </View>
        ) : null}
      </View>

      <View style={styles.row}>
        <View style={styles.rowField}>
          <Text style={[styles.label, { color: theme.tokens.text.secondary }]}>{m.loop_max_iteracoes()}</Text>
          <TextInput
            style={[styles.input, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.default, color: theme.tokens.text.primary }]}
            value={String(maxIters)}
            onChangeText={(value) => onMaxItersChange(Math.min(100, Math.max(1, Number(value) || 1)))}
            keyboardType="number-pad"
            accessibilityLabel={m.loop_max_iteracoes()}
          />
        </View>
        <View style={styles.rowField}>
          <Text style={[styles.label, { color: theme.tokens.text.secondary }]}>{m.loop_exigir_branch()}</Text>
          <View style={styles.toggle} accessibilityRole="radiogroup" accessibilityLabel={m.loop_exigir_branch()}>
            <Pressable
              onPress={() => onRequireBranchChange(true)}
              style={[styles.toggleButton, { backgroundColor: requireBranch ? theme.tokens.accent.dim : theme.tokens.bg.surface, borderColor: requireBranch ? theme.tokens.accent.base : theme.tokens.border.default }]}
              accessibilityRole="radio"
              accessibilityState={{ selected: requireBranch }}
            >
              <Text style={[styles.toggleText, { color: theme.tokens.text.secondary }]}>{m.comandos_sim()}</Text>
            </Pressable>
            <Pressable
              onPress={() => onRequireBranchChange(false)}
              style={[styles.toggleButton, { backgroundColor: !requireBranch ? theme.tokens.accent.dim : theme.tokens.bg.surface, borderColor: !requireBranch ? theme.tokens.accent.base : theme.tokens.border.default }]}
              accessibilityRole="radio"
              accessibilityState={{ selected: !requireBranch }}
            >
              <Text style={[styles.toggleText, { color: theme.tokens.text.secondary }]}>{m.comandos_nao()}</Text>
            </Pressable>
          </View>
        </View>
      </View>

      {createErr ? (
        <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>
          {createErr}
        </Text>
      ) : null}

      <Pressable
        onPress={onStart}
        disabled={creating || !goalTrimmed}
        style={[styles.primary, { backgroundColor: theme.tokens.accent.base }, (creating || !goalTrimmed) && styles.disabled]}
        accessibilityRole="button"
      >
        <Text style={styles.primaryText}>{creating ? m.loop_iniciando() : m.loop_iniciar()}</Text>
      </Pressable>

      <Pressable onPress={onToggleGuide} style={[styles.guideToggle, { borderTopColor: theme.tokens.border.subtle }]} accessibilityRole="button">
        <Text style={[styles.guideToggleText, { color: theme.tokens.text.secondary }]}>{m.loop_como_escrever()}</Text>
        <Text style={[styles.chevron, { color: theme.tokens.text.muted }, guideOpen && styles.chevronOpen]}>›</Text>
      </Pressable>
      {guideOpen ? (
        <View style={styles.guide}>
          {LOOP_GUIDE.map((section, index) => {
            const open = guideOpenSections.has(index);
            return (
              <View key={section.title} style={[styles.guideSection, { borderBottomColor: theme.tokens.border.subtle }]}>
                <Pressable onPress={() => onToggleGuideSection(index)} style={styles.guideHead} accessibilityRole="button" accessibilityState={{ expanded: open }}>
                  <Text style={[styles.guideHeadText, { color: theme.tokens.text.primary }]}>{GUIDE_MESSAGES[section.title]()}</Text>
                  <Text style={[styles.chevron, { color: theme.tokens.text.muted }, open && styles.chevronOpen]}>›</Text>
                </Pressable>
                {open ? <Text style={[styles.guideBody, { color: theme.tokens.text.secondary }]}>{GUIDE_MESSAGES[section.body]()}</Text> : null}
              </View>
            );
          })}
        </View>
      ) : null}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  scroll: {
    padding: theme.base.space[4],
    gap: theme.base.space[3],
    paddingBottom: 40,
  },
  title: {
    fontSize: 20,
    fontWeight: '600',
  },
  field: {
    gap: theme.base.space[2],
  },
  fieldHead: {
    minHeight: 28,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: theme.base.space[2],
  },
  fieldActions: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  label: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  smallAction: {
    minHeight: 32,
    justifyContent: 'center',
  },
  smallActionText: {
    fontSize: theme.base.text.xs,
    textDecorationLine: 'underline',
  },
  refineButton: {
    minHeight: 32,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    justifyContent: 'center',
  },
  refineText: {
    fontSize: theme.base.text.xs,
    fontWeight: '500',
  },
  input: {
    minHeight: 44,
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    fontSize: 16,
    paddingHorizontal: theme.base.space[3],
  },
  goalInput: {
    minHeight: 112,
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    fontSize: 16,
    lineHeight: 22,
    padding: theme.base.space[3],
  },
  chips: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
  },
  chip: {
    minHeight: 32,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    justifyContent: 'center',
  },
  chipText: {
    fontFamily: theme.base.fontMono,
    fontSize: theme.base.text.xs,
  },
  row: {
    flexDirection: 'row',
    gap: theme.base.space[3],
  },
  rowField: {
    flex: 1,
    minWidth: 0,
    gap: theme.base.space[2],
  },
  toggle: {
    flexDirection: 'row',
    gap: theme.base.space[2],
  },
  toggleButton: {
    minHeight: 44,
    flex: 1,
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
  },
  toggleText: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  error: {
    fontSize: theme.base.text.sm,
  },
  primary: {
    minHeight: 50,
    borderRadius: theme.base.radius.md,
    alignItems: 'center',
    justifyContent: 'center',
  },
  primaryText: {
    color: '#fff',
    fontSize: theme.base.text.base,
    fontWeight: '600',
  },
  disabled: {
    opacity: 0.5,
  },
  guideToggle: {
    minHeight: 44,
    paddingHorizontal: theme.base.space[1],
    borderTopWidth: 1,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  guideToggleText: {
    fontSize: theme.base.text.sm,
  },
  chevron: {
    fontSize: 20,
  },
  chevronOpen: {
    transform: [{ rotate: '90deg' }],
  },
  guide: {
    gap: theme.base.space[1],
  },
  guideSection: {
    borderBottomWidth: 1,
    paddingBottom: theme.base.space[2],
  },
  guideHead: {
    minHeight: 44,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
  },
  guideHeadText: {
    flex: 1,
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  guideBody: {
    paddingBottom: theme.base.space[2],
    fontSize: theme.base.text.sm,
    lineHeight: 21,
  },
}));
