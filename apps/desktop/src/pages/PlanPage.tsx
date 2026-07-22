import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type { ImportPlan, ImportPlanAlbum, ImportPlanImage, ImportAlbumStatus, SourceFileMode } from '../lib/ipc/types';
import { Button, EmptyState, PageHeader, Skeleton, StatusBadge, StatusBanner } from '../components/ui';

interface PlanPageProps {
  onNavigate: (route: Route) => void;
  onGoCommit?: (importRunId: string) => void;
  onWorkflowAbandoned?: () => void;
  onNavigationBlockedChange?: (blocked: boolean) => void;
  initialImportRunId?: string | null;
}

interface PlanAlbumGroup {
  albumId: string;
  albumName: string;
  included: boolean;
  imageCount: number;
  skippedImageCount: number;
  totalSize: number;
  images: ImportPlanImage[];
}

function planAlbumsForDisplay(plan: ImportPlan): PlanAlbumGroup[] {
  if (plan.albums.length > 0) {
    return plan.albums.map((album: ImportPlanAlbum) => ({
      albumId: album.album_id,
      albumName: album.album_name,
      included: album.included,
      imageCount: album.images.filter((img) => img.included).length,
      skippedImageCount: album.images.filter((img) => !img.included).length,
      totalSize: album.images
        .filter((img) => img.included)
        .reduce((sum, img) => sum + img.file_size, 0),
      images: album.images,
    }));
  }
  const groups = new Map<string, PlanAlbumGroup>();
  for (const image of plan.kept_images) {
    const current = groups.get(image.album_id) ?? {
      albumId: image.album_id,
      albumName: image.album_name,
      included: true,
      imageCount: 0,
      skippedImageCount: 0,
      totalSize: 0,
      images: [],
    };
    current.images.push(image);
    current.imageCount += image.included ? 1 : 0;
    current.skippedImageCount += image.included ? 0 : 1;
    current.totalSize += image.included ? image.file_size : 0;
    current.included = current.images.some((item) => item.included);
    groups.set(image.album_id, current);
  }
  return [...groups.values()];
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

export function PlanPage({
  onNavigate,
  onGoCommit,
  onWorkflowAbandoned,
  onNavigationBlockedChange,
  initialImportRunId = null,
}: PlanPageProps) {
  const queryClient = useQueryClient();
  const [importRunId] = useState<string | null>(initialImportRunId);
  const [plan, setPlan] = useState<ImportPlan | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [visibleAlbums, setVisibleAlbums] = useState(50);
  const editActive = useRef(false);

  const draftQuery = useQuery({
    queryKey: ['importPlanDraftSummary', importRunId],
    queryFn: () => api.getImportPlanDraftSummary(importRunId!),
    enabled: !!importRunId,
  });

  const frozenQuery = useQuery({
    queryKey: ['frozenImportPlanSummary', importRunId],
    queryFn: () => api.getFrozenImportPlanSummary(importRunId!),
    enabled: !!importRunId,
  });

  const runAlbumsQuery = useQuery({
    queryKey: ['importRunAlbums', importRunId],
    queryFn: () => api.getImportRunAlbums(importRunId!),
    enabled: !!importRunId,
  });

  useEffect(() => {
    if (frozenQuery.data) {
      setPlan(frozenQuery.data);
    } else if (draftQuery.data) {
      setPlan(draftQuery.data);
    }
  }, [draftQuery.data, frozenQuery.data]);

  const locked = Boolean(plan?.plan_hash);

  const applyEdit = useCallback(
    async (edit: () => Promise<ImportPlan>) => {
      if (editActive.current) return;
      editActive.current = true;
      setBusy(true);
      onNavigationBlockedChange?.(true);
      setMessage(null);
      try {
        const next = await edit();
        setPlan(next);
        queryClient.setQueryData(['importPlanDraftSummary', next.import_run_id], next);
      } catch (error) {
        setMessage(String(error));
      } finally {
        editActive.current = false;
        setBusy(false);
        onNavigationBlockedChange?.(false);
      }
    },
    [onNavigationBlockedChange, queryClient],
  );

  const freeze = useMutation({
    mutationFn: () => api.freezeImportPlan(importRunId!),
    onMutate: () => onNavigationBlockedChange?.(true),
    onSuccess: (nextPlan) => {
      setPlan(nextPlan);
      queryClient.setQueryData(['importPlanDraftSummary', nextPlan.import_run_id], null);
      queryClient.setQueryData(['frozenImportPlanSummary', nextPlan.import_run_id], nextPlan);
      if (onGoCommit) onGoCommit(nextPlan.import_run_id);
      else onNavigate('commit');
    },
    onError: (error) => setMessage(String(error)),
    onSettled: () => onNavigationBlockedChange?.(false),
  });

  const albums = useMemo(() => (plan ? planAlbumsForDisplay(plan) : []), [plan]);
  const displayed = albums.slice(0, visibleAlbums);
  const moveMode = plan?.source_file_mode === 'move_selected_without_backup';

  if (!importRunId) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="人工复核入库计划" />
        <EmptyState
          title="暂无可复核的入库计划"
          description="完成重复图片审核后，可以生成入库计划并在此复核。"
          action={<Button onClick={() => onNavigate('review')}>前往审核</Button>}
        />
      </div>
    );
  }

  if (draftQuery.isLoading && !plan) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="人工复核入库计划" description="正在读取计划草稿。" />
        <Skeleton width="100%" height={280} />
      </div>
    );
  }

  if (!plan && !draftQuery.isLoading) {
    return (
      <div className="review-page plan-page--m3">
        <PageHeader title="人工复核入库计划" />
        <EmptyState
          title="尚未生成入库计划"
          description="请先在审核页完成重复图片审核，然后生成入库计划。"
          action={<Button onClick={() => onNavigate('review')}>前往审核</Button>}
        />
      </div>
    );
  }

  if (!plan) return null;

  return (
    <div className="review-page plan-page--m3">
      <PageHeader
        title={locked ? '入库计划已锁定' : '人工复核入库计划'}
        description={
          locked
            ? `计划包含 ${plan.total_albums} 个图集、${plan.total_images} 张图片；确认入库页只读取这份计划。`
            : `检查 ${plan.total_albums} 个图集、${plan.total_images} 张图片，并在锁定前调整导入选择。`
        }
        meta={
          <StatusBadge tone={locked ? 'success' : 'warning'}>
            {locked ? '计划已锁定' : '尚未锁定'}
          </StatusBadge>
        }
        actions={
          locked ? (
            <Button variant="primary" onClick={() => (onGoCommit ? onGoCommit(plan.import_run_id) : onNavigate('commit'))} disabled={busy}>
              前往确认入库
            </Button>
          ) : (
            <Button
              variant="primary"
              onClick={() => freeze.mutate()}
              disabled={busy || plan.kept_images.length === 0}
              loading={freeze.isPending}
              loadingLabel="正在锁定…"
            >
              锁定入库计划
            </Button>
          )
        }
      />

      {message && (
        <StatusBanner tone="danger" title="计划更新失败">
          {message}
        </StatusBanner>
      )}

      <section className={`plan-source-mode ${moveMode ? 'plan-source-mode--danger' : ''}`}>
        <div>
          <h2>源文件处理</h2>
          <p>
            {moveMode
              ? '移动入库：发布并写入数据库成功后，仅删除计划中已入库的源图片，不创建备份。'
              : '复制并归档：保留默认安全行为，整图集完成后归档源目录。'}
          </p>
        </div>
        <label className="plan-source-mode__toggle">
          <input
            type="checkbox"
            checked={moveMode}
            disabled={busy || locked}
            onChange={(event) =>
              void applyEdit(() =>
                api.setImportPlanSourceFileMode(
                  plan.import_run_id,
                  event.target.checked ? 'move_selected_without_backup' : 'copy_and_archive',
                ),
              )
            }
          />
          <span>移动已选源图片（无备份）</span>
        </label>
        {moveMode && (
          <StatusBanner tone="warning" title="不可撤销的源文件操作">
            仅在发布文件、manifest 和数据库记录全部校验通过后执行。排除图片、sidecar
            和目录不会删除。
          </StatusBanner>
        )}
      </section>

      <section className="plan-album-list" aria-label={locked ? '已锁定计划图集' : '待复核计划图集'}>
        {displayed.map((album) => (
          <details className="plan-album-card" key={album.albumId}>
            <summary>
              <span>
                <strong>{album.albumName}</strong>
                <small>
                  {album.imageCount} 张保留 · {album.skippedImageCount} 张排除 ·{' '}
                  {formatBytes(album.totalSize)}
                </small>
              </span>
              <Button
                variant={album.included ? 'secondary' : 'primary'}
                disabled={busy || locked}
                onClick={(event) => {
                  event.preventDefault();
                  void applyEdit(() =>
                    api.setImportPlanAlbumIncluded(plan.import_run_id, album.albumId, !album.included),
                  );
                }}
              >
                {album.included ? '排除整组' : '恢复整组'}
              </Button>
            </summary>
            <div className="plan-image-grid">
              {album.images.slice(0, 100).map((image) => (
                <div
                  className={`plan-image-row ${image.included ? '' : 'plan-image-row--excluded'}`}
                  key={image.image_id}
                >
                  <button
                    type="button"
                    className="plan-image-row__toggle"
                    disabled={busy || locked}
                    onClick={() =>
                      void applyEdit(() =>
                        api.setImportPlanImageIncluded(
                          plan.import_run_id,
                          image.image_id,
                          image.target_album_id || album.albumId,
                          !image.included,
                        ),
                      )
                    }
                  >
                    <span>{image.relative_path}</span>
                    <span>{image.included ? '保留' : '排除'}</span>
                  </button>
                  {!locked && image.included && (
                    <label className="plan-image-row__target">
                      <select
                        value={image.target_album_id}
                        disabled={busy}
                        onChange={(event) =>
                          void applyEdit(() =>
                            api.setImportPlanImageIncluded(
                              plan.import_run_id,
                              image.image_id,
                              event.target.value,
                              true,
                            ),
                          )
                        }
                      >
                        {(runAlbumsQuery.data ?? []).map((a: ImportAlbumStatus) => (
                          <option key={a.id} value={a.id}>
                            {a.source_name}
                          </option>
                        ))}
                      </select>
                      <small>{image.target_relative_path}</small>
                    </label>
                  )}
                </div>
              ))}
            </div>
          </details>
        ))}
      </section>
      {visibleAlbums < albums.length && (
        <Button onClick={() => setVisibleAlbums((count) => count + 50)}>加载更多图集</Button>
      )}
      <div className="plan-footer-actions">
        <Button
          variant="danger"
          disabled={busy}
          onClick={() => {
            const abandon = locked
              ? api.abandonFrozenImportWorkflow(plan.import_run_id)
              : api.abandonImportRun(plan.import_run_id);
            void abandon
              .then(() => {
                setPlan(null);
                onWorkflowAbandoned?.();
                onNavigate('dashboard');
              })
              .catch((error) => setMessage(String(error)));
          }}
        >
          放弃这次导入
        </Button>
        {locked ? (
          <Button variant="primary" disabled={busy} onClick={() => (onGoCommit ? onGoCommit(plan.import_run_id) : onNavigate('commit'))}>
            前往确认入库
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={busy || plan.kept_images.length === 0}
            loading={freeze.isPending}
            loadingLabel="正在锁定…"
            onClick={() => freeze.mutate()}
          >
            锁定入库计划
          </Button>
        )}
      </div>
    </div>
  );
}
