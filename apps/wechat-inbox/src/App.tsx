import { useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import type { FormEvent, Ref } from 'react';
import { useAppSnapshot } from './useAppSnapshot';
import { useLocalDay } from './useLocalDay';
import {
  AppWindow, Archive, ArrowDownToLine, Circle, CircleAlert, CircleCheck, FileText, FolderOpen,
  Inbox, ListChecks, LoaderCircle, MessageCircle, Pause, Play, Plug, Plus, RotateCcw,
  Search, Settings2, Square, SquareCheck, Trash2, X,
} from 'lucide-react';
import { version } from '../package.json';
import { bridge, isDesktop } from './bridge';
import { dueLabel, groupByDue } from './due';
import type { AppSnapshot, InboxRecord, Page, Priority, RecordPatch, RecordStatus, SettingsPatch, UnlockReminder, WechatStatus } from './types';

import Modal from './components/Modal';
import PickerButton from './components/PickerButton';
import ModelConfiguration from './components/ModelConfiguration';
import Switch from './components/Switch';

type Filter = 'active' | 'attention' | 'completed' | 'archived';
type Notice = { text: string; tone: 'success' | 'error'; undoId?: string };
type Notify = (notice: Notice) => void;
const statusLabels: Record<RecordStatus, string> = { saved: '进行中', needs_review: '需确认', failed: '处理失败', completed: '已完成', archived: '已归档' };
const pageLabels: Record<Page, string> = { inbox: '待办', connections: '连接', settings: '设置' };
const wechatLabels: Record<WechatStatus, string> = {
  not_connected: '未连接微信', connecting: '正在连接', waiting_scan: '等待扫码',
  scanned: '等待手机确认', need_verifycode: '需要验证码', connected: '微信已连接',
  paused: '已暂停接收', reconnecting: '正在重连', expired: '二维码已过期', error: '微信连接异常',
};
const setupHintKey = 'shijian.setupHintHidden';
const emptyRecords: InboxRecord[] = [];

function errorText(reason: unknown) { return reason instanceof Error ? reason.message : String(reason); }
function isToday(date: Date) { return date.toDateString() === new Date().toDateString(); }
function dayLabel(value: string) {
  const date = new Date(value); const yesterday = new Date(); yesterday.setDate(yesterday.getDate() - 1);
  if (isToday(date)) return '今天';
  if (date.toDateString() === yesterday.toDateString()) return '昨天';
  return date.toLocaleDateString('zh-CN', { year: date.getFullYear() !== new Date().getFullYear() ? 'numeric' : undefined, month: 'long', day: 'numeric' });
}
function createdLabel(value: string) {
  const date = new Date(value);
  return isToday(date) ? date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false }) : dayLabel(value);
}

function RecordForm({ record, onSave, onClose, onBusyChange }: { record?: InboxRecord; onSave: (title: string, due: string | null, priority: Priority, time: string | null) => Promise<void>; onClose: () => void; onBusyChange: (busy: boolean) => void }) {
  const [title, setTitle] = useState(record?.title ?? '');
  const [due, setDue] = useState(record?.dueDate ?? '');
  const [time, setTime] = useState(record?.dueTime ?? '');
  const [priority, setPriority] = useState<Priority>(record?.priority ?? 'normal');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function submit(event: FormEvent) {
    event.preventDefault(); if (busy) return; setBusy(true); onBusyChange(true); setError('');
    try { await onSave(title, due || null, priority, due && time ? time : null); }
    catch (reason) { setError(errorText(reason)); }
    finally { setBusy(false); onBusyChange(false); }
  }
  return <form onSubmit={submit} className="record-form">
    <label className="field">待办内容<textarea data-autofocus disabled={busy} value={title} onChange={e => setTitle(e.target.value)} placeholder="要做什么？" maxLength={500} rows={3} required /></label>
    <div className="field-columns three"><div className="field"><span>截止日期</span><PickerButton type="date" label="截止日期" value={due} placeholder="添加日期" disabled={busy} onChange={value => { setDue(value ?? ''); if (!value) setTime(''); }} /></div>
      <div className="field"><span>时间</span><PickerButton type="time" label="截止时间" value={due ? time : ''} placeholder={due ? '添加时间' : '先选日期'} disabled={busy || !due} onChange={value => setTime(value ?? '')} /></div>
      <label className="field">优先级<select disabled={busy} value={priority} onChange={e => setPriority(e.target.value as Priority)}><option value="normal">普通</option><option value="high">优先</option></select></label></div>
    {error && <p role="alert" className="inline-error">{error}</p>}
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={busy}>取消</button><button className="button primary" disabled={busy || !title.trim()}>{busy && <LoaderCircle className="spin" size={16} />}{busy ? '正在保存…' : record ? '保存修改' : '保存'}</button></div>
  </form>;
}

function QuickAdd({ onAdd }: { onAdd: (title: string) => Promise<void> }) {
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  async function submit(event: FormEvent) {
    event.preventDefault();
    const title = text.trim(); if (!title || busy) return;
    setBusy(true);
    try { await onAdd(title); setText(''); }
    catch { /* The parent reports the failure; keep the draft for retry. */ }
    finally { setBusy(false); }
  }
  return <form className="quick-add" onSubmit={submit}>
    {busy ? <LoaderCircle className="spin" size={18} /> : <Plus size={18} />}
    <input aria-label="添加待办" placeholder="添加待办，按回车保存" disabled={busy} value={text} maxLength={500} onChange={event => setText(event.target.value)} />
    {text.trim() && <kbd>Enter</kbd>}
  </form>;
}

