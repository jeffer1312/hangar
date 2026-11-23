import { useLocalSearchParams } from 'expo-router';
import { PortTunnelScreen } from '../../../../src/chat/PortTunnelScreen';
import { one } from '../../../../src/chat/tools/pageRoute';

export default function PortRoute() {
  const params = useLocalSearchParams<{ server: string; name: string; port: string }>();
  return <PortTunnelScreen serverId={one(params.server)} name={one(params.name)} port={Number(one(params.port))} />;
}
