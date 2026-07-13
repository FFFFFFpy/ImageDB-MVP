import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { LibraryAlbumPage, LibraryImagePage } from '../../lib/ipc/types';
import { LibraryPage } from '../../pages/LibraryPage';
import { Layout } from '../Layout';

const albums: LibraryAlbumPage = {
  albums: [
    {
      album_id: 'fixture-album-travel',
      library_root_id: 'fixture-root',
      library_root_path: 'D:/ImageDB/Library',
      display_name: '旅行风光',
      relative_path: '旅行风光',
      image_count: 1254,
      total_size: 14_889_337_856,
      state: 'committed',
      committed_at: '2026-07-12T09:32:00+08:00',
    },
    {
      album_id: 'fixture-album-city',
      library_root_id: 'fixture-root',
      library_root_path: 'D:/ImageDB/Library',
      display_name: '城市建筑',
      relative_path: '城市建筑',
      image_count: 1128,
      total_size: 13_421_772_800,
      state: 'committed',
      committed_at: '2026-07-10T18:16:00+08:00',
    },
    {
      album_id: 'fixture-album-pets',
      library_root_id: 'fixture-root',
      library_root_path: 'D:/ImageDB/Library',
      display_name: '可爱宠物',
      relative_path: '可爱宠物',
      image_count: 1034,
      total_size: 12_348_162_048,
      state: 'committed',
      committed_at: '2026-07-08T21:08:00+08:00',
    },
    {
      album_id: 'fixture-album-life',
      library_root_id: 'fixture-root',
      library_root_path: 'D:/ImageDB/Library',
      display_name: '生活记录',
      relative_path: '生活记录',
      image_count: 862,
      total_size: 9_662_611_456,
      state: 'committed',
      committed_at: '2026-07-05T11:42:00+08:00',
    },
  ],
  total_albums: 6,
  total_images: 5808,
  total_size: 68_934_574_080,
  offset: 0,
  limit: 50,
};

const travelImages: LibraryImagePage = {
  album_id: 'fixture-album-travel',
  images: [
    {
      image_id: 'fixture-image-mountain',
      relative_path: 'IMG_20260701_083015.jpg',
      file_size: 3_812_452,
      width: 4032,
      height: 3024,
      format: 'jpg',
      state: 'committed',
    },
    {
      image_id: 'fixture-image-lake',
      relative_path: 'IMG_20260701_101244.jpg',
      file_size: 4_221_816,
      width: 4032,
      height: 3024,
      format: 'jpg',
      state: 'committed',
    },
    {
      image_id: 'fixture-image-cloud',
      relative_path: 'DSC_4821.webp',
      file_size: 2_094_118,
      width: 3840,
      height: 2160,
      format: 'webp',
      state: 'committed',
    },
  ],
  total_images: 3,
  total_size: 10_128_386,
  offset: 0,
  limit: 24,
};

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['library-albums'], { pages: [albums], pageParams: [0] });
fixtureClient.setQueryData(['library-images', 'fixture-album-travel'], {
  pages: [travelImages],
  pageParams: [0],
});
fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 0, pending_review_count: 0, recovery_required_run_count: 0 },
});

export function LibraryFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="library" onNavigate={() => undefined} enablePolling={false}>
        <LibraryPage onNavigate={() => undefined} />
      </Layout>
    </QueryClientProvider>
  );
}
