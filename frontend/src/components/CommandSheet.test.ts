// @vitest-environment happy-dom
// Bloqueador 1 da revisão final (parecer-final-kimi-2f863d0): `/model` e `/effort` viraram DUAS
// caixas quando o esforço ganhou pill própria, mas os dois comandos continuaram despachando pro
// seletor de MODELO. Quem digitava `/effort` caía na lista de modelos e tinha que fechar e achar a
// outra pill — o nível ficou sem caminho pelo comando.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import CommandSheet from './CommandSheet.svelte';
import type { CommandInfo } from '@hangar/core';

const COMANDOS: CommandInfo[] = [
  { name: 'model', display: '/model', source: 'builtin' },
  { name: 'effort', display: '/effort', source: 'builtin' },
  { name: 'clear', display: '/clear', source: 'builtin' },
];

function montar(onOpenModelEffort: (q: 'model' | 'effort') => void) {
  document.body.innerHTML = '';
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  return mount(CommandSheet, {
    target: alvo,
    props: {
      open: true,
      commands: COMANDOS,
      onCommand: vi.fn(),
      onFill: vi.fn(),
      onOpenModelEffort,
      onClose: vi.fn(),
    },
  });
}

function tocar(rotulo: string) {
  // A folha vive num portal pro body (BottomSheet).
  const linha = [...document.querySelectorAll<HTMLButtonElement>('.cmd-row')]
    .find((b) => b.textContent?.includes(rotulo));
  expect(linha, `linha ${rotulo} não encontrada`).toBeTruthy();
  linha!.click();
}

beforeEach(() => vi.clearAllMocks());

describe('CommandSheet — /model e /effort abrem caixas diferentes', () => {
  it('/effort pede a caixa de esforço', async () => {
    const abrir = vi.fn();
    const comp = montar(abrir);
    await tick();
    tocar('/effort');
    expect(abrir).toHaveBeenCalledWith('effort');
    unmount(comp);
  });

  it('/model pede a caixa de modelo', async () => {
    const abrir = vi.fn();
    const comp = montar(abrir);
    await tick();
    tocar('/model');
    expect(abrir).toHaveBeenCalledWith('model');
    unmount(comp);
  });
});
