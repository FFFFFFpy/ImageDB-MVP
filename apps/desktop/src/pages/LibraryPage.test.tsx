import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import type { LibraryAlbumPage, LibraryImagePage } from '../lib/ipc/types';
import { LIBRARY_ALBUM_BATCH_SIZE, LIBRARY_IMAGE_BATCH_SIZE, LibraryPage } from './LibraryPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderLibrary() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <LibraryPage onNavigate={vi.fn()} />
    </QueryClientProvider>,
  );
}

function albumPage(offset: number): LibraryAlbumPage {
  const albums =
    offset === 0
      ? [
          {
            album_id: 'album-a',
            library_root_id: 'root-a',
            library_root_path: 'D:/Library',
            display_name: '图集甲',
            relative_path: '图集甲',
            image_count: 2,
            total_size: 300,
            state: 'committed',
            committed_at: '2026-07-13T10:00:00Z',
          },
          {
            album_id: 'album-b',
            library_root_id: 'root-a',
            library_root_path: 'D:/Library',
            display_name: '图集乙',
            relative_path: '图集乙',
            image_count: 0,
            total_size: 0,
            state: 'committed',
            committed_at: '2026-07-12T10:00:00Z',
          },
        ]
      : [
          {
            album_id: 'album-c',
            library_root_id: 'root-a',
            library_root_path: 'D:/Library',
            display_name: '图集丙',
            relative_path: '图集丙',
            image_count: 1,
            total_size: 120,
            state: 'committed',
            committed_at: '2026-07-11T10:00:00Z',
          },
        ];
  return {
    albums,
    total_albums: 3,
    total_images: 3,
    total_size: 420,
    offset,
    limit: LIBRARY_ALBUM_BATCH_SIZE,
  };
}

describe('LibraryPage', () => {
  test('loads albums and expanded images in bounded batches', async () => {
    const getAlbums = vi
      .spyOn(api, 'getLibraryAlbums')
      .mockImplementation(async (offset) => albumPage(offset));
    const getImages = vi
      .spyOn(api, 'getLibraryImages')
      .mockImplementation(async (_albumId, offset): Promise<LibraryImagePage> => ({
        album_id: 'album-a',
        images: [
          {
            image_id: offset === 0 ? 'image-a' : 'image-b',
            relative_path: offset === 0 ? 'a.jpg' : 'b.jpg',
            file_size: offset === 0 ? 100 : 200,
            width: 800,
            height: 600,
            format: 'jpg',
            state: 'committed',
          },
        ],
        total_images: 2,
        total_size: 300,
        offset,
        limit: LIBRARY_IMAGE_BATCH_SIZE,
      }));

    renderLibrary();
    const albumLabels = await screen.findAllByText('图集甲');
    expect(albumLabels[0]).toBeVisible();
    expect(getAlbums).toHaveBeenCalledWith(0, LIBRARY_ALBUM_BATCH_SIZE);

    fireEvent.click(albumLabels[0]);
    expect(await screen.findByText('a.jpg')).toBeVisible();
    expect(getImages).toHaveBeenCalledWith('album-a', 0, LIBRARY_IMAGE_BATCH_SIZE);
    fireEvent.click(screen.getByRole('button', { name: `再显示 ${LIBRARY_IMAGE_BATCH_SIZE} 张` }));
    expect(await screen.findByText('b.jpg')).toBeVisible();
    expect(getImages).toHaveBeenCalledWith('album-a', 1, LIBRARY_IMAGE_BATCH_SIZE);

    fireEvent.click(
      screen.getByRole('button', { name: `再显示 ${LIBRARY_ALBUM_BATCH_SIZE} 个图集` }),
    );
    expect((await screen.findAllByText('图集丙'))[0]).toBeVisible();
    expect(getAlbums).toHaveBeenCalledWith(2, LIBRARY_ALBUM_BATCH_SIZE);
  });

  test('does not disguise a loading failure as an empty library', async () => {
    vi.spyOn(api, 'getLibraryAlbums').mockRejectedValue(new Error('catalog unavailable'));
    renderLibrary();

    expect(await screen.findByText(/catalog unavailable/)).toBeVisible();
    expect(screen.queryByText('图库还是空的')).not.toBeInTheDocument();
  });

  test('shows a dedicated empty state after a successful empty response', async () => {
    vi.spyOn(api, 'getLibraryAlbums').mockResolvedValue({
      albums: [],
      total_albums: 0,
      total_images: 0,
      total_size: 0,
      offset: 0,
      limit: LIBRARY_ALBUM_BATCH_SIZE,
    });
    renderLibrary();

    expect(await screen.findByText('图库还是空的')).toBeVisible();
  });
});
