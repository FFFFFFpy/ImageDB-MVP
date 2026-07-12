import { describe, expect, test } from 'vitest';
import type { CommitProgress } from '../lib/ipc/types';
import { PLAN_ALBUM_BATCH_SIZE, PLAN_IMAGE_BATCH_SIZE } from '../lib/import-plan-ui';
import { COMMIT_PIPELINE, commitPipelineStepState, isTerminalProgress } from './CommitPage';

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
});