function RecordRow({ record, active, picking, picked, onOpen, onToggle, onPick, onArchive, onRestore, onDelete }: {
  record: InboxRecord; active: boolean; picking: boolean; picked: boolean;
  onOpen: (record: InboxRecord) => void; onToggle: (record: InboxRecord, completed: boolean) => void; onPick: (record: InboxRecord) => void;
  onArchive: (record: InboxRecord) => void; onRestore: (record: InboxRecord) => void; onDelete: (record: InboxRecord) => void;
}) {
  const done = record.status === 'completed';
  const archived = record.status === 'archived';
  const due = done || archived ? null : dueLabel(record);
  return <div data-detail-leave className={`record-row ${active ? 'record-selected' : ''} ${done ? 'record-done' : ''} ${picked ? 'record-picked' : ''}`}>
    {picking
      ? <button className="record-check pick" role="checkbox" aria-checked={picked} aria-label={`选择：${record.title}`} onClick={() => onPick(record)}>{picked ? <SquareCheck size={19} /> : <Square size={19} />}</button>
      : archived
        ? <span className="record-check" aria-hidden="true"><Archive size={17} /></span>
        : <button className="record-check" aria-label={done ? `重新打开：${record.title}` : `完成：${record.title}`} onClick={() => onToggle(record, !done)}>{done ? <CircleCheck size={20} /> : <Circle size={20} />}</button>}
    <button className="record-main" onClick={() => picking ? onPick(record) : onOpen(record)}>
      <span className="record-title"><span>{record.title}</span>{record.priority === 'high' && <span className="priority-label">优先</span>}</span>
      <span className="record-meta">
        {record.status === 'needs_review' && <span className="tag tag-review">需确认</span>}
        {record.status === 'failed' && <span className="tag tag-failed">处理失败</span>}
        {due && <span className={`tag tag-${due.tone}`}>{due.text}</span>}
        {record.source === 'wechat' && <span className="record-source"><MessageCircle size={12} />微信</span>}
        <time dateTime={record.createdAt}>{createdLabel(record.createdAt)}</time>
      </span>
    </button>
    {!picking && <span className="row-actions">
      {archived
        ? <><button className="icon-button" aria-label={`恢复：${record.title}`} title="恢复" onClick={() => onRestore(record)}><RotateCcw size={16} /></button>
          <button className="icon-button danger-icon" aria-label={`删除：${record.title}`} title="永久删除" onClick={() => onDelete(record)}><Trash2 size={16} /></button></>
        : <button className="icon-button" aria-label={`归档：${record.title}`} title="归档" onClick={() => onArchive(record)}><Archive size={16} /></button>}
    </span>}
  </div>;
}

type RecordChange = RecordPatch;
type DetailHandle = { flushTitle: () => Promise<boolean> };

function DetailPane({ record, remindersEnabled, onClose, onSave, onComplete, onArchive, onRestore, onDelete, ref }: {
  record: InboxRecord; remindersEnabled: boolean; onClose: () => void; onSave: (record: InboxRecord, change: RecordChange) => Promise<boolean>;
  onComplete: (record: InboxRecord, completed: boolean) => void; onArchive: (record: InboxRecord) => void; onRestore: (record: InboxRecord) => void; onDelete: (record: InboxRecord) => void;
  ref: Ref<DetailHandle>;
}) {
  const editable = record.status !== 'completed' && record.status !== 'archived';
  const [title, setTitle] = useState(record.title);
  const draft = useRef(record.title);
  const persistedTitle = useRef(record.title);
  const dirty = useRef(false);
  const pendingTitle = useRef<Promise<boolean> | null>(null);
  useEffect(() => {
    if (!dirty.current) { persistedTitle.current = record.title; draft.current = record.title; setTitle(record.title); }
  }, [record.id, record.title]);
  function save(change: RecordChange) { return onSave(record, change); }
  function commitTitle(): Promise<boolean> {
    if (pendingTitle.current) return pendingTitle.current;
    if (!editable || !dirty.current) return Promise.resolve(true);
    const write = async () => {
      for (;;) {
        const next = draft.current.trim();
        if (!next || next === persistedTitle.current) {
          dirty.current = false; draft.current = persistedTitle.current; setTitle(persistedTitle.current);
          return true;
        }
        if (!await save({ title: next })) return false;
        persistedTitle.current = next;
        if (draft.current.trim() === next) {
          dirty.current = false; draft.current = next; setTitle(next);
          return true;
        }
      }
    };
    pendingTitle.current = write().finally(() => { pendingTitle.current = null; });
    return pendingTitle.current;
  }
  useImperativeHandle(ref, () => ({ flushTitle: commitTitle }));
  const source = record.source === 'wechat' ? '来自微信' : '手动添加';
  const showOriginal = record.source === 'wechat' || record.originalText.trim() !== record.title.trim();
  const reminder = !record.dueDate ? '设置截止日期后提醒' : !remindersEnabled ? '提醒已关闭' : record.dueTime ? `${record.dueTime} 准时提醒` : '到期后按提醒设置提醒';
  return <aside className="detail-pane" aria-label="待办详情">
    <div className="detail-top" data-detail-leave><span className={`detail-status status-${record.status}`}>{statusLabels[record.status]}</span><button className="icon-button" aria-label="关闭详情" onClick={onClose}><X size={18} /></button></div>
    {editable
      ? <textarea className="detail-title" aria-label="待办标题" value={title} rows={1} maxLength={500} onChange={event => { dirty.current = true; draft.current = event.target.value; setTitle(event.target.value); }} onBlur={event => {
        // Leaving actions own the save, so a failed blur cannot race a close or retry twice.
        if (event.relatedTarget instanceof Element && event.relatedTarget.closest('[data-detail-leave]')) return;
        void commitTitle();
      }} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); event.currentTarget.blur(); } }} />
      : <h2 className="detail-title static">{record.title}</h2>}
    <p className="detail-meta">{source} · {new Date(record.createdAt).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</p>
    {record.error && <div className={`detail-alert ${record.status === 'needs_review' ? 'review-alert' : ''}`}><CircleAlert size={16} /><p>{record.error}</p></div>}
    <div className="detail-fields">
      <div className="detail-field"><span>截止日期</span><PickerButton type="date" label="截止日期" value={record.dueDate ?? ''} placeholder={editable ? '添加日期' : '未设置'} disabled={!editable} onChange={value => void save({ dueDate: value, ...(value ? {} : { dueTime: null }) })} /></div>
      <div className="detail-field"><span>时间</span><PickerButton type="time" label="截止时间" value={record.dueTime ?? ''} placeholder={!editable ? '未设置' : record.dueDate ? '添加时间' : '先选日期'} disabled={!editable || !record.dueDate} onChange={value => void save({ dueTime: value })} /></div>
      <div className="detail-field"><span>优先级</span><div className="segmented" role="group" aria-label="优先级">{(['normal', 'high'] as const).map(value => <button key={value} aria-pressed={record.priority === value} disabled={!editable} onClick={() => { if (record.priority !== value) void save({ priority: value }); }}>{value === 'high' ? '优先' : '普通'}</button>)}</div></div>
      <div className="detail-field"><span>提醒</span><span className="detail-value">{reminder}</span></div>
    </div>
    {showOriginal && <div className="detail-section"><h3>{record.source === 'wechat' ? '微信原文' : '最初内容'}</h3><blockquote>{record.originalText}</blockquote></div>}
    <div className="detail-actions" data-detail-leave>
      {record.status === 'archived' ? <button className="button primary" onClick={() => onRestore(record)}><RotateCcw size={16} />恢复</button>
        : record.status === 'completed' ? <button className="button primary" onClick={() => onComplete(record, false)}><RotateCcw size={16} />重新打开</button>
        : <button className="button primary" onClick={() => onComplete(record, true)}><CircleCheck size={16} />标记完成</button>}
      {record.status !== 'archived' && <button className="button secondary" onClick={() => onArchive(record)}><Archive size={16} />归档</button>}
      <span className="spacer" />
      <button className="text-button danger" onClick={() => onDelete(record)}>永久删除</button>
    </div>
  </aside>;
}

