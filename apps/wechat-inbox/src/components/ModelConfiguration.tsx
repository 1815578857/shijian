import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { Check, ChevronDown, CircleCheck, LoaderCircle, Search, Sparkles } from 'lucide-react';
import { bridge, isDesktop, validateSettings } from '../bridge';
import type { AppSnapshot, ReasoningEffort } from '../types';
import Modal from './Modal';
import Switch from './Switch';

type Notice = { text: string; tone: 'success' | 'error' };
const effortLabels: Record<ReasoningEffort, string> = { default: '服务默认', none: '关闭思考', low: '低', medium: '中', high: '高', max: '最高' };
function normalizedAddress(value: string) {
  try { return new URL(value.trim()).toString().replace(/\/+$/, ''); } catch { return value.trim(); }
}
function errorText(reason: unknown) { return reason instanceof Error ? reason.message : String(reason); }

function ModelPicker({ models, selected, onSelect, onClose }: { models: string[]; selected: string; onSelect: (id: string) => void; onClose: () => void }) {
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const visible = models.filter(id => id.toLowerCase().includes(query.toLowerCase()));
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => { list.current?.children[active]?.scrollIntoView?.({ block: 'nearest' }); }, [active]);
  return <Modal title="选择模型" subtitle={`共 ${models.length} 个模型`} onClose={onClose}>
    <div className="model-picker-search"><Search size={17} /><input data-autofocus role="combobox" aria-label="搜索模型" aria-expanded="true" aria-controls="model-options" aria-autocomplete="list" aria-activedescendant={visible[active] ? `model-option-${active}` : undefined} value={query} onChange={event => { setQuery(event.target.value); setActive(0); }} onKeyDown={event => {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setActive(index => Math.max(0, Math.min(visible.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))); }
      if (event.key === 'Enter') { event.preventDefault(); if (visible[active]) onSelect(visible[active]); }
    }} placeholder="搜索模型" autoComplete="off" /></div>
    <div className="model-picker-list" id="model-options" role="listbox" aria-label="可用模型" ref={list}>
      {visible.map((id, index) => <button type="button" role="option" aria-selected={id === selected} id={`model-option-${index}`} tabIndex={-1} className={`model-picker-option ${index === active ? 'active' : ''}`} key={id} onMouseMove={() => setActive(index)} onClick={() => onSelect(id)}><span>{id}</span>{id === selected && <Check size={16} />}</button>)}
      {!visible.length && <p className="model-picker-empty">没有匹配的模型</p>}
    </div>
  </Modal>;
}

