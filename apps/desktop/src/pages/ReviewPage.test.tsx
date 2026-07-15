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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function reviewCandidate(
  candidateId = 'candidate-1',
  albumName = '审核图集',
): ReviewCandidateSummary {
  return {
    candidate_id: candidateId,
    source_image_id: `${candidateId}-source`,
    candidate_source_image_id: `${candidateId}-match`,
    candidate_library_image_id: null,
    scope: 'intra_album',
    match_type: 'perceptual_near',
    transform_type: 'identity',
    confidence: 0.9,
    album_name: albumName,
    has_decision: false,
  };
}

function reviewDetail(
  candidate: ReviewCandidateSummary,
  albumId = `${candidate.candidate_id}-album`,
): ReviewCandidateDetail {
  return {
    candidate_id: candidate.candidate_id,
    source_image_id: candidate.source_image_id,
    source_image_path: `D:/${candidate.album_name}/source.jpg`,
    source_image_file_size: 100,
    source_image_width: 100,
    source_image_height: 100,
    candidate_source_image_id: candidate.candidate_source_image_id,
    candidate_source_image_path: `D:/${candidate.album_name}/candidate.jpg`,
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
    block_distance: 3,
    double_gradient_distance: 4,
    block_distance_ratio: 3 / 256,
    double_gradient_distance_ratio: 4 / 544,
    transform_type: candidate.transform_type,
    confidence: candidate.confidence,
    album_name: candidate.album_name,
    album_id: albumId,
    existing_decision: null,
  };
}

function reviewProgress(importRunId: string, total = 1): ReviewProgress {
  return {
    import_run_id: importRunId,
    total_review_candidates: total,
    decided_count: 0,
    remaining_count: total,
    all_decided: false,
  };
}

function seededReviewClient(
  importRunId: string,
  candidates: ReviewCandidateSummary[],
  details: ReviewCandidateDetail[] = [],
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(['reviewQueue', importRunId], candidates);
  client.setQueryData(
    ['reviewProgress', importRunId],
    reviewProgress(importRunId, candidates.length),
  );
  client.setQueryData(['reviewFrozenImportPlanSummary', importRunId], null);
  details.forEach((detail) => {
    client.setQueryData(['reviewDetail', detail.candidate_id], detail);
  });
  return client;
}