function WechatConnection({ snapshot, refresh, notify }: { snapshot: AppSnapshot; refresh: () => Promise<void>; notify: Notify }) {
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const [code, setCode] = useState('');
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  const connection = snapshot.wechat;
  const paused = snapshot.settings.paused;
  const activeLogin = ['connecting', 'waiting_scan', 'scanned', 'need_verifycode'].includes(connection.status);
  const online = ['connected', 'paused', 'reconnecting'].includes(connection.status);
  const canStart = !online;
  const canDisconnect = activeLogin || !!connection.accountId || connection.status === 'error';
  const disconnectLabel = connection.accountId || connection.status === 'error' ? '断开连接' : '取消连接';
  const showMessage = connection.status !== 'not_connected' && connection.message !== wechatLabels[connection.status];
  useEffect(() => { if (connection.status !== 'need_verifycode') setCode(''); }, [connection.status]);
  async function act(action: string, operation: () => Promise<unknown>) {
    setBusy(action); setError('');
    try { await operation(); await refresh(); }
    catch (reason) { setError(errorText(reason)); }
    finally { setBusy(''); }
  }
  return <section className="card" aria-label="微信连接">
    <header className="card-head"><span className="card-icon wechat-icon"><MessageCircle size={20} /></span><div><h2>微信</h2><p>把要做的事发到微信，拾笺会自动收进待办。</p></div><span className={`pill ${connection.status === 'connected' ? 'pill-ok' : paused ? 'pill-warn' : ''}`}>{wechatLabels[connection.status]}</span></header>
    <div className="card-body">
      {showMessage && <p className="wechat-message" role="status">{connection.message}</p>}
      {connection.qrCodeSvg && activeLogin && <figure className="wechat-qr"><img src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(connection.qrCodeSvg)}`} alt="微信连接二维码，请使用手机微信扫一扫" width="200" height="200" />
        <figcaption><strong>用手机微信扫一扫</strong>扫码后在手机上确认登录。{connection.qrExpiresAt && <span>二维码 {new Date(connection.qrExpiresAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })} 前有效</span>}</figcaption></figure>}
      {connection.status === 'connecting' && <p className="muted-line"><LoaderCircle className="spin" size={15} />正在获取二维码…</p>}
      {connection.status === 'need_verifycode' && <form className="wechat-code-form" onSubmit={event => { event.preventDefault(); void act('code', async () => { await bridge.submitWechatCode(code); setCode(''); }); }}>
        <label className="field">微信验证码<input value={code} onChange={event => setCode(event.target.value)} inputMode="numeric" autoComplete="off" pattern="[0-9]{4,12}" minLength={4} maxLength={12} required placeholder="输入手机上显示的数字" /></label>
        <button className="button primary" disabled={!!busy || !/^\d{4,12}$/.test(code)}>{busy === 'code' ? '正在验证…' : '提交验证码'}</button>
      </form>}
      {(connection.accountId || connection.lastReceivedAt) && <dl className="info-list">
        {connection.accountId && <div><dt>账号</dt><dd>{connection.accountId}</dd></div>}
        {connection.lastReceivedAt && <div><dt>最近收到消息</dt><dd>{new Date(connection.lastReceivedAt).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</dd></div>}
      </dl>}
      {connection.failedReplies > 0 && <div className="inline-warning"><CircleAlert size={16} /><span>{connection.failedReplies} 条微信回复发送失败{connection.pendingReplies > 0 && `，${connection.pendingReplies} 条等待发送`}</span><button className="button secondary small" disabled={!!busy || paused || !isDesktop} onClick={() => void act('retry', async () => { await bridge.retryWechatReplies(); notify({ text: '正在重新发送', tone: 'success' }); })}>重新发送</button></div>}
      {connection.failedReplies === 0 && connection.pendingReplies > 0 && <p className="muted-line">{connection.pendingReplies} 条回复等待发送</p>}
      {error && !confirmDisconnect && <p className="inline-error" role="alert">{error}</p>}
      {!isDesktop && <p className="muted-line">浏览器预览不能连接微信，请使用桌面版。</p>}
      <div className="card-actions">
        {canStart && <button className="button primary" disabled={!!busy || !isDesktop || connection.status === 'connecting'} onClick={() => void act('start', () => bridge.startWechatLogin())}>{busy === 'start' && <LoaderCircle className="spin" size={16} />}{activeLogin || connection.status === 'expired' ? '刷新二维码' : connection.accountId ? '重新连接' : '连接微信'}</button>}
        {(online || paused) && <button className="button secondary" disabled={!!busy || !isDesktop} onClick={() => void act('pause', async () => { await bridge.setPaused(!paused); notify({ text: paused ? '已继续接收微信消息' : '已暂停接收微信消息', tone: 'success' }); })}>{paused ? <Play size={15} /> : <Pause size={15} />}{paused ? '继续接收' : '暂停接收'}</button>}
        {canDisconnect && <button className="text-button danger" disabled={!!busy || !isDesktop} onClick={() => { setError(''); setConfirmDisconnect(true); }}>{disconnectLabel}</button>}
      </div>
    </div>
    {confirmDisconnect && <Modal title={disconnectLabel === '取消连接' ? '取消本次连接？' : '断开微信连接？'} subtitle="将停止接收消息并退出微信登录，已有待办不受影响。" onClose={() => setConfirmDisconnect(false)} busy={busy === 'disconnect'}>{error && <p role="alert" className="inline-error">{error}</p>}<div className="modal-actions"><button className="button secondary" disabled={!!busy} onClick={() => setConfirmDisconnect(false)}>取消</button><button className="button danger-button" disabled={!!busy} onClick={() => void act('disconnect', async () => { await bridge.disconnectWechat(); setConfirmDisconnect(false); })}>{busy === 'disconnect' ? '正在断开…' : '确认断开'}</button></div></Modal>}
  </section>;
}

function AppSettings({ snapshot, refresh, notify }: { snapshot: AppSnapshot; refresh: () => Promise<void>; notify: Notify }) {
  const [busy, setBusy] = useState(false);
  const [confirmQuit, setConfirmQuit] = useState(false);
  const [quitting, setQuitting] = useState(false);
  const [quitError, setQuitError] = useState('');
  const [exporting, setExporting] = useState(false);
  const settings = snapshot.settings;
  const reminders = settings.remindersEnabled;
  const hours = (minutes: number) => String(Math.round(minutes / 60 * 100) / 100);
  const [intervalText, setIntervalText] = useState(() => hours(settings.remindIntervalMinutes));
  useEffect(() => { setIntervalText(hours(settings.remindIntervalMinutes)); }, [settings.remindIntervalMinutes]);
  function commitInterval() {
    const value = Number(intervalText.trim());
    const minutes = Math.round(value * 60);
    if (!intervalText.trim() || !Number.isFinite(value) || minutes < 15 || minutes > 1440) {
      setIntervalText(hours(settings.remindIntervalMinutes));
      notify({ text: '请输入 0.25 到 24 之间的小时数，例如 1.5', tone: 'error' });
      return;
    }
    if (minutes !== settings.remindIntervalMinutes) void update({ remindIntervalMinutes: minutes });
    else setIntervalText(hours(minutes));
  }
  async function quit() {
    if (quitting) return; setQuitting(true); setQuitError('');
    try { await bridge.quit(); }
    catch (reason) { setQuitError(errorText(reason)); setQuitting(false); }
  }
  async function update(change: SettingsPatch) {
    setBusy(true);
    try { await bridge.saveSettings(change); await refresh(); notify({ text: '设置已保存', tone: 'success' }); }
    catch (reason) { notify({ text: errorText(reason), tone: 'error' }); } finally { setBusy(false); }
  }
  async function togglePanel(visible: boolean) {
    setBusy(true);
    try { await bridge.setPanelVisible(visible); await refresh(); }
    catch (reason) { notify({ text: errorText(reason), tone: 'error' }); } finally { setBusy(false); }
  }
  async function exportData() {
    if (exporting) return; setExporting(true);
    try {
      if (isDesktop) {
        const path = await bridge.exportRecordsFile();
        if (path) notify({ text: `已导出到 ${path}`, tone: 'success' });
        return;
      }
      const json = await bridge.exportRecords();
      const url = URL.createObjectURL(new Blob([json], { type: 'application/json' }));
      const anchor = document.createElement('a'); anchor.href = url; anchor.download = `shijian-records-${new Date().toISOString().slice(0, 10)}.json`; anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1500);
      notify({ text: '已开始下载', tone: 'success' });
    } catch (reason) { notify({ text: errorText(reason), tone: 'error' }); }
    finally { setExporting(false); }
  }
  return <div className="page-stack">
    <section className="card" aria-labelledby="settings-general"><h2 className="card-title" id="settings-general">常规</h2>
      <div className="setting-row"><div><h3>关闭主窗口时</h3><p>最小化后仍会接收微信消息和提醒。</p></div><select aria-label="关闭主窗口时" value={settings.closeBehavior} onChange={e => update({ closeBehavior: e.target.value as 'hide' | 'quit' })} disabled={busy}><option value="hide">最小化到托盘</option><option value="quit">退出拾笺</option></select></div>
      <div className="setting-row"><div><h3>开机自动启动</h3><p>登录 Windows 后在后台启动。</p></div><Switch label="开机自动启动" checked={settings.launchAtLogin} onChange={value => update({ launchAtLogin: value })} disabled={busy} /></div>
      <div className="setting-row"><div><h3>桌面待办面板</h3><p>在桌面上显示待办，随时添加和完成；也可以在侧边栏或托盘菜单开关。</p></div><Switch label="桌面待办面板" checked={settings.panelEnabled} onChange={value => void togglePanel(value)} disabled={busy || !isDesktop} /></div>
    </section>
    <section className="card" aria-labelledby="settings-reminders"><h2 className="card-title" id="settings-reminders">提醒</h2>
      <div className="setting-row"><div><h3>到期提醒</h3><p>有今天到期或已过期的待办时，用系统通知提醒你。</p></div><Switch label="到期提醒" checked={settings.remindersEnabled} onChange={value => update({ remindersEnabled: value })} disabled={busy} /></div>
      <div className="setting-row sub-row"><div><h3>解锁电脑时</h3><p>{settings.unlockReminder === 'every_unlock' ? '每次解锁屏幕都提醒。' : '每天第一次解锁屏幕时提醒。'}</p></div><div className="row-controls"><select aria-label="解锁提醒方式" value={settings.unlockReminder} onChange={e => update({ unlockReminder: e.target.value as UnlockReminder })} disabled={busy || !reminders || !settings.remindOnUnlock}><option value="first_of_day">每天首次解锁</option><option value="every_unlock">每次解锁</option></select><Switch label="解锁电脑时提醒" checked={settings.remindOnUnlock} onChange={value => update({ remindOnUnlock: value })} disabled={busy || !reminders} /></div></div>
      <div className="setting-row sub-row"><div><h3>启动拾笺时</h3><p>{settings.launchAtLogin ? '已开启开机自动启动，开机后会提醒。' : '打开拾笺时提醒；开启开机自动启动后，相当于开机提醒。'}</p></div><Switch label="启动拾笺时提醒" checked={settings.remindOnStartup} onChange={value => update({ remindOnStartup: value })} disabled={busy || !reminders} /></div>
      <div className="setting-row sub-row"><div><h3>定时重复</h3><p>距上次提醒满设定时长后，再提醒一次未完成的到期待办。</p></div><div className="row-controls"><label className="interval-input">每<input aria-label="重复间隔（小时）" inputMode="decimal" value={intervalText} disabled={busy || !reminders || !settings.remindRepeat} onChange={e => setIntervalText(e.target.value)} onBlur={commitInterval} onKeyDown={e => { if (e.key === 'Enter') e.currentTarget.blur(); }} />小时</label><Switch label="定时重复提醒" checked={settings.remindRepeat} onChange={value => update({ remindRepeat: value })} disabled={busy || !reminders} /></div></div>
      <p className="card-note">设置了具体时间的待办，到点会单独提醒，不受以上选项影响。</p>
    </section>
    <section className="card" aria-labelledby="settings-data"><h2 className="card-title" id="settings-data">数据</h2>
      <div className="setting-row"><div className="setting-path"><h3>数据位置</h3><p>{snapshot.dataDir}</p></div>{isDesktop && <button className="button secondary small" onClick={async () => { try { await bridge.openDataDir(); } catch (reason) { notify({ text: errorText(reason), tone: 'error' }); } }}><FolderOpen size={15} />打开</button>}</div>
      <div className="setting-row"><div><h3>导出待办</h3><p>导出为 JSON 文件，包含已归档的待办。</p></div><button className="button secondary small" onClick={exportData} disabled={exporting}>{exporting ? <LoaderCircle className="spin" size={15} /> : <ArrowDownToLine size={15} />}{exporting ? '正在导出…' : '导出'}</button></div>
      <div className="setting-row"><div><h3>归档保留</h3><p>从归档时起，超过保留天数后自动删除。</p></div><select aria-label="归档保留" value={settings.retentionDays} onChange={e => update({ retentionDays: Number(e.target.value) })} disabled={busy}>{Array.from(new Set([7, 30, 90, 180, 365, settings.retentionDays])).sort((a, b) => a - b).map(days => <option key={days} value={days}>{days} 天</option>)}</select></div>
    </section>
    <section className="card" aria-labelledby="settings-about"><h2 className="card-title" id="settings-about">关于</h2>
      <div className="setting-row about-row"><div className="about-brand"><span className="brand-mark small"><FileText size={17} strokeWidth={1.8} /></span><div><h3>拾笺</h3><p>版本 {version}</p></div></div>{isDesktop && <button className="button secondary small" onClick={() => setConfirmQuit(true)}>退出拾笺</button>}</div>
    </section>
    {confirmQuit && <Modal title="退出拾笺？" subtitle="退出后将不再接收微信消息和到期提醒。" onClose={() => setConfirmQuit(false)} busy={quitting}>{quitError && <p role="alert" className="inline-error">{quitError}</p>}<div className="modal-actions"><button className="button secondary" disabled={quitting} onClick={() => setConfirmQuit(false)}>取消</button><button className="button primary" disabled={quitting} onClick={quit}>{quitting ? '正在退出…' : '退出拾笺'}</button></div></Modal>}
  </div>;
}

export default function App() {
  const [page, setPage] = useState<Page>('inbox');
  const [filter, setFilter] = useState<Filter>('active');
  const [search, setSearch] = useState('');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<InboxRecord[] | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [recordSaving, setRecordSaving] = useState(false);
  const [picking, setPicking] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const [batchBusy, setBatchBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [panelBusy, setPanelBusy] = useState(false);
  const [setupHidden, setSetupHidden] = useState(() => localStorage.getItem(setupHintKey) === '1');
  const searchRef = useRef<HTMLInputElement>(null);
  const detailRef = useRef<DetailHandle>(null);
  const leavingDetail = useRef(false);
  const leaveDetail = useCallback(async (action: () => void | Promise<void>) => {
    if (leavingDetail.current) return;
    leavingDetail.current = true;
    try { if (await (detailRef.current?.flushTitle() ?? Promise.resolve(true))) await action(); }
    finally { leavingDetail.current = false; }
  }, []);
  const notify = useCallback((message: Notice) => setNotice(message), []);
  const snapshotError = useCallback((text: string) => notify({ text, tone: 'error' }), [notify]);
  const { snapshot, loadError, syncError, refresh, retrySync } = useAppSnapshot(snapshotError);
  const closeForm = useCallback(() => setCreating(false), []);
  useEffect(() => {
    if (!notice) return;
    const timeout = setTimeout(() => setNotice(null), notice.undoId ? 10000 : 5000);
    return () => clearTimeout(timeout);
  }, [notice]);
  useEffect(() => {
    function shortcut(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && ['n', 'k'].includes(event.key.toLowerCase()) && document.querySelector('[role="dialog"][aria-modal="true"]')) { event.preventDefault(); return; }
      if (event.key === 'Escape' && !document.querySelector('[role="dialog"][aria-modal="true"]')) { void leaveDetail(() => { setSelectedId(null); setPicking(false); setPicked(new Set()); }); }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k' && !creating) { event.preventDefault(); void leaveDetail(() => { setPage('inbox'); requestAnimationFrame(() => searchRef.current?.focus()); }); }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'n' && !creating) { event.preventDefault(); void leaveDetail(() => setCreating(true)); }
    }
    window.addEventListener('keydown', shortcut); return () => window.removeEventListener('keydown', shortcut);
  }, [creating, leaveDetail]);
  const records = snapshot?.records ?? emptyRecords;
  const { activeRecords, attention, completedRecords, archivedRecords } = useMemo(() => {
    const activeRecords: InboxRecord[] = [], attention: InboxRecord[] = [], completedRecords: InboxRecord[] = [], archivedRecords: InboxRecord[] = [];
    for (const item of records) {
      if (item.status === 'archived') archivedRecords.push(item);
      else if (item.status === 'completed') completedRecords.push(item);
      else { activeRecords.push(item); if (item.status === 'needs_review' || item.status === 'failed') attention.push(item); }
    }
    return { activeRecords, attention, completedRecords, archivedRecords };
  }, [records]);
  const query = search.trim().toLocaleLowerCase();
  const displayed = useMemo(() => (filter === 'archived' ? archivedRecords : filter === 'completed' ? completedRecords : filter === 'attention' ? attention : activeRecords)
    .filter(item => !query || `${item.title} ${item.originalText}`.toLocaleLowerCase().includes(query)), [filter, query, activeRecords, attention, completedRecords, archivedRecords]);
  const today = useLocalDay();
  const groups = useMemo(() => {
    if (filter === 'active' || filter === 'attention') return groupByDue(displayed);
    const map = new Map<string, InboxRecord[]>();
    for (const record of [...displayed].sort((a, b) => b.createdAt.localeCompare(a.createdAt))) {
      const day = dayLabel(record.createdAt), items = map.get(day);
      if (items) items.push(record); else map.set(day, [record]);
    }
    return Array.from(map.entries());
  }, [filter, displayed, today]);
  if (!snapshot) return <div className="loading-screen"><div className="brand-mark"><FileText size={22} /></div>{loadError ? <><p role="alert">{loadError}</p><button className="button primary" onClick={() => { void refresh().catch(snapshotError); }}>重新加载</button></> : <p><LoaderCircle size={16} className="spin" />正在打开拾笺</p>}</div>;
  const selected = records.find(record => record.id === selectedId) ?? null;
  const pickedRecords = displayed.filter(record => picked.has(record.id));
  const showSetup = !setupHidden && !snapshot.wechat.accountId;
  const fail = (reason: unknown) => notify({ text: errorText(reason), tone: 'error' });
  function go(next: Page) { void leaveDetail(() => { setPage(next); setSelectedId(null); stopPicking(); }); }
  function changeFilter(next: Filter) { void leaveDetail(() => { setFilter(next); setSelectedId(null); stopPicking(); }); }
  function stopPicking() { setPicking(false); setPicked(new Set()); }
  function togglePick(record: InboxRecord) { setPicked(previous => { const next = new Set(previous); if (!next.delete(record.id)) next.add(record.id); return next; }); }
  async function quickAdd(title: string) {
    try { await bridge.createRecord(title, title, null, 'normal'); }
    catch (reason) { fail(reason); throw reason; }
    setFilter('active'); setSearch('');
    await refresh().catch(reason => notify({ text: `已添加，但刷新失败：${errorText(reason)}`, tone: 'error' }));
  }
  async function saveRecord(record: InboxRecord, change: RecordChange) {
    try { await bridge.updateRecord(record.id, change); }
    catch (reason) { fail(reason); return false; }
    await refresh().catch(reason => notify({ text: `修改已保存，但刷新失败：${errorText(reason)}`, tone: 'error' }));
    return true;
  }
  async function archive(record: InboxRecord) {
    await leaveDetail(async () => {
      try { await bridge.archiveRecord(record.id); if (selectedId === record.id) setSelectedId(null); await refresh(); notify({ text: '已归档', tone: 'success', undoId: record.id }); } catch (reason) { fail(reason); }
    });
  }
  async function restore(record: InboxRecord) {
    await leaveDetail(async () => {
      try { await bridge.restoreRecord(record.id); if (selectedId === record.id) setSelectedId(null); await refresh(); notify({ text: '已恢复', tone: 'success' }); } catch (reason) { fail(reason); }
    });
  }
  async function complete(record: InboxRecord, completed: boolean) {
    await leaveDetail(async () => {
      try { await bridge.completeRecord(record.id, completed); if (selectedId === record.id) setSelectedId(null); await refresh(); notify({ text: completed ? '已完成' : '已重新打开', tone: 'success' }); } catch (reason) { fail(reason); }
    });
  }
  async function batch(label: string, operation: (record: InboxRecord) => Promise<unknown>, targets = pickedRecords) {
    if (!targets.length || batchBusy) return;
    setBatchBusy(true);
    let done = 0;
    try { for (const record of targets) { await operation(record); done++; } notify({ text: `已${label} ${done} 条`, tone: 'success' }); stopPicking(); }
    catch (reason) { notify({ text: `已${label} ${done} 条，其余失败：${errorText(reason)}`, tone: 'error' }); }
    finally { setBatchBusy(false); if (targets.some(record => record.id === selectedId)) setSelectedId(null); await refresh().catch(fail); }
  }
  async function removeForever() {
    if (!confirmDelete || deleting) return;
    const targets = confirmDelete;
    setDeleting(true);
    await batch('永久删除', record => bridge.deleteRecord(record.id), targets);
    setDeleting(false); setConfirmDelete(null);
  }
  const tabs: { id: Filter; label: string; count?: number; warn?: boolean }[] = [
    { id: 'active', label: '进行中', count: activeRecords.length },
    { id: 'attention', label: '需确认', count: attention.length, warn: true },
    { id: 'completed', label: '已完成', count: completedRecords.length },
    { id: 'archived', label: '已归档', count: archivedRecords.length },
  ];
  const emptyCopy: Record<Filter, [string, string]> = {
    active: ['没有进行中的待办', '在上方输入要做的事，按回车保存。'],
    attention: ['没有需要确认的待办', '缺少信息或处理失败的消息会出现在这里。'],
    completed: ['还没有已完成的待办', '完成的待办会保留在这里。'],
    archived: ['没有已归档的待办', '归档的待办可以在这里恢复或删除。'],
  };
  const firstRun = !records.length && !query && filter === 'active';
  const allPicked = displayed.length > 0 && pickedRecords.length === displayed.length;

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark"><FileText size={18} strokeWidth={1.8} /></span><span className="brand-name">拾笺</span></div>
      <nav aria-label="主导航" data-detail-leave>{([{ id: 'inbox', icon: Inbox }, { id: 'connections', icon: Plug }, { id: 'settings', icon: Settings2 }] as const).map(item => <button key={item.id} className={`nav-item ${page === item.id ? 'active' : ''}`} aria-label={pageLabels[item.id]} aria-current={page === item.id ? 'page' : undefined} onClick={() => go(item.id)}><item.icon size={18} strokeWidth={1.8} /><span>{pageLabels[item.id]}</span>{item.id === 'inbox' && attention.length > 0 && <span className="nav-count" title={`${attention.length} 件需确认`}>{attention.length}</span>}</button>)}</nav>
      <div className="sidebar-panel" title="在桌面上显示待办小窗"><AppWindow size={17} strokeWidth={1.8} /><span>桌面面板</span><Switch label="显示桌面面板" checked={snapshot.settings.panelEnabled} disabled={!isDesktop || panelBusy} onChange={async visible => { setPanelBusy(true); try { await bridge.setPanelVisible(visible); await refresh(); } catch (reason) { fail(reason); } finally { setPanelBusy(false); } }} /></div>
      <button className="connection-summary" data-detail-leave onClick={() => go('connections')}><span className={`status-dot ${snapshot.wechatStatus === 'connected' ? 'dot-ok' : snapshot.wechatStatus === 'paused' ? 'dot-warn' : ''}`} /><span>{wechatLabels[snapshot.wechatStatus]}</span></button>
    </aside>
    <main className={`main-content ${page === 'inbox' && selected ? 'wide' : ''}`}>
      {!isDesktop && <div className="preview-banner">浏览器预览 · 数据只保存在当前标签页，微信与模型请使用桌面版</div>}
      {syncError && <div className="banner" role="alert"><CircleAlert size={16} /><span>{syncError}</span><button className="text-button" onClick={retrySync}>重试同步</button></div>}
      {snapshot.modelCredentialError && <div className="banner" role="alert"><CircleAlert size={16} /><span>{snapshot.modelCredentialError}</span><button className="text-button" data-detail-leave onClick={() => go('connections')}>检查模型配置</button></div>}
      <div className="page-heading">
        <div><h1>{pageLabels[page]}</h1>{page === 'inbox' && activeRecords.length > 0 && <p>{activeRecords.length} 件进行中{attention.length > 0 && ` · ${attention.length} 件需确认`}</p>}</div>
        {page === 'inbox' && <button className="button primary" title="Ctrl + N" data-detail-leave onClick={() => void leaveDetail(() => setCreating(true))}><Plus size={16} />新建待办</button>}
      </div>
      {page === 'inbox' ? <>
        {showSetup && <div className="banner"><MessageCircle size={16} /><span>连接微信后，发到微信的待办会自动收进拾笺。</span><button className="text-button" data-detail-leave onClick={() => go('connections')}>去连接</button><button className="icon-button" aria-label="不再提示" onClick={() => { localStorage.setItem(setupHintKey, '1'); setSetupHidden(true); }}><X size={15} /></button></div>}
        <div className={`inbox-layout ${selected ? 'with-detail' : ''}`}>
          <section className="inbox-panel" aria-label="待办列表">
            {picking ? <div className="list-toolbar batch-bar" role="toolbar" aria-label="批量操作">
              <button className="text-button" onClick={() => setPicked(allPicked ? new Set() : new Set(displayed.map(record => record.id)))}>{allPicked ? '取消全选' : '全选'}</button>
              <span className="batch-count">已选 {pickedRecords.length} 项</span>
              <span className="spacer" />
              {(filter === 'active' || filter === 'attention') && <button className="button secondary small" disabled={!pickedRecords.length || batchBusy} onClick={() => void batch('完成', record => bridge.completeRecord(record.id, true))}><CircleCheck size={15} />完成</button>}
              {filter !== 'archived' && <button className="button secondary small" disabled={!pickedRecords.length || batchBusy} onClick={() => void batch('归档', record => bridge.archiveRecord(record.id))}><Archive size={15} />归档</button>}
              {filter === 'archived' && <button className="button secondary small" disabled={!pickedRecords.length || batchBusy} onClick={() => void batch('恢复', record => bridge.restoreRecord(record.id))}><RotateCcw size={15} />恢复</button>}
              {(filter === 'archived' || filter === 'completed') && <button className="button secondary small danger-outline" disabled={!pickedRecords.length || batchBusy} onClick={() => setConfirmDelete(pickedRecords)}><Trash2 size={15} />永久删除</button>}
              <button className="button secondary small" onClick={stopPicking}>完成选择</button>
            </div> : <div className="list-toolbar">
              <div className="filter-tabs" role="group" aria-label="记录筛选" data-detail-leave>{tabs.map(tab => <button key={tab.id} className={filter === tab.id ? 'selected' : ''} aria-pressed={filter === tab.id} onClick={() => changeFilter(tab.id)}>{tab.label}{!!tab.count && <span className={tab.warn ? 'count-warn' : ''}>{tab.count}</span>}</button>)}</div>
              <div className="toolbar-tools">
                <label className="search-box"><Search size={15} /><input ref={searchRef} aria-label="搜索记录" placeholder="搜索" value={search} onChange={e => setSearch(e.target.value)} /></label>
                {filter === 'archived' && archivedRecords.length > 0 && <button className="button secondary small danger-outline" onClick={() => setConfirmDelete(archivedRecords)}><Trash2 size={14} />清空归档</button>}
                <button className="icon-button" aria-label="批量选择" title="批量选择" data-detail-leave disabled={!displayed.length} onClick={() => void leaveDetail(() => { setPicking(true); setSelectedId(null); })}><ListChecks size={18} /></button>
              </div>
            </div>}
            {!picking && filter === 'active' && <QuickAdd onAdd={quickAdd} />}
            {groups.length ? <div className="record-groups">{groups.map(([name, items]) => <div className="record-group" key={name}>{groups.length > 1 && <div className={`group-heading ${name === '已过期' ? 'group-overdue' : ''}`}>{name}<span>{items.length}</span></div>}{items.map(record => <RecordRow key={record.id} record={record} active={selected?.id === record.id} picking={picking} picked={picked.has(record.id)} onOpen={item => void leaveDetail(() => setSelectedId(previous => previous === item.id ? null : item.id))} onToggle={complete} onPick={togglePick} onArchive={archive} onRestore={restore} onDelete={item => setConfirmDelete([item])} />)}</div>)}</div>
              : <div className="empty-state">{firstRun ? <><div className="empty-symbol"><CircleCheck size={26} /></div><h2>还没有待办</h2><p>在上方输入要做的事，或连接微信后直接发消息。</p></>
                : query ? <><div className="empty-symbol"><Search size={24} /></div><h2>没有找到相关待办</h2><p>换个关键词试试。</p><button className="button secondary small" onClick={() => setSearch('')}>清除搜索</button></>
                : <><h2>{emptyCopy[filter][0]}</h2><p>{emptyCopy[filter][1]}</p></>}</div>}
          </section>
          {selected && <DetailPane ref={detailRef} key={selected.id} record={selected} remindersEnabled={snapshot.settings.remindersEnabled} onClose={() => void leaveDetail(() => setSelectedId(null))} onSave={saveRecord} onComplete={complete} onArchive={archive} onRestore={restore} onDelete={record => setConfirmDelete([record])} />}
        </div>
      </> : page === 'connections' ? <div className="page-stack" key="connections">
        <WechatConnection snapshot={snapshot} refresh={refresh} notify={notify} />
        <ModelConfiguration snapshot={snapshot} refresh={refresh} notify={notify} />
      </div> : <AppSettings snapshot={snapshot} refresh={refresh} notify={notify} />}
    </main>
    {creating && <Modal title="新建待办" onClose={closeForm} busy={recordSaving}><RecordForm onBusyChange={setRecordSaving} onClose={closeForm} onSave={async (title, due, priority, time) => { await bridge.createRecord(title, title, due, priority, time); setCreating(false); setPage('inbox'); setFilter('active'); setSearch(''); stopPicking(); await refresh(); notify({ text: '已添加', tone: 'success' }); }} /></Modal>}
    {confirmDelete && <Modal title={confirmDelete.length === 1 ? '永久删除这条待办？' : `永久删除 ${confirmDelete.length} 条待办？`} subtitle={confirmDelete.length === 1 ? `「${confirmDelete[0].title.slice(0, 30)}」删除后无法恢复。` : '删除后无法恢复。'} onClose={() => setConfirmDelete(null)} busy={deleting}><div className="modal-actions"><button className="button secondary" disabled={deleting} onClick={() => setConfirmDelete(null)}>取消</button><button className="button danger-button" disabled={deleting} onClick={removeForever}>{deleting ? '正在删除…' : '永久删除'}</button></div></Modal>}
    {notice && <div className={`toast toast-${notice.tone}`} role={notice.tone === 'error' ? 'alert' : 'status'}>{notice.tone === 'error' ? <CircleAlert size={17} /> : <CircleCheck size={17} />}<span>{notice.text}</span>{notice.undoId && <button onClick={() => { const record = records.find(item => item.id === notice.undoId); if (record) void restore(record); }}>撤销</button>}<button className="icon-button" aria-label="关闭提示" onClick={() => setNotice(null)}><X size={15} /></button></div>}
  </div>;
}
