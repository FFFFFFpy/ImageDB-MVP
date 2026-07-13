import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import type {
  ImportPlanImage,
  ReviewCandidateDetail,
  ReviewCandidateSummary,
  ReviewProgress,
} from '../lib/ipc/types';
import {
  groupImportPlanImagesByAlbum,
  invalidateReviewWorkflowQueries,
  REVIEW_DECISION_OPTIONS,
  ReviewPage,
  shouldIgnoreReviewShortcut,
  zoomViewAtPointer,
} from './ReviewPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function image(overrides: Partial<ImportPlanImage>): ImportPlanImage {
  return {
    image_id: overrides.image_id ?? crypto.randomUUID(),
    source_path: overrides.source_path ?? 'D:/source/image.jpg',
    relative_path: overrides.relative_path ?? 'image.jpg',
    file_size: overrides.file_size ?? 1024,
    album_name: overrides.album_name ?? 'Album',
    album_id: overrides.album_id ?? overrides.album_name ?? 'Album',
    source_album_id:
      overrides.source_album_id ?? overrides.album_id ?? overrides.album_name ?? 'Album',
    included: overrides.included ?? true,
  };
}

describe('groupImportPlanImagesByAlbum', () => {
  test('groups kept import-plan images by album and summarizes count and size', () => {
    const groups = groupImportPlanImagesByAlbum([
      image({ image_id: '1', album_name: 'Album A', relative_path: 'a.jpg', file_size: 100 }),
      image({ image_id: '2', album_name: 'Album B', relative_path: 'b.jpg', file_size: 300 }),
      image({ image_id: '3', album_name: 'Album A', relative_path: 'c.jpg', file_size: 200 }),
      image({
        image_id: '4',
        album_name: 'Album A',
        relative_path: 'skipped.jpg',
        file_size: 900,
        included: false,
      }),
    ]);

    expect(groups).toHaveLength(2);
    expect(groups[0]).toMatchObject({
      albumName: 'Album A',
      imageCount: 2,
      skippedImageCount: 1,
      totalSize: 300,
    });
    expect(groups[0].images.map((img) => img.relative_path)).toEqual([
      'a.jpg',
      'c.jpg',
      'skipped.jpg',
    ]);
    expect(groups[1]).toMatchObject({
      albumName: 'Album B',
      imageCount: 1,
      skippedImageCount: 0,
      totalSize: 300,
    });
  });
});

describe('invalidateReviewWorkflowQueries', () => {
  test('refreshes review, dashboard, and album-status queries after decisions', () => {
    const queryClient = {
      invalidateQueries: vi.fn(),
    };

    invalidateReviewWorkflowQueries(queryClient);

    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['reviewQueue'] });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['reviewProgress'] });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({
      queryKey: ['import-runs-dashboard'],
    });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({
      queryKey: ['database-info-dashboard'],
    });
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith({ queryKey: ['import-run-albums'] });
  });
});

