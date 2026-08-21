// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import FileTree from './FileTree.svelte';
import type { TreeEntry } from '@hangar/core';
import { overwriteGetLocale } from '../../paraglide/runtime';

interface Props {
  entries: TreeEntry[];
  abertos: Set<string>;
  selecionado: string | null;
  onToggle: (p: string) => void;
  onPick: (p: string) => void;
}

const ent = (o: Partial<TreeEntry> = {}): TreeEntry => ({
  name: 'a.txt', path: 'a.txt', is_dir: false, size: 1,
  changed: 'M', add: 4, del: 2, ...o,
} as TreeEntry);

function montar(props: Props) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileTree, { target: el, props }) };
}

const base = { abertos: new Set<string>(), selecionado: null, onToggle: vi.fn(), onPick: vi.fn() };

describe('FileTree', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));

  it('pasta tem aria-expanded, arquivo nao', () => {
    const { el, comp } = montar({
      ...base,
      entries: [ent({ name: 'src', path: 'src', is_dir: true }), ent()],
      abertos: new Set(['src']),
    });
    const linhas = el.querySelectorAll('.no');
    expect(linhas[0].getAttribute('aria-expanded')).toBe('true');
    expect(linhas[1].getAttribute('aria-expanded')).toBeNull();
    unmount(comp);
  });

  it('mostra +N -M e some quando nao mudou', () => {
    const { el, comp } = montar({
      ...base,
      entries: [ent(), ent({ name: 'b.txt', path: 'b.txt', changed: null, add: 0, del: 0 })],
    });
    const nums = [...el.querySelectorAll('.num')].map((n) => n.textContent?.trim());
    expect(nums[0]).toContain('+4');
    expect(nums[1]).toBe('');
    unmount(comp);
  });

  it('clique em arquivo chama onPick com o caminho', async () => {
    const onPick = vi.fn();
    const { el, comp } = montar({ ...base, entries: [ent()], onPick });
    el.querySelector('.no')!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await tick();
    expect(onPick).toHaveBeenCalledWith('a.txt');
    unmount(comp);
  });

  it('Enter em pasta alterna (onToggle), Enter em arquivo escolhe (onPick)', async () => {
    const onToggle = vi.fn();
    const onPick = vi.fn();
    const { el, comp } = montar({
      entries: [ent({ name: 'src', path: 'src', is_dir: true }), ent({ name: 'a.txt', path: 'src/a.txt' })],
      abertos: new Set(['src']),
      selecionado: 'src',
      onToggle,
      onPick,
    });
    const linhas = el.querySelectorAll('.no');
    // foco na pasta (a selecionada é a linha com tabindex 0) e Enter: alterna, não escolhe
    (linhas[0] as HTMLButtonElement).focus();
    linhas[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onToggle).toHaveBeenCalledWith('src');
    expect(onPick).not.toHaveBeenCalled();
    // ↓ move o foco pro arquivo; Enter aí escolhe
    linhas[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    await tick();
    expect(document.activeElement?.getAttribute('data-path')).toBe('src/a.txt');
    document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(onPick).toHaveBeenCalledTimes(1);
    expect(onPick).toHaveBeenCalledWith('src/a.txt');
    expect(onToggle).toHaveBeenCalledTimes(1);
    unmount(comp);
  });

  it('arvore nomeada, niveis, selecao e roving tabindex', async () => {
    const { el, comp } = montar({
      ...base,
      entries: [ent({ name: 'src', path: 'src', is_dir: true }), ent({ name: 'a.txt', path: 'src/a.txt' })],
      abertos: new Set(['src']),
      selecionado: 'src/a.txt',
    });
    await tick();   // o roving tabindex sincroniza com a seleção num effect
    const arv = el.querySelector('[role="tree"]');
    expect(arv?.getAttribute('aria-label')).toBe('Arquivos');   // m.arq_aba() no locale pt
    const linhas = el.querySelectorAll('.no');
    expect(linhas[0].getAttribute('aria-level')).toBe('1');
    expect(linhas[1].getAttribute('aria-level')).toBe('2');
    expect(linhas[0].getAttribute('aria-selected')).toBe('false');
    expect(linhas[1].getAttribute('aria-selected')).toBe('true');
    // roving: só a selecionada recebe a parada de Tab
    expect(linhas[0].getAttribute('tabindex')).toBe('-1');
    expect(linhas[1].getAttribute('tabindex')).toBe('0');
    expect((linhas[0] as HTMLButtonElement).type).toBe('button');
    expect(linhas[0].querySelector('.chev')?.getAttribute('aria-hidden')).toBe('true');
    unmount(comp);
  });
});
