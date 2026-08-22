import { useEffect, useState } from 'react';
export function useElapsedTime(date: Date | number | null | undefined): number {
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  useEffect(() => {
    if (!date) { setElapsedSeconds(0); return; }
    const ts = date instanceof Date ? date.getTime() : date;
    const update = () => setElapsedSeconds(Math.max(0, Math.floor((Date.now() - ts) / 1000)));
    update();
    const id = setInterval(update, 1000);
    return () => clearInterval(id);
  }, [date]);
  return elapsedSeconds;
}
