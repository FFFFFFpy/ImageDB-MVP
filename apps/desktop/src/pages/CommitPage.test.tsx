import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import { api } from '../lib/ipc/api';
import type { CommitProgress, ImportPlan } from '../lib/ipc/types';
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

function planForRun(importRunId: string): ImportPlan {
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

function renderCommit(
  props: Partial<React.ComponentProps<typeof CommitPage>> = {},
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <CommitPage
        initialImportRunId="run-a"
        enablePolling={false}
        onNavigate={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
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

  test('loads only the explicit Frozen plan and never starts Commit on load', async () => {
    const getPlan = vi
      .spyOn(api, 'getFrozenImportPlanSummary')
      .mockResolvedValue(planForRun('run-a'));
    const latest = vi.spyOn(api, 'getLatestCommittableImportRun');
    const startCommit = vi.spyOn(api, 'startImportCommit').mockResolvedValue('commit started');

    renderCommit();

    expect(await screen.findByText('计划哈希：hash-run-a')).toBeVisible();
    expect(getPlan).toHaveBeenCalledExactlyOnceWith('run-a');
    expect(startCommit).not.toHaveBeenCalled();
    expect(latest).not.toHaveBeenCalled();
  });

  test('starts Commit only after the explicit confirmation click', async () => {
    const startCommit = vi.spyOn(api, 'startImportCommit').mockResolvedValue('commit started');
    renderCommit({ initialPlan: planForRun('run-a'), fileTransactionCount: 0 });

    expect(startCommit).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: '确认并开始入库' }));

    await waitFor(() =>
      expect(startCommit).toHaveBeenCalledExactlyOnceWith('run-a', 'hash-run-a'),
    );
  });

  test('coalesces a double click into one Commit start request', async () => {
    const start = deferred<string>();
    const startCommit = vi.spyOn(api, 'startImportCommit').mockReturnValue(start.promise);
    renderCommit({ initialPlan: planForRun('run-a'), fileTransactionCount: 0 });

    const button = screen.getByRole('button', { name: '确认并开始入库' });
    fireEvent.click(button);
    fireEvent.click(button);

    await waitFor(() => expect(startCommit).toHaveBeenCalledTimes(1));
    start.resolve('commit started');
    expect(await screen.findByRole('heading', { name: '正在入库' })).toBeVisible();
  });

  test('blocks confirmation when a file transaction already exists', () => {
    const startCommit = vi.spyOn(api, 'startImportCommit').mockResolvedValue('commit started');
    renderCommit({ initialPlan: planForRun('run-a'), fileTransactionCount: 1 });

    expect(screen.getByText('检测到已有文件事务，禁止再次启动')).toBeVisible();
    expect(screen.getByRole('button', { name: '确认并开始入库' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: '确认并开始入库' }));
    expect(startCommit).not.toHaveBeenCalled();
  });

  test('renders the Frozen plan as a read-only source-to-target allocation', () => {
    renderCommit({ initialPlan: planForRun('run-a') });

    expect(screen.getByRole('heading', { name: '最后一次只读审阅' })).toBeVisible();
    expect(screen.getByText('D:/ImageDB/Library')).toBeVisible();
    expect(screen.getByText('复制并归档')).toBeVisible();
    expect(screen.getByText('确认边界已就绪：文件事务数量为 0')).toBeVisible();
    expect(screen.queryByRole('radio')).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    for (const name of ['导入', '跳过', '移动到其他目标图集', '保存路径', '解锁', '返回编辑']) {
      expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
    }
  });

  test('requires a second confirmation before abandoning a Frozen workflow', async () => {
    const abandon = vi.spyOn(api, 'abandonFrozenImportWorkflow').mockResolvedValue(undefined);
    const onWorkflowAbandoned = vi.fn();
    renderCommit({
      initialPlan: planForRun('run-a'),
      onWorkflowAbandoned,
    });

    fireEvent.click(screen.getByRole('button', { name: '放弃本次导入' }));
    expect(abandon).not.toHaveBeenCalled();
    expect(screen.getByText('确认放弃本次导入？')).toBeVisible();
    expect(screen.getByRole('button', { name: '确认并开始入库' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: '确认放弃并返回工作台' }));
    await waitFor(() => expect(abandon).toHaveBeenCalledWith('run-a'));
    await waitFor(() => expect(onWorkflowAbandoned).toHaveBeenCalledOnce());
  });

  test('shows an explicit empty state when no runId is supplied', () => {
    const latest = vi.spyOn(api, 'getLatestCommittableImportRun');
    const getPlan = vi.spyOn(api, 'getFrozenImportPlanSummary');
    renderCommit({ initialImportRunId: null });

    expect(screen.getByText('没有指定可提交任务')).toBeVisible();
    expect(latest).not.toHaveBeenCalled();
    expect(getPlan).not.toHaveBeenCalled();
  });

  test('blocks global navigation while Commit starts and runs, then releases on terminal state', async () => {
    const start = deferred<string>();
    vi.spyOn(api, 'startImportCommit').mockReturnValue(start.promise);
    vi.spyOn(api, 'getCommitProgress').mockResolvedValue({
      ...progress('completed'),
      albums_completed: 1,
      images_committed: 1,
    });
    const onNavigationBlockedChange = vi.fn();

    renderCommit({
      initialPlan: planForRun('run-a'),
      enablePolling: true,
      onNavigationBlockedChange,
    });

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
