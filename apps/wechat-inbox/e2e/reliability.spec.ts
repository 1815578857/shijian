import { expect, test } from '@playwright/test';

test('Escape saves the title before closing the detail', async ({ page }) => {
  await page.goto('/');
  const quickAdd = page.getByRole('textbox', { name: '添加待办', exact: true });
  await quickAdd.fill('关闭前的标题'); await quickAdd.press('Enter');
  await page.getByRole('button', { name: /^关闭前的标题/ }).click();
  await page.getByRole('textbox', { name: '待办标题' }).fill('Esc 后也要保留');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('textbox', { name: '待办标题' })).toHaveCount(0);
  await page.getByRole('button', { name: /^Esc 后也要保留/ }).click();
  await expect(page.getByRole('textbox', { name: '待办标题' })).toHaveValue('Esc 后也要保留');
  await page.reload();
  await expect(page.getByRole('button', { name: /^Esc 后也要保留/ })).toBeVisible();
});

test('main window and panel share due ordering and refresh across midnight', async ({ page }) => {
  await page.clock.install({ time: new Date(2030, 0, 2, 23, 59, 59) });
  await page.addInitScript(() => {
    const common = { originalText: '隔离的回归测试', status: 'saved', source: 'local', dueDate: '2030-01-02', priority: 'normal', error: null, isDemo: false };
    sessionStorage.setItem('shijian-preview-v1', JSON.stringify({ records: [
      { ...common, id: 'late', title: '下午任务', dueTime: '16:00', createdAt: '2030-01-02T02:00:00Z' },
      { ...common, id: 'date-only', title: '只有日期', dueTime: null, createdAt: '2030-01-02T03:00:00Z' },
      { ...common, id: 'undated', title: '没有日期', dueDate: null, dueTime: null, createdAt: '2030-01-02T04:00:00Z' },
      { ...common, id: 'early', title: '上午任务', dueTime: '09:00', createdAt: '2030-01-02T01:00:00Z' },
    ] }));
  });
  await page.goto('/');
  const ordered = ['上午任务', '下午任务', '只有日期', '没有日期'];
  await expect(page.locator('.record-title > span:first-child')).toHaveText(ordered);
  await expect(page.getByText('今天到期', { exact: true })).toBeVisible();
  await page.clock.runFor(1000);
  await expect(page.getByText('已过期 1 天', { exact: true })).toHaveCount(3);
  await expect(page.locator('.group-heading').filter({ hasText: '已过期' })).toBeVisible();

  await page.clock.setSystemTime(new Date(2030, 0, 2, 23, 59, 59));
  await page.goto('/?view=panel');
  await expect(page.locator('.panel-row-title')).toHaveText(ordered);
  await expect(page.getByText('今天到期', { exact: true })).toBeVisible();
  await page.clock.runFor(1000);
  await expect(page.getByText('已过期 1 天', { exact: true })).toHaveCount(3);
});
