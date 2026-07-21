import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import { PlanPage } from './PlanPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const runId = 'fixture-run-plan';

function renderPlan(props: Partial<React.ComponentProps<typeof PlanPage>> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const result = render(
    <QueryClientProvider client={client}>
      <PlanPage
        initialImportRunId={runId}
        enablePolling={false}
        onNavigate={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
  return { ...result, client };
}

function setupDraftMocks(draftPlan = { ...importPlanFixture, plan_hash: null }) {
  vi.spyOn(api, 'getLatestReviewableImportRun').mockResolvedValue(runId);
  vi.spyOn(api, 'getImportPlanDraftSummary').mockResolvedValue(draftPlan);
  vi.spyOn(api, 'getFrozenImportPlanSummary').mockResolvedValue(null);
  return draftPlan;
}

function setupFrozenMocks(frozenPlan = importPlanFixture) {
  vi.spyOn(api, 'getLatestReviewableImportRun').mockResolvedValue(runId);
  vi.spyOn(api, 'getImportPlanDraftSummary').mockResolvedValue(null);
  vi.spyOn(api, 'getFrozenImportPlanSummary').mockResolvedValue(frozenPlan);
  return frozenPlan;
}

describe('PlanPage draft editing', () => {
  test('renders editable draft with album and image controls', async () => {
    setupDraftMocks();
    renderPlan();

    expect(await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' })).toBeEnabled();
    expect(screen.getByText('尚未锁定')).toBeVisible();
    const lockButtons = screen.getAllByRole('button', { name: '锁定入库计划' });
    expect(lockButtons[0]).toBeEnabled();
  });

  test('toggles source file mode and shows danger banner', async () => {
    const draftPlan = setupDraftMocks();
    const editedDraft = {
      ...draftPlan,
      source_file_mode: 'move_selected_without_backup' as const,
    };
    vi.spyOn(api, 'setImportPlanSourceFileMode').mockResolvedValue(editedDraft);
    renderPlan();

    const toggle = await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    fireEvent.click(toggle);

    await waitFor(() =>
      expect(api.setImportPlanSourceFileMode).toHaveBeenCalledWith(
        runId,
        'move_selected_without_backup',
      ),
    );
    expect(await screen.findByText('不可撤销的源文件操作')).toBeVisible();
  });

  test('excludes an entire album', async () => {
    const draftPlan = setupDraftMocks();
    const editedDraft = { ...draftPlan };
    vi.spyOn(api, 'setImportPlanAlbumIncluded').mockResolvedValue(editedDraft);
    renderPlan();

    const excludeButtons = await screen.findAllByRole('button', { name: '排除整组' });
    fireEvent.click(excludeButtons[0]);

    await waitFor(() =>
      expect(api.setImportPlanAlbumIncluded).toHaveBeenCalledWith(runId, 'album-travel', false),
    );
  });

  test('toggles individual image inclusion', async () => {
    const draftPlan = setupDraftMocks();
    const editedDraft = { ...draftPlan };
    vi.spyOn(api, 'setImportPlanImageIncluded').mockResolvedValue(editedDraft);
    renderPlan();

    await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    const albumDetails = document.querySelector('.plan-album-card') as HTMLElement;
    const summary = albumDetails.querySelector('summary') as HTMLElement;
    fireEvent.click(summary);

    const imageButtons = await screen.findAllByRole('button', { name: /IMG_0001\.jpg/ });
    fireEvent.click(imageButtons[0]);

    await waitFor(() =>
      expect(api.setImportPlanImageIncluded).toHaveBeenCalledWith(
        runId,
        'album-travel-image-1',
        'album-travel',
        false,
      ),
    );
  });

  test('skipped images retain their record and can be re-included', async () => {
    const draftWithSkipped = {
      ...importPlanFixture,
      plan_hash: null,
      albums: importPlanFixture.albums.map((album, index) =>
        index === 0
          ? {
              ...album,
              images: album.images.map((img, imgIndex) =>
                imgIndex === 0 ? { ...img, included: false } : img,
              ),
            }
          : album,
      ),
    };
    setupDraftMocks(draftWithSkipped);
    const reIncludedDraft = { ...draftWithSkipped };
    vi.spyOn(api, 'setImportPlanImageIncluded').mockResolvedValue(reIncludedDraft);
    renderPlan();

    await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    const albumDetails = document.querySelector('.plan-album-card') as HTMLElement;
    const summary = albumDetails.querySelector('summary') as HTMLElement;
    fireEvent.click(summary);

    const imageButtons = await screen.findAllByRole('button', { name: /IMG_0001\.jpg/ });
    expect(imageButtons[0]).toHaveClass('plan-image-row--excluded');
    fireEvent.click(imageButtons[0]);

    await waitFor(() =>
      expect(api.setImportPlanImageIncluded).toHaveBeenCalledWith(
        runId,
        'album-travel-image-1',
        'album-travel',
        true,
      ),
    );
  });
});

describe('PlanPage locking', () => {
  test('locks the draft and navigates to commit page', async () => {
    const draftPlan = setupDraftMocks();
    const frozenPlan = { ...draftPlan, plan_hash: 'locked-hash' };
    vi.spyOn(api, 'freezeImportPlan').mockResolvedValue(frozenPlan);
    const onGoCommit = vi.fn();
    renderPlan({ onGoCommit });

    const lockButtons = await screen.findAllByRole('button', { name: '锁定入库计划' });
    fireEvent.click(lockButtons[0]);

    await waitFor(() => expect(api.freezeImportPlan).toHaveBeenCalledWith(runId));
    await waitFor(() => expect(onGoCommit).toHaveBeenCalledWith(runId));
  });

  test('disables all edit controls after locking', async () => {
    setupFrozenMocks();
    renderPlan();

    const toggle = await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    expect(toggle).toBeDisabled();
    expect(screen.getByText('计划已锁定')).toBeVisible();
    for (const button of screen.getAllByRole('button', { name: '排除整组' })) {
      expect(button).toBeDisabled();
    }
  });

  test('does not allow locking when no images are included', async () => {
    const emptyDraft = {
      ...importPlanFixture,
      plan_hash: null,
      kept_images: [],
      albums: importPlanFixture.albums.map((album) => ({
        ...album,
        images: album.images.map((img) => ({ ...img, included: false })),
      })),
    };
    setupDraftMocks(emptyDraft);
    renderPlan();

    const toggle = await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    expect(toggle).toBeEnabled();
    const lockButtons = screen.getAllByRole('button', { name: '锁定入库计划' });
    for (const button of lockButtons) {
      expect(button).toBeDisabled();
    }
  });
});

describe('PlanPage commit boundary', () => {
  test('does not start commit on page load', async () => {
    vi.spyOn(api, 'startImportCommit').mockResolvedValue('started');
    setupFrozenMocks();
    renderPlan();

    await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    expect(api.startImportCommit).not.toHaveBeenCalled();
  });

  test('navigates to commit page without starting commit', async () => {
    vi.spyOn(api, 'startImportCommit').mockResolvedValue('started');
    setupFrozenMocks();
    const onGoCommit = vi.fn();
    renderPlan({ onGoCommit });

    await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    const commitButtons = screen.getAllByRole('button', { name: '前往确认入库' });
    fireEvent.click(commitButtons[0]);

    expect(onGoCommit).toHaveBeenCalledWith(runId);
    expect(api.startImportCommit).not.toHaveBeenCalled();
  });

  test('abandoning a frozen plan does not create file transactions', async () => {
    vi.spyOn(api, 'abandonFrozenImportWorkflow').mockResolvedValue();
    setupFrozenMocks();
    const onWorkflowAbandoned = vi.fn();
    const onNavigate = vi.fn();
    renderPlan({ onWorkflowAbandoned, onNavigate });

    await screen.findByRole('checkbox', { name: '移动已选源图片（无备份）' });
    fireEvent.click(screen.getByRole('button', { name: '放弃这次导入' }));

    await waitFor(() => expect(api.abandonFrozenImportWorkflow).toHaveBeenCalledWith(runId));
    expect(onWorkflowAbandoned).toHaveBeenCalled();
    expect(onNavigate).toHaveBeenCalledWith('dashboard');
  });
});
