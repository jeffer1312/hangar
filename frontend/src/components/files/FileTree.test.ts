// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import FileTree from './FileTree.svelte';
import type { TreeEntry } from '../../lib/types';
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
});
