import { useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { useAppSnapshot } from './useAppSnapshot';
import {
  Archive, Check, CircleAlert, Circle, CircleHelp, ExternalLink, FileText, LoaderCircle, LocateFixed,
  Pin, PinOff, Plus, X,
} from 'lucide-react';
import { bridge, isDesktop } from './bridge';
import { compareByDue, dueLabel } from './due';
import { useLocalDay } from './useLocalDay';
import type { InboxRecord } from './types';
const emptyRecords: InboxRecord[] = [];

function errorText(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

export default function Panel() {
  useLocalDay();
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listInnerRef = useRef<HTMLDivElement>(null);
  const { snapshot, loadError, syncError, refresh, retrySync } = useAppSnapshot(setNotice);
  const measuredSize = useRef('');
  useEffect(() => {
    document.documentElement.style.overflow = 'hidden';
    document.body.style.overflow = 'hidden';
    return () => { document.documentElement.style.overflow = ''; document.body.style.overflow = ''; };
  }, []);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(''), 5000);
    return () => clearTimeout(timer);
  }, [notice]);
  // 高度自适应：几条待办就多高的窗口，超出上限后列表内部滚动。
  useEffect(() => {
    if (!isDesktop) return;
    const shell = shellRef.current;
    const list = listRef.current;
    const inner = listInnerRef.current;
    if (!shell || !list || !inner) return;
    const chrome = shell.offsetHeight - list.offsetHeight;
    const desired = chrome + inner.offsetHeight + 12;
    const clamped = Math.round(Math.min(700, Math.max(240, desired)));
    const size = `320:${clamped}`;
    if (Math.abs(clamped - window.innerHeight) > 14 && measuredSize.current !== size) {
      measuredSize.current = size;
      void bridge.resizePanel(320, clamped).catch(reason => setNotice(errorText(reason)));
    }
  }, [snapshot?.records, snapshot?.modelCredentialError, loadError, syncError, notice, busy]);

  const records = snapshot?.records ?? emptyRecords;
  const pinned = !!snapshot?.settings.panelPinned;
  const active = useMemo(() => records
    .filter(item => item.status !== 'archived' && item.status !== 'completed')
    .sort(compareByDue), [records]);
  const completed = useMemo(() => records
    .filter(item => item.status === 'completed')
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
    .slice(0, 10), [records]);

  async function run(operation: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    try { await operation(); await refresh(); }
    catch (reason) { setNotice(errorText(reason)); }
    finally { setBusy(false); }
  }
  async function add(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || busy) return;
    await run(async () => { await bridge.createRecord(text, text, null, 'normal'); setDraft(''); });
    inputRef.current?.focus();
  }

  if (!snapshot) {
    return <div className="panel-shell" ref={shellRef}><div className="panel-empty"><LoaderCircle size={18} className="spin" /><p>{loadError || '正在打开待办面板'}</p>{loadError && <button className="button secondary small" onClick={() => refresh().catch(reason => setNotice(errorText(reason)))}>重试</button>}</div></div>;
  }
  return <div className="panel-shell" ref={shellRef}>
    <header className="panel-head" data-tauri-drag-region>
      <FileText size={16} className="panel-mark" data-tauri-drag-region />
      <strong data-tauri-drag-region>拾笺</strong>
      <span className="panel-count" data-tauri-drag-region>{active.length} 件待办</span>
      <button className="icon-button" aria-label="重置面板位置" title="回到默认位置" onClick={() => bridge.resetPanelPosition().catch(reason => setNotice(errorText(reason)))}><LocateFixed size={15} /></button>
      <button className="icon-button" aria-label="打开主窗口" title="打开主窗口" onClick={() => bridge.openMainWindow().catch(reason => setNotice(errorText(reason)))}><ExternalLink size={15} /></button>
      <button className={`icon-button ${pinned ? 'pinned' : ''}`} aria-label="置顶面板" aria-pressed={pinned} title={pinned ? '取消置顶' : '置顶：始终显示在其他窗口上面'} onClick={() => void run(() => bridge.setPanelPinned(!pinned))}>{pinned ? <Pin size={15} /> : <PinOff size={15} />}</button>
      <button className="icon-button" aria-label="关闭面板" title="关闭面板（可在主窗口侧边栏或托盘重新打开）" onClick={() => { bridge.setPanelVisible(false).catch(reason => setNotice(errorText(reason))); }}><X size={16} /></button>
    </header>
    <form className="panel-add" onSubmit={add}>
      <input ref={inputRef} value={draft} onChange={event => setDraft(event.target.value)} placeholder="添加待办，按回车保存" aria-label="快速添加待办" maxLength={500} disabled={busy} />
      <button className="button primary small" disabled={busy || !draft.trim()} aria-label="添加待办">{busy ? <LoaderCircle size={14} className="spin" /> : <Plus size={14} />}</button>
    </form>
    {syncError && <p className="panel-notice" role="alert">{syncError}<button className="text-button" onClick={retrySync}>重试同步</button></p>}
    {loadError && <p className="panel-notice" role="alert">刷新失败：{loadError}<button className="text-button" onClick={() => refresh().catch(reason => setNotice(errorText(reason)))}>重试刷新</button></p>}
    {snapshot.modelCredentialError && <p className="panel-notice" role="alert">模型凭据暂不可读，本地待办仍可使用。</p>}
    {notice && <p className="panel-notice" role="status">{notice}</p>}
    <div className="panel-list" ref={listRef}>
      <div ref={listInnerRef}>
      {active.length === 0 && completed.length === 0 && <div className="panel-empty"><p>没有待办</p></div>}
      {active.map(record => {
        const due = dueLabel(record);
        return <div key={record.id} className={`panel-row ${record.status === 'failed' || record.status === 'needs_review' ? 'panel-attention' : ''}`}>
          <button className="panel-check" aria-label={`完成：${record.title}`} disabled={busy} onClick={() => { const target = record; void run(() => bridge.completeRecord(target.id, true)); }}><Circle size={19} strokeWidth={1.7} /></button>
          <div className="panel-row-body">
            <span className="panel-row-title">{record.title}</span>
            <span className="panel-row-meta">
              {record.status === 'needs_review' && <span className="panel-tag tag-review"><CircleHelp size={11} />待确认</span>}
              {record.status === 'failed' && <span className="panel-tag tag-failed"><CircleAlert size={11} />失败</span>}
              {due && <span className={`panel-tag tag-${due.tone}`}>{due.text}</span>}
              {record.source === 'wechat' && '微信'}
            </span>
          </div>
          <button className="icon-button" aria-label={`归档：${record.title}`} title="归档" disabled={busy} onClick={() => { const target = record; void run(() => bridge.archiveRecord(target.id)); }}><Archive size={14} /></button>
        </div>;
      })}
      {completed.length > 0 && <div className="panel-section">已完成</div>}
      {completed.map(record => (
        <div key={record.id} className="panel-row panel-done">
          <button className="panel-check" aria-label={`重新打开：${record.title}`} disabled={busy} onClick={() => { const target = record; void run(() => bridge.completeRecord(target.id, false)); }}><Check size={16} strokeWidth={2} /></button>
          <div className="panel-row-body"><span className="panel-row-title">{record.title}</span></div>
          <button className="icon-button" aria-label={`归档：${record.title}`} title="归档" disabled={busy} onClick={() => { const target = record; void run(() => bridge.archiveRecord(target.id)); }}><Archive size={14} /></button>
        </div>
      ))}
      </div>
    </div>
  </div>;
}
