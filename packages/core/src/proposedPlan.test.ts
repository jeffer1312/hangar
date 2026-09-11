import { describe, it, expect } from 'vitest';
import { proposedPlan, planDisplayText, planTitle } from './proposedPlan';

it('usa o primeiro título real, ignorando cercas e código indentado', () => {
  expect(planTitle('```md\n# Exemplo\n```\n    # Código\n## Título real ###\n# Outro')).toBe('Título real');
  expect(planTitle('~~~~md\n# Exemplo\n~~~\n# Ainda exemplo\n~~~~\nTítulo setext\n=====')).toBe('Título setext');
  expect(planTitle('Sem título\n\n- Etapa')).toBeNull();
  expect(planTitle('# ' + 'Longo '.repeat(80))).toBe('Longo '.repeat(80).trim());
});

describe('plano proposto pelo Codex', () => {
  it('extrai o plano completo preservando o Markdown', () => {
    const text = 'Segue o plano.\n<proposed_plan>\n# Plano\n\n- **Primeiro**\n</proposed_plan>';
    expect(proposedPlan(text)).toBe('# Plano\n\n- **Primeiro**');
    expect(planDisplayText(text)).toBe('Segue o plano.\n\n# Plano\n\n- **Primeiro**\n');
  });
  it('não oferece aprovação durante a escrita', () => {
    expect(proposedPlan('<proposed_plan>\n# Incompleto')).toBeNull();
    expect(planDisplayText('<proposed_plan>\n# Incompleto')).toBe('\n# Incompleto');
  });
  it('preserva exemplos de tags dentro de blocos de código', () => {
    const example = '```xml\n<proposed_plan>\ntexto\n</proposed_plan>\n```';
    expect(proposedPlan(example)).toBeNull();
    expect(planDisplayText(example)).toBe(example);
  });
  it('tira o bloco de citação da memória inteiro, fora de cercas', () => {
    const cite = '<oai-mem-citation>\n<citation_entries>\nMEMORY.md:32-36|note=[x]\n</citation_entries>\n</oai-mem-citation>';
    expect(planDisplayText(`Resposta.\n\n${cite}`)).toBe('Resposta.\n\n');
    const example = '```\n' + cite + '\n```';
    expect(planDisplayText(example)).toBe(example);
    expect(planDisplayText('<oai-mem-citation>\nincompleto')).toBe('<oai-mem-citation>\nincompleto');
  });
  it('reconhece a tag final sem abertura no trecho recebido', () => {
    expect(proposedPlan('# Plano\n\nEtapas.\n</proposed_plan>')).toBe('# Plano\n\nEtapas.');
  });
});
