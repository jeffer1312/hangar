import * as ImagePicker from 'expo-image-picker';
import * as DocumentPicker from 'expo-document-picker';
import type { PastedFile } from '@mattermost/react-native-paste-input';
import type { DraftAttachment } from '../stores/drafts';
import * as m from '../paraglide/messages';

export type PickedAttachment = DraftAttachment & { size?: number };

const IMAGE_NAME = /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i;

const fromImageAsset = (asset: ImagePicker.ImagePickerAsset): PickedAttachment => ({
  uri: asset.uri,
  name: asset.fileName ?? 'imagem.jpg',
  mime: asset.mimeType ?? 'image/jpeg',
  kind: 'image',
  size: asset.fileSize,
});

export const fromPastedFile = (file: PastedFile): PickedAttachment => {
  const mime = file.type || 'application/octet-stream';
  return { uri: file.uri, name: file.fileName, mime, kind: mime.startsWith('image/') ? 'image' : 'file', size: file.fileSize };
};

const isDenied = (e: unknown) => {
  const code = (e as { code?: string } | null)?.code;
  return code === 'ERR_USER_REJECTED_PERMISSIONS' || (e instanceof Error && /permission/i.test(e.message));
};

// Galeria do aparelho. Permissão negada vira o aviso de fotos; cancelar devolve a lista vazia.
export async function pickImages(multiple = false): Promise<PickedAttachment[]> {
  try {
    const res = await ImagePicker.launchImageLibraryAsync({
      mediaTypes: ImagePicker.MediaTypeOptions.Images,
      quality: 0.8,
      allowsMultipleSelection: multiple,
    });
    if (res.canceled) return [];
    return (res.assets ?? []).map(fromImageAsset);
  } catch (e) {
    if (isDenied(e)) throw new Error(m.composer_sem_acesso_fotos());
    throw e;
  }
}

// Foto na hora. A permissão é pedida antes: negada, o aviso diz onde liberar em vez de falhar calado.
export async function pickCamera(): Promise<PickedAttachment | null> {
  try {
    const perm = await ImagePicker.requestCameraPermissionsAsync();
    if (!perm.granted) throw new Error(m.composer_sem_acesso_camera());
    const res = await ImagePicker.launchCameraAsync({
      mediaTypes: ImagePicker.MediaTypeOptions.Images,
      quality: 0.8,
    });
    if (res.canceled || !res.assets?.[0]) return null;
    return fromImageAsset(res.assets[0]);
  } catch (e) {
    if (isDenied(e)) throw new Error(m.composer_sem_acesso_camera());
    throw e;
  }
}

// Qualquer arquivo; versões antigas do picker devolvem o asset solto, sem `assets`.
export async function pickFiles(multiple = false): Promise<PickedAttachment[]> {
  const res = await DocumentPicker.getDocumentAsync({ type: '*/*', copyToCacheDirectory: true, multiple });
  if (res.canceled) return [];
  type Asset = { uri: string; name: string; mimeType?: string; size?: number };
  const assets = (res as unknown as { assets?: Asset[] }).assets ?? [res as unknown as Asset];
  return assets.filter((asset) => asset?.uri).map((asset) => ({
    uri: asset.uri,
    name: asset.name ?? 'arquivo',
    mime: asset.mimeType ?? 'application/octet-stream',
    kind: IMAGE_NAME.test(asset.name ?? '') ? 'image' : 'file',
    size: asset.size,
  }));
}

export const pickFile = async (): Promise<PickedAttachment | null> => (await pickFiles())[0] ?? null;

/** Imagem copiada vira anexo; senão devolve o texto. Vazia = null. */
export async function readClipboard(): Promise<PickedAttachment | string | null> {
  const Clipboard = await import('expo-clipboard');
  if (await Clipboard.hasImageAsync()) {
    const image = await Clipboard.getImageAsync({ format: 'png' });
    if (image?.data) {
      const FileSystem = await import('expo-file-system/legacy');
      const uri = `${FileSystem.cacheDirectory}colado-${Date.now()}.png`;
      await FileSystem.writeAsStringAsync(uri, image.data.replace(/^data:[^,]*,/, ''), { encoding: FileSystem.EncodingType.Base64 });
      return { uri, name: 'colado.png', mime: 'image/png', kind: 'image' };
    }
  }
  return (await Clipboard.getStringAsync()) || null;
}
