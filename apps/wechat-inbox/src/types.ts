export type Page = 'inbox' | 'connections' | 'settings';
export type RecordStatus = 'saved' | 'needs_review' | 'failed' | 'completed' | 'archived';
export type Priority = 'normal' | 'high';
export type UnlockReminder = 'every_unlock' | 'first_of_day';
export type ReasoningEffort = 'default' | 'none' | 'low' | 'medium' | 'high' | 'max';
export interface ModelDiscovery { message: string; models: string[] }
export interface ModelDraftRequest { baseUrl: string; apiKey?: string; useSavedKey: boolean }

export interface Settings {
  modelBaseUrl: string;
  modelName: string;
  destination: 'local' | 'external';
  destinationLabel: string;
  paused: boolean;
  closeBehavior: 'hide' | 'quit';
  retentionDays: number;
  reasoningEffort: ReasoningEffort;
  launchAtLogin: boolean;
  remindersEnabled: boolean;
  remindOnUnlock: boolean;
  unlockReminder: UnlockReminder;
  remindOnStartup: boolean;
  remindRepeat: boolean;
  remindIntervalMinutes: number;
  parsingEnabled: boolean;
  panelEnabled: boolean;
  panelPinned: boolean;
  panelBounds: { x: number; y: number; width: number; height: number } | null;
}

export type SettingsPatch = Partial<Omit<Settings, 'panelBounds'>>;
export type RecordPatch = { title?: string; dueDate?: string | null; dueTime?: string | null; priority?: Priority };

export interface InboxRecord {
  id: string;
  title: string;
  originalText: string;
  status: RecordStatus;
  source: 'local' | 'wechat';
  createdAt: string;
  dueDate: string | null;
  dueTime?: string | null;
  priority: Priority;
  error: string | null;
  isDemo: boolean;
  statusBeforeArchive?: RecordStatus;
  archivedAt?: string | null;
}

export type WechatStatus = 'not_connected' | 'connecting' | 'waiting_scan' | 'scanned' | 'need_verifycode' | 'connected' | 'paused' | 'reconnecting' | 'expired' | 'error';

export interface WechatSnapshot {
  status: WechatStatus;
  message: string;
  qrCodeSvg: string | null;
  qrExpiresAt: string | null;
  accountId: string | null;
  pendingReplies: number;
  failedReplies: number;
  lastReceivedAt: string | null;
}

export const disconnectedWechat: WechatSnapshot = {
  status: 'not_connected', message: '尚未连接微信', qrCodeSvg: null,
  qrExpiresAt: null, accountId: null, pendingReplies: 0, failedReplies: 0, lastReceivedAt: null,
};

export interface AppSnapshot {
  settings: Settings;
  records: InboxRecord[];
  hasModelKey: boolean;
  modelCredentialError?: string | null;
  dataDir: string;
  runtime: 'desktop' | 'preview';
  wechatStatus: WechatStatus;
  wechat: WechatSnapshot;
}

export const defaultSettings: Settings = {
  modelBaseUrl: '', modelName: '', destination: 'local',
  destinationLabel: '本地收件箱', paused: false, closeBehavior: 'hide', retentionDays: 30,
  reasoningEffort: 'default', launchAtLogin: false, remindersEnabled: true, remindOnUnlock: true, unlockReminder: 'first_of_day',
  remindOnStartup: true, remindRepeat: false, remindIntervalMinutes: 90,
  parsingEnabled: false, panelEnabled: false, panelPinned: false, panelBounds: null,
};