export default function ModelConfiguration({ snapshot, refresh, notify }: { snapshot: AppSnapshot; refresh: () => Promise<void>; notify: (notice: Notice) => void }) {
  const [form, setForm] = useState(snapshot.settings);
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const [result, setResult] = useState('');
  const [models, setModels] = useState<string[]>([]);
  const [picker, setPicker] = useState(false);
  const [manual, setManual] = useState(false);
  const [removeKey, setRemoveKey] = useState(false);
  const [keyError, setKeyError] = useState('');
  const [capability, setCapability] = useState<{ address: string; model: string; options: ReasoningEffort[] } | null>(null);
  const requestVersion = useRef(0);
  const address = normalizedAddress(form.modelBaseUrl);
  const changedAddress = address !== normalizedAddress(snapshot.settings.modelBaseUrl);
  const credentialUnavailable = !!snapshot.modelCredentialError && !changedAddress;
  const savedKeyAvailable = snapshot.hasModelKey && !changedAddress && !credentialUnavailable;
  const options = capability?.address === address && capability.model === form.modelName ? capability.options : ['default'] as ReasoningEffort[];
  const capabilityReady = capability?.address === address && capability.model === form.modelName;
  const effort = options.includes(form.reasoningEffort) ? form.reasoningEffort : 'default';
  const unsupportedEffort = effort !== form.reasoningEffort;
  const dirty = address !== normalizedAddress(snapshot.settings.modelBaseUrl) || form.modelName.trim() !== snapshot.settings.modelName || effort !== snapshot.settings.reasoningEffort || !!key.trim();
  const configured = !!snapshot.settings.modelBaseUrl && !!snapshot.settings.modelName;
  const fetchDisabled = !isDesktop || !!busy || !form.modelBaseUrl.trim() || (credentialUnavailable && !key.trim());
  const state = busy === 'fetch' ? '正在获取' : busy === 'save' ? '正在保存' : dirty ? '未保存' : configured ? '已配置' : '未配置';

  useEffect(() => {
    let current = true;
    bridge.modelReasoningOptions(address, form.modelName).then(next => {
      if (current) setCapability({ address, model: form.modelName, options: next });
    }).catch(() => { if (current) setCapability({ address, model: form.modelName, options: ['default'] }); });
    return () => { current = false; };
  }, [address, form.modelName]);
  useEffect(() => { return () => { requestVersion.current++; }; }, []);
  useEffect(() => { setModels([]); setResult(''); requestVersion.current++; }, [snapshot.hasModelKey]);

  function invalidate() { requestVersion.current++; setModels([]); setResult(''); setError(''); setPicker(false); }
  function chooseModel(id: string) { setForm(previous => ({ ...previous, modelName: id, reasoningEffort: id === previous.modelName ? previous.reasoningEffort : 'default' })); setPicker(false); setError(''); }
  async function fetchModels() {
    if (fetchDisabled) return;
    setBusy('fetch'); setError(''); setResult(''); setModels([]);
    const version = ++requestVersion.current;
    try {
      validateSettings({ ...snapshot.settings, modelBaseUrl: form.modelBaseUrl });
      const response = await bridge.discoverModels({ baseUrl: address, apiKey: key.trim() || undefined, useSavedKey: savedKeyAvailable && !key.trim() });
      if (version !== requestVersion.current) return;
      setModels(response.models); setResult(response.message);
      if (response.models.length) { setManual(false); setPicker(true); }
    } catch (reason) { if (version === requestVersion.current) setError(errorText(reason)); }
    finally { setBusy(''); }
  }
  async function save(event: FormEvent) {
    event.preventDefault(); if (busy || !dirty || !capabilityReady) return;
    setBusy('save'); setError('');
    let keySaved = false;
    try {
      const next = { modelBaseUrl: address, modelName: form.modelName.trim(), reasoningEffort: effort };
      validateSettings({ ...snapshot.settings, ...next });
      if (key.trim()) { await bridge.saveModelKey(key.trim(), address); keySaved = true; }
      const saved = await bridge.saveSettings(next);
      setForm(saved); setKey('');
      await refresh(); notify({ text: '模型配置已保存', tone: 'success' });
    } catch (reason) { setError(`${keySaved ? '密钥已保存，但其他配置保存失败，请重试。' : ''}${errorText(reason)}`); }
    finally { setBusy(''); }
  }
  async function updateParsing(enabled: boolean) {
    if (busy) return;
    setBusy('parsing'); setError('');
    try {
      await bridge.saveSettings({ parsingEnabled: enabled });
      await refresh(); notify({ text: enabled ? '已开启自动识别' : '已关闭自动识别', tone: 'success' });
    } catch (reason) { setError(errorText(reason)); } finally { setBusy(''); }
  }
  async function deleteKey() {
    if (busy) return; setBusy('delete'); setKeyError('');
    try { await bridge.deleteModelKey(); invalidate(); await refresh(); setRemoveKey(false); notify({ text: '模型密钥已删除', tone: 'success' }); }
    catch (reason) { setKeyError(errorText(reason)); } finally { setBusy(''); }
  }

  return <section className="card model-configuration" aria-label="模型配置">
    <header className="card-head"><span className="card-icon model-icon"><Sparkles size={20} /></span><div><h2>模型</h2><p>用你自己的模型服务，识别微信消息里的待办。</p></div><span className={`pill ${state === '已配置' ? 'pill-ok' : ''}`}>{state}</span></header>
    <form className="card-body model-form" onSubmit={save} aria-busy={!!busy}>
      <fieldset className="model-fields" disabled={!!busy}>
        <label className="field">API 地址<input aria-label="API 地址" type="url" placeholder="https://api.example.com/v1" value={form.modelBaseUrl} onChange={event => { invalidate(); setForm(previous => ({ ...previous, modelBaseUrl: event.target.value })); }} /><span className="field-help">兼容 OpenAI 接口的服务地址</span></label>
        <div className="model-key-row"><label className="field" htmlFor="model-key">API 密钥{(key.trim() || savedKeyAvailable) && <span className="optional">{key.trim() ? '待保存' : '已保存'}</span>}<input id="model-key" type="password" autoComplete="new-password" disabled={!isDesktop} value={key} onChange={event => { invalidate(); setKey(event.target.value); }} placeholder={credentialUnavailable ? '已保存的密钥暂不可读' : savedKeyAvailable ? '留空则使用已保存的密钥' : '粘贴 API Key'} /><span className="field-help">{changedAddress && snapshot.hasModelKey ? '更换地址后需要重新填写密钥' : '密钥加密保存在本机'}</span></label><button type="button" className="button secondary" onClick={fetchModels} disabled={fetchDisabled}>{busy === 'fetch' ? <LoaderCircle size={15} className="spin" /> : <Search size={15} />}{busy === 'fetch' ? '正在获取…' : '获取模型'}</button></div>
        <div className="model-selection-grid"><div className="field"><label htmlFor={manual ? 'manual-model' : 'select-model'}>使用模型</label>{manual ? <input id="manual-model" aria-label="模型 ID" value={form.modelName} maxLength={200} placeholder="填写模型 ID" autoComplete="off" onChange={event => chooseModel(event.target.value)} /> : <button id="select-model" type="button" className="model-select-button" onClick={() => setPicker(true)} disabled={!models.length}><span className={form.modelName ? '' : 'model-placeholder'}>{form.modelName || '获取模型后选择'}</span><ChevronDown size={16} /></button>}<button type="button" className="text-button model-mode-button" onClick={() => setManual(previous => !previous)}>{manual ? '从列表选择' : '手动填写模型 ID'}</button></div>
          <label className="field">思考强度<select aria-label="思考强度" value={effort} onChange={event => setForm(previous => ({ ...previous, reasoningEffort: event.target.value as ReasoningEffort }))} disabled={!capabilityReady || options.length === 1 || !!busy}>{options.map(value => <option key={value} value={value}>{effortLabels[value]}</option>)}</select><span className="field-help">{options.length > 1 ? '强度越高，响应越慢、用量越多' : '该模型不支持调整'}</span></label></div>
      </fieldset>
      {unsupportedEffort && capabilityReady && <p className="model-warning">之前选择的“{effortLabels[form.reasoningEffort]}”档已不适用，保存后改为服务默认。</p>}
      {result && <p role="status" className="model-fetch-result"><CircleCheck size={15} />{result}</p>}
      {credentialUnavailable && <p role="alert" className="model-warning">模型凭据暂时无法读取，不代表密钥未保存。请检查系统凭据服务，或填写密钥后重新保存。</p>}
      {error && <p role="alert" className="inline-error">{error}</p>}
      <div className="form-actions"><div>{(snapshot.hasModelKey || credentialUnavailable) && <button type="button" className="text-button danger" disabled={!!busy} onClick={() => { setKeyError(''); setRemoveKey(true); }}>删除密钥</button>}</div><button className="button primary" disabled={!!busy || !dirty || !capabilityReady}>{busy === 'save' && <LoaderCircle size={15} className="spin" />}{busy === 'save' ? '正在保存…' : '保存配置'}</button></div>
    </form>
    <div className="setting-row card-footer-row"><div><h3>自动识别微信消息</h3><p>{dirty ? '请先保存模型配置。' : !configured ? '完成模型配置后可开启。' : '开启后，微信消息会发送给所选模型，识别出标题、日期和优先级。'}</p></div><Switch label="自动识别微信消息" checked={snapshot.settings.parsingEnabled} onChange={value => void updateParsing(value)} disabled={!!busy || !isDesktop || (!snapshot.settings.parsingEnabled && (!configured || dirty))} /></div>
    {picker && <ModelPicker models={models} selected={form.modelName} onSelect={chooseModel} onClose={() => setPicker(false)} />}
    {removeKey && <Modal title="删除模型密钥？" subtitle="删除后需要重新填写密钥才能使用模型。" onClose={() => setRemoveKey(false)} busy={busy === 'delete'}>{keyError && <p role="alert" className="inline-error">{keyError}</p>}<div className="modal-actions"><button className="button secondary" disabled={!!busy} onClick={() => setRemoveKey(false)}>取消</button><button className="button danger-button" disabled={!!busy} onClick={deleteKey}>{busy === 'delete' ? '正在删除…' : '删除密钥'}</button></div></Modal>}
  </section>;
}
