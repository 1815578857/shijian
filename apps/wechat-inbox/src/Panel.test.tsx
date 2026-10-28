import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
import Panel from './Panel';
import { bridge } from './bridge';

describe('desktop todo panel in browser-preview mode', () => {
  it('uses date and time ordering shared with the main list', async () => {
    await bridge.createRecord('上午任务', '上午任务', '2030-01-02', 'normal', '09:00');
    await bridge.createRecord('下午任务', '下午任务', '2030-01-02', 'normal', '16:00');
    await bridge.createRecord('只有日期', '只有日期', '2030-01-02', 'normal');
    await bridge.createRecord('没有日期', '没有日期', null, 'normal');
    const view = render(<Panel />);
    await screen.findByText('上午任务');
    expect(Array.from(view.container.querySelectorAll('.panel-row-title'), element => element.textContent)).toEqual(['上午任务', '下午任务', '只有日期', '没有日期']);
  });

  it('updates due labels across midnight without another snapshot', async () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date(2030, 0, 2, 23, 59, 59));
    await bridge.createRecord('面板跨日任务', '面板跨日任务', '2030-01-02', 'normal');
    const snapshot = vi.spyOn(bridge, 'snapshot');
    let view!: ReturnType<typeof render>;
    await act(async () => { view = render(<Panel />); });
    expect(screen.getByText('今天到期')).toBeVisible();
    const reads = snapshot.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(screen.getByText('已过期 1 天')).toBeVisible();
    expect(snapshot).toHaveBeenCalledTimes(reads);
    view.unmount();
  });

  it('retains drafts on create failure and clears committed drafts on refresh failure', async () => {
    const user = userEvent.setup();
    render(<Panel />);
    const input = await screen.findByRole('textbox', { name: '快速添加待办' });
    await user.type(input, '不能丢的草稿');
    vi.spyOn(bridge, 'createRecord').mockRejectedValueOnce(new Error('保存失败'));
    await user.keyboard('{Enter}');
    expect(await screen.findByText('保存失败')).toBeVisible();
    expect(input).toHaveValue('不能丢的草稿');
    vi.spyOn(bridge, 'snapshot').mockRejectedValueOnce(new Error('刷新失败'));
    await user.keyboard('{Enter}');
    expect(await screen.findByText('刷新失败', { exact: true })).toBeVisible();
    expect(input).toHaveValue('');
  });
  it('starts empty with an honest hint and quick-add', async () => {
    render(<Panel />);
    expect(await screen.findByText('拾笺')).toBeVisible();
    expect(screen.getByText('没有待办')).toBeVisible();
    expect(screen.getByRole('textbox', { name: '快速添加待办' })).toBeVisible();
  });
  it('adds, completes, reopens and archives records from the panel', async () => {
    const user = userEvent.setup();
    render(<Panel />);
    const input = await screen.findByRole('textbox', { name: '快速添加待办' });
    await user.type(input, '面板快速记录');
    await user.keyboard('{Enter}');
    expect(await screen.findByText('面板快速记录')).toBeVisible();
    expect(screen.getByText('1 件待办')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '完成：面板快速记录' }));
    expect(await screen.findByText('已完成')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '重新打开：面板快速记录' }));
    expect(await screen.findByText('面板快速记录')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '归档：面板快速记录' }));
    expect(await screen.findByText('没有待办')).toBeVisible();
    expect((await bridge.snapshot()).records[0].status).toBe('archived');
  });
  it('pins and closes the panel through persisted settings', async () => {
    const user = userEvent.setup();
    render(<Panel />);
    const pin = await screen.findByRole('button', { name: '置顶面板' });
    expect(pin).toHaveAttribute('aria-pressed', 'false');
    await user.click(pin);
    expect(await screen.findByRole('button', { name: '置顶面板', pressed: true })).toBeVisible();
    expect((await bridge.snapshot()).settings.panelPinned).toBe(true);
    await bridge.setPanelVisible(true);
    await user.click(screen.getByRole('button', { name: '关闭面板' }));
    expect((await bridge.snapshot()).settings.panelEnabled).toBe(false);
  });
  it('rejects empty drafts without creating records', async () => {
    const user = userEvent.setup();
    render(<Panel />);
    await screen.findByRole('textbox', { name: '快速添加待办' });
    await user.keyboard('{Enter}');
    expect((await bridge.snapshot()).records).toHaveLength(0);
  });
});
