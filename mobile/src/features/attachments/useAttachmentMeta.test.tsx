// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import type { Server, UploadFile } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const h = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  listUploads: h.list,
  resolverCitados: vi.fn(async () => ({ ok: {}, faltam: [] })),
  registrarDiag: vi.fn(),
}));
vi.mock('../../paraglide/messages', () => ({}));

import { useGalleryMeta } from './useAttachmentMeta';

const server: Server = { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 't' };
const upload = (size: number): UploadFile => ({ filename: 'a.png', size, mtime: 1, expires_in_days: null });

function Probe({ session }: { session: string }) {
  const meta = useGalleryMeta(server, session, [], true);
  return createElement('span', null, String(meta.uploads.get('a.png')?.size ?? 'none'));
}

it('switching sessions never shows the previous session sizes while the new ones load', async () => {
  let finishB!: (v: { files: UploadFile[] }) => void;
  h.list.mockResolvedValueOnce({ files: [upload(100)] })
    .mockImplementationOnce(() => new Promise((resolve) => { finishB = resolve; }));
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(Probe, { session: 'a' })));
  expect(container.textContent).toBe('100');
  await act(async () => root.render(createElement(Probe, { session: 'b' })));
  expect(h.list).toHaveBeenLastCalledWith('b', server);
  expect(container.textContent).toBe('none');
  await act(async () => finishB({ files: [upload(200)] }));
  expect(container.textContent).toBe('200');
  act(() => root.unmount());
});