function renderReview(
  client: QueryClient,
  importRunId: string,
  props: Partial<React.ComponentProps<typeof ReviewPage>> = {},
) {
  return render(
    <QueryClientProvider client={client}>
      <ReviewPage
        initialImportRunId={importRunId}
        enablePolling={false}
        onNavigate={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
}

async function loadVisibleReviewPreviews() {
  const source = await screen.findByAltText('源图片');
  const candidate = await screen.findByAltText('候选图片');
  fireEvent.load(source);
  fireEvent.load(candidate);
  await waitFor(() => expect(screen.getByRole('button', { name: /保留源图片/ })).toBeEnabled());
  return { source, candidate };
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
  test('refreshes review, dashboard, and album-status queries with refetch errors enabled', async () => {
    const queryClient = {
      invalidateQueries: vi.fn().mockResolvedValue(undefined),
    };

    await invalidateReviewWorkflowQueries(queryClient);

    expect(queryClient.invalidateQueries).toHaveBeenCalledWith(
      { queryKey: ['reviewQueue'] },
      { throwOnError: true },
    );
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith(
      { queryKey: ['reviewProgress'] },
      { throwOnError: true },
    );
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith(
      {
        queryKey: ['import-runs-dashboard'],
      },
      { throwOnError: true },
    );
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith(
      {
        queryKey: ['database-info-dashboard'],
      },
      { throwOnError: true },
    );
    expect(queryClient.invalidateQueries).toHaveBeenCalledWith(
      { queryKey: ['import-run-albums'] },
      { throwOnError: true },
    );
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
      block_distance: 3,
      double_gradient_distance: 4,
      block_distance_ratio: 3 / 256,
      double_gradient_distance_ratio: 4 / 544,
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
    vi.spyOn(api, 'getReviewQueue').mockResolvedValue([]);
    vi.spyOn(api, 'getReviewProgress').mockResolvedValue({
      ...progress,
      decided_count: 1,
      remaining_count: 0,
      all_decided: true,
    });

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

    const sourcePreview = await screen.findByAltText('源图片');
    const candidatePreview = await screen.findByAltText('候选图片');
    fireEvent.click(screen.getByText('查看图片与匹配详情'));
    expect(screen.getByText('3 / 256（距离 1.2%）')).toBeVisible();
    expect(screen.getByText('4 / 544（距离 0.7%）')).toBeVisible();
    expect(screen.getByText('原方向')).toBeVisible();
    expect(screen.getByText('90.0%')).toBeVisible();
    expect(screen.getByRole('button', { name: /保留源图片/ })).toBeDisabled();
    fireEvent.load(sourcePreview);
    fireEvent.load(candidatePreview);
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

  test('requires confirmation and abandons the whole pending import workflow', async () => {
    const importRunId = importPlanFixture.import_run_id;
    const abandon = vi.spyOn(api, 'abandonFrozenImportWorkflow').mockResolvedValue(undefined);
    const onWorkflowAbandoned = vi.fn();
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
          onWorkflowAbandoned={onWorkflowAbandoned}
        />
      </QueryClientProvider>,
    );

    fireEvent.click(await screen.findByRole('button', { name: '撤销这次导入' }));
    expect(abandon).not.toHaveBeenCalled();
    expect(screen.getByText('确认撤销这次导入任务？')).toBeVisible();
    expect(screen.getByRole('button', { name: '前往提交确认' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: '撤销并返回工作台' }));
    await waitFor(() => expect(abandon).toHaveBeenCalledWith(importRunId));
    await waitFor(() => expect(onWorkflowAbandoned).toHaveBeenCalledOnce());
    expect(client.getQueryData(['reviewFrozenImportPlanSummary', importRunId])).toBeNull();
    expect(client.getQueryData(['frozenImportPlanSummary', importRunId])).toBeNull();
  });

  test('does not submit while the current candidate detail is pending', async () => {
    const importRunId = 'detail-pending-run';
    const candidate = reviewCandidate('detail-pending');
    const pendingDetail = deferred<ReviewCandidateDetail>();
    vi.spyOn(api, 'getReviewCandidateDetail').mockReturnValue(pendingDetail.promise);
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);

    renderReview(seededReviewClient(importRunId, [candidate]), importRunId);

    expect(await screen.findByLabelText('正在加载候选')).toBeVisible();
    fireEvent.keyDown(window, { key: '1' });
    expect(submit).not.toHaveBeenCalled();
  });

  test('shows a retryable detail error and refuses decisions or stale album skips', async () => {
    const importRunId = 'detail-error-run';
    const candidate = reviewCandidate('detail-error');
    const detailRequest = vi
      .spyOn(api, 'getReviewCandidateDetail')
      .mockRejectedValue(new Error('detail database unavailable'));
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);
    const skip = vi.spyOn(api, 'skipReviewAlbum').mockResolvedValue(1);

    renderReview(seededReviewClient(importRunId, [candidate]), importRunId);

    expect(await screen.findByText(/detail database unavailable/)).toBeVisible();
    expect(screen.getByRole('button', { name: '重新加载详情' })).toBeVisible();
    fireEvent.keyDown(window, { key: '1' });
    fireEvent.keyDown(window, { key: '4' });
    expect(submit).not.toHaveBeenCalled();
    expect(skip).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: '重新加载详情' }));
    await waitFor(() => expect(detailRequest).toHaveBeenCalledTimes(2));
  });

  test('rejects a detail whose candidate id does not match the current queue item', async () => {
    const importRunId = 'detail-mismatch-run';
    const current = reviewCandidate('candidate-b');
    const stale = reviewDetail(reviewCandidate('candidate-a'), 'album-a');
    vi.spyOn(api, 'getReviewCandidateDetail').mockResolvedValue(stale);
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);
    const skip = vi.spyOn(api, 'skipReviewAlbum').mockResolvedValue(1);

    renderReview(seededReviewClient(importRunId, [current]), importRunId);

    expect(await screen.findByText('候选详情与当前审核项不匹配')).toBeVisible();
    fireEvent.keyDown(window, { key: '1' });
    fireEvent.keyDown(window, { key: '4' });
    expect(submit).not.toHaveBeenCalled();
    expect(skip).not.toHaveBeenCalled();
  });

  test('keeps decisions disabled while either preview is still loading', async () => {
    const importRunId = 'preview-pending-run';
    const candidate = reviewCandidate('preview-pending');
    vi.spyOn(api, 'getImagePreview').mockImplementation((_candidateId, side) =>
      side === 'source'
        ? new Promise(() => undefined)
        : Promise.resolve({ data_url: 'data:image/png;base64,candidate-ready' }),
    );
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);

    renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
    );

    expect(await screen.findByText('正在加载源图片预览…')).toBeVisible();
    fireEvent.load(await screen.findByAltText('候选图片'));
    expect(screen.getByRole('button', { name: /保留候选图片/ })).toBeDisabled();
    fireEvent.keyDown(window, { key: '2' });
    expect(submit).not.toHaveBeenCalled();
  });

  test('does not let delayed previews from candidate A overwrite candidate B', async () => {
    const importRunId = 'preview-race-run';
    const candidateA = reviewCandidate('candidate-a', '图集 A');
    const candidateB = reviewCandidate('candidate-b', '图集 B');
    const sourceA = deferred<{ data_url: string }>();
    const matchA = deferred<{ data_url: string }>();
    const preview = vi.spyOn(api, 'getImagePreview').mockImplementation((candidateId, side) => {
      if (candidateId === candidateA.candidate_id) {
        return side === 'source' ? sourceA.promise : matchA.promise;
      }
      return Promise.resolve({ data_url: `data:image/png;base64,${candidateId}-${side}` });
    });
    const client = seededReviewClient(
      importRunId,
      [candidateA, candidateB],
      [reviewDetail(candidateA), reviewDetail(candidateB)],
    );

    renderReview(client, importRunId);
    await waitFor(() => expect(preview).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole('button', { name: '下一个 →' }));

    const sourceB = await screen.findByAltText('源图片');
    const matchB = await screen.findByAltText('候选图片');
    expect(sourceB).toHaveAttribute('src', expect.stringContaining('candidate-b-source'));
    expect(matchB).toHaveAttribute('src', expect.stringContaining('candidate-b-candidate'));

    await act(async () => {
      sourceA.resolve({ data_url: 'data:image/png;base64,late-a-source' });
      matchA.resolve({ data_url: 'data:image/png;base64,late-a-candidate' });
      await Promise.resolve();
    });
    expect(screen.getByAltText('源图片')).toHaveAttribute(
      'src',
      expect.stringContaining('candidate-b-source'),
    );
    expect(screen.getByAltText('候选图片')).toHaveAttribute(
      'src',
      expect.stringContaining('candidate-b-candidate'),
    );
  });

  test('shows preview failures and recovers through the side-specific retry action', async () => {
    const importRunId = 'preview-error-run';
    const candidate = reviewCandidate('preview-error');
    let sourceCalls = 0;
    vi.spyOn(api, 'getImagePreview').mockImplementation((_candidateId, side) => {
      if (side === 'source' && sourceCalls++ === 0) {
        return Promise.reject(new Error('source preview unavailable'));
      }
      return Promise.resolve({ data_url: `data:image/png;base64,retry-${side}` });
    });

    renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
    );

    expect(await screen.findByText('无法加载源图片预览')).toBeVisible();
    expect(screen.getByText(/source preview unavailable/)).toBeVisible();
    const candidateImage = await screen.findByAltText('候选图片');
    fireEvent.load(candidateImage);
    fireEvent.click(screen.getByRole('button', { name: '重试源图片预览' }));
    const sourceImage = await screen.findByAltText('源图片');
    fireEvent.load(sourceImage);
    await waitFor(() => expect(screen.getByRole('button', { name: /保留源图片/ })).toBeEnabled());
  });

  test('surfaces decision failures without removing the current candidate', async () => {
    const importRunId = 'decision-error-run';
    const candidate = reviewCandidate('decision-error');
    vi.spyOn(api, 'submitReviewDecision').mockRejectedValue(new Error('decision conflict'));
    const client = seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]);

    renderReview(client, importRunId, {
      initialPreviews: { left: 'data:image/png;base64,left', right: 'data:image/png;base64,right' },
    });
    await loadVisibleReviewPreviews();
    fireEvent.click(screen.getByRole('button', { name: /保留源图片/ }));

    expect(await screen.findByText(/decision conflict/)).toBeVisible();
    expect(screen.getByText(/请重新点击刚才的审核决定重试/)).toBeVisible();
    expect(screen.getByRole('heading', { name: `审核：${candidate.album_name}` })).toBeVisible();
    expect(screen.getByRole('button', { name: /保留源图片/ })).toBeEnabled();
  });

  test('allows only one decision during rapid clicks on different choices', async () => {
    const importRunId = 'rapid-click-run';
    const candidate = reviewCandidate('rapid-click');
    const pendingSubmit = deferred<void>();
    const submit = vi.spyOn(api, 'submitReviewDecision').mockReturnValue(pendingSubmit.promise);
    vi.spyOn(api, 'getReviewQueue').mockResolvedValue([]);
    vi.spyOn(api, 'getReviewProgress').mockResolvedValue({
      ...reviewProgress(importRunId),
      decided_count: 1,
      remaining_count: 0,
      all_decided: true,
    });
    renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
      {
        initialPreviews: {
          left: 'data:image/png;base64,left',
          right: 'data:image/png;base64,right',
        },
      },
    );
    await loadVisibleReviewPreviews();

    fireEvent.click(screen.getByRole('button', { name: /保留源图片/ }));
    fireEvent.click(screen.getByRole('button', { name: /保留候选图片/ }));
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
    expect(submit).toHaveBeenCalledWith(candidate.candidate_id, 'keep_source');

    await act(async () => pendingSubmit.resolve());
  });

  test('allows only one decision during rapid conflicting shortcuts', async () => {
    const importRunId = 'rapid-key-run';
    const candidate = reviewCandidate('rapid-key');
    const pendingSubmit = deferred<void>();
    const submit = vi.spyOn(api, 'submitReviewDecision').mockReturnValue(pendingSubmit.promise);
    vi.spyOn(api, 'getReviewQueue').mockResolvedValue([]);
    vi.spyOn(api, 'getReviewProgress').mockResolvedValue({
      ...reviewProgress(importRunId),
      decided_count: 1,
      remaining_count: 0,
      all_decided: true,
    });
    renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
      {
        initialPreviews: {
          left: 'data:image/png;base64,left',
          right: 'data:image/png;base64,right',
        },
      },
    );
    await loadVisibleReviewPreviews();

    fireEvent.keyDown(window, { key: '1' });
    fireEvent.keyDown(window, { key: '2' });
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
    expect(submit).toHaveBeenCalledWith(candidate.candidate_id, 'keep_source');

    await act(async () => pendingSubmit.resolve());
  });

  test('keeps decision controls locked until queue and progress refreshes finish', async () => {
    const importRunId = 'refresh-lock-run';
    const candidate = reviewCandidate('refresh-lock');
    const queueRefresh = deferred<ReviewCandidateSummary[]>();
    const progressRefresh = deferred<ReviewProgress>();
    vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);
    vi.spyOn(api, 'getReviewQueue').mockReturnValue(queueRefresh.promise);
    vi.spyOn(api, 'getReviewProgress').mockReturnValue(progressRefresh.promise);
    renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
      {
        initialPreviews: {
          left: 'data:image/png;base64,left',
          right: 'data:image/png;base64,right',
        },
      },
    );
    await loadVisibleReviewPreviews();

    fireEvent.click(screen.getByRole('button', { name: /保留源图片/ }));
    await waitFor(() => expect(screen.getByRole('button', { name: '正在保存…' })).toBeDisabled());
    await act(async () => queueRefresh.resolve([]));
    expect(screen.getByRole('button', { name: '正在保存…' })).toBeDisabled();
    await act(async () =>
      progressRefresh.resolve({
        ...reviewProgress(importRunId),
        decided_count: 1,
        remaining_count: 0,
        all_decided: true,
      }),
    );
    expect(await screen.findByRole('heading', { name: '审核完成' })).toBeVisible();
  });

  test('surfaces skip-album failures and keeps the current album actionable', async () => {
    const importRunId = 'skip-error-run';
    const candidate = reviewCandidate('skip-error');
    vi.spyOn(api, 'skipReviewAlbum').mockRejectedValue(new Error('skip album conflict'));
    renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
    );

    const skipButton = await screen.findByRole('button', { name: /跳过图集/ });
    fireEvent.click(skipButton);
    expect(await screen.findByText(/skip album conflict/)).toBeVisible();
    expect(screen.getByText(/请再次点击“跳过图集”重试/)).toBeVisible();
    expect(screen.getByRole('button', { name: /跳过图集/ })).toBeEnabled();
  });

  test('refreshes queue, progress, albums, and dashboard after skipping the current album', async () => {
    const importRunId = 'skip-success-run';
    const candidate = reviewCandidate('skip-success');
    const detail = reviewDetail(candidate, 'current-album');
    const skip = vi.spyOn(api, 'skipReviewAlbum').mockResolvedValue(1);
    vi.spyOn(api, 'getReviewQueue').mockResolvedValue([]);
    vi.spyOn(api, 'getReviewProgress').mockResolvedValue({
      ...reviewProgress(importRunId),
      decided_count: 1,
      remaining_count: 0,
      all_decided: true,
    });
    const client = seededReviewClient(importRunId, [candidate], [detail]);
    const invalidate = vi.spyOn(client, 'invalidateQueries');
    renderReview(client, importRunId);

    fireEvent.click(await screen.findByRole('button', { name: /跳过图集/ }));
    await waitFor(() => expect(skip).toHaveBeenCalledWith(importRunId, 'current-album'));
    await waitFor(() => expect(screen.getByRole('heading', { name: '审核完成' })).toBeVisible());
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ['reviewQueue'] }, { throwOnError: true });
    expect(invalidate).toHaveBeenCalledWith(
      { queryKey: ['reviewProgress'] },
      { throwOnError: true },
    );
    expect(invalidate).toHaveBeenCalledWith(
      { queryKey: ['import-runs-dashboard'] },
      { throwOnError: true },
    );
    expect(invalidate).toHaveBeenCalledWith(
      { queryKey: ['database-info-dashboard'] },
      { throwOnError: true },
    );
    expect(invalidate).toHaveBeenCalledWith(
      { queryKey: ['import-run-albums'] },
      { throwOnError: true },
    );
  });

  test('does not allow a conflicting retry when skip succeeds but queue refresh fails', async () => {
    const importRunId = 'skip-refresh-error-run';
    const candidate = reviewCandidate('skip-refresh-error');
    const detail = reviewDetail(candidate, 'already-skipped-album');
    const skip = vi.spyOn(api, 'skipReviewAlbum').mockResolvedValue(1);
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);
    vi.spyOn(api, 'getReviewQueue').mockRejectedValue(new Error('queue refresh failed'));
    vi.spyOn(api, 'getReviewProgress').mockResolvedValue(reviewProgress(importRunId));
    const client = seededReviewClient(importRunId, [candidate], [detail]);

    renderReview(client, importRunId, {
      initialPreviews: {
        left: 'data:image/png;base64,left',
        right: 'data:image/png;base64,right',
      },
    });
    await loadVisibleReviewPreviews();

    fireEvent.click(screen.getByRole('button', { name: /跳过图集/ }));

    expect(await screen.findByText('审核操作可能已保存')).toBeVisible();
    expect(screen.getAllByText(/queue refresh failed/)).toHaveLength(2);
    expect(screen.getByText(/请重新加载审核页确认，不要重复提交/)).toBeVisible();
    expect(skip).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: /跳过图集/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /保留源图片/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重新加载审核数据' })).toBeVisible();
    fireEvent.keyDown(window, { key: '1' });
    fireEvent.keyDown(window, { key: '4' });
    expect(submit).not.toHaveBeenCalled();
    expect(skip).toHaveBeenCalledTimes(1);
  });

  test('keeps skip locked when a decision saves but the real queue refetch fails', async () => {
    const importRunId = 'decision-refresh-error-run';
    const candidate = reviewCandidate('decision-refresh-error');
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);
    const skip = vi.spyOn(api, 'skipReviewAlbum').mockResolvedValue(1);
    vi.spyOn(api, 'getReviewQueue').mockRejectedValue(new Error('decision queue refresh failed'));
    vi.spyOn(api, 'getReviewProgress').mockResolvedValue(reviewProgress(importRunId));
    const client = seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]);

    renderReview(client, importRunId, {
      initialPreviews: {
        left: 'data:image/png;base64,left',
        right: 'data:image/png;base64,right',
      },
    });
    await loadVisibleReviewPreviews();

    fireEvent.click(screen.getByRole('button', { name: /保留源图片/ }));

    expect(await screen.findByText('审核操作可能已保存')).toBeVisible();
    expect(screen.getAllByText(/decision queue refresh failed/)).toHaveLength(2);
    expect(submit).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: /保留源图片/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /跳过图集/ })).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: '2' });
    fireEvent.keyDown(window, { key: '4' });
    expect(submit).toHaveBeenCalledTimes(1);
    expect(skip).not.toHaveBeenCalled();
  });

  test('does not attach a late refresh failure from candidate A to candidate B', async () => {
    const importRunId = 'late-refresh-error-run';
    const candidateA = reviewCandidate('late-refresh-a', '图集 A');
    const candidateB = reviewCandidate('late-refresh-b', '图集 B');
    const lateFailure = deferred<void>();
    const submit = vi.spyOn(api, 'submitReviewDecision').mockResolvedValue(undefined);
    const client = seededReviewClient(
      importRunId,
      [candidateA, candidateB],
      [reviewDetail(candidateA), reviewDetail(candidateB)],
    );
    vi.spyOn(client, 'invalidateQueries').mockImplementation(async (filters) => {
      const key = filters?.queryKey?.[0];
      if (key === 'reviewQueue') {
        client.setQueryData(['reviewQueue', importRunId], [candidateB]);
      }
      if (key === 'import-runs-dashboard') {
        return lateFailure.promise;
      }
    });

    renderReview(client, importRunId, {
      initialPreviews: {
        left: 'data:image/png;base64,left',
        right: 'data:image/png;base64,right',
      },
    });
    await loadVisibleReviewPreviews();
    fireEvent.click(screen.getByRole('button', { name: /保留源图片/ }));

    expect(await screen.findByRole('heading', { name: '审核：图集 B' })).toBeVisible();
    await act(async () => lateFailure.reject(new Error('late dashboard refresh failed')));
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
    expect(screen.queryByText('审核操作可能已保存')).not.toBeInTheDocument();
    expect(screen.queryByText(/late dashboard refresh failed/)).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: '审核：图集 B' })).toBeVisible();
  });

  test('registers non-passive wheel listeners and removes the same handlers on unmount', async () => {
    const add = vi.spyOn(HTMLElement.prototype, 'addEventListener');
    const remove = vi.spyOn(HTMLElement.prototype, 'removeEventListener');
    const importRunId = 'wheel-listener-run';
    const candidate = reviewCandidate('wheel-listener');
    vi.spyOn(api, 'getImagePreview').mockReturnValue(new Promise(() => undefined));
    const { unmount } = renderReview(
      seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]),
      importRunId,
    );

    await screen.findByText('正在加载源图片预览…');
    const registrations = add.mock.calls.filter(
      ([type, _listener, options]) =>
        type === 'wheel' && typeof options === 'object' && options?.passive === false,
    );
    expect(registrations).toHaveLength(2);
    unmount();
    registrations.forEach(([_type, listener]) => {
      expect(remove).toHaveBeenCalledWith('wheel', listener);
    });
  });

  test('prevents wheel scrolling inside both review modes but leaves outside wheel events alone', async () => {
    const importRunId = 'wheel-behavior-run';
    const candidate = reviewCandidate('wheel-behavior');
    const client = seededReviewClient(importRunId, [candidate], [reviewDetail(candidate)]);
    const { container } = render(
      <QueryClientProvider client={client}>
        <main className="app-main">
          <ReviewPage
            initialImportRunId={importRunId}
            initialPreviews={{
              left: 'data:image/png;base64,left',
              right: 'data:image/png;base64,right',
            }}
            enablePolling={false}
            onNavigate={vi.fn()}
          />
        </main>
      </QueryClientProvider>,
    );
    const { source, candidate: candidateImage } = await loadVisibleReviewPreviews();
    const appMain = container.querySelector<HTMLElement>('.app-main')!;
    appMain.scrollTop = 180;
    const imageRegions = Array.from(
      container.querySelectorAll<HTMLElement>('.review-image-container'),
    );
    expect(imageRegions).toHaveLength(2);
    const [sourceRegion, candidateRegion] = imageRegions;
    const imageRect = {
      left: 0,
      top: 0,
      width: 400,
      height: 300,
      right: 400,
      bottom: 300,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    };
    vi.spyOn(sourceRegion, 'getBoundingClientRect').mockReturnValue(imageRect);
    vi.spyOn(candidateRegion, 'getBoundingClientRect').mockReturnValue(imageRect);
    const insideWheel = new WheelEvent('wheel', {
      bubbles: true,
      cancelable: true,
      clientX: 200,
      clientY: 150,
      deltaY: -1,
    });
    sourceRegion.dispatchEvent(insideWheel);
    expect(insideWheel.defaultPrevented).toBe(true);
    expect(appMain.scrollTop).toBe(180);
    await waitFor(() => expect(source.style.transform).toContain('scale(1.1)'));

    fireEvent.click(screen.getByRole('button', { name: '重置视图' }));
    await waitFor(() => expect(source.style.transform).toContain('scale(1)'));
    const candidateWheel = new WheelEvent('wheel', {
      bubbles: true,
      cancelable: true,
      clientX: 200,
      clientY: 150,
      deltaY: -1,
    });
    candidateRegion.dispatchEvent(candidateWheel);
    expect(candidateWheel.defaultPrevented).toBe(true);
    expect(appMain.scrollTop).toBe(180);
    await waitFor(() => {
      expect(source.style.transform).toContain('scale(1.1)');
      expect(candidateImage.style.transform).toContain('scale(1.1)');
    });

    fireEvent.click(screen.getByRole('button', { name: '重置视图' }));
    await waitFor(() => expect(source.style.transform).toContain('scale(1)'));
    fireEvent.click(screen.getByRole('button', { name: '叠加比较' }));
    const overlayWheel = new WheelEvent('wheel', {
      bubbles: true,
      cancelable: true,
      clientX: 200,
      clientY: 150,
      deltaY: -1,
    });
    sourceRegion.dispatchEvent(overlayWheel);
    expect(overlayWheel.defaultPrevented).toBe(true);
    await waitFor(() => {
      expect(source.style.transform).toContain('scale(1.1)');
      expect(candidateImage.style.transform).toContain('scale(1.1)');
    });

    const outsideWheel = new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: 50 });
    appMain.dispatchEvent(outsideWheel);
    expect(outsideWheel.defaultPrevented).toBe(false);
  });
});
