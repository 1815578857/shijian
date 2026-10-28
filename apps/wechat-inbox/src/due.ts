import type { InboxRecord } from './types';

export type DueTone = 'overdue' | 'today' | 'soon' | 'calm';

function daysUntil(date: string) {
  const due = new Date(`${date}T00:00:00`);
  const today = new Date(new Date().toDateString());
  return Math.round((due.getTime() - today.getTime()) / 86400000);
}

function shortDate(date: string) {
  const due = new Date(`${date}T00:00:00`);
  return due.toLocaleDateString('zh-CN', { year: due.getFullYear() !== new Date().getFullYear() ? 'numeric' : undefined, month: 'numeric', day: 'numeric' });
}

/** 列表和面板共用的截止日期提示。 */
export function dueLabel(record: Pick<InboxRecord, 'dueDate' | 'dueTime'>): { text: string; tone: DueTone } | null {
  if (!record.dueDate) return null;
  const days = daysUntil(record.dueDate);
  const time = record.dueTime ? ` ${record.dueTime}` : '';
  if (days < 0) return { text: `已过期 ${-days} 天`, tone: 'overdue' };
  if (days === 0) return { text: time ? `今天${time}` : '今天到期', tone: 'today' };
  if (days === 1) return { text: time ? `明天${time}` : '明天到期', tone: 'soon' };
  if (days <= 3) return { text: `${days} 天后到期${time}`, tone: 'soon' };
  return { text: `${shortDate(record.dueDate)}${time} 到期`, tone: 'calm' };
}

const dueGroups = ['已过期', '今天', '明天', '7 天内', '以后', '未设截止日期'] as const;

export function compareByDue(a: InboxRecord, b: InboxRecord) {
  return `${a.dueDate ?? '9999-12-31'} ${a.dueTime ?? '99:99'}`.localeCompare(`${b.dueDate ?? '9999-12-31'} ${b.dueTime ?? '99:99'}`) || b.createdAt.localeCompare(a.createdAt);
}

/** 进行中的待办按截止日期分组，组内先到期的在前。 */
export function groupByDue(records: InboxRecord[]) {
  const groups = new Map<string, InboxRecord[]>(dueGroups.map(name => [name, []]));
  const sorted = [...records].sort(compareByDue);
  for (const record of sorted) {
    const days = record.dueDate ? daysUntil(record.dueDate) : null;
    const name = days === null ? '未设截止日期' : days < 0 ? '已过期' : days === 0 ? '今天' : days === 1 ? '明天' : days <= 7 ? '7 天内' : '以后';
    groups.get(name)!.push(record);
  }
  return Array.from(groups.entries()).filter(([, items]) => items.length);
}
