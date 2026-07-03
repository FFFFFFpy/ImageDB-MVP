import type { DiagnosticItem, TaggedStatus } from './ipc/types';

const statusLabels: Record<string, string> = {
  not_initialized: '未初始化',
  initializing: '初始化中',
  ready: '就绪',
  connected: '已连接',
  Error: '错误',
  BinariesMissing: '缺少 PostgreSQL 运行文件',
};

function snakeCase(input: string): string {
  return input.replace(/[A-Z]/g, (m, i) => `${i === 0 ? '' : '_'}${m.toLowerCase()}`);
}

export function taggedStatusCode(status: TaggedStatus | null | undefined): string {
  if (typeof status === 'string') return status;
  if (!status) return 'unknown';
  const [key] = Object.keys(status);
  return key ? snakeCase(key) : 'unknown';
}

export function formatTaggedStatus(status: TaggedStatus | null | undefined): string {
  if (typeof status === 'string') {
    return statusLabels[status] ?? status;
  }
  if (!status) return '未知';
  const [[key, value] = []] = Object.entries(status);
  if (!key) return '未知';
  const label = statusLabels[key] ?? snakeCase(key);
  return value ? `${label}: ${value}` : label;
}

export function formatDiagnostic(item: DiagnosticItem): string {
  if (item === null) return 'null';
  if (typeof item === 'string') return item;
  if (typeof item === 'number' || typeof item === 'boolean') return String(item);
  const [[key, value] = []] = Object.entries(item);
  if (!key) return JSON.stringify(item);
  return value === undefined || value === null
    ? key
    : `${key}: ${typeof value === 'object' ? JSON.stringify(value) : String(value)}`;
}
