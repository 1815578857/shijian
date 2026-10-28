import { expect, test } from '@playwright/test';
import { defaultSettings, disconnectedWechat } from '../src/types';

test('model draft discovery, keyboard choice, reasoning and responsive layout with mocked IPC', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(({ settings, wechat }) => {
    const globals = window as unknown as Record<string, unknown>;
    const state = { settings, wechat, records: [], hasModelKey: false, runtime: 'desktop', dataDir: '隔离的 UI 测试', wechatStatus: 'not_connected' };
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    globals.isTauri = true;
    globals.mockModelCalls = calls;
    globals.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    globals.__TAURI_INTERNALS__ = {
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: async (command: string, args: Record<string, unknown>) => {
        calls.push({ command, args });
        if (command === 'get_snapshot') return structuredClone(state);
        if (command.startsWith('plugin:event|')) return 1;
        if (command === 'model_reasoning_options') return args.baseUrl === 'https://api.deepseek.com' && ['deepseek-flash', 'deepseek-v4-pro'].includes(String(args.modelName)) ? ['default', 'none', 'low', 'high', 'max'] : ['default'];
        if (command === 'discover_models') return { message: '已获取 2 个模型', models: ['deepseek-flash', 'deepseek-v4-pro'] };
        if (command === 'save_model_key') { state.hasModelKey = true; return true; }
        if (command === 'save_settings') { state.settings = { ...state.settings, ...args.patch as Partial<typeof settings> }; return state.settings; }
        throw new Error(`Unexpected mock command: ${command}`);
      },
    };
  }, { settings: defaultSettings, wechat: disconnectedWechat });
  await page.goto('/');
  await page.getByRole('navigation').getByRole('button', { name: '连接', exact: true }).click();
  const region = page.getByRole('region', { name: '模型配置' });
  await region.getByRole('textbox', { name: 'API 地址' }).fill('https://api.deepseek.com');
  await region.getByLabel(/API 密钥/).fill('mock-key-not-real');
  await region.getByRole('button', { name: '获取模型' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.screenshot({ path: '.artifacts/model-picker.png', fullPage: true });
  const search = page.getByRole('combobox', { name: '搜索模型' });
  await expect(search).toBeFocused();
  await search.press('ArrowDown');
  await search.press('Enter');
  await expect(region.getByRole('button', { name: '使用模型' })).toHaveText('deepseek-v4-pro');
  await region.getByRole('combobox', { name: '思考强度' }).selectOption('high');
  const before = await page.evaluate(() => (window as unknown as { mockModelCalls: { command: string }[] }).mockModelCalls.map(call => call.command));
  expect(before).not.toContain('save_settings');
  expect(before).not.toContain('save_model_key');
  await region.getByRole('button', { name: '保存配置' }).click();
  await expect(region.getByText('已配置', { exact: true })).toBeVisible();
  await expect(region.getByRole('button', { name: '保存配置' })).toBeDisabled();
  await region.scrollIntoViewIfNeeded();
  await page.screenshot({ path: '.artifacts/model-configuration.png', fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await region.getByRole('button', { name: '获取模型' }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: '.artifacts/model-picker-narrow.png', fullPage: true });
  await page.keyboard.press('Escape');
  expect(errors).toEqual([]);
});
