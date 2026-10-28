import { invoke, isTauri } from '@tauri-apps/api/core';
import { defaultSettings, disconnectedWechat } from './types';
import type { AppSnapshot, InboxRecord, ModelDiscovery, ModelDraftRequest, Priority, ReasoningEffort, RecordPatch, Settings, SettingsPatch, WechatSnapshot } from './types';

const previewKey = 'shijian-preview-v1';
export const isDesktop = isTauri();

function readPreview(): AppSnapshot {
  const empty: AppSnapshot = {
    settings: { ...defaultSettings }, records: [], hasModelKey: false,
    runtime: 'preview', wechatStatus: 'not_connected', wechat: { ...disconnectedWechat }, dataDir: '当前浏览器标签页（预览）',
  };
  try {
    const saved = JSON.parse(sessionStorage.getItem(previewKey) || 'null');
    return saved ? { ...empty, settings: { ...empty.settings, ...saved.settings }, records: saved.records ?? [] } : empty;
  } catch { return empty; }
}

function writePreview(snapshot: AppSnapshot) {
  sessionStorage.setItem(previewKey, JSON.stringify({ settings: snapshot.settings, records: snapshot.records }));
}

export function validateSettings(settings: Settings) {
  if (!Number.isInteger(settings.retentionDays) || settings.retentionDays < 1 || settings.retentionDays > 365) {
    throw new Error('记录保留时间应为 1–365 天的整数');
  }
  if (!Number.isInteger(settings.remindIntervalMinutes) || settings.remindIntervalMinutes < 15 || settings.remindIntervalMinutes > 1440) {
    throw new Error('重复提醒间隔应在 0.25 到 24 小时之间');
  }
  if (settings.destination !== 'local') throw new Error('外部待办接入尚未开放，请使用本地收件箱');
  if (settings.modelBaseUrl.trim()) {
    let url: URL;
    try { url = new URL(settings.modelBaseUrl); } catch { throw new Error('请输入完整的模型 API 地址'); }
    const local = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
    if (url.username || url.password || url.search || url.hash || (url.protocol !== 'https:' && !(url.protocol === 'http:' && local))) {
      throw new Error('API 地址需使用 HTTPS；仅本机服务可使用 HTTP，且不能包含凭据、查询参数或片段');
    }
  }
}

function validateRecord(title: string, dueDate: string | null, dueTime: string | null) {
  if (!title.trim() || title.trim().length > 500) throw new Error('待办标题应为 1–500 个字符');
  if (dueDate) {
    const parsed = new Date(dueDate + 'T00:00:00Z');
    if (!/^\d{4}-\d{2}-\d{2}$/.test(dueDate) || dueDate.startsWith('0000') || Number.isNaN(parsed.getTime()) || parsed.toISOString().slice(0, 10) !== dueDate) {
      throw new Error('请选择有效的截止日期');
    }
  }
  if (dueTime && !dueDate) throw new Error('设置时间前请先选择截止日期');
  if (dueTime && !/^([01]\d|2[0-3]):[0-5]\d$/.test(dueTime)) throw new Error('请选择有效的时间');
}

const recordWrites = new Map<string, Promise<unknown>>();
function serializeRecordWrite<T>(id: string, write: () => Promise<T>): Promise<T> {
  const previous = recordWrites.get(id) ?? Promise.resolve();
  const next = previous.catch(() => {}).then(write);
  recordWrites.set(id, next);
  void next.finally(() => { if (recordWrites.get(id) === next) recordWrites.delete(id); }).catch(() => {});
  return next;
}

