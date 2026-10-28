import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { listen } from '@tauri-apps/api/event';
import { bridge } from './bridge';
import { createSnapshotRefresh, useAppSnapshot } from './useAppSnapshot';
import { defaultSettings, disconnectedWechat } from './types';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
vi.mock('./bridge', async original => {
  const actual = await original<typeof import('./bridge')>();
  return { ...actual, isDesktop: true, bridge: { ...actual.bridge, snapshot: vi.fn() } };
});
const snapshot = { records: [], settings: defaultSettings, hasModelKey: false, runtime: 'desktop' as const, dataDir: 'isolated', wechatStatus: 'not_connected' as const, wechat: disconnectedWechat };

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(listen).mockResolvedValue(() => {});
  vi.mocked(bridge.snapshot).mockResolvedValue(structuredClone(snapshot));
});

describe('snapshot refresh ordering', () => {
  it('coalesces concurrent reads and never applies an invalidated snapshot', async () => {
    let finish!: (value: number) => void;
    const load = vi.fn().mockImplementationOnce(() => new Promise<number>(resolve => { finish = resolve; })).mockResolvedValue(2);
    const apply = vi.fn();
    const refresh = createSnapshotRefresh(load, apply);
    const first = refresh();
    await Promise.resolve();
    const second = refresh(), third = refresh();
    finish(1);
    await Promise.all([first, second, third]);
    expect(load).toHaveBeenCalledTimes(2);
    expect(apply).toHaveBeenCalledExactlyOnceWith(2);
  });
  it('retries a read invalidated by a newer request even when it fails', async () => {
    let fail!: (reason: Error) => void;
    const load = vi.fn().mockImplementationOnce(() => new Promise<number>((_, reject) => { fail = reject; })).mockResolvedValue(2);
    const apply = vi.fn(), refresh = createSnapshotRefresh(load, apply);
    const first = refresh(); await Promise.resolve();
    const second = refresh(); fail(new Error('old read failed'));
    await Promise.all([first, second]);
    expect(apply).toHaveBeenCalledExactlyOnceWith(2);
  });
  it('shows subscription failures without blocking data and cleans up late listeners', async () => {
    let finish!: (value: () => void) => void;
    const unlisten = vi.fn();
    vi.mocked(listen).mockRejectedValueOnce(new Error('permission denied')).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    const view = renderHook(() => useAppSnapshot(vi.fn()));
    await waitFor(() => expect(view.result.current.snapshot).not.toBeNull());
    await act(async () => { finish(unlisten); });
    await waitFor(() => expect(view.result.current.syncError).toContain('实时同步不可用'));
    view.unmount(); expect(unlisten).toHaveBeenCalledOnce();
  });
  it('unsubscribes listeners that resolve after unmount', async () => {
    let finish!: (value: () => void) => void;
    vi.mocked(listen).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    const view = renderHook(() => useAppSnapshot(vi.fn()));
    view.unmount();
    const unlisten = vi.fn();
    await act(async () => { finish(unlisten); });
    expect(unlisten).toHaveBeenCalledOnce();
  });
});
