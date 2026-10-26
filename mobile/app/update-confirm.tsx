import { useState } from 'react';
import { useRouter } from 'expo-router';
import { UpdateConfirmSheet } from '../src/features/updates/UpdateConfirmSheet';
import { currentConfirmRequest } from '../src/features/updates/pending';

export default function UpdateConfirmRoute() {
  const router = useRouter();
  const [request] = useState(currentConfirmRequest);
  return <UpdateConfirmSheet request={request} onClose={() => router.back()} />;
}
