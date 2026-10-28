import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useLocalDay } from './useLocalDay';

afterEach(() => { vi.useRealTimers(); });

describe('local day clock', () => {
  it('updates at local midnight without database reads and cleans up timers', () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date(2030, 0, 2, 23, 59, 59));
    const view = renderHook(() => useLocalDay());
    expect(view.result.current).toBe(new Date(2030, 0, 2).toDateString());
    act(() => { vi.advanceTimersByTime(1000); });
    expect(view.result.current).toBe(new Date(2030, 0, 3).toDateString());
    view.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('corrects forward and backward clock changes on focus and periodic checks', () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date(2030, 0, 2, 12));
    const view = renderHook(() => useLocalDay());
    act(() => { vi.setSystemTime(new Date(2030, 0, 5, 12)); window.dispatchEvent(new Event('focus')); });
    expect(view.result.current).toBe(new Date(2030, 0, 5).toDateString());
    act(() => { vi.setSystemTime(new Date(2030, 0, 1, 12)); vi.advanceTimersByTime(60000); });
    expect(view.result.current).toBe(new Date(2030, 0, 1).toDateString());
    act(() => { vi.setSystemTime(new Date(2030, 0, 8, 12)); document.dispatchEvent(new Event('visibilitychange')); });
    expect(view.result.current).toBe(new Date(2030, 0, 8).toDateString());
    view.unmount();
  });
});
