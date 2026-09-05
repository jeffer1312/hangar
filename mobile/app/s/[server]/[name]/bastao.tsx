import { useLocalSearchParams } from 'expo-router';
import { BastaoSheet } from '../../../../src/chat/BastaoSheet';

export default function BastaoRoute() {
  const { name } = useLocalSearchParams<{ server: string; name: string }>();
  return <BastaoSheet name={String(name ?? '')} />;
}
