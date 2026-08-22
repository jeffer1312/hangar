import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { CameraView, useCameraPermissions } from 'expo-camera';
import * as m from '../../paraglide/messages';

export function QrScanner({ onScan, onClose }: { onScan: (texto: string) => void; onClose: () => void }) {
  const [perm, request] = useCameraPermissions();

  if (!perm) {
    return (
      <View style={styles.center}>
        <Text style={styles.msg}>…</Text>
      </View>
    );
  }

  if (!perm.granted) {
    return (
      <View style={styles.center}>
        <Text style={styles.msg}>{m.login_permitir_camera()}</Text>
        <Pressable style={styles.btn} onPress={request}>
          <Text style={styles.btnText}>{m.login_permitir_camera()}</Text>
        </Pressable>
        <Pressable style={styles.btnGhost} onPress={onClose}>
          <Text style={styles.btnGhostText}>Fechar</Text>
        </Pressable>
      </View>
    );
  }

  return (
    <View style={styles.full}>
      <CameraView
        style={styles.camera}
        barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
        onBarcodeScanned={(e) => onScan(e.data)}
      />
      <Pressable style={styles.close} onPress={onClose}>
        <Text style={styles.closeText}>✕</Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  center: {
    flex: 1,
    justifyContent: 'center',
    alignItems: 'center',
    padding: theme.base.space[6],
    gap: theme.base.space[4],
    backgroundColor: theme.tokens.bg.base,
  },
  msg: {
    color: theme.tokens.text.primary,
    fontSize: theme.base.text.base,
    textAlign: 'center',
  },
  btn: {
    backgroundColor: theme.tokens.accent.base,
    paddingVertical: theme.base.space[3],
    paddingHorizontal: theme.base.space[6],
    borderRadius: theme.base.radius.md,
    minHeight: 44,
    justifyContent: 'center',
    alignItems: 'center',
  },
  btnText: {
    color: '#fff',
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
  },
  btnGhost: {
    paddingVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[4],
  },
  btnGhostText: {
    color: theme.tokens.text.secondary,
    fontSize: theme.base.text.sm,
  },
  full: { flex: 1, backgroundColor: '#000' },
  camera: { flex: 1 },
  close: {
    position: 'absolute',
    top: 50,
    right: 16,
    width: 44,
    height: 44,
    borderRadius: 22,
    backgroundColor: 'rgba(0,0,0,0.5)',
    justifyContent: 'center',
    alignItems: 'center',
  },
  closeText: { color: '#fff', fontSize: 20, fontWeight: '600' },
}));
