import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { bridge } from './bridge';
import { defaultSettings, disconnectedWechat } from './types';
import type { AppSnapshot, WechatSnapshot } from './types';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('./bridge', async importOriginal => {
  const actual = await importOriginal<typeof import('./bridge')>();
  return {
    ...actual, isDesktop: true,
    bridge: {
      ...actual.bridge, snapshot: vi.fn(), startWechatLogin: vi.fn(), submitWechatCode: vi.fn(),
      disconnectWechat: vi.fn(), retryWechatReplies: vi.fn(), saveSettings: vi.fn(),
      testModel: vi.fn(), discoverModels: vi.fn(), modelReasoningOptions: vi.fn(), saveModelKey: vi.fn(), quit: vi.fn(), deleteModelKey: vi.fn(),
    },
  };
});

let state: AppSnapshot;
function setWechat(update: Partial<WechatSnapshot>) {
  state.wechat = { ...state.wechat, ...update };
  state.wechatStatus = state.wechat.status;
}
async function openConnections() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(within(await screen.findByRole('navigation')).getByRole('button', { name: '连接' }));
  return user;
}

beforeEach(() => {
  vi.clearAllMocks();
  state = {
    settings: { ...defaultSettings }, records: [], hasModelKey: false,
    runtime: 'desktop', dataDir: 'test-directory', wechatStatus: 'not_connected', wechat: { ...disconnectedWechat },
  };
  vi.mocked(bridge.snapshot).mockImplementation(async () => structuredClone(state));
  vi.mocked(bridge.saveSettings).mockImplementation(async patch => { state.settings = { ...state.settings, ...patch }; return state.settings; });
  vi.mocked(bridge.modelReasoningOptions).mockImplementation(async (url, model) => url === 'https://api.deepseek.com' && ['deepseek-flash', 'deepseek-v4-pro'].includes(model) ? ['default', 'none', 'low', 'high', 'max'] : ['default']);
  vi.mocked(bridge.discoverModels).mockResolvedValue({ message: '已获取 2 个模型', models: ['deepseek-flash', 'deepseek-v4-pro'] });
});

describe('desktop WeChat connection controls', () => {
  it('displays only a QR returned by the native login command and allows cancellation', async () => {
    vi.mocked(bridge.startWechatLogin).mockImplementation(async () => {
      setWechat({ status: 'waiting_scan', message: '请用手机微信扫码', qrCodeSvg: '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect width="1" height="1"/></svg>', qrExpiresAt: '2026-10-23T23:00:00Z' });
      return state.wechat;
    });
    vi.mocked(bridge.disconnectWechat).mockImplementation(async () => { setWechat({ ...disconnectedWechat }); });
    const user = await openConnections();
    expect(screen.queryByRole('img', { name: /微信连接二维码/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '连接微信' }));
    expect(await screen.findByRole('img', { name: /微信连接二维码/ })).toHaveAttribute('src', expect.stringContaining('data:image/svg+xml'));
    expect(bridge.startWechatLogin).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: '取消连接' }));
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: '确认断开' }));
    expect(await screen.findByRole('button', { name: '连接微信' })).toBeVisible();
    expect(screen.queryByRole('img', { name: /微信连接二维码/ })).not.toBeInTheDocument();
  });
  it('submits a phone verification code without claiming connection success', async () => {
    setWechat({ status: 'need_verifycode', message: '请输入手机验证码' });
    vi.mocked(bridge.submitWechatCode).mockImplementation(async () => { setWechat({ status: 'scanned', message: '等待手机确认' }); });
    const user = await openConnections();
    const button = screen.getByRole('button', { name: '提交验证码' });
    expect(button).toBeDisabled();
    await user.type(screen.getByRole('textbox', { name: '微信验证码' }), '123456');
    await user.click(button);
    expect(bridge.submitWechatCode).toHaveBeenCalledWith('123456');
    expect((await screen.findAllByText('等待手机确认'))[0]).toBeVisible();
    expect(screen.queryByText('微信已连接')).not.toBeInTheDocument();
  });
  it('shows request errors and never fabricates a QR or connected state', async () => {
    vi.mocked(bridge.startWechatLogin).mockRejectedValue(new Error('微信服务连接超时'));
    const user = await openConnections();
    await user.click(screen.getByRole('button', { name: '连接微信' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('微信服务连接超时');
    expect(screen.queryByRole('img', { name: /微信连接二维码/ })).not.toBeInTheDocument();
    expect(screen.queryByText('微信已连接')).not.toBeInTheDocument();
  });
  it('keeps expired QR hidden and offers an explicit refresh', async () => {
    setWechat({ status: 'expired', message: '二维码已过期，请刷新', qrCodeSvg: '<svg/>' });
    await openConnections();
    expect(screen.getByRole('button', { name: '刷新二维码' })).toBeEnabled();
    expect(screen.queryByRole('img', { name: /微信连接二维码/ })).not.toBeInTheDocument();
  });
  it('shows failed receipts separately and retries only when not paused', async () => {
    setWechat({ status: 'connected', accountId: 'test-bot', pendingReplies: 1, failedReplies: 2, message: '微信接收中' });
    const user = await openConnections();
    expect(screen.getByText(/2 条微信回复发送失败，1 条等待发送/)).toBeVisible();
    await user.click(screen.getByRole('button', { name: '重新发送' }));
    expect(bridge.retryWechatReplies).toHaveBeenCalledTimes(1);
  });
  it('disables receipt retry while receiving and sending are paused', async () => {
    state.settings.paused = true;
    setWechat({ status: 'paused', accountId: 'test-bot', failedReplies: 2, message: '收发已暂停' });
    await openConnections();
    expect(screen.getByRole('button', { name: '重新发送' })).toBeDisabled();
  });
  it('allows clearing damaged stored credentials even when no account ID could be restored', async () => {
    setWechat({ status: 'error', accountId: null, message: '微信凭据损坏，请断开后重新扫码' });
    const user = await openConnections();
    await user.click(screen.getByRole('button', { name: '断开连接' }));
    expect(screen.getByRole('dialog')).toHaveAccessibleName('断开微信连接？');
    expect(within(screen.getByRole('dialog')).getByRole('button', { name: '确认断开' })).toBeEnabled();
  });
  it('shows disconnect errors inside the confirmation instead of behind its backdrop', async () => {
    setWechat({ status: 'connected', accountId: 'test-bot' });
    vi.mocked(bridge.disconnectWechat).mockRejectedValueOnce(new Error('无法清除本应用微信凭据'));
    const user = await openConnections();
    await user.click(screen.getByRole('button', { name: '断开连接' }));
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: '确认断开' }));
    expect(await within(screen.getByRole('dialog')).findByRole('alert')).toHaveTextContent('无法清除本应用微信凭据');
    expect(screen.getAllByRole('alert')).toHaveLength(1);
  });
  it('does not open record forms or navigate behind disconnect confirmation', async () => {
    setWechat({ status: 'connected', accountId: 'test-bot' });
    const user = await openConnections();
    await user.click(screen.getByRole('button', { name: '断开连接' }));
    await user.keyboard('{Control>}n{/Control}{Control>}k{/Control}');
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.getByRole('dialog')).toHaveAccessibleName('断开微信连接？');
    expect(screen.queryByRole('textbox', { name: '记下什么' })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { level: 1, name: '连接' })).toBeVisible();
  });
});
