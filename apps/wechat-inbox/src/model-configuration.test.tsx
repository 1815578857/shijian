import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import ModelConfiguration from './components/ModelConfiguration';
import { bridge } from './bridge';
import { defaultSettings, disconnectedWechat } from './types';
import type { AppSnapshot } from './types';

vi.mock('./bridge', async importOriginal => {
  const actual = await importOriginal<typeof import('./bridge')>();
  return { ...actual, isDesktop: true, bridge: { ...actual.bridge, snapshot: vi.fn(), saveSettings: vi.fn(), discoverModels: vi.fn(), modelReasoningOptions: vi.fn(), saveModelKey: vi.fn(), deleteModelKey: vi.fn() } };
});
let state: AppSnapshot;
const notify = vi.fn();
async function openModel() {
  const user = userEvent.setup();
  render(<ModelConfiguration snapshot={structuredClone(state)} refresh={async () => {}} notify={notify} />);
  await screen.findByRole('region', { name: '模型配置' });
  return user;
}
beforeEach(() => {
  vi.resetAllMocks();
  state = { settings: { ...defaultSettings }, records: [], hasModelKey: false, runtime: 'desktop', dataDir: 'test-directory', wechatStatus: 'not_connected', wechat: { ...disconnectedWechat } };
  vi.mocked(bridge.snapshot).mockImplementation(async () => structuredClone(state));
  vi.mocked(bridge.saveSettings).mockImplementation(async patch => { state.settings = { ...state.settings, ...patch }; return state.settings; });
  vi.mocked(bridge.modelReasoningOptions).mockImplementation(async (url, model) => url === 'https://api.deepseek.com' && ['deepseek-flash', 'deepseek-v4-pro'].includes(model) ? ['default', 'none', 'low', 'high', 'max'] : ['default']);
  vi.mocked(bridge.discoverModels).mockResolvedValue({ message: '已获取 2 个模型', models: ['deepseek-flash', 'deepseek-v4-pro'] });
});

