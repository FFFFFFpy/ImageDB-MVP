import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import type {
  ImportPlan,
  ImportPlanAlbum,
  ImportPlanImage,
  SourceFileMode,
} from '../lib/ipc/types';
import {
  AppIcon,
  Button,
  EmptyState,
  PageHeader,
  Skeleton,
  StatusBadge,
  StatusBanner,
} from '../components/ui';

interface PlanPageProps {
  onNavigate: (route: Route) => void;
  onGoCommit?: (importRunId: string) => void;
  onNavigationBlockedChange?: (blocked: boolean) => void;
  initialImportRunId?: string | null;
  initialPlan?: ImportPlan | null;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function includedCount(album: ImportPlanAlbum): number {
  return album.images.filter((image) => image.included).length;
}

function planImageKey(image: ImportPlanImage): string {
  return `${image.album_id}:${image.image_id}`;
}

export function PlanPage({
  onNavigate,
  onGoCommit,
  onNavigationBlockedChange,
  initialImportRunId = null,
  initialPlan = null,
}: PlanPageProps) {
  const queryClient = useQueryClient();
  const [plan, setPlan] = useState<ImportPlan | null>(initialPlan);
  const [message, setMessage] = useState<string | null>(null);
  const [savedMessage, setSavedMessage] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);
  const [editBusy, setEditBusy] = useState(false);
  const [albumPaths, setAlbumPaths] = useState<Record<string, string>>({});
  const [imagePaths, setImagePaths] = useState<Record<string, string>>({});
  const [dirtyAlbumPaths, setDirtyAlbumPaths] = useState<Set<string>>(new Set());
  const [dirtyImagePaths, setDirtyImagePaths] = useState<Set<string>>(new Set());
  const editActiveRef = useRef(false);
  const albumPathsRef = useRef<Record<string, string>>({});
  const imagePathsRef = useRef<Record<string, string>>({});
  const importRunId = initialImportRunId ?? initialPlan?.import_run_id ?? null;

  const draftQuery = useQuery({
    queryKey: ['reviewImportPlanDraftSummary', importRunId],
    queryFn: () => api.getImportPlanDraftSummary(importRunId!),
    enabled: !!importRunId && !initialPlan,
    retry: false,
  });

  useEffect(() => {
    if (draftQuery.data) setPlan(draftQuery.data);
  }, [draftQuery.data]);