export const bridge = {
  async snapshot(): Promise<AppSnapshot> {
    return isDesktop ? invoke('get_snapshot') : readPreview();
  },
  async saveSettings(patch: SettingsPatch): Promise<Settings> {
    const clean = { ...patch };
    if (clean.modelBaseUrl !== undefined) clean.modelBaseUrl = clean.modelBaseUrl.trim().replace(/\/+$/, '');
    if (clean.modelName !== undefined) clean.modelName = clean.modelName.trim();
    if (isDesktop) return invoke('save_settings', { patch: clean });
    const state = readPreview();
    if (Object.keys(clean).some(key => key === 'panelBounds' || !(key in defaultSettings))) throw new Error('不支持的设置字段');
    const next = { ...state.settings, ...clean };
    validateSettings(next);
    state.settings = next; writePreview(state); return next;
  },
  async setPaused(paused: boolean): Promise<Settings> {
    if (isDesktop) return invoke('set_paused', { paused });
    const state = readPreview(); state.settings.paused = paused; writePreview(state); return state.settings;
  },
  async createRecord(title: string, originalText: string, dueDate: string | null, priority: Priority, dueTime: string | null = null): Promise<InboxRecord> {
    validateRecord(title, dueDate, dueTime);
    if (isDesktop) return invoke('create_record', { title, originalText, dueDate, dueTime, priority });
    const record: InboxRecord = {
      id: crypto.randomUUID(), title: title.trim(), originalText, dueDate, dueTime, priority,
      createdAt: new Date().toISOString(), source: 'local', status: 'saved', isDemo: false, error: null,
    };
    const state = readPreview(); state.records.unshift(record); writePreview(state); return record;
  },
  async updateRecord(id: string, patch: RecordPatch): Promise<InboxRecord> {
    return serializeRecordWrite(id, async () => {
      if (isDesktop) return invoke<InboxRecord>('update_record', { id, patch });
      if (Object.keys(patch).some(key => !['title', 'dueDate', 'dueTime', 'priority'].includes(key))) throw new Error('不支持的待办字段');
      const state = readPreview();
      const record = state.records.find(item => item.id === id);
      if (!record) throw new Error('记录不存在');
      if (record.status === 'completed' || record.status === 'archived') throw new Error('请先重新打开或恢复待办再编辑');
      const next = { ...record, ...patch };
      if (patch.dueDate === null) {
        if (patch.dueTime) throw new Error('设置时间前请先选择截止日期');
        next.dueTime = null;
      }
      validateRecord(next.title, next.dueDate, next.dueTime ?? null);
      if (!['normal', 'high'].includes(next.priority)) throw new Error('请选择有效的优先级');
      next.title = next.title.trim();
      if (next.title !== record.title || next.priority !== record.priority || next.dueDate !== record.dueDate || (next.dueTime ?? null) !== (record.dueTime ?? null)) {
        Object.assign(record, next, { status: 'saved', error: null });
      }
      writePreview(state); return record;
    });
  },
  async archiveRecord(id: string) {
    if (isDesktop) return invoke<void>('archive_record', { id });
    const state = readPreview(); const record = state.records.find(item => item.id === id);
    if (!record) throw new Error('记录不存在');
    if (record.status !== 'archived') { record.statusBeforeArchive = record.status; record.status = 'archived'; record.archivedAt = new Date().toISOString(); }
    writePreview(state);
  },
  async restoreRecord(id: string): Promise<InboxRecord> {
    if (isDesktop) return invoke('restore_record', { id });
    const state = readPreview(); const record = state.records.find(item => item.id === id);
    if (!record) throw new Error('记录不存在');
    if (record.status !== 'archived') return record;
    record.status = record.statusBeforeArchive ?? 'saved';
    if (record.status === 'saved' || record.status === 'completed') record.error = null;
    record.archivedAt = null;
    delete record.statusBeforeArchive;
    writePreview(state); return record;
  },
  async completeRecord(id: string, completed: boolean): Promise<InboxRecord> {
    if (isDesktop) return invoke('complete_record', { id, completed });
    const state = readPreview(); const record = state.records.find(item => item.id === id);
    if (!record) throw new Error('记录不存在');
    if (record.status === 'archived') throw new Error('请先恢复已归档的待办');
    record.status = completed ? 'completed' : 'saved';
    record.error = null;
    writePreview(state); return record;
  },
  async deleteRecord(id: string): Promise<void> {
    if (isDesktop) return invoke<void>('delete_record', { id });
    const state = readPreview();
    if (!state.records.some(item => item.id === id)) throw new Error('记录不存在');
    state.records = state.records.filter(item => item.id !== id); writePreview(state);
  },
  async startWechatLogin(): Promise<WechatSnapshot> {
    if (!isDesktop) throw new Error('浏览器预览不能连接微信，请在桌面应用中扫码');
    return invoke('start_wechat_login');
  },
  async submitWechatCode(code: string): Promise<void> {
    if (!isDesktop) throw new Error('请在桌面应用中输入微信验证码');
    return invoke('submit_wechat_code', { code });
  },
  async disconnectWechat(): Promise<void> {
    if (!isDesktop) throw new Error('浏览器预览没有微信连接');
    return invoke('disconnect_wechat');
  },
  async retryWechatReplies(): Promise<void> {
    if (!isDesktop) throw new Error('浏览器预览不能发送微信消息');
    return invoke('retry_wechat_replies');
  },
  async saveModelKey(apiKey: string, baseUrl: string) {
    if (!isDesktop) throw new Error('浏览器预览不保存密钥，请在桌面应用中配置');
    return invoke<boolean>('save_model_key', { apiKey, baseUrl });
  },
  async deleteModelKey() {
    if (!isDesktop) throw new Error('请在桌面应用中管理密钥');
    return invoke<void>('delete_model_key');
  },
  async testModel(): Promise<{ message: string; models: string[] }> {
    if (!isDesktop) throw new Error('浏览器预览不发送 API 请求，请在桌面应用中测试');
    return invoke('test_model_connection');
  },
  async discoverModels(draft: ModelDraftRequest): Promise<ModelDiscovery> {
    if (!isDesktop) throw new Error('浏览器预览不发送 API 请求，请使用桌面版获取模型');
    return invoke('discover_models', { baseUrl: draft.baseUrl, apiKey: draft.apiKey ?? null, useSavedKey: draft.useSavedKey });
  },
  async modelReasoningOptions(baseUrl: string, modelName: string): Promise<ReasoningEffort[]> {
    return isDesktop ? invoke('model_reasoning_options', { baseUrl, modelName }) : ['default'];
  },
  async openDataDir() {
    if (!isDesktop) throw new Error('浏览器预览没有桌面数据目录');
    return invoke<void>('open_data_dir');
  },
  async exportRecords(): Promise<string> {
    return isDesktop ? invoke('export_records') : JSON.stringify({ version: 1, exportedAt: new Date().toISOString(), records: readPreview().records }, null, 2);
  },
  async exportRecordsFile(): Promise<string> {
    if (!isDesktop) throw new Error('浏览器预览请使用浏览器下载');
    return invoke('export_records_file');
  },
  async quit() {
    if (!isDesktop) throw new Error('浏览器预览请直接关闭标签页');
    return invoke<void>('quit_app');
  },
  async togglePanel(): Promise<boolean> {
    if (!isDesktop) throw new Error('桌面待办面板只在桌面应用中可用');
    return invoke<boolean>('toggle_panel');
  },
  async setPanelVisible(visible: boolean): Promise<void> {
    if (isDesktop) return invoke<void>('set_panel_visible', { visible });
    const state = readPreview(); state.settings.panelEnabled = visible; writePreview(state);
  },
  async setPanelPinned(pinned: boolean): Promise<void> {
    if (isDesktop) return invoke<void>('set_panel_pinned', { pinned });
    const state = readPreview(); state.settings.panelPinned = pinned; writePreview(state);
  },
  async openMainWindow(): Promise<void> {
    if (!isDesktop) throw new Error('浏览器预览没有主窗口');
    return invoke<void>('open_main_window');
  },
  async resizePanel(width: number, height: number): Promise<void> {
    if (!isDesktop) throw new Error('桌面待办面板只在桌面应用中可用');
    return invoke<void>('resize_panel', { width, height });
  },
  async resetPanelPosition(): Promise<void> {
    if (!isDesktop) throw new Error('桌面待办面板只在桌面应用中可用');
    return invoke<void>('reset_panel_position');
  },
};
