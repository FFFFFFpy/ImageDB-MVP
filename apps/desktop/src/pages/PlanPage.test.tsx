import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import type { ImportPlan } from '../lib/ipc/types';
import { PlanPage } from './PlanPage';

const runId = '11111111-1111-1111-1111-111111111111';
const albumA = '22222222-2222-2222-2222-222222222222';
const albumB = '33333333-3333-3333-3333-333333333333';

function draftPlan(): ImportPlan {
  const images = [
    {
      image_id: 'image-a',
      source_path: 'D:/Source/A/a.jpg',
      relative_path: 'a.jpg',
      file_size: 100,
      source_album_name: '源图集 A',
      album_name: '目标图集 A',
      album_id: albumA,
      source_album_id: albumA,
      included: true,
    },
    {
      image_id: 'image-b',
      source_path: 'D:/Source/A/b.jpg',
      relative_path: 'nested/b.jpg',
      file_size: 200,
      source_album_name: '源图集 A',
      album_name: '目标图集 A',
      album_id: albumA,
      source_album_id: albumA,
      included: true,
    },
    {
      image_id: 'image-c',
      source_path: 'D:/Source/B/c.jpg',
      relative_path: 'c.jpg',
      file_size: 300,
      source_album_name: '源图集 B',
      album_name: '目标图集 B',
      album_id: albumB,
      source_album_id: albumB,
      included: false,
    },
  ];
  return {
    import_run_id: runId,
    plan_hash: null,
    source_file_mode: 'copy_and_archive',
    library_root_path: 'D:/ImageDB/Library',
    total_albums: 2,
    total_images: 3,
    kept_images: images.filter((image) => image.included),
    excluded_count: 1,
    skipped_albums: ['源图集 B'],
    albums: [
      {
        album_id: albumA,
        source_album_name: '源图集 A',
        album_name: '目标图集 A',
        included: true,
        image_count: 2,
        total_size: 300,
        images: images.slice(0, 2),
      },
      {
        album_id: albumB,
        source_album_name: '源图集 B',
        album_name: '目标图集 B',
        included: false,
        image_count: 0,
        total_size: 0,
        images: images.slice(2),
      },
    ],
  };
}

function withImageIncluded(plan: ImportPlan, imageId: string, included: boolean): ImportPlan {
  const albums = plan.albums.map((album) => {
    const images = album.images.map((image) =>
      image.image_id === imageId ? { ...image, included } : image,
    );
    return {
      ...album,
      images,
      included: images.some((image) => image.included),
      image_count: images.filter((image) => image.included).length,
    };
  });
  const allImages = albums.flatMap((album) => album.images);
  return {
    ...plan,
    albums,
    kept_images: allImages.filter((image) => image.included),
    excluded_count: allImages.filter((image) => !image.included).length,
  };
}

