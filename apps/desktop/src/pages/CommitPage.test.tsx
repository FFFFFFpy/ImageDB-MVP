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

  test('requires confirmation before withdrawing the plan from commit confirmation', async () => {
    const withdraw = vi.spyOn(api, 'withdrawFrozenImportPlan').mockResolvedValue(undefined);
    const onGoReview = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });

    render(
      <QueryClientProvider client={client}>
        <CommitPage
          initialPlan={importPlanFixture}
          initialImportRunId={importPlanFixture.import_run_id}
          enablePolling={false}
          onNavigate={vi.fn()}
          onGoReview={onGoReview}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: '撤销计划' }));
    expect(withdraw).not.toHaveBeenCalled();
    expect(screen.getByText('确认撤销这份导入计划？')).toBeVisible();
    expect(screen.getByRole('button', { name: '确认并开始入库' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: '确认撤销' }));
    await waitFor(() => expect(withdraw).toHaveBeenCalledWith(importPlanFixture.import_run_id));
    await waitFor(() => expect(onGoReview).toHaveBeenCalledWith(importPlanFixture.import_run_id));
  });
});
