import { describe, expect, it } from 'vitest';
import { compareByDue, groupByDue } from './due';
import type { InboxRecord } from './types';

function record(id: string, dueDate: string | null, dueTime: string | null, createdAt = '2030-01-01T00:00:00Z'): InboxRecord {
  return { id, title: id, originalText: id, dueDate, dueTime, createdAt, priority: 'normal', status: 'saved', source: 'local', error: null, isDemo: false };
}

describe('shared due ordering', () => {
  it('orders by date, time, then newest creation without mutating records', () => {
    const records = [record('undated', null, null), record('date-only', '2030-01-02', null), record('late', '2030-01-02', '16:00'), record('early-old', '2030-01-02', '09:00'), record('early-new', '2030-01-02', '09:00', '2030-01-01T01:00:00Z'), record('next-day', '2030-01-03', '08:00')];
    const original = records.map(item => item.id);
    const ordered = [...records].sort(compareByDue).map(item => item.id);
    expect(ordered).toEqual(['early-new', 'early-old', 'late', 'date-only', 'next-day', 'undated']);
    expect(groupByDue(records).flatMap(([, items]) => items.map(item => item.id))).toEqual(ordered);
    expect(records.map(item => item.id)).toEqual(original);
  });
});
