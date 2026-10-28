import { useRef } from 'react';
import { CalendarDays, Clock3, X } from 'lucide-react';

const weekdays = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];

function formatDate(value: string) {
  const date = new Date(`${value}T00:00:00`);
  const today = new Date(new Date().toDateString());
  const days = Math.round((date.getTime() - today.getTime()) / 86400000);
  const relative = days === 0 ? '今天 · ' : days === 1 ? '明天 · ' : days === -1 ? '昨天 · ' : '';
  const year = date.getFullYear() !== today.getFullYear() ? `${date.getFullYear()}年` : '';
  return `${relative}${year}${date.getMonth() + 1}月${date.getDate()}日 ${weekdays[date.getDay()]}`;
}

/** 用可读文字代替原生日期/时间框的“yyyy/mm/日”“--:--”，点击时打开系统选择器。 */
export default function PickerButton({ type, label, value, placeholder, disabled = false, onChange }: {
  type: 'date' | 'time'; label: string; value: string; placeholder: string; disabled?: boolean; onChange: (value: string | null) => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const Icon = type === 'date' ? CalendarDays : Clock3;
  function open() {
    const element = input.current;
    if (!element) return;
    try { element.showPicker(); } catch { element.focus(); }
  }
  return <span className={`picker ${value ? 'has-value' : ''}`}>
    <button type="button" className="picker-button" disabled={disabled} onClick={open}>
      <Icon size={15} />
      <span>{value ? (type === 'date' ? formatDate(value) : value) : placeholder}</span>
    </button>
    {value && !disabled && <button type="button" className="picker-clear" aria-label={`清除${label}`} onClick={() => onChange(null)}><X size={13} /></button>}
    <input ref={input} className="picker-native" type={type} aria-label={label} tabIndex={-1} value={value} disabled={disabled}
      min={type === 'date' ? '1900-01-01' : undefined} max={type === 'date' ? '9999-12-31' : undefined}
      onChange={event => { if (event.target.value) onChange(event.target.value); }} />
  </span>;
}
