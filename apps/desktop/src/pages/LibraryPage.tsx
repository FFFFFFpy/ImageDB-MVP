import { useState } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import type { Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import type { LibraryAlbumSummary } from '../lib/ipc/types';
import {
  AppIcon,
  Button,
  EmptyState,
  PageHeader,
  Skeleton,
  StatusBadge,
  StatusBanner,
} from '../components/ui';

export const LIBRARY_ALBUM_BATCH_SIZE = 50;
export const LIBRARY_IMAGE_BATCH_SIZE = 24;

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function formatCommittedAt(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

function LibraryAlbumImages({ album }: { album: LibraryAlbumSummary }) {
  const imagesQuery = useInfiniteQuery({
    queryKey: ['library-images', album.album_id],
    queryFn: ({ pageParam }) =>
      api.getLibraryImages(album.album_id, pageParam, LIBRARY_IMAGE_BATCH_SIZE),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
  });

  const images = imagesQuery.data?.pages.flatMap((page) => page.images) ?? [];

  if (imagesQuery.isLoading) {
    return (
      <div
        className="library-image-loading"
        role="status"
        aria-label={`正在加载 ${album.display_name}`}
      >
        <Skeleton height={48} />
        <Skeleton height={48} />
      </div>
    );
  }

  if (imagesQuery.isError) {
    return (
      <StatusBanner tone="danger" title="无法加载图集图片">
        {String(imagesQuery.error)}
      </StatusBanner>
    );
  }

  if (images.length === 0) {
    return <p className="library-album-empty">这个图集没有已登记的图片。</p>;
  }

  return (
    <div className="library-image-list">
      {images.map((image) => (
        <div className="library-image-row" key={image.image_id}>
          <span className="library-image-mark" aria-hidden="true">
            <AppIcon name="brand" size={18} />
          </span>
          <div className="library-image-copy">
            <strong className="mono" title={image.relative_path}>
              {image.relative_path}
            </strong>
            <span>
              {image.width} × {image.height} · {image.format.toUpperCase()}
            </span>
          </div>
          <span className="library-image-size">{formatFileSize(image.file_size)}</span>
          <StatusBadge tone={image.state === 'committed' ? 'success' : 'neutral'}>
            {image.state === 'committed' ? '已入库' : image.state}
          </StatusBadge>
        </div>
      ))}
      {imagesQuery.hasNextPage && (
        <Button
          variant="quiet"
          className="library-load-more"
          loading={imagesQuery.isFetchingNextPage}
          loadingLabel="正在加载…"
          onClick={() => imagesQuery.fetchNextPage()}
        >
          再显示 {LIBRARY_IMAGE_BATCH_SIZE} 张
        </Button>
      )}
    </div>
  );
}

interface LibraryPageProps {
  onNavigate: (route: Route) => void;
}

export function LibraryPage({ onNavigate }: LibraryPageProps) {
  const [openAlbums, setOpenAlbums] = useState<Set<string>>(new Set());
  const albumsQuery = useInfiniteQuery({
    queryKey: ['library-albums'],
    queryFn: ({ pageParam }) => api.getLibraryAlbums(pageParam, LIBRARY_ALBUM_BATCH_SIZE),
    initialPageParam: null as string | null,
    getNextPageParam: (lastPage) => lastPage.next_cursor ?? undefined,
  });

  const albums = albumsQuery.data?.pages.flatMap((page) => page.albums) ?? [];
  const totals = albumsQuery.data?.pages[0];

  return (
    <div className="library-page">
      <PageHeader
        title="图库明细"
        description="查看已经完成文件发布与数据库提交的图集；此页只读，不会修改图库内容。"
        actions={
          <Button variant="quiet" onClick={() => onNavigate('dashboard')}>
            返回工作台
          </Button>
        }
      />

      {albumsQuery.isLoading ? (
        <div className="library-loading" role="status" aria-label="正在加载图库明细">
          <Skeleton height={92} />
          <Skeleton height={56} />
          <Skeleton height={56} />
        </div>
      ) : albumsQuery.isError ? (
        <StatusBanner
          tone="danger"
          title="无法加载图库明细"
          actions={<Button onClick={() => albumsQuery.refetch()}>重新加载</Button>}
        >
          {String(albumsQuery.error)}
        </StatusBanner>
      ) : albums.length === 0 ? (
        <EmptyState
          title="图库还是空的"
          description="完成一次导入后，已经提交的图集会显示在这里。"
          action={<Button onClick={() => onNavigate('scan')}>新建导入</Button>}
        />
      ) : (
        <>
          <section className="library-summary" aria-label="图库汇总">
            <div className="plan-stat">
              <span>已入库图集</span>
              <strong>{totals?.total_albums ?? 0}</strong>
            </div>
            <div className="plan-stat">
              <span>已入库图片</span>
              <strong>{totals?.total_images ?? 0}</strong>
            </div>
            <div className="plan-stat">
              <span>登记大小</span>
              <strong>{formatFileSize(totals?.total_size ?? 0)}</strong>
            </div>
          </section>

          <section className="library-catalog" aria-labelledby="library-catalog-title">
            <div className="library-catalog-heading">
              <div>
                <span className="section-kicker">已提交内容</span>
                <h2 id="library-catalog-title">图集清单</h2>
                <p>展开图集后再分批读取图片明细。</p>
              </div>
              <StatusBadge>
                {albums.length} / {totals?.total_albums ?? albums.length} 个图集
              </StatusBadge>
            </div>

            <div className="library-album-list">
              {albums.map((album) => {
                const isOpen = openAlbums.has(album.album_id);
                return (
                  <details
                    className="library-album"
                    key={album.album_id}
                    open={isOpen}
                    onToggle={(event) => {
                      const nextOpen = event.currentTarget.open;
                      setOpenAlbums((current) => {
                        const next = new Set(current);
                        if (nextOpen) next.add(album.album_id);
                        else next.delete(album.album_id);
                        return next;
                      });
                    }}
                  >
                    <summary>
                      <span className="library-album-icon" aria-hidden="true">
                        <AppIcon name="commit" size={19} />
                      </span>
                      <span className="library-album-copy">
                        <strong>{album.display_name}</strong>
                        <span
                          className="mono"
                          title={`${album.library_root_path}/${album.relative_path}`}
                        >
                          {album.library_root_path} / {album.relative_path}
                        </span>
                      </span>
                      <span className="library-album-meta">
                        {album.image_count} 张 · {formatFileSize(album.total_size)}
                      </span>
                      <span className="library-album-time">
                        {formatCommittedAt(album.committed_at)}
                      </span>
                    </summary>
                    {isOpen && <LibraryAlbumImages album={album} />}
                  </details>
                );
              })}
            </div>

            {albumsQuery.hasNextPage && (
              <Button
                variant="quiet"
                className="library-load-more"
                loading={albumsQuery.isFetchingNextPage}
                loadingLabel="正在加载…"
                onClick={() => albumsQuery.fetchNextPage()}
              >
                再显示 {LIBRARY_ALBUM_BATCH_SIZE} 个图集
              </Button>
            )}
          </section>
        </>
      )}
    </div>
  );
}
