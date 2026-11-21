import { beforeEach, describe, expect, it, vi } from 'vitest';

const picker = vi.hoisted(() => ({
  launchImageLibraryAsync: vi.fn(),
  launchCameraAsync: vi.fn(),
  requestCameraPermissionsAsync: vi.fn(),
}));
vi.mock('expo-image-picker', () => picker);
vi.mock('expo-document-picker', () => ({ getDocumentAsync: vi.fn() }));
vi.mock('../paraglide/messages', () => ({
  composer_sem_acesso_fotos: () => 'sem-fotos',
  composer_sem_acesso_camera: () => 'sem-camera',
}));

import { pickCamera, pickImages } from './attachmentPicker';

const video = { uri: 'file:///v.mov', type: 'video', fileName: 'IMG_1.MOV', mimeType: 'video/quicktime', fileSize: 9, width: 1, height: 1 };
const photo = { uri: 'file:///p.jpg', type: 'image', fileName: 'IMG_2.JPG', mimeType: 'image/jpeg', fileSize: 3, width: 1, height: 1 };

describe('galeria e câmera', () => {
  beforeEach(() => vi.clearAllMocks());

  it('a galeria oferece fotos e vídeos; o vídeo vira arquivo para subir com quadros e fala', async () => {
    picker.launchImageLibraryAsync.mockResolvedValue({ canceled: false, assets: [video, photo] });
    const picked = await pickImages(true);
    expect(picker.launchImageLibraryAsync).toHaveBeenCalledWith(expect.objectContaining({ mediaTypes: ['images', 'videos'] }));
    expect(picked).toEqual([
      { uri: 'file:///v.mov', name: 'IMG_1.MOV', mime: 'video/quicktime', kind: 'file', size: 9 },
      { uri: 'file:///p.jpg', name: 'IMG_2.JPG', mime: 'image/jpeg', kind: 'image', size: 3 },
    ]);
  });

  it('vídeo sem nome nem tipo ganha os de um mp4', async () => {
    picker.requestCameraPermissionsAsync.mockResolvedValue({ granted: true });
    picker.launchCameraAsync.mockResolvedValue({ canceled: false, assets: [{ ...video, fileName: null, mimeType: undefined }] });
    expect(await pickCamera()).toMatchObject({ name: 'video.mp4', mime: 'video/mp4', kind: 'file' });
    expect(picker.launchCameraAsync).toHaveBeenCalledWith(expect.objectContaining({ mediaTypes: ['images', 'videos'] }));
  });
});
