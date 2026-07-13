import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import { ProbesPage } from './ProbesPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderProbesPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <ProbesPage enablePolling={false} />
    </QueryClientProvider>,
  );
}

test('locks PostgreSQL-affecting probes while a critical task is active', async () => {
  vi.spyOn(api, 'getCriticalOperationGuardStatus').mockResolvedValue({
    is_blocked: true,
    blocking_reason: 'Database and library settings are locked while import commit is running',
    active_task_kinds: ['commit'],
    active_operation: null,
  });

  renderProbesPage();

  expect(await screen.findByText('当前任务运行期间已锁定数据库探针')).toBeVisible();
  expect(screen.getByRole('button', { name: '运行全部探针' })).toBeDisabled();
  expect(screen.getByRole('button', { name: '运行数据库探针' })).toBeDisabled();

  fireEvent.click(screen.getByRole('button', { name: '图片指纹' }));
  expect(screen.getByRole('button', { name: '运行指纹探针' })).toBeEnabled();
});

test('enables PostgreSQL probes after confirming that no critical task is active', async () => {
  vi.spyOn(api, 'getCriticalOperationGuardStatus').mockResolvedValue({
    is_blocked: false,
    blocking_reason: null,
    active_task_kinds: [],
    active_operation: null,
  });

  renderProbesPage();

  await waitFor(() => {
    expect(screen.getByRole('button', { name: '运行全部探针' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '运行数据库探针' })).toBeEnabled();
  });
});

test('fails closed when guard status cannot be loaded and supports retry', async () => {
  const guardStatus = vi
    .spyOn(api, 'getCriticalOperationGuardStatus')
    .mockRejectedValueOnce(new Error('guard unavailable'))
    .mockResolvedValue({
      is_blocked: false,
      blocking_reason: null,
      active_task_kinds: [],
      active_operation: null,
    });

  renderProbesPage();

  expect(await screen.findByText('无法确认任务状态，数据库探针已锁定')).toBeVisible();
  expect(screen.getByRole('button', { name: '运行全部探针' })).toBeDisabled();
  expect(screen.getByRole('button', { name: '运行数据库探针' })).toBeDisabled();

  fireEvent.click(screen.getByRole('button', { name: '重试' }));

  await waitFor(() => expect(guardStatus).toHaveBeenCalledTimes(2));
  await waitFor(() => {
    expect(screen.getByRole('button', { name: '运行全部探针' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '运行数据库探针' })).toBeEnabled();
  });
});

test('shows the backend rejection when a stale guard status allows run all', async () => {
  vi.spyOn(api, 'getCriticalOperationGuardStatus').mockResolvedValue({
    is_blocked: false,
    blocking_reason: null,
    active_task_kinds: [],
    active_operation: null,
  });
  vi.spyOn(api, 'runAllProbes').mockRejectedValue(
    new Error('cannot probe managed database while import commit is running'),
  );

  renderProbesPage();

  const runAll = await screen.findByRole('button', { name: '运行全部探针' });
  await waitFor(() => expect(runAll).toBeEnabled());
  fireEvent.click(runAll);

  expect(await screen.findByText('无法运行全部探针')).toBeVisible();
  expect(
    screen.getByText(/cannot probe managed database while import commit is running/),
  ).toBeVisible();
});
