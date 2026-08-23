import { prefs } from '../stores/prefs';

const KEY = 'cp_table_chart';

export type TableChartPref = 'chart' | 'table';

export function getTableChartPref(): TableChartPref {
  try {
    const v = prefs.getString(KEY);
    return v === 'chart' ? 'chart' : 'table';
  } catch {
    return 'table';
  }
}

export function setTableChartPref(v: TableChartPref): void {
  try {
    prefs.set(KEY, v);
  } catch {
    // modo sem storage: ignora
  }
}
