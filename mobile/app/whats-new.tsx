import { useLocalSearchParams, useRouter } from 'expo-router';
import { WhatsNewSheet } from '../src/features/updates/WhatsNewSheet';

export default function WhatsNewRoute() {
  const router = useRouter();
  const { server } = useLocalSearchParams<{ server?: string }>();
  return <WhatsNewSheet serverId={String(server ?? '')} onClose={() => router.back()} />;
}
