import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import { api } from '../lib/ipc/api';
import type { ImportPlanImage } from '../lib/ipc/types';
import { groupImportPlanImagesByAlbum, PlanPage } from './PlanPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPlan(plan = { ...importPlanFixture, plan_hash: null as string | null }) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onGoCommit = vi.fn();
  const result = render(
    <QueryClientProvider client={client}>
      <PlanPage
        initialPlan={plan}
        enablePolling={false}
        onNavigate={vi.fn()}
        onGoCommit={onGoCommit}
      />
    </QueryClientProvider>,
  );
  return { ...result, client, onGoCommit };
}

describe('groupImportPlanImagesByAlbum', () => {
  test('groups images and preserves independent include decisions', () => {
    const images: ImportPlanImage[] = [
      {
        image_id: 'a',
        source_path: 'D:/a.jpg',
        relative_path: 'a.jpg',
        file_size: 10,
        album_name: 'A',
        album_id: 'album-a',
        source_album_id: 'album-a',
        included: true,
      },
      {
        image_id: 'b',
        source_path: 'D:/b.jpg',
        relative_path: 'b.jpg',
        file_size: 20,
        album_name: 'A',
        album_id: 'album-a',
        source_album_id: 'album-a',
        included: false,
      },
    ];

    expect(groupImportPlanImagesByAlbum(images)).toMatchObject([
      { albumId: 'album-a', imageCount: 1, skippedImageCount: 1, totalSize: 10 },
    ]);
  });
});

test('renders the restored expandable per-image checklist', () => {
  renderPlan();

  expect(screen.getByRole('heading', { name: '入库调整' })).toBeVisible();
  expect(screen.getByText('图集清单')).toBeVisible();
  expect(screen.getAllByRole('button', { name: '排除整组' }).length).toBeGreaterThan(0);
});

test('locks only on the planning page and does not jump to commit automatically', async () => {
  const draft = { ...importPlanFixture, plan_hash: null };
  const frozen = { ...draft, plan_hash: 'locked-plan-hash' };
  vi.spyOn(api, 'freezeImportPlan').mockResolvedValue(frozen);
  const { onGoCommit } = renderPlan(draft);

  fireEvent.click(screen.getAllByRole('button', { name: '锁定入库计划' })[0]);

  await waitFor(() => expect(api.freezeImportPlan).toHaveBeenCalledWith(draft.import_run_id));
  expect(await screen.findByRole('heading', { name: '入库计划已锁定' })).toBeVisible();
  expect(onGoCommit).not.toHaveBeenCalled();
});

test('reopens a locked plan as a new editable draft through an explicit confirmation', async () => {
  const frozen = { ...importPlanFixture, plan_hash: 'locked-plan-hash' };
  const reopened = { ...frozen, plan_hash: null };
  vi.spyOn(api, 'reopenFrozenImportPlan').mockResolvedValue(reopened);
  renderPlan(frozen);

  fireEvent.click(screen.getByRole('button', { name: '恢复入库调整' }));
  expect(screen.getByText('恢复为可调整计划？')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: '确认恢复调整' }));

  await waitFor(() =>
    expect(api.reopenFrozenImportPlan).toHaveBeenCalledWith(frozen.import_run_id),
  );
  expect(await screen.findByRole('heading', { name: '入库调整' })).toBeVisible();
  expect(screen.getByText('可调整草稿')).toBeVisible();
});