describe('model draft workflow', () => {
  it('distinguishes unreadable credentials from missing keys and supports recovery', async () => {
    state.settings.modelBaseUrl = 'https://api.deepseek.com';
    state.modelCredentialError = '凭据读取失败';
    const user = await openModel();
    expect(screen.getByText(/不代表密钥未保存/)).toBeVisible();
    expect(screen.getByRole('button', { name: '获取模型' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '删除密钥' })).toBeVisible();
    await user.type(screen.getByLabelText(/API 密钥/), 'replacement-key');
    expect(screen.getByRole('button', { name: '获取模型' })).toBeEnabled();
  });
  it('shows one selection workflow, not duplicate model fields', async () => {
    await openModel();
    expect(screen.getByText('兼容 OpenAI 接口的服务地址')).toBeVisible();
    expect(screen.getByRole('button', { name: '获取模型' })).toBeDisabled();
    expect(screen.queryByRole('textbox', { name: '模型名称' })).not.toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: '思考强度' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '保存配置' })).toBeDisabled();
  });
  it('fetches unsaved credentials, then saves model and effort together', async () => {
    const user = await openModel();
    await user.type(screen.getByRole('textbox', { name: 'API 地址' }), 'https://api.deepseek.com');
    await user.type(screen.getByLabelText(/API 密钥/), 'draft-test-key');
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    expect(bridge.discoverModels).toHaveBeenCalledWith({ baseUrl: 'https://api.deepseek.com', apiKey: 'draft-test-key', useSavedKey: false });
    expect(bridge.saveSettings).not.toHaveBeenCalled();
    expect(bridge.saveModelKey).not.toHaveBeenCalled();
    await user.type(await screen.findByRole('combobox', { name: '搜索模型' }), 'v4');
    await user.keyboard('{Enter}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '使用模型' })).toBeVisible();
    await user.selectOptions(screen.getByRole('combobox', { name: '思考强度' }), 'max');
    expect(bridge.saveSettings).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    expect(bridge.saveModelKey).toHaveBeenCalledWith('draft-test-key', 'https://api.deepseek.com');
    expect(bridge.saveSettings).toHaveBeenCalledWith({ modelBaseUrl: 'https://api.deepseek.com', modelName: 'deepseek-v4-pro', reasoningEffort: 'max' });
  });
  it('uses a saved key only for the address it was saved with', async () => {
    state.hasModelKey = true; state.settings.modelBaseUrl = 'https://api.deepseek.com'; state.settings.modelName = 'deepseek-flash';
    const user = await openModel();
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    expect(bridge.discoverModels).toHaveBeenLastCalledWith({ baseUrl: 'https://api.deepseek.com', apiKey: undefined, useSavedKey: true });
    await screen.findByRole('dialog'); await user.keyboard('{Escape}');
    const address = screen.getByRole('textbox', { name: 'API 地址' });
    await user.clear(address); await user.type(address, 'http://127.0.0.1:1234/v1');
    expect(screen.getByText('更换地址后需要重新填写密钥')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    expect(bridge.discoverModels).toHaveBeenLastCalledWith({ baseUrl: 'http://127.0.0.1:1234/v1', apiKey: undefined, useSavedKey: false });
  });
  it('invalidates fetched models on key edits', async () => {
    state.settings.modelBaseUrl = 'https://api.deepseek.com';
    const user = await openModel();
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    await screen.findByRole('dialog'); await user.keyboard('{Escape}');
    expect(screen.getByText('已获取 2 个模型')).toBeVisible();
    await user.type(screen.getByLabelText(/API 密钥/), 'new-key');
    expect(screen.queryByText('已获取 2 个模型')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '使用模型' })).toBeDisabled();
  });
  it('locks fetch controls and restores draft on errors', async () => {
    state.settings.modelBaseUrl = 'https://api.deepseek.com';
    let fail!: (reason: Error) => void;
    vi.mocked(bridge.discoverModels).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
    const user = await openModel();
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    expect(screen.getByRole('textbox', { name: 'API 地址' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '正在获取…' })).toBeDisabled();
    fail(new Error('模型服务返回 HTTP 401'));
    expect(await screen.findByRole('alert')).toHaveTextContent('HTTP 401');
    expect(screen.getByRole('textbox', { name: 'API 地址' })).toBeEnabled();
    expect(screen.getByRole('textbox', { name: 'API 地址' })).toHaveValue('https://api.deepseek.com');
  });
  it('allows empty-list manual IDs with default reasoning', async () => {
    state.settings.modelBaseUrl = 'https://proxy.example.com/v1';
    vi.mocked(bridge.discoverModels).mockResolvedValueOnce({ message: '未获取到可用模型，可手动填写模型 ID', models: [] });
    const user = await openModel();
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    expect(await screen.findByText('未获取到可用模型，可手动填写模型 ID')).toBeVisible();
    await user.click(screen.getByRole('button', { name: '手动填写模型 ID' }));
    await user.type(screen.getByRole('textbox', { name: '模型 ID' }), 'private-model');
    expect(screen.getByRole('combobox', { name: '思考强度' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    expect(bridge.saveSettings).toHaveBeenCalledWith(expect.objectContaining({ modelName: 'private-model', reasoningEffort: 'default' }));
  });
  it('normalizes legacy effort only on save and merges latest unrelated settings', async () => {
    state.settings.modelBaseUrl = 'https://api.deepseek.com'; state.settings.modelName = 'deepseek-flash'; state.settings.reasoningEffort = 'medium';
    const user = await openModel();
    expect(await screen.findByText(/“中”档已不适用/)).toBeVisible();
    expect(bridge.saveSettings).not.toHaveBeenCalled();
    state.settings.parsingEnabled = true; state.settings.paused = true;
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    expect(bridge.saveSettings).toHaveBeenCalledWith({ modelBaseUrl: 'https://api.deepseek.com', modelName: 'deepseek-flash', reasoningEffort: 'default' });
    expect(state.settings).toMatchObject({ parsingEnabled: true, paused: true });
    expect(bridge.snapshot).not.toHaveBeenCalled();
  });
  it('reports partial save failure and retains the entered key for retry', async () => {
    state.settings.modelBaseUrl = 'https://api.deepseek.com'; state.settings.modelName = 'deepseek-flash';
    vi.mocked(bridge.saveSettings).mockRejectedValueOnce(new Error('配置保存失败'));
    const user = await openModel();
    await user.type(screen.getByLabelText(/API 密钥/), 'draft-retry-key');
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('密钥已保存，但其他配置保存失败');
    expect(screen.getByLabelText(/API 密钥/)).toHaveValue('draft-retry-key');
    expect(notify).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: '保存配置' }));
    expect(notify).toHaveBeenCalledWith({ text: '模型配置已保存', tone: 'success' });
  });
  it('ignores a pending discovery after the credential snapshot changes', async () => {
    state.settings.modelBaseUrl = 'https://api.deepseek.com';
    let finish!: (result: { message: string; models: string[] }) => void;
    vi.mocked(bridge.discoverModels).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    const user = userEvent.setup();
    const view = render(<ModelConfiguration snapshot={structuredClone(state)} refresh={async () => {}} notify={notify} />);
    await user.click(screen.getByRole('button', { name: '获取模型' }));
    state.hasModelKey = true;
    view.rerender(<ModelConfiguration snapshot={structuredClone(state)} refresh={async () => {}} notify={notify} />);
    finish({ message: '过时的返回结果', models: ['stale-model'] });
    await screen.findByRole('button', { name: '获取模型' });
    expect(screen.queryByText('过时的返回结果')).not.toBeInTheDocument();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it('keeps deletion errors and keyboard focus in confirmation', async () => {
    state.hasModelKey = true;
    vi.mocked(bridge.deleteModelKey).mockRejectedValueOnce(new Error('无法删除密钥'));
    const user = await openModel();
    await user.click(screen.getByRole('button', { name: '删除密钥' }));
    const dialog = screen.getByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: '删除密钥' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('无法删除密钥');
    expect(within(dialog).getByRole('button', { name: '删除密钥' })).toHaveFocus();
  });
});