function renderPlan(
  props: Partial<React.ComponentProps<typeof PlanPage>> = {},
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <PlanPage
        initialImportRunId={runId}
        initialPlan={draftPlan()}
        onNavigate={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
}

function albumDetails(name: string): HTMLElement {
  const heading = screen
    .getAllByText(name)
    .find((element) => element.tagName.toLowerCase() === 'strong');
  if (!heading) throw new Error(`album heading ${name} not found`);
  return heading.closest('details') as HTMLElement;
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

test('reloads the same Draft by explicit runId and exposes editable controls', async () => {
  const getDraft = vi.spyOn(api, 'getImportPlanDraftSummary').mockResolvedValue(draftPlan());
  renderPlan({ initialPlan: null });

  expect(await screen.findByRole('heading', { name: '人工复核入库计划' })).toBeVisible();
  expect(getDraft).toHaveBeenCalledExactlyOnceWith(runId);
  expect(await screen.findByRole('radio', { name: '复制并归档' })).toBeEnabled();
  expect(screen.getAllByRole('button', { name: '锁定入库计划' })[0]).toBeEnabled();
});

test('skipping an image keeps its row and re-enabling restores the previous allocation', async () => {
  const original = draftPlan();
  const skipped = withImageIncluded(original, 'image-a', false);
  const setIncluded = vi
    .spyOn(api, 'setImportPlanImageIncluded')
    .mockResolvedValueOnce(skipped)
    .mockResolvedValueOnce(original);
  renderPlan({ initialPlan: original });

  fireEvent.click(albumDetails('源图集 A').querySelector('summary') as HTMLElement);
  const imageCard = screen.getByText('a.jpg').closest('article') as HTMLElement;
  const targetPath = within(imageCard).getByLabelText('目标相对路径');
  expect(targetPath).toHaveValue('a.jpg');

  fireEvent.click(within(imageCard).getByRole('button', { name: '跳过' }));
  await waitFor(() => expect(setIncluded).toHaveBeenCalledTimes(1));
  expect(screen.getByText('a.jpg').closest('article')).toHaveClass('plan-image-editor--excluded');
  expect(within(screen.getByText('a.jpg').closest('article') as HTMLElement).getByLabelText(
    '目标相对路径',
  )).toHaveValue('a.jpg');

  fireEvent.click(
    within(screen.getByText('a.jpg').closest('article') as HTMLElement).getByRole('button', {
      name: '导入',
    }),
  );
  await waitFor(() => expect(setIncluded).toHaveBeenCalledTimes(2));
  expect(setIncluded).toHaveBeenNthCalledWith(2, runId, 'image-a', albumA, true);
  expect(screen.getByText('a.jpg').closest('article')).not.toHaveClass(
    'plan-image-editor--excluded',
  );
});

test('supports whole-album import and skip with visible counts', async () => {
  const original = draftPlan();
  const skippedAlbum = {
    ...original,
    albums: original.albums.map((album) =>
      album.album_id === albumA
        ? {
            ...album,
            included: false,
            image_count: 0,
            total_size: 0,
            images: album.images.map((image) => ({ ...image, included: false })),
          }
        : album,
    ),
    kept_images: [],
    excluded_count: 3,
  };
  const setAlbum = vi
    .spyOn(api, 'setImportPlanAlbumIncluded')
    .mockResolvedValueOnce(skippedAlbum)
    .mockResolvedValueOnce(original);
  renderPlan({ initialPlan: original });

  const albumCard = albumDetails('源图集 A');
  fireEvent.click(albumCard.querySelector('summary') as HTMLElement);
  fireEvent.click(within(albumCard).getByRole('button', { name: '整个图集跳过' }));
  await waitFor(() => expect(setAlbum).toHaveBeenCalledWith(runId, albumA, false));
  expect(screen.getByText('导入 0 张 · 跳过 2 张 · 0 B')).toBeVisible();

  fireEvent.click(within(albumCard).getByRole('button', { name: '整个图集导入' }));
  await waitFor(() => expect(setAlbum).toHaveBeenLastCalledWith(runId, albumA, true));
});

test('saves album and image target paths with explicit feedback and navigation protection', async () => {
  const original = draftPlan();
  const albumEdited = {
    ...original,
    albums: original.albums.map((album) =>
      album.album_id === albumA ? { ...album, album_name: '新目标图集' } : album,
    ),
  };
  const imageEdited = {
    ...albumEdited,
    albums: albumEdited.albums.map((album) => ({
      ...album,
      images: album.images.map((image) =>
        image.image_id === 'image-a' ? { ...image, relative_path: 'renamed/a.jpg' } : image,
      ),
    })),
  };
  vi.spyOn(api, 'setImportPlanAlbumTargetPath').mockResolvedValue(albumEdited);
  vi.spyOn(api, 'setImportPlanImageTargetPath').mockResolvedValue(imageEdited);
  const onNavigationBlockedChange = vi.fn();
  renderPlan({ initialPlan: original, onNavigationBlockedChange });

  const albumCard = albumDetails('源图集 A');
  fireEvent.click(albumCard.querySelector('summary') as HTMLElement);
  const albumPath = within(albumCard).getByLabelText('目标图集及相对路径');
  fireEvent.change(albumPath, { target: { value: '新目标图集' } });
  await waitFor(() => expect(onNavigationBlockedChange).toHaveBeenLastCalledWith(true));
  fireEvent.click(within(albumCard).getByRole('button', { name: '保存图集路径' }));
  await waitFor(() =>
    expect(api.setImportPlanAlbumTargetPath).toHaveBeenCalledWith(
      runId,
      albumA,
      '新目标图集',
    ),
  );
  expect(
    await screen.findByText('图集“源图集 A”的目标路径已保存。'),
  ).toBeVisible();

  const imageCard = screen.getByText('a.jpg').closest('article') as HTMLElement;
  const imagePath = within(imageCard).getByLabelText('目标相对路径');
  fireEvent.change(imagePath, { target: { value: 'renamed/a.jpg' } });
  fireEvent.click(within(imageCard).getByRole('button', { name: '保存' }));
  await waitFor(() =>
    expect(api.setImportPlanImageTargetPath).toHaveBeenCalledWith(
      runId,
      'image-a',
      albumA,
      'renamed/a.jpg',
    ),
  );
  expect(await screen.findByText('图片目标路径已保存。')).toBeVisible();
  await waitFor(() => expect(onNavigationBlockedChange).toHaveBeenLastCalledWith(false));
});

test('documents and disables unsupported cross-source album movement', () => {
  renderPlan();

  expect(screen.getByText('跨源图集移动暂不可用')).toBeVisible();
  fireEvent.click(albumDetails('源图集 A').querySelector('summary') as HTMLElement);
  const imageCard = screen.getByText('a.jpg').closest('article') as HTMLElement;
  expect(
    within(imageCard).getByLabelText('图片 a.jpg 的目标图集'),
  ).toBeDisabled();
  expect(within(imageCard).getByText('源图集 A')).toBeVisible();
});

test('locking removes every editing control and navigates to Commit with the same runId', async () => {
  const frozen = { ...draftPlan(), plan_hash: 'frozen-hash' };
  vi.spyOn(api, 'freezeImportPlan').mockResolvedValue(frozen);
  const onGoCommit = vi.fn();
  renderPlan({ onGoCommit });

  fireEvent.click(screen.getAllByRole('button', { name: '锁定入库计划' })[0]);

  expect(await screen.findByRole('heading', { name: '入库计划已锁定' })).toBeVisible();
  expect(api.freezeImportPlan).toHaveBeenCalledExactlyOnceWith(runId);
  expect(onGoCommit).toHaveBeenCalledExactlyOnceWith(runId);
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('radio')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: '锁定入库计划' })).not.toBeInTheDocument();
});