  useEffect(() => {
    if (!plan) return;
    setAlbumPaths((current) => {
      const next = Object.fromEntries(
        plan.albums.map((album) => [
          album.album_id,
          dirtyAlbumPaths.has(album.album_id)
            ? (current[album.album_id] ?? album.album_name)
            : album.album_name,
        ]),
      );
      albumPathsRef.current = next;
      return next;
    });
    setImagePaths((current) => {
      const next = Object.fromEntries(
        plan.albums.flatMap((album) =>
          album.images.map((image) => {
            const key = planImageKey(image);
            return [
              key,
              dirtyImagePaths.has(key) ? (current[key] ?? image.relative_path) : image.relative_path,
            ];
          }),
        ),
      );
      imagePathsRef.current = next;
      return next;
    });
    // Path drafts are reconciled only when the persisted plan changes. A
    // keystroke must never trigger this effect and overwrite its own value.
    // The closure for a plan-changing render carries the current dirty sets,
    // so unrelated saved edits still preserve unsaved path fields.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [plan]);

  const hasUnsavedPaths = dirtyAlbumPaths.size > 0 || dirtyImagePaths.size > 0;
  const navigationBlocked = editBusy || hasUnsavedPaths;

  useEffect(() => {
    onNavigationBlockedChange?.(navigationBlocked);
    return () => onNavigationBlockedChange?.(false);
  }, [navigationBlocked, onNavigationBlockedChange]);

  useEffect(() => {
    if (!navigationBlocked) return;
    const preventClose = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', preventClose);
    return () => window.removeEventListener('beforeunload', preventClose);
  }, [navigationBlocked]);

  const updatePlan = (next: ImportPlan, success: string) => {
    setPlan(next);
    setSavedMessage(success);
    queryClient.setQueryData(['reviewImportPlanDraftSummary', next.import_run_id], next);
  };

  const applyEdit = async (operation: () => Promise<ImportPlan>, success: string) => {
    if (editActiveRef.current) return;
    editActiveRef.current = true;
    setEditBusy(true);
    setMessage(null);
    setSavedMessage(null);
    try {
      updatePlan(await operation(), success);
    } catch (error) {
      setMessage(String(error));
    } finally {
      editActiveRef.current = false;
      setEditBusy(false);
    }
  };

  const freeze = useMutation({
    mutationFn: () => api.freezeImportPlan(importRunId!),
    onMutate: () => {
      setMessage(null);
      setSavedMessage(null);
    },
    onSuccess: (frozenPlan) => {
      setLocked(true);
      setPlan(frozenPlan);
      queryClient.setQueryData(
        ['reviewImportPlanDraftSummary', frozenPlan.import_run_id],
        null,
      );
      queryClient.setQueryData(
        ['frozenImportPlanSummary', frozenPlan.import_run_id],
        frozenPlan,
      );
      void queryClient.invalidateQueries({ queryKey: ['workflow-resolution'] });
      if (onGoCommit) onGoCommit(frozenPlan.import_run_id);
      else onNavigate('commit');
    },
    onError: (error) => setMessage(String(error)),
  });

  const includedImages = useMemo(
    () => plan?.albums.flatMap((album) => album.images).filter((image) => image.included) ?? [],
    [plan],
  );
  const excludedImages = Math.max(0, (plan?.total_images ?? 0) - includedImages.length);

  if (!importRunId) {
    return (
      <div className="plan-page plan-page--m3">
        <PageHeader title="人工复核入库计划" />
        <EmptyState
          icon={<AppIcon name="review" size={30} />}
          title="没有可编辑的入库计划"
          description="请先完成重复图片审核，再生成人工复核入库计划。"
          action={<Button onClick={() => onNavigate('review')}>返回审核</Button>}
        />
      </div>
    );
  }

  if (!plan && draftQuery.isLoading) {
    return (
      <div className="plan-page plan-page--m3">
        <PageHeader title="人工复核入库计划" description="正在读取已保存的 Draft。" />
        <Skeleton width="100%" height={420} />
      </div>
    );
  }

  if (!plan) {
    return (
      <div className="plan-page plan-page--m3">
        <PageHeader title="人工复核入库计划" />
        <EmptyState
          icon={<AppIcon name="review" size={30} />}
          title="Draft 不存在或已锁定"
          description={draftQuery.error ? String(draftQuery.error) : '中央工作流路由会带你回到当前阶段。'}
          action={<Button onClick={() => onNavigate('dashboard')}>返回工作台</Button>}
        />
      </div>
    );
  }

  if (locked) {
    return (
      <div className="plan-page plan-page--m3">
        <PageHeader
          title="入库计划已锁定"
          description="计划已经不可编辑，正在进入最后一次只读审阅。"
          meta={<StatusBadge tone="success">Frozen · 只读</StatusBadge>}
        />
        <StatusBanner tone="success" title="锁定完成">
          此计划不会回到 Draft，也不会在此阶段创建文件事务。
        </StatusBanner>
      </div>
    );
  }

  const busy = editBusy || freeze.isPending;

  return (
    <div className="plan-page plan-page--m3">
      <PageHeader
        title="人工复核入库计划"
        description="锁定前可调整导入范围和目标路径；每次操作都会明确保存到当前 Draft。"
        meta={<StatusBadge tone="warning">Draft · 可编辑</StatusBadge>}
        actions={
          <Button
            variant="primary"
            loading={freeze.isPending}
            disabled={busy || hasUnsavedPaths || includedImages.length === 0}
            onClick={() => freeze.mutate()}
          >
            锁定入库计划
          </Button>
        }
      />

      {message && (
        <StatusBanner tone="danger" title="计划修改未保存">
          {message}
        </StatusBanner>
      )}
      {savedMessage && (
        <StatusBanner tone="success" title="修改已保存">
          {savedMessage}
        </StatusBanner>
      )}
      {hasUnsavedPaths && (
        <StatusBanner tone="warning" title="有尚未保存的路径修改">
          请保存或撤销路径输入后再锁定计划或离开页面。
        </StatusBanner>
      )}
      <StatusBanner tone="info" title="跨源图集移动暂不可用">
        当前文件事务模型无法安全表达跨源图集分配。本分支保留源图片身份和目标字段，
        但不会扩改底层入库协议；限制已记录为后续任务。
      </StatusBanner>

      <section className="import-plan-stats" aria-label="计划统计">
        <article className="plan-stat">
          <span>源图集</span>
          <strong>{plan.total_albums}</strong>
        </article>
        <article className="plan-stat">
          <span>全部图片</span>
          <strong>{plan.total_images}</strong>
        </article>
        <article className="plan-stat plan-stat--success">
          <span>将导入</span>
          <strong>{includedImages.length}</strong>
        </article>
        <article className="plan-stat plan-stat--warning">
          <span>将跳过</span>
          <strong>{excludedImages}</strong>
        </article>
      </section>

      <section
        className={`plan-source-mode ${
          plan.source_file_mode === 'move_selected_without_backup'
            ? 'plan-source-mode--danger'
            : ''
        }`}
      >
        <div>
          <h2>源文件处理模式</h2>
          <p>
            {plan.source_file_mode === 'copy_and_archive'
              ? '复制入库并保留现有归档流程。'
              : '移动已选图片，不创建源文件备份。'}
          </p>
        </div>
        <div className="plan-source-mode__options" role="radiogroup" aria-label="源文件处理模式">
          {(
            [
              ['copy_and_archive', '复制并归档'],
              ['move_selected_without_backup', '移动且不备份'],
            ] as [SourceFileMode, string][]
          ).map(([mode, label]) => (
            <label key={mode}>
              <input
                type="radio"
                name="source-file-mode"
                value={mode}
                checked={plan.source_file_mode === mode}
                disabled={busy}
                onChange={() =>
                  void applyEdit(
                    () => api.setImportPlanSourceFileMode(importRunId, mode),
                    '源文件处理模式已保存。',
                  )
                }
              />
              {label}
            </label>
          ))}
        </div>
      </section>

      <section aria-labelledby="plan-album-heading">
        <div className="plan-list-heading">
          <div>
            <h2 id="plan-album-heading">图集与图片分配</h2>
            <p>展开图集可逐张调整导入状态和目标相对路径。</p>
          </div>
          <StatusBadge tone={includedImages.length > 0 ? 'success' : 'danger'}>
            {includedImages.length > 0 ? `${includedImages.length} 张可锁定` : '至少导入 1 张'}
          </StatusBadge>
        </div>

        <div className="plan-album-list">
          {plan.albums.map((album) => {
            const imported = includedCount(album);
            const skipped = album.images.length - imported;
            const albumPath = albumPaths[album.album_id] ?? album.album_name;
            const albumPathDirty = dirtyAlbumPaths.has(album.album_id);
            return (
              <details className="plan-album-card" key={album.album_id}>
                <summary>
                  <span>
                    <strong>{album.source_album_name}</strong>
                    <small>
                      导入 {imported} 张 · 跳过 {skipped} 张 · {formatBytes(album.total_size)}
                    </small>
                  </span>
                  <StatusBadge tone={imported > 0 ? 'success' : 'warning'}>
                    {imported > 0 ? '参与入库' : '整个图集跳过'}
                  </StatusBadge>
                </summary>

                <div className="plan-album-editor">
                  <div className="plan-album-actions">
                    <Button
                      variant={imported > 0 ? 'secondary' : 'primary'}
                      disabled={busy || imported === album.images.length}
                      onClick={() =>
                        void applyEdit(
                          () => api.setImportPlanAlbumIncluded(importRunId, album.album_id, true),
                          `图集“${album.source_album_name}”已全部设为导入。`,
                        )
                      }
                    >
                      整个图集导入
                    </Button>
                    <Button
                      variant="quiet"
                      disabled={busy || imported === 0}
                      onClick={() =>
                        void applyEdit(
                          () => api.setImportPlanAlbumIncluded(importRunId, album.album_id, false),
                          `图集“${album.source_album_name}”已全部设为跳过。`,
                        )
                      }
                    >
                      整个图集跳过
                    </Button>
                  </div>

                  <label className="plan-path-editor">
                    <span>目标图集及相对路径</span>
                    <span className="plan-path-editor__controls">
                      <input
                        value={albumPath}
                        disabled={busy}
                        onChange={(event) => {
                          const value = event.target.value;
                          setAlbumPaths((current) => ({
                            ...current,
                            [album.album_id]: value,
                          }));
                          albumPathsRef.current = {
                            ...albumPathsRef.current,
                            [album.album_id]: value,
                          };
                          setDirtyAlbumPaths((current) => {
                            const next = new Set(current);
                            if (value === album.album_name) next.delete(album.album_id);
                            else next.add(album.album_id);
                            return next;
                          });
                        }}
                      />
                      <Button
                        variant="secondary"
                        disabled={busy || !albumPathDirty || !albumPath.trim()}
                        onClick={() =>
                          void applyEdit(
                            async () => {
                              const next = await api.setImportPlanAlbumTargetPath(
                                importRunId,
                                album.album_id,
                                albumPathsRef.current[album.album_id] ?? albumPath,
                              );
                              setDirtyAlbumPaths((current) => {
                                const updated = new Set(current);
                                updated.delete(album.album_id);
                                return updated;
                              });
                              return next;
                            },
                            `图集“${album.source_album_name}”的目标路径已保存。`,
                          )
                        }
                      >
                        保存图集路径
                      </Button>
                    </span>
                  </label>

                  <div className="plan-image-list">
                    {album.images.map((image) => {
                      const key = planImageKey(image);
                      const path = imagePaths[key] ?? image.relative_path;
                      const pathDirty = dirtyImagePaths.has(key);
                      return (
                        <article
                          className={`plan-image-editor ${
                            image.included ? '' : 'plan-image-editor--excluded'
                          }`}
                          key={key}
                        >
                          <header>
                            <div>
                              <strong>{image.relative_path.split(/[\\/]/).pop()}</strong>
                              <small>{formatBytes(image.file_size)}</small>
                            </div>
                            <Button
                              variant={image.included ? 'quiet' : 'primary'}
                              disabled={busy}
                              onClick={() =>
                                void applyEdit(
                                  () =>
                                    api.setImportPlanImageIncluded(
                                      importRunId,
                                      image.image_id,
                                      image.album_id,
                                      !image.included,
                                    ),
                                  image.included
                                    ? '图片已设为跳过；其目标分配仍被保留。'
                                    : '图片已重新设为导入，并恢复此前目标分配。',
                                )
                              }
                            >
                              {image.included ? '跳过' : '导入'}
                            </Button>
                          </header>

                          <dl className="plan-image-allocation">
                            <div>
                              <dt>源图集</dt>
                              <dd>{image.source_album_name}</dd>
                            </div>
                            <div>
                              <dt>目标图集</dt>
                              <dd>
                                <select
                                  value={image.album_id}
                                  disabled
                                  aria-label={`图片 ${image.relative_path} 的目标图集`}
                                >
                                  {plan.albums.map((target) => (
                                    <option key={target.album_id} value={target.album_id}>
                                      {target.album_name}
                                    </option>
                                  ))}
                                </select>
                              </dd>
                            </div>
                          </dl>

                          <label className="plan-path-editor">
                            <span>目标相对路径</span>
                            <span className="plan-path-editor__controls">
                              <input
                                value={path}
                                disabled={busy}
                                onChange={(event) => {
                                  const value = event.target.value;
                                  setImagePaths((current) => ({ ...current, [key]: value }));
                                  imagePathsRef.current = {
                                    ...imagePathsRef.current,
                                    [key]: value,
                                  };
                                  setDirtyImagePaths((current) => {
                                    const next = new Set(current);
                                    if (value === image.relative_path) next.delete(key);
                                    else next.add(key);
                                    return next;
                                  });
                                }}
                              />
                              <Button
                                variant="secondary"
                                disabled={busy || !pathDirty || !path.trim()}
                                onClick={() =>
                                  void applyEdit(
                                    async () => {
                                      const next = await api.setImportPlanImageTargetPath(
                                        importRunId,
                                        image.image_id,
                                        image.album_id,
                                        imagePathsRef.current[key] ?? path,
                                      );
                                      setDirtyImagePaths((current) => {
                                        const updated = new Set(current);
                                        updated.delete(key);
                                        return updated;
                                      });
                                      return next;
                                    },
                                    '图片目标路径已保存。',
                                  )
                                }
                              >
                                保存
                              </Button>
                            </span>
                          </label>
                        </article>
                      );
                    })}
                  </div>
                </div>
              </details>
            );
          })}
        </div>
      </section>

      <footer className="plan-footer-actions">
        <span className="plan-save-state" role="status" aria-live="polite">
          {busy ? '正在保存…' : hasUnsavedPaths ? '有未保存修改' : '所有修改均已保存'}
        </span>
        <Button
          variant="primary"
          loading={freeze.isPending}
          disabled={busy || hasUnsavedPaths || includedImages.length === 0}
          onClick={() => freeze.mutate()}
        >
          锁定入库计划
        </Button>
      </footer>
    </div>
  );
}
