import { useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useState } from "react";
import { ErrorBox } from "@/components/kit";
import { Logo } from "@/components/logo";
import { isUnauthorized, useMe } from "@/lib/api";
import { Join, Login } from "@/pages/auth";

// Os gráficos ficam num arquivo à parte: a entrada e o convite abrem sem baixá-los.
const Dashboard = lazy(() => import("@/pages/dashboard").then(m => ({ default: m.Dashboard })));
const Splash = () => <div className="grid min-h-dvh place-items-center"><Logo size={40} /></div>;

export function App() {
  const [joining, setJoining] = useState(() => location.pathname.replace(/\/$/, "").endsWith("/join"));
  const me = useMe();
  const client = useQueryClient();
  const enter = () => { setJoining(false); client.resetQueries(); };
  if (joining) return <Join onDone={enter} />;
  if (me.isPending) return <Splash />;
  if (isUnauthorized(me.error)) return <Login onDone={enter} />;
  if (me.isError) return <div className="mx-auto max-w-lg p-6 pt-24"><ErrorBox message="Não foi possível falar com o painel." detail={(me.error as Error).message} onRetry={() => me.refetch()} /></div>;
  return <Suspense fallback={<Splash />}><Dashboard email={me.data.email} /></Suspense>;
}
