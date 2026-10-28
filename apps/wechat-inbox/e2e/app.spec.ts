import { expect, test } from '@playwright/test';

test('empty state, local CRUD, undo, and no runtime errors', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByRole('heading', { name: '还没有待办' })).toBeVisible();
  await page.screenshot({ path: '.artifacts/home.png', fullPage: true });
  await page.getByRole('button', { name: /新建待办/ }).click();
  await page.getByRole('textbox', { name: '待办内容' }).fill('验证拾笺本地记录');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await page.getByRole('button', { name: /^验证拾笺本地记录/ }).click();
  await page.getByRole('textbox', { name: '待办标题' }).fill('修改后仍然保留原始消息');
  await page.getByRole('textbox', { name: '待办标题' }).press('Enter');
  await expect(page.getByRole('button', { name: /^修改后仍然保留原始消息/ })).toBeVisible();
  await page.screenshot({ path: '.artifacts/detail.png' });
  await expect(page.getByText('验证拾笺本地记录', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: '归档', exact: true }).click();
  await expect(page.getByRole('button', { name: '撤销', exact: true })).toBeVisible();
  await page.getByRole('button', { name: '撤销', exact: true }).click();
  await expect(page.getByRole('button', { name: /^修改后仍然保留原始消息/ })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('button', { name: /^修改后仍然保留原始消息/ })).toBeVisible();
  expect(errors).toEqual([]);
});

test('connection limitations and settings', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('navigation').getByRole('button', { name: '连接', exact: true }).click();
  await expect(page.getByText(/浏览器预览不能连接微信/)).toBeVisible();
  await expect(page.getByRole('button', { name: '连接微信', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: '获取模型' })).toBeDisabled();
  const model = page.getByRole('region', { name: '模型配置' });
  await expect(model.getByRole('switch', { name: '自动识别微信消息' })).toBeVisible();
  const modelForm = model.getByRole('button', { name: '保存配置' }).locator('xpath=ancestor::form');
  await expect(modelForm.getByRole('combobox', { name: '思考强度' })).toBeDisabled();
  await expect(model.getByRole('button', { name: '保存配置' })).toBeDisabled();
  await model.getByRole('button', { name: '手动填写模型 ID' }).click();
  await model.getByRole('textbox', { name: '模型 ID', exact: true }).fill('preview-only-test-model');
  await expect(model.getByText('未保存', { exact: true })).toBeVisible();
  await expect(model.getByRole('button', { name: '保存配置' })).toBeEnabled();
  await model.getByRole('button', { name: '保存配置' }).click();
  await expect(model.getByRole('button', { name: '保存配置' })).toBeDisabled();
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1080, height: 740 });
  await page.screenshot({ path: '.artifacts/connections.png', fullPage: true });
  await page.getByRole('navigation').getByRole('button', { name: '设置', exact: true }).click();
  await expect(page.getByRole('switch', { name: '开机自动启动' })).toBeVisible();
  await expect(page.getByRole('switch', { name: '桌面待办面板' })).toBeVisible();
  await expect(page.getByRole('switch', { name: '到期提醒' })).toBeVisible();
  const unlock = page.getByRole('combobox', { name: '解锁提醒方式' });
  await expect(unlock).toHaveValue('first_of_day');
  await unlock.selectOption('every_unlock');
  await expect(page.getByText('设置已保存', { exact: true })).toBeVisible();
  await page.getByRole('switch', { name: '定时重复提醒' }).click();
  const interval = page.getByRole('textbox', { name: '重复间隔（小时）' });
  await expect(interval).toBeEnabled();
  await interval.fill('1.5');
  await interval.press('Enter');
  await expect(interval).toHaveValue('1.5');
  await page.getByRole('switch', { name: '到期提醒' }).click();
  await expect(unlock).toBeDisabled();
  await page.getByRole('switch', { name: '到期提醒' }).click();
  await expect(unlock).toBeEnabled();
  await expect(unlock).toHaveValue('every_unlock');
  await expect(page.getByRole('combobox', { name: '归档保留' })).toBeVisible();
  await page.screenshot({ path: '.artifacts/settings.png', fullPage: true });
});

test('saving with an unmatched search returns to a visible saved record', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('textbox', { name: '搜索记录' }).fill('不匹配的旧搜索');
  await page.getByRole('button', { name: /新建待办/ }).click();
  await page.getByRole('textbox', { name: '待办内容' }).fill('搜索状态下新增的记录');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('textbox', { name: '搜索记录' })).toHaveValue('');
  await expect(page.getByRole('button', { name: /^搜索状态下新增的记录/ })).toBeVisible();
});

test('new-record shortcut from settings returns to inbox after saving', async ({ page }) => {
  await page.goto('/');
  await page.getByRole('navigation').getByRole('button', { name: '设置', exact: true }).click();
  await page.keyboard.press('Control+n');
  await page.getByRole('textbox', { name: '待办内容' }).fill('在设置页通过快捷键新增');
  await page.keyboard.press('Control+k');
  await expect(page.getByRole('textbox', { name: '待办内容' })).toBeFocused();
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: '待办' })).toBeVisible();
  await expect(page.getByRole('button', { name: /^在设置页通过快捷键新增/ })).toBeVisible();
});

test('narrow window remains usable without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 780, height: 560 });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: '还没有待办' })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: '.artifacts/narrow.png', fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: '.artifacts/mobile.png', fullPage: true });
});
