import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
import App from './App';
import { bridge } from './bridge';

describe('desktop interface in browser-preview mode', () => {
  it('saves an unblurred title before Escape closes the detail', async () => {
    await bridge.createRecord('关闭前标题', '关闭前标题', null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /^关闭前标题/ }));
    await user.clear(screen.getByRole('textbox', { name: '待办标题' }));
    await user.type(screen.getByRole('textbox', { name: '待办标题' }), 'Esc 保存标题');
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('textbox', { name: '待办标题' })).not.toBeInTheDocument());
    expect((await bridge.snapshot()).records[0].title).toBe('Esc 保存标题');
  });

  it.each(['Escape', '关闭', '切换记录', '切换筛选', '导航', '归档', '完成'])('keeps a failed title draft when leaving via %s', async action => {
    await bridge.createRecord('当前任务', '当前任务', null, 'normal');
    await bridge.createRecord('另一任务', '另一任务', null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /^当前任务/ }));
    const title = screen.getByRole('textbox', { name: '待办标题' });
    await user.clear(title); await user.type(title, '不能丢的标题草稿');
    const update = vi.spyOn(bridge, 'updateRecord').mockRejectedValue(new Error('标题无法保存'));
    const archive = vi.spyOn(bridge, 'archiveRecord');
    const complete = vi.spyOn(bridge, 'completeRecord');
    if (action === 'Escape') await user.keyboard('{Escape}');
    else if (action === '关闭') await user.click(screen.getByRole('button', { name: '关闭详情' }));
    else if (action === '切换记录') await user.click(screen.getByRole('button', { name: /^另一任务/ }));
    else if (action === '切换筛选') await user.click(within(screen.getByRole('group', { name: '记录筛选' })).getByRole('button', { name: /已完成/ }));
    else if (action === '导航') await user.click(within(screen.getByRole('navigation')).getByRole('button', { name: '设置' }));
    else if (action === '归档') await user.click(screen.getByRole('button', { name: /^归档$/ }));
    else await user.click(screen.getByRole('button', { name: '标记完成' }));
    expect(await screen.findByText('标题无法保存')).toBeVisible();
    expect(screen.getByRole('textbox', { name: '待办标题' })).toHaveValue('不能丢的标题草稿');
    expect(update).toHaveBeenCalledTimes(1);
    expect(archive).not.toHaveBeenCalled(); expect(complete).not.toHaveBeenCalled();
  });

  it('waits for a pending blur save and the latest draft before closing', async () => {
    await bridge.createRecord('原始草稿', '原始草稿', null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /^原始草稿/ }));
    const title = screen.getByRole('textbox', { name: '待办标题' });
    await user.clear(title); await user.type(title, '第一版');
    let finish!: () => void;
    const waiting = new Promise<void>(resolve => { finish = resolve; });
    const original = bridge.updateRecord.bind(bridge);
    const update = vi.spyOn(bridge, 'updateRecord').mockImplementationOnce(async (id, patch) => { await waiting; return original(id, patch); });
    fireEvent.blur(title);
    await user.clear(title); await user.type(title, '保存期间的新草稿');
    await user.keyboard('{Escape}{Escape}');
    expect(title).toBeInTheDocument(); expect(update).toHaveBeenCalledTimes(1);
    await act(async () => { finish(); });
    await waitFor(() => expect(screen.queryByRole('textbox', { name: '待办标题' })).not.toBeInTheDocument());
    expect((await bridge.snapshot()).records[0].title).toBe('保存期间的新草稿');
    expect(update).toHaveBeenCalledTimes(2);
  });

  it('does not repeat a successful title write when the refresh fails', async () => {
    await bridge.createRecord('刷新故障测试', '刷新故障测试', null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /^刷新故障测试/ }));
    await user.clear(screen.getByRole('textbox', { name: '待办标题' }));
    await user.type(screen.getByRole('textbox', { name: '待办标题' }), '已经持久化的标题');
    const update = vi.spyOn(bridge, 'updateRecord');
    vi.spyOn(bridge, 'snapshot').mockRejectedValueOnce(new Error('刷新故障'));
    await user.keyboard('{Escape}');
    expect(await screen.findByText(/修改已保存，但刷新失败/)).toBeVisible();
    await waitFor(() => expect(screen.queryByRole('textbox', { name: '待办标题' })).not.toBeInTheDocument());
    expect((await bridge.snapshot()).records[0].title).toBe('已经持久化的标题');
    expect(update).toHaveBeenCalledTimes(1);
  });

  it('waits for the title before archiving and submits it only once', async () => {
    await bridge.createRecord('归档顺序', '归档顺序', null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /^归档顺序/ }));
    await user.clear(screen.getByRole('textbox', { name: '待办标题' }));
    await user.type(screen.getByRole('textbox', { name: '待办标题' }), '归档前保存');
    let finish!: () => void;
    const waiting = new Promise<void>(resolve => { finish = resolve; });
    const original = bridge.updateRecord.bind(bridge);
    const update = vi.spyOn(bridge, 'updateRecord').mockImplementationOnce(async (id, patch) => { await waiting; return original(id, patch); });
    const archive = vi.spyOn(bridge, 'archiveRecord');
    await user.click(screen.getByRole('button', { name: /^归档$/ }));
    expect(update).toHaveBeenCalledTimes(1); expect(archive).not.toHaveBeenCalled();
    await act(async () => { finish(); });
    await waitFor(() => expect(archive).toHaveBeenCalledTimes(1));
    expect((await bridge.snapshot()).records[0]).toMatchObject({ title: '归档前保存', status: 'archived' });
    expect(update).toHaveBeenCalledTimes(1);
  });

  it('updates date groups and labels across midnight without loading another snapshot', async () => {
    vi.useFakeTimers(); vi.setSystemTime(new Date(2030, 0, 2, 23, 59, 59));
    await bridge.createRecord('跨日任务', '跨日任务', '2030-01-02', 'normal');
    await bridge.createRecord('次日任务', '次日任务', '2030-01-03', 'normal');
    const snapshot = vi.spyOn(bridge, 'snapshot');
    let view!: ReturnType<typeof render>;
    await act(async () => { view = render(<App />); });
    expect(screen.getByText('今天到期')).toBeVisible();
    expect(screen.getByText('明天到期')).toBeVisible();
    const reads = snapshot.mock.calls.length;
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(screen.getByText('已过期 1 天')).toBeVisible();
    expect(screen.getByText('今天到期')).toBeVisible();
    expect(screen.queryByText('明天到期')).not.toBeInTheDocument();
    expect(screen.getByText('已过期', { selector: '.group-heading' })).toBeVisible();
    expect(snapshot).toHaveBeenCalledTimes(reads);
    view.unmount();
  });

  it('submits independent inline fields as patches and retains failed titles', async () => {
    await bridge.createRecord('原始标题', '原始标题', null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /^原始标题/ }));
    const update = vi.spyOn(bridge, 'updateRecord');
    const title = screen.getByRole('textbox', { name: '待办标题' });
    await user.clear(title); await user.type(title, '连续修改');
    await user.click(within(screen.getByRole('group', { name: '优先级' })).getByRole('button', { name: '优先' }));
    await waitFor(async () => expect((await bridge.snapshot()).records[0]).toMatchObject({ title: '连续修改', priority: 'high' }));
    expect(update.mock.calls.map(call => call[1])).toEqual([{ title: '连续修改' }, { priority: 'high' }]);
    update.mockRejectedValueOnce(new Error('标题保存失败'));
    await user.clear(title); await user.type(title, '保留未保存标题{Enter}');
    expect(await screen.findByText('标题保存失败')).toBeVisible();
    expect(title).toHaveValue('保留未保存标题');
  });
  it('quick-add keeps failed input and does not repeat a successful create after refresh failure', async () => {
    const user = userEvent.setup(); render(<App />);
    const input = await screen.findByRole('textbox', { name: '添加待办' });
    const create = vi.spyOn(bridge, 'createRecord').mockRejectedValueOnce(new Error('创建失败'));
    await user.type(input, '安全新增{Enter}');
    expect(await screen.findByText('创建失败')).toBeVisible(); expect(input).toHaveValue('安全新增');
    vi.spyOn(bridge, 'snapshot').mockRejectedValueOnce(new Error('读回失败'));
    await user.keyboard('{Enter}');
    expect(await screen.findByText(/已添加，但刷新失败/)).toBeVisible();
    expect(input).toHaveValue(''); expect(create).toHaveBeenCalledTimes(2);
  });
  it('shows an honest empty state and explicit preview limitations', async () => {
    render(<App />);
    expect(await screen.findByRole('heading', { name: '还没有待办' })).toBeVisible();
    expect(screen.getByText('未连接微信')).toBeVisible();
    expect(screen.getByText(/只保存在当前标签页/)).toBeVisible();
  });
  it('supports creating, editing, archiving and undoing a local record', async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('button', { name: /新建待办/ }));
    await user.type(screen.getByRole('textbox', { name: '待办内容' }), '完成界面验证');
    await user.click(screen.getByRole('button', { name: '保存' }));
    await user.click(await screen.findByRole('button', { name: /^完成界面验证/ }));
    const title = screen.getByRole('textbox', { name: '待办标题' });
    await user.clear(title); await user.type(title, '验证修改结果{Enter}');
    expect(await screen.findByRole('button', { name: /^验证修改结果/ })).toBeVisible();
    await user.click(within(screen.getByRole('group', { name: '优先级' })).getByRole('button', { name: '优先' }));
    await waitFor(async () => expect((await bridge.snapshot()).records[0]).toMatchObject({ title: '验证修改结果', priority: 'high' }));
    await user.click(screen.getByRole('button', { name: '归档' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: /^验证修改结果/ })).not.toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '撤销' }));
    expect(await screen.findByRole('button', { name: /^验证修改结果/ })).toBeVisible();
  });
  it('searches without changing persisted records', async () => {
    await bridge.createRecord('买一本书', '买一本书', null, 'normal');
    await bridge.createRecord('整理项目', '整理项目', null, 'normal');
    render(<App />);
    const input = await screen.findByRole('textbox', { name: '搜索记录' });
    fireEvent.change(input, { target: { value: '买书没有' } });
    expect(await screen.findByRole('heading', { name: '没有找到相关待办' })).toBeVisible();
    expect((await bridge.snapshot()).records).toHaveLength(2);
  });
  it('shows a newly saved record even when the old search does not match', async () => {
    const user = userEvent.setup(); render(<App />);
    await user.type(await screen.findByRole('textbox', { name: '搜索记录' }), '旧关键词');
    await user.click(screen.getByRole('button', { name: /新建待办/ }));
    await user.type(screen.getByRole('textbox', { name: '待办内容' }), '新增后应当可见');
    await user.click(screen.getByRole('button', { name: '保存' }));
    expect(await screen.findByRole('button', { name: /^新增后应当可见/ })).toBeVisible();
    expect(screen.getByRole('textbox', { name: '搜索记录' })).toHaveValue('');
  });
  it('archives from row actions and clears the archive in one confirmation', async () => {
    for (const title of ['第一条', '第二条', '第三条']) await bridge.createRecord(title, title, null, 'normal');
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: '归档：第一条' }));
    await user.click(screen.getByRole('button', { name: '批量选择' }));
    await user.click(screen.getByRole('button', { name: '全选' }));
    expect(screen.getByText('已选 2 项')).toBeVisible();
    await user.click(within(screen.getByRole('toolbar', { name: '批量操作' })).getByRole('button', { name: '归档' }));
    await waitFor(async () => expect((await bridge.snapshot()).records.every(record => record.status === 'archived')).toBe(true));
    await user.click(within(screen.getByRole('group', { name: '记录筛选' })).getByRole('button', { name: /已归档/ }));
    await user.click(screen.getByRole('button', { name: '清空归档' }));
    expect(screen.getByRole('dialog')).toHaveAccessibleName('永久删除 3 条待办？');
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: '永久删除' }));
    await waitFor(async () => expect((await bridge.snapshot()).records).toHaveLength(0));
  });
  it('does not let global search shortcuts steal record form focus', async () => {
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /新建待办/ }));
    await user.keyboard('{Control>}k{/Control}');
    expect(screen.getByRole('textbox', { name: '待办内容' })).toHaveFocus();
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
  });
  it('keeps real WeChat login desktop-only and offers working runtime settings', async () => {
    const user = userEvent.setup(); render(<App />);
    const nav = await screen.findByRole('navigation', { name: '主导航' });
    await user.click(within(nav).getByRole('button', { name: '连接' }));
    expect(screen.getByText(/浏览器预览不能连接微信/)).toBeVisible();
    expect(screen.getByRole('button', { name: '连接微信' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '获取模型' })).toBeDisabled();
    expect(screen.getByRole('switch', { name: '自动识别微信消息' })).toBeVisible();
    await user.click(within(nav).getByRole('button', { name: '设置' }));
    expect(screen.getByRole('switch', { name: '开机自动启动' })).toBeVisible();
    expect(screen.getByRole('switch', { name: '桌面待办面板' })).toBeVisible();
    expect(screen.getByRole('switch', { name: '到期提醒' })).toBeVisible();
    expect(screen.getByRole('combobox', { name: '归档保留' })).toBeVisible();
  });
  it('defaults to unlock and startup reminders and saves the repeat interval in hours', async () => {
    const user = userEvent.setup(); render(<App />);
    await user.click(within(await screen.findByRole('navigation', { name: '主导航' })).getByRole('button', { name: '设置' }));
    expect(screen.getByRole('switch', { name: '解锁电脑时提醒' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('switch', { name: '启动拾笺时提醒' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('switch', { name: '定时重复提醒' })).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByRole('combobox', { name: '解锁提醒方式' })).toHaveValue('first_of_day');
    await user.selectOptions(screen.getByRole('combobox', { name: '解锁提醒方式' }), 'every_unlock');
    expect((await bridge.snapshot()).settings.unlockReminder).toBe('every_unlock');
    const interval = screen.getByRole('textbox', { name: '重复间隔（小时）' });
    expect(interval).toBeDisabled();
    await user.click(screen.getByRole('switch', { name: '定时重复提醒' }));
    await waitFor(() => expect(interval).toBeEnabled());
    await user.clear(interval); await user.type(interval, '1.5{Enter}');
    await waitFor(async () => expect((await bridge.snapshot()).settings.remindIntervalMinutes).toBe(90));
    await user.clear(interval); await user.type(interval, '0.1{Enter}');
    expect(await screen.findByText(/0.25 到 24/)).toBeVisible();
    expect(interval).toHaveValue('1.5');
    // 总开关关闭后，各触发方式都不可调，避免留下看似生效的设置。
    await user.click(screen.getByRole('switch', { name: '到期提醒' }));
    await waitFor(() => expect(screen.getByRole('switch', { name: '解锁电脑时提醒' })).toBeDisabled());
    expect((await bridge.snapshot()).settings.remindersEnabled).toBe(false);
  });
  it('saves a due time with the date and clears it with the date', async () => {
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /新建待办/ }));
    await user.type(screen.getByRole('textbox', { name: '待办内容' }), '下午开会');
    expect(screen.getByLabelText('截止时间')).toBeDisabled();
    fireEvent.change(screen.getByLabelText('截止日期'), { target: { value: '2030-01-02' } });
    fireEvent.change(screen.getByLabelText('截止时间'), { target: { value: '15:00' } });
    await user.click(screen.getByRole('button', { name: '保存' }));
    expect(await screen.findByRole('button', { name: /^下午开会.*15:00/ })).toBeVisible();
    expect((await bridge.snapshot()).records[0]).toMatchObject({ dueDate: '2030-01-02', dueTime: '15:00' });
  });
  it('completes, reopens and permanently deletes a local record', async () => {
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: /新建待办/ }));
    await user.type(screen.getByRole('textbox', { name: '待办内容' }), '完成删除验证');
    await user.click(screen.getByRole('button', { name: '保存' }));
    await user.click(await screen.findByRole('button', { name: /^完成删除验证/ }));
    await user.click(screen.getByRole('button', { name: '标记完成' }));
    const tabs = () => screen.getByRole('group', { name: '记录筛选' });
    await user.click(within(tabs()).getByRole('button', { name: /已完成/ }));
    expect(await screen.findByRole('button', { name: /^完成删除验证/ })).toBeVisible();
    expect((await bridge.snapshot()).records[0].status).toBe('completed');
    await user.click(screen.getByRole('button', { name: /^完成删除验证/ }));
    await user.click(screen.getByRole('button', { name: '重新打开' }));
    await user.click(within(tabs()).getByRole('button', { name: /进行中/ }));
    expect(await screen.findByRole('button', { name: /^完成删除验证/ })).toBeVisible();
    expect((await bridge.snapshot()).records[0].status).toBe('saved');
    await user.click(screen.getByRole('button', { name: /^完成删除验证/ }));
    await user.click(screen.getByRole('button', { name: '永久删除' }));
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: '永久删除' }));
    expect(await screen.findByRole('heading', { name: '还没有待办' })).toBeVisible();
    expect((await bridge.snapshot()).records).toHaveLength(0);
  });
});