describe('REVIEW_DECISION_OPTIONS', () => {
  test('keeps left, right, and keep-all actions bound to the frozen review semantics', () => {
    expect(REVIEW_DECISION_OPTIONS).toEqual([
      { decision: 'keep_source', shortcut: '1', label: '保留源图片' },
      { decision: 'keep_candidate', shortcut: '2', label: '保留候选图片' },
      { decision: 'keep_all', shortcut: '3', label: '全部保留' },
    ]);
  });

  test('submits the source decision from keyboard shortcut 1', async () => {
    const importRunId = 'keyboard-run';
    const candidate: ReviewCandidateSummary = {
      candidate_id: 'candidate-1',
      source_image_id: 'source-1',
      candidate_source_image_id: 'source-2',
      candidate_library_image_id: null,
      scope: 'intra_album',
      match_type: 'perceptual_near',
      transform_type: 'identity',
      confidence: 0.9,
      album_name: '键盘图集',
      has_decision: false,
    };
    const detail: ReviewCandidateDetail = {
      candidate_id: candidate.candidate_id,
      source_image_id: candidate.source_image_id,
      source_image_path: 'D:/键盘图集/源图片.jpg',
      source_image_file_size: 100,
      source_image_width: 100,
      source_image_height: 100,
      candidate_source_image_id: candidate.candidate_source_image_id,
      candidate_source_image_path: 'D:/键盘图集/候选图片.jpg',
      candidate_source_image_file_size: 100,
      candidate_source_image_width: 100,
      candidate_source_image_height: 100,
      candidate_library_image_id: null,
      candidate_library_image_path: null,
      candidate_library_image_file_size: null,
      candidate_library_image_width: null,
      candidate_library_image_height: null,
      scope: candidate.scope,
      match_type: candidate.match_type,
      blake3_equal: false,
      pixel_hash_equal: false,
      gradient_distance: 2,
      block_distance: 3,
      median_distance: 2,
      transform_type: 'identity',
      confidence: 0.9,
      album_name: candidate.album_name,
      album_id: 'keyboard-album',
      existing_decision: null,
    };
    const progress: ReviewProgress = {
      import_run_id: importRunId,
      total_review_candidates: 1,
      decided_count: 0,
      remaining_count: 1,
      all_decided: false,
    };
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    client.setQueryData(['reviewQueue', importRunId], [candidate]);
    client.setQueryData(['reviewProgress', importRunId], progress);
    client.setQueryData(['reviewDetail', candidate.candidate_id], detail);
    client.setQueryData(['reviewFrozenImportPlanSummary', importRunId], null);
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);

    render(
      <QueryClientProvider client={client}>
        <ReviewPage
          initialImportRunId={importRunId}
          initialPreviews={{
            left: 'data:image/png;base64,AA==',
            right: 'data:image/png;base64,AA==',
          }}
          enablePolling={false}
          onNavigate={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByRole('button', { name: /保留源图片/ })).toBeEnabled();
    fireEvent.keyDown(window, { key: '1' });
    await waitFor(() => expect(submit).toHaveBeenCalledWith(candidate.candidate_id, 'keep_source'));
  });

  test('shows a frozen import plan when the run has no review candidates', async () => {
    const importRunId = importPlanFixture.import_run_id;
    const progress: ReviewProgress = {
      import_run_id: importRunId,
      total_review_candidates: 0,
      decided_count: 0,
      remaining_count: 0,
      all_decided: true,
    };
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    client.setQueryData(['reviewQueue', importRunId], []);
    client.setQueryData(['reviewProgress', importRunId], progress);

    render(
      <QueryClientProvider client={client}>
        <ReviewPage
          initialImportRunId={importRunId}
          initialPlan={importPlanFixture}
          initialShowPlan
          enablePolling={false}
          onNavigate={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByRole('heading', { name: '导入计划' })).toBeVisible();
    expect(screen.queryByText('没有待审核候选')).not.toBeInTheDocument();
  });
});

describe('review workflow hardening', () => {
  test('keeps the pointed image coordinate fixed while zooming', () => {
    const next = zoomViewAtPointer(
      { scale: 1, offsetX: 0, offsetY: 0 },
      175,
      125,
      { left: 100, top: 50, width: 100, height: 100 },
      -1,
    );

    expect(next.scale).toBeCloseTo(1.1);
    expect(next.offsetX).toBeCloseTo(-2.5);
    expect(next.offsetY).toBeCloseTo(-2.5);
  });

  test('guards shortcuts in selects, editable regions, modals, composition, and modifiers', () => {
    const select = document.createElement('select');
    const selectEvent = new KeyboardEvent('keydown', { key: '1' });
    Object.defineProperty(selectEvent, 'target', { value: select });
    expect(shouldIgnoreReviewShortcut(selectEvent, false)).toBe(true);

    const editable = document.createElement('div');
    editable.setAttribute('contenteditable', 'true');
    const editableEvent = new KeyboardEvent('keydown', { key: '1' });
    Object.defineProperty(editableEvent, 'target', { value: editable });
    expect(shouldIgnoreReviewShortcut(editableEvent, false)).toBe(true);
    expect(shouldIgnoreReviewShortcut(new KeyboardEvent('keydown', { key: '1' }), true)).toBe(true);
    expect(
      shouldIgnoreReviewShortcut(new KeyboardEvent('keydown', { key: '1', ctrlKey: true }), false),
    ).toBe(true);
    expect(
      shouldIgnoreReviewShortcut(
        new KeyboardEvent('keydown', { key: '1', isComposing: true }),
        false,
      ),
    ).toBe(true);
  });

  test('shows loading and error states instead of claiming the review queue is empty', async () => {
    const importRunId = 'loading-run';
    let rejectProgress!: (error: Error) => void;
    vi.spyOn(api, 'getReviewQueue').mockReturnValue(new Promise(() => undefined));
    vi.spyOn(api, 'getReviewProgress').mockReturnValue(
      new Promise((_resolve, reject) => {
        rejectProgress = reject;
      }),
    );
    vi.spyOn(api, 'getFrozenImportPlanSummary').mockResolvedValue(null);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });

    render(
      <QueryClientProvider client={client}>
        <ReviewPage initialImportRunId={importRunId} enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    expect(await screen.findByLabelText('正在加载审核数据')).toBeInTheDocument();
    expect(screen.queryByText('没有待审核候选')).not.toBeInTheDocument();

    act(() => rejectProgress(new Error('review progress unavailable')));
    expect(await screen.findByText(/review progress unavailable/)).toBeInTheDocument();
    expect(screen.queryByText('没有待审核候选')).not.toBeInTheDocument();
  });

  test('surfaces frozen-plan generation failures', async () => {
    const importRunId = 'freeze-error-run';
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    client.setQueryData(['reviewQueue', importRunId], []);
    client.setQueryData(['reviewProgress', importRunId], {
      import_run_id: importRunId,
      total_review_candidates: 0,
      decided_count: 0,
      remaining_count: 0,
      all_decided: true,
    });
    client.setQueryData(['reviewFrozenImportPlanSummary', importRunId], null);
    vi.spyOn(api, 'freezeImportPlan').mockRejectedValue(new Error('freeze transaction failed'));

    render(
      <QueryClientProvider client={client}>
        <ReviewPage initialImportRunId={importRunId} enablePolling={false} onNavigate={vi.fn()} />
      </QueryClientProvider>,
    );

    fireEvent.click(await screen.findByRole('button', { name: '生成导入计划' }));
    expect(await screen.findByText(/freeze transaction failed/)).toBeInTheDocument();
  });

  test('blocks plan navigation until the active edit and query refreshes finish', async () => {
    let resolveEdit!: (plan: typeof importPlanFixture) => void;
    const edit = vi.spyOn(api, 'setImportPlanImageIncluded').mockReturnValue(
      new Promise((resolve) => {
        resolveEdit = resolve;
      }),
    );
    vi.spyOn(api, 'getImportPlanImagePreview').mockResolvedValue({ data_url: '' });
    const onGoCommit = vi.fn();
    const onPlanEditPendingChange = vi.fn();
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    const { container } = render(
      <QueryClientProvider client={client}>
        <ReviewPage
          initialImportRunId={importPlanFixture.import_run_id}
          initialPlan={importPlanFixture}
          initialShowPlan
          enablePolling={false}
          onNavigate={vi.fn()}
          onGoCommit={onGoCommit}
          onPlanEditPendingChange={onPlanEditPendingChange}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(await screen.findByText('旅行风光'));
    const imageToggle = await waitFor(() => {
      const element = container.querySelector<HTMLButtonElement>(
        '.import-plan-image-row .plan-toggle',
      );
      expect(element).not.toBeNull();
      return element!;
    });
    fireEvent.click(imageToggle);
    expect(edit).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: '返回审核' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '正在保存计划…' })).toBeDisabled();
    expect(onPlanEditPendingChange).toHaveBeenLastCalledWith(true);

    fireEvent.click(screen.getByRole('button', { name: '正在保存计划…' }));
    expect(onGoCommit).not.toHaveBeenCalled();

    await act(async () => resolveEdit({ ...importPlanFixture, plan_hash: 'updated-hash' }));
    await waitFor(() => expect(screen.getByRole('button', { name: '前往提交确认' })).toBeEnabled());
    expect(onPlanEditPendingChange).toHaveBeenLastCalledWith(false);
    fireEvent.click(screen.getByRole('button', { name: '前往提交确认' }));
    expect(onGoCommit).toHaveBeenCalledWith(importPlanFixture.import_run_id);
    expect(
      client.getQueryData(['frozenImportPlanSummary', importPlanFixture.import_run_id]),
    ).toMatchObject({
      plan_hash: 'updated-hash',
    });
  });

  test('requires confirmation and withdraws a frozen plan without losing review context', async () => {
    const importRunId = importPlanFixture.import_run_id;
    const withdraw = vi.spyOn(api, 'withdrawFrozenImportPlan').mockResolvedValue(undefined);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    client.setQueryData(['reviewQueue', importRunId], []);
    client.setQueryData(['reviewProgress', importRunId], {
      import_run_id: importRunId,
      total_review_candidates: 1,
      decided_count: 1,
      remaining_count: 0,
      all_decided: true,
    });
    client.setQueryData(['reviewFrozenImportPlanSummary', importRunId], importPlanFixture);
    client.setQueryData(['frozenImportPlanSummary', importRunId], importPlanFixture);

    render(
      <QueryClientProvider client={client}>
        <ReviewPage
          initialImportRunId={importRunId}
          initialPlan={importPlanFixture}
          initialShowPlan
          enablePolling={false}
          onNavigate={vi.fn()}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(await screen.findByRole('button', { name: '撤销计划' }));
    expect(withdraw).not.toHaveBeenCalled();
    expect(screen.getByText('确认撤销这份导入计划？')).toBeVisible();
    expect(screen.getByRole('button', { name: '前往提交确认' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: '确认撤销' }));
    await waitFor(() => expect(withdraw).toHaveBeenCalledWith(importRunId));
    expect(await screen.findByText('所有候选已审核')).toBeVisible();
    expect(client.getQueryData(['reviewFrozenImportPlanSummary', importRunId])).toBeNull();
    expect(client.getQueryData(['frozenImportPlanSummary', importRunId])).toBeNull();
  });
});
