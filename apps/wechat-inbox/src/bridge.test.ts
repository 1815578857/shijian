import { describe, expect, it } from 'vitest';
import { bridge, validateSettings } from './bridge';
import { defaultSettings } from './types';

describe('configuration boundaries', () => {
  it('allows HTTPS and local HTTP without embedded credentials', () => {
    for (const modelBaseUrl of ['', 'https://api.example.com/v1', 'http://localhost:1234/v1', 'http://127.0.0.1:1234/v1', 'http://[::1]:1234/v1']) {
      expect(() => validateSettings({ ...defaultSettings, modelBaseUrl })).not.toThrow();
    }
  });
  it('rejects unsafe URLs and unsupported destinations', () => {
    for (const modelBaseUrl of ['http://example.com/v1', 'https://user:secret@example.com', 'javascript:alert(1)', 'https://example.com?key=secret', 'https://example.com#key']) {
      expect(() => validateSettings({ ...defaultSettings, modelBaseUrl })).toThrow();
    }
    expect(() => validateSettings({ ...defaultSettings, destination: 'external' })).toThrow();
    expect(() => validateSettings({ ...defaultSettings, retentionDays: 0 })).toThrow();
  });
  it('never stores credentials or sends model requests from browser preview', async () => {
    await expect(bridge.saveModelKey('secret', 'https://example.com/v1')).rejects.toThrow('浏览器预览不保存密钥');
    await expect(bridge.testModel()).rejects.toThrow('浏览器预览不发送 API 请求');
    await expect(bridge.discoverModels({ baseUrl: 'https://example.com/v1', apiKey: 'secret', useSavedKey: false })).rejects.toThrow('浏览器预览不发送 API 请求');
    expect(await bridge.modelReasoningOptions('https://example.com/v1', 'test-model')).toEqual(['default']);
    await expect(bridge.startWechatLogin()).rejects.toThrow('浏览器预览不能连接微信');
    await expect(bridge.submitWechatCode('123456')).rejects.toThrow('请在桌面应用中');
    await expect(bridge.disconnectWechat()).rejects.toThrow('浏览器预览没有微信连接');
    await expect(bridge.retryWechatReplies()).rejects.toThrow('浏览器预览不能发送微信消息');
    await expect(bridge.togglePanel()).rejects.toThrow('桌面待办面板只在桌面应用中可用');
    await expect(bridge.openMainWindow()).rejects.toThrow('浏览器预览没有主窗口');
    expect(JSON.stringify(sessionStorage)).not.toContain('secret');
  });
});

describe('patch boundaries', () => {
  it('merges independent fields from old callers without overwriting', async () => {
    const record = await bridge.createRecord('原题', '原文', '2030-01-02', 'normal', '15:00');
    await Promise.all([
      bridge.updateRecord(record.id, { title: '新题' }),
      bridge.updateRecord(record.id, { priority: 'high' }),
    ]);
    expect((await bridge.snapshot()).records[0]).toMatchObject({ title: '新题', priority: 'high', dueDate: '2030-01-02', dueTime: '15:00' });
    await bridge.updateRecord(record.id, { dueDate: null });
    expect((await bridge.snapshot()).records[0]).toMatchObject({ dueDate: null, dueTime: null });
    await expect(bridge.updateRecord(record.id, { dueTime: '16:00' })).rejects.toThrow();
  });
  it('preserves unrelated settings and rejects hidden fields', async () => {
    await bridge.setPanelPinned(true);
    await bridge.saveSettings({ retentionDays: 90 });
    await bridge.saveSettings({ remindRepeat: true });
    expect((await bridge.snapshot()).settings).toMatchObject({ panelPinned: true, retentionDays: 90, remindRepeat: true });
    await expect(bridge.saveSettings({ panelBounds: null } as never)).rejects.toThrow('不支持');
  });
  it('keeps archive time stable until restore and protects inactive records', async () => {
    const record = await bridge.createRecord('归档', '归档', null, 'normal');
    await bridge.archiveRecord(record.id);
    const archivedAt = (await bridge.snapshot()).records[0].archivedAt;
    expect(archivedAt).toBeTruthy();
    await bridge.archiveRecord(record.id);
    expect((await bridge.snapshot()).records[0].archivedAt).toBe(archivedAt);
    await expect(bridge.updateRecord(record.id, { title: '不能隐式恢复' })).rejects.toThrow('先重新打开');
    await bridge.restoreRecord(record.id);
    expect((await bridge.snapshot()).records[0].archivedAt).toBeNull();
    await bridge.completeRecord(record.id, true);
    await expect(bridge.updateRecord(record.id, { priority: 'high' })).rejects.toThrow();
  });
});

describe('local record lifecycle', () => {
  it('starts empty, with no fake connection or seeded records', async () => {
    expect(await bridge.snapshot()).toMatchObject({ records: [], wechatStatus: 'not_connected', hasModelKey: false, runtime: 'preview' });
  });
  it('creates, updates, archives and restores using a stable ID', async () => {
    const created = await bridge.createRecord('  测试记录  ', '原始消息', '2028-02-29', 'normal');
    expect(created.title).toBe('测试记录');
    const edited = await bridge.updateRecord(created.id, { title: '修改后的记录', dueDate: null, priority: 'high' });
    expect(edited).toMatchObject({ id: created.id, originalText: '原始消息', title: '修改后的记录', priority: 'high' });
    await bridge.archiveRecord(created.id);
    expect((await bridge.snapshot()).records[0].status).toBe('archived');
    await bridge.restoreRecord(created.id);
    expect((await bridge.snapshot()).records[0].status).toBe('saved');
  });
  it('rejects empty titles and invalid calendar dates', async () => {
    await expect(bridge.createRecord(' ', '', null, 'normal')).rejects.toThrow();
    await expect(bridge.createRecord('日期', '日期', '2025-02-29', 'normal')).rejects.toThrow();
  });
  it('completes, reopens and permanently deletes records', async () => {
    const created = await bridge.createRecord('完成删除', '原始消息', null, 'normal');
    expect((await bridge.completeRecord(created.id, true)).status).toBe('completed');
    expect((await bridge.completeRecord(created.id, false)).status).toBe('saved');
    await expect(bridge.completeRecord('00000000-0000-4000-8000-000000000000', true)).rejects.toThrow('记录不存在');
    await bridge.deleteRecord(created.id);
    expect((await bridge.snapshot()).records).toHaveLength(0);
    await expect(bridge.deleteRecord(created.id)).rejects.toThrow('记录不存在');
  });
  it('restores archived records to their pre-archive status', async () => {
    const created = await bridge.createRecord('完成归档', '原始消息', null, 'normal');
    await bridge.completeRecord(created.id, true);
    await bridge.archiveRecord(created.id);
    expect((await bridge.snapshot()).records[0].status).toBe('archived');
    await bridge.restoreRecord(created.id);
    expect((await bridge.snapshot()).records[0].status).toBe('completed');
    // 旧版行为回归保护：未完成记录归档恢复后仍是待处理
    await bridge.archiveRecord(created.id);
    await expect(bridge.completeRecord(created.id, false)).rejects.toThrow('请先恢复');
    await bridge.restoreRecord(created.id);
    await bridge.completeRecord(created.id, false);
    await bridge.archiveRecord(created.id);
    await bridge.restoreRecord(created.id);
    expect((await bridge.snapshot()).records[0].status).toBe('saved');
  });
});
