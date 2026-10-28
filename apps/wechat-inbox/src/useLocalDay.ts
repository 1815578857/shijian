import { useEffect, useState } from 'react';

export function useLocalDay() {
  const [day, setDay] = useState(() => new Date().toDateString());
  useEffect(() => {
    let midnight: ReturnType<typeof setTimeout>;
    function checkDay() {
      const now = new Date();
      setDay(now.toDateString());
      clearTimeout(midnight);
      const next = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
      midnight = setTimeout(checkDay, next.getTime() - now.getTime());
    }
    const visible = () => { if (document.visibilityState === 'visible') checkDay(); };
    checkDay();
    // Also correct the midnight timer after sleep, clock changes, or a timezone change.
    const interval = setInterval(checkDay, 60000);
    window.addEventListener('focus', checkDay);
    document.addEventListener('visibilitychange', visible);
    return () => {
      clearTimeout(midnight); clearInterval(interval);
      window.removeEventListener('focus', checkDay);
      document.removeEventListener('visibilitychange', visible);
    };
  }, []);
  return day;
}
