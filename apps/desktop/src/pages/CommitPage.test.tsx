import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import { api } from '../lib/ipc/api';
import type { CommitProgress } from '../lib/ipc/types';
import { PLAN_ALBUM_BATCH_SIZE, PLAN_IMAGE_BATCH_SIZE } from '../lib/import-plan-ui';
import {
  CommitPage,
  COMMIT_PIPELINE,
  commitPipelineStepState,
  isTerminalProgress,
} from './CommitPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function progress(state: string): CommitProgress {
  return {
    state,
    import_run_id: 'run-1',
    current_stage: 'preparing',
    current_album: null,
    albums_total: 1,
    albums_completed: 0,
    albums_skipped: 0,
    albums_failed: 0,
    images_committed: 0,
    errors: [],
  };
}

function planForRun(importRunId: string) {
  return {
    ...importPlanFixture,
    import_run_id: importRunId,
    plan_hash: `hash-${importRunId}`,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe('commit progress semantics', () => {
  test('only persisted terminal states end commit polling', () => {
    expect(isTerminalProgress(progress('completed'))).toBe(true);
    expect(isTerminalProgress(progress('failed'))).toBe(true);
    expect(isTerminalProgress(progress('recovery_required'))).toBe(true);
    expect(isTerminalProgress(progress('cancelled'))).toBe(true);
    expect(isTerminalProgress(progress('committing'))).toBe(false);
  });

  test('maps a persisted stage onto the six ordered file-transaction steps', () => {
    expect(COMMIT_PIPELINE.map((step) => step.key)).toEqual([
      'preparing',
      'staging',
      'verifying',
      'publishing',
      'db',
      'archiving',
    ]);
    expect(COMMIT_PIPELINE.map((_, index) => commitPipelineStepState('verifying', index))).toEqual([
      'done',
      'done',
      'active',
      'pending',
      'pending',
      'pending',
    ]);
  });

  test('limits expanded plan albums to a bounded preview batch', () => {
    expect(PLAN_ALBUM_BATCH_SIZE).toBe(50);
    expect(PLAN_ALBUM_BATCH_SIZE).toBeLessThan(1000);
    expect(PLAN_IMAGE_BATCH_SIZE).toBe(24);
    expect(PLAN_IMAGE_BATCH_SIZE).toBeLessThan(100);
  });

  test('requires confirmation before abandoning the whole pending import workflow', async () => {
    const abandon = vi.spyOn(api, 'abandonFrozenImportWorkflow').mockResolvedValue(undefined);
    const onWorkflowAbandoned = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });

    render(
      <QueryClientProvider client={client}>
        <CommitPage
          initialPlan={importPlanFixture}
          initialImportRunId={importPlanFixture.import_run_id}
          enablePolling={false}
          onNavigate={vi.fn()}
          onWorkflowAbandoned={onWorkflowAbandoned}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: '撤销这次导入' }));
    expect(abandon).not.toHaveBeenCalled();
    expect(screen.getByText('确认撤销这次导入任务？')).toBeVisible();
    expect(screen.getByRole('button', { name: '确认并开始入库' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: '撤销并返回工作台' }));
    await waitFor(() => expect(abandon).toHaveBeenCalledWith(importPlanFixture.import_run_id));
    await waitFor(() => expect(onWorkflowAbandoned).toHaveBeenCalledOnce());
  });

  test('prefers an explicit commit run over stale latest-run cache data', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(['latestCommittableImportRun'], 'run-b');
    const getPlan = vi
      .spyOn(api, 'getFrozenImportPlanSummary')
      .mockImplementation(async (runId) => planForRun(runId));
    const startCommit = vi.spyOn(api, 'startImportCommit').mockResolvedValue('commit started');

    render(
      <QueryClientProvider client={client}>
        <CommitPage initialImportRunId="run-a" enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    expect(await screen.findByText('计划哈希：hash-run-a')).toBeVisible();
    expect(getPlan).toHaveBeenCalledTimes(1);
    expect(getPlan).toHaveBeenCalledWith('run-a');
    expect(getPlan).not.toHaveBeenCalledWith('run-b');

    fireEvent.click(screen.getByRole('button', { name: '确认并开始入库' }));
    await waitFor(() => expect(startCommit).toHaveBeenCalledWith('run-a', 'hash-run-a'));
  });

  test('uses the latest committable run only when no explicit run was provided', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.spyOn(api, 'getLatestCommittableImportRun').mockResolvedValue('run-b');
    const getPlan = vi
      .spyOn(api, 'getFrozenImportPlanSummary')
      .mockImplementation(async (runId) => planForRun(runId));
    const startCommit = vi.spyOn(api, 'startImportCommit').mockResolvedValue('commit started');

    render(
      <QueryClientProvider client={client}>
        <CommitPage enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    expect(await screen.findByText('计划哈希：hash-run-b')).toBeVisible();
    expect(getPlan).toHaveBeenCalledWith('run-b');
    fireEvent.click(screen.getByRole('button', { name: '确认并开始入库' }));
    await waitFor(() => expect(startCommit).toHaveBeenCalledWith('run-b', 'hash-run-b'));
  });

  test('shows loading while the latest committable run query is pending', () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.spyOn(api, 'getLatestCommittableImportRun').mockImplementation(() => new Promise(() => {}));

    render(
      <QueryClientProvider client={client}>
        <CommitPage enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    expect(screen.getByRole('status', { name: '正在加载可提交的导入任务' })).toBeVisible();
    expect(screen.queryByText('没有可提交的计划')).not.toBeInTheDocument();
  });

  test('distinguishes latest-run query errors from empty state and supports retry', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const latest = vi
      .spyOn(api, 'getLatestCommittableImportRun')
      .mockRejectedValueOnce(new Error('database unavailable'))
      .mockResolvedValueOnce(null);

    render(
      <QueryClientProvider client={client}>
        <CommitPage enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    expect(await screen.findByText('无法查询可提交任务')).toBeVisible();
    expect(screen.getByText(/database unavailable/)).toBeVisible();
    expect(screen.queryByText('没有可提交的计划')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '重新加载' }));
    await waitFor(() => expect(latest).toHaveBeenCalledTimes(2));
    expect(await screen.findByText('没有可提交的计划')).toBeVisible();
  });

  test('shows the true empty state only after a successful null latest-run response', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    vi.spyOn(api, 'getLatestCommittableImportRun').mockResolvedValue(null);

    render(
      <QueryClientProvider client={client}>
        <CommitPage enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    expect(await screen.findByText('没有可提交的计划')).toBeVisible();
    expect(screen.queryByText('无法查询可提交任务')).not.toBeInTheDocument();
  });

  test('blocks global navigation while commit starts and runs, then releases on terminal state', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const start = deferred<string>();
    vi.spyOn(api, 'startImportCommit').mockReturnValue(start.promise);
    vi.spyOn(api, 'getCommitProgress').mockResolvedValue({
      ...progress('completed'),
      albums_completed: 1,
      images_committed: 1,
    });
    const onNavigationBlockedChange = vi.fn();

    render(
      <QueryClientProvider client={client}>
        <CommitPage
          initialPlan={planForRun('run-a')}
          initialImportRunId="run-a"
          onNavigate={vi.fn()}
          onNavigationBlockedChange={onNavigationBlockedChange}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: '确认并开始入库' }));
    await waitFor(() => expect(onNavigationBlockedChange).toHaveBeenLastCalledWith(true));

    start.resolve('commit started');
    expect(await screen.findByRole('heading', { name: '正在入库' })).toBeVisible();
    expect(onNavigationBlockedChange).toHaveBeenLastCalledWith(true);

    expect(
      await screen.findByRole('heading', { name: '入库结果' }, { timeout: 2500 }),
    ).toBeVisible();
    await waitFor(() => expect(onNavigationBlockedChange).toHaveBeenLastCalledWith(false));
  });
});
