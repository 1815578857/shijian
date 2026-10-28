import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { bridge, isDesktop } from './bridge';
import type { AppSnapshot } from './types';

export function errorText(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

// A request arriving during a read invalidates that read and guarantees a follow-up.
export function createSnapshotRefresh<T>(load: () => Promise<T>, apply: (value: T) => void) {
  let requested = 0;
  let running: Promise<void> | null = null;
  return () => {
    requested++;
    if (!running) {
      running = Promise.resolve().then(async () => {
        for (;;) {
          const version = requested;
          try {
            const value = await load();
            if (version === requested) { apply(value); return; }
          } catch (reason) {
            if (version === requested) throw reason;
          }
        }
      }).finally(() => { running = null; });
    }
    return running;
  };
}

export function useAppSnapshot(onError: (message: string) => void) {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [loadError, setLoadError] = useState('');
  const [syncError, setSyncError] = useState('');
  const [subscriptionVersion, setSubscriptionVersion] = useState(0);
  const active = useRef(false);
  const errorHandler = useRef(onError);
  errorHandler.current = onError;
  const loader = useRef<ReturnType<typeof createSnapshotRefresh<AppSnapshot>> | null>(null);
  if (!loader.current) {
    loader.current = createSnapshotRefresh(() => bridge.snapshot(), state => {
      if (active.current) { setSnapshot(state); setLoadError(''); }
    });
  }
  const refresh = useCallback(async () => {
    try { await loader.current!(); }
    catch (reason) { if (active.current) setLoadError(errorText(reason)); throw reason; }
  }, []);
  const retrySync = useCallback(() => setSubscriptionVersion(value => value + 1), []);

  useEffect(() => {
    active.current = true;
    let disposed = false;
    const listeners: (() => void)[] = [];
    const report = (reason: unknown) => { if (!disposed) errorHandler.current(errorText(reason)); };
    const read = () => { void refresh().catch(report); };
    const visible = () => { if (document.visibilityState === 'visible') read(); };
    window.addEventListener('focus', read);
    document.addEventListener('visibilitychange', visible);
    setSyncError('');
    read();
    if (isDesktop) {
      const register = async (promise: Promise<() => void>) => {
        const unlisten = await promise;
        if (disposed) unlisten(); else listeners.push(unlisten);
      };
      void Promise.allSettled([
        register(listen('app-state-changed', read)),
        register(listen<string>('app-error', event => { if (!disposed) errorHandler.current(event.payload); })),
      ]).then(results => {
        if (disposed) return;
        if (results.some(result => result.status === 'rejected')) setSyncError('实时同步不可用，请重试；切回窗口仍会刷新。');
        read();
      });
    }
    return () => {
      disposed = true; active.current = false;
      listeners.forEach(unlisten => unlisten());
      window.removeEventListener('focus', read);
      document.removeEventListener('visibilitychange', visible);
    };
  }, [refresh, subscriptionVersion]);

  return { snapshot, loadError, syncError, refresh, retrySync };
}
