import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { QueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { Route } from '../hooks/use-router';
import type {
  ImportPlan,
  ImportPlanAlbum,
  ImportPlanImage,
  ReviewGroupDetail,
  ReviewGroupMember,
  ReviewGroupMemberAction,
  ReviewGroupMemberDecision,
  SourceFileMode,
} from '../lib/ipc/types';
import {
  AppIcon,
  Button,
  EmptyState,
  ImagePreviewDialog,
  PageHeader,
  Skeleton,
  StatusBadge,
  StatusBanner,
} from '../components/ui';

interface ReviewPageProps {
  onNavigate: (route: Route) => void;
  onGoCommit?: (importRunId: string) => void;
  onWorkflowAbandoned?: () => void;
  onPlanEditPendingChange?: (pending: boolean) => void;
  initialImportRunId?: string | null;
  initialPreviews?: { left: string; right: string } | null;
  initialPlan?: ImportPlan | null;
  initialShowPlan?: boolean;
  enablePolling?: boolean;
}

interface ViewState {
  scale: number;
  offsetX: number;
  offsetY: number;
}

export type PreviewState = 'idle' | 'loading' | 'success' | 'error';

export interface ImportPlanAlbumGroup {
  albumId: string;
  albumName: string;
  included: boolean;
  imageCount: number;
  skippedImageCount: number;
  totalSize: number;
  images: ImportPlanImage[];
}

/** Kept for fixture/test compatibility; group review no longer submits pair decisions. */
export const REVIEW_DECISION_OPTIONS = [
  { decision: 'keep_source', label: '保留来源图' },
  { decision: 'keep_candidate', label: '保留候选图' },
  { decision: 'keep_all', label: '全部保留' },
] as const;

export function zoomViewAtPointer(
  view: ViewState,
  clientX: number,
  clientY: number,
  rect: Pick<DOMRect, 'left' | 'top' | 'width' | 'height'>,
  deltaY: number,
): ViewState {
  const nextScale = Math.max(0.1, Math.min(10, view.scale * (deltaY > 0 ? 0.9 : 1.1)));
  const ratio = nextScale / view.scale;
  const pointerX = clientX - (rect.left + rect.width / 2);
  const pointerY = clientY - (rect.top + rect.height / 2);
  return {
    scale: nextScale,
    offsetX: pointerX - (pointerX - view.offsetX) * ratio,
    offsetY: pointerY - (pointerY - view.offsetY) * ratio,
  };
}

export function shouldIgnoreReviewShortcut(event: KeyboardEvent, previewOpen: boolean): boolean {
  if (
    previewOpen ||
    event.defaultPrevented ||
    event.isComposing ||
    event.repeat ||
    event.ctrlKey ||
    event.altKey ||
    event.metaKey
  ) {
    return true;
  }
  const target = event.target;
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement &&
      (target.isContentEditable || target.closest('[contenteditable="true"]') !== null))
  );
}

export async function invalidateReviewWorkflowQueries(
  queryClient: QueryClient,
  importRunId?: string,
) {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ['reviewGroups', importRunId] }),
    queryClient.invalidateQueries({ queryKey: ['reviewProgress', importRunId] }),
    queryClient.invalidateQueries({ queryKey: ['reviewGroupDetail'] }),
    queryClient.invalidateQueries({ queryKey: ['reviewImportPlanDraftSummary', importRunId] }),
    queryClient.invalidateQueries({ queryKey: ['reviewFrozenImportPlanSummary', importRunId] }),
    queryClient.invalidateQueries({ queryKey: ['frozenImportPlanSummary', importRunId] }),
    queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
  ]);
}

export function groupImportPlanImagesByAlbum(images: ImportPlanImage[]): ImportPlanAlbumGroup[] {
  const groups = new Map<string, ImportPlanAlbumGroup>();
  for (const image of images) {
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

function planAlbumsForDisplay(plan: ImportPlan): ImportPlanAlbumGroup[] {
  if (plan.albums.length > 0) {
    return plan.albums.map((album: ImportPlanAlbum) => ({
      albumId: album.album_id,
      albumName: album.album_name,
      included: album.included,
      imageCount: album.images.filter((image) => image.included).length,
      skippedImageCount: album.images.filter((image) => !image.included).length,
      totalSize: album.images
        .filter((image) => image.included)
        .reduce((sum, image) => sum + image.file_size, 0),
      images: album.images,
    }));
  }
  return groupImportPlanImagesByAlbum(plan.kept_images);
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
}

function formatDistance(value: number | null, bitLength: number, ratio: number | null): string {
  if (value === null) return '不适用';
  const normalized = ratio ?? value / bitLength;
  return `${value} / ${bitLength}（差异 ${(normalized * 100).toFixed(1)}%）`;
}

function formatMatchType(matchType: string): string {
  const labels: Record<string, string> = {
    file_exact: '文件完全一致',
    pixel_exact: '像素完全一致',
    perceptual_near: '感知近似',
    perceptual_similar: '感知相似',
  };
  return labels[matchType] ?? matchType;
}

function matchTypePriority(matchType: string): number {
  return ['perceptual_similar', 'perceptual_near', 'pixel_exact', 'file_exact'].indexOf(matchType);
}

function formatScope(scope: string): string {
  const labels: Record<string, string> = {
    intra_album: '图集内',
    cross_album: '跨图集',
    library: '历史图库',
  };
  return labels[scope] ?? scope;
}

function formatTransform(transform: string | null): string {
  if (!transform) return '不适用';
  const labels: Record<string, string> = {
    identity: '原方向',
    rot90: '旋转 90°',
    rot180: '旋转 180°',
    rot270: '旋转 270°',
    flip_h: '水平翻转',
    flip_v: '垂直翻转',
    transpose: '主对角线翻转',
    transverse: '副对角线翻转',
  };
  return labels[transform] ?? transform;
}

function GroupMemberCard({
  groupId,
  member,
  action,
  keepCount,
  groupResolved,
  onChange,
  onOpen,
}: {
  groupId: string;
  member: ReviewGroupMember;
  action: ReviewGroupMemberAction;
  keepCount: number;
  groupResolved: boolean;
  onChange: (action: ReviewGroupMemberAction) => void;
  onOpen: (member: ReviewGroupMember, dataUrl: string | null) => void;
}) {
  const preview = useQuery({
    queryKey: ['reviewGroupMemberPreview', groupId, member.image_source, member.image_id],
    queryFn: () => api.getReviewGroupMemberPreview(groupId, member.image_id, member.image_source),
    staleTime: Infinity,
  });
  const libraryReadonly = member.image_source === 'library';
  const readonly = libraryReadonly;
  const cannotExcludeLast = action === 'keep' && keepCount <= 1;
  return (
    <article className={`review-group-member review-group-member--${action}`}>
      <button
        className="review-group-member__preview"
        type="button"
        style={
          member.width && member.height
            ? { aspectRatio: `${member.width} / ${member.height}` }
            : undefined
        }
        onClick={() => onOpen(member, preview.data?.data_url ?? null)}
        aria-label={`查看 ${member.relative_path}`}
      >
        {preview.data?.data_url ? (
          <img src={preview.data.data_url} alt="" />
        ) : preview.isError ? (
          <span>预览不可用</span>
        ) : (
          <Skeleton width="100%" height="100%" />
        )}
      </button>
      <div className="review-group-member__body">
        <div className="review-group-member__heading">
          <strong title={member.relative_path}>{member.relative_path}</strong>
          <StatusBadge tone={libraryReadonly ? 'info' : action === 'keep' ? 'success' : 'neutral'}>
            {libraryReadonly
              ? '库内图片'
              : groupResolved
                ? action === 'keep'
                  ? '草稿保留'
                  : '草稿排除'
                : action === 'keep'
                  ? '保留'
                  : '排除'}
          </StatusBadge>
        </div>
        <p>{member.album_name}</p>
        <p>
          {member.width && member.height ? `${member.width} × ${member.height}` : '尺寸未知'} ·{' '}
          {formatBytes(member.file_size)} · {member.format ?? '格式未知'}
        </p>
        <div className="review-group-member__actions" role="group" aria-label="入库处理">
          <Button
            variant={action === 'keep' ? 'primary' : 'secondary'}
            disabled={readonly}
            onClick={() => onChange('keep')}
          >
            保留
          </Button>
          <Button
            variant={action === 'exclude' ? 'danger' : 'secondary'}
            disabled={readonly || cannotExcludeLast}
            onClick={() => onChange('exclude')}
          >
            排除
          </Button>
        </div>
        <details className="review-group-member__details">
          <summary>查看完整图片信息</summary>
          <dl>
            <div>
              <dt>图片来源</dt>
              <dd>{libraryReadonly ? '历史图库' : '本批导入'}</dd>
            </div>
            <div>
              <dt>图集</dt>
              <dd>{member.album_name}</dd>
            </div>
            <div>
              <dt>尺寸</dt>
              <dd>
                {member.width && member.height ? `${member.width} × ${member.height}` : '未知'}
              </dd>
            </div>
            <div>
              <dt>文件大小</dt>
              <dd>{formatBytes(member.file_size)}</dd>
            </div>
            <div>
              <dt>格式</dt>
              <dd>{member.format ?? '未知'}</dd>
            </div>
            <div>
              <dt>相对路径</dt>
              <dd className="mono">{member.relative_path}</dd>
            </div>
            <div>
              <dt>完整路径</dt>
              <dd className="mono">{member.source_path}</dd>
            </div>
            <div>
              <dt>图片 ID</dt>
              <dd className="mono">{member.image_id}</dd>
            </div>
            <div>
              <dt>答案来源</dt>
              <dd>{member.decision_source === 'user' ? '人工选择' : '系统默认'}</dd>
            </div>
          </dl>
        </details>
        {libraryReadonly ? (
          <small>库内成员只读，并始终保留。</small>
        ) : groupResolved ? (
          <small>这是已保存的草稿；冻结导入计划前仍可调整。</small>
        ) : null}
      </div>
    </article>
  );
}

function GroupEvidence({ detail }: { detail: ReviewGroupDetail }) {
  const memberById = new Map(detail.members.map((member) => [member.image_id, member]));
  const strongest = detail.evidence.reduce<(typeof detail.evidence)[number] | null>(
    (current, edge) =>
      current === null || matchTypePriority(edge.match_type) > matchTypePriority(current.match_type)
        ? edge
        : current,
    null,
  );
  const highestConfidence = detail.evidence.reduce<number | null>(
    (highest, edge) =>
      edge.confidence === null ? highest : Math.max(highest ?? edge.confidence, edge.confidence),
    null,
  );
  return (
    <details className="review-group-evidence">
      <summary>
        <span>匹配证据</span>
        <span className="review-group-evidence__summary">
          {strongest ? formatMatchType(strongest.match_type) : '无匹配类型'} · 最高相似度{' '}
          {highestConfidence === null ? '未计算' : `${(highestConfidence * 100).toFixed(1)}%`} ·{' '}
          {detail.evidence.length} 条边
        </span>
      </summary>
      <div className="review-group-evidence__list">
        {detail.evidence.map((edge) => {
          const source = memberById.get(edge.source_image_id);
          const candidate = memberById.get(edge.candidate_image_id);
          return (
            <article key={edge.candidate_id} className="review-group-evidence__item">
              <header>
                <div>
                  <strong>{formatMatchType(edge.match_type)}</strong>
                  <span>{formatScope(edge.scope)}</span>
                </div>
                <StatusBadge tone={edge.automatic ? 'success' : 'warning'}>
                  {edge.automatic ? '自动判定' : '需人工审核'}
                </StatusBadge>
              </header>
              <div className="review-group-evidence__pair" aria-label="匹配图片">
                <span title={source?.source_path}>
                  {source?.relative_path ?? edge.source_image_id}
                </span>
                <b aria-hidden="true">↔</b>
                <span title={candidate?.source_path}>
                  {candidate?.relative_path ?? edge.candidate_image_id}
                </span>
              </div>
              <dl className="review-group-evidence__metrics">
                <div>
                  <dt>综合相似度</dt>
                  <dd>
                    {edge.confidence === null ? '未计算' : `${(edge.confidence * 100).toFixed(1)}%`}
                  </dd>
                </div>
                <div>
                  <dt>几何变换</dt>
                  <dd>{formatTransform(edge.transform_type)}</dd>
                </div>
                <div>
                  <dt>BLAKE3</dt>
                  <dd>{edge.blake3_equal ? '相同' : '不同'}</dd>
                </div>
                <div>
                  <dt>像素哈希</dt>
                  <dd>{edge.pixel_hash_equal ? '相同' : '不同'}</dd>
                </div>
                <div>
                  <dt>BlockHash 距离</dt>
                  <dd>{formatDistance(edge.block_distance, 256, edge.block_distance_ratio)}</dd>
                </div>
                <div>
                  <dt>DoubleGradient 距离</dt>
                  <dd>
                    {formatDistance(
                      edge.double_gradient_distance,
                      544,
                      edge.double_gradient_distance_ratio,
                    )}
                  </dd>
                </div>
              </dl>
              <footer>
                <span>证据 ID</span>
                <code>{edge.candidate_id}</code>
              </footer>
            </article>
          );
        })}
      </div>
    </details>
  );
}

function PlanView({
  plan,
  busy,
  freezing,
  onEditAlbum,
  onEditImage,
  onMode,
  onFreeze,
  onCommit,
  onAbandon,
}: {
  plan: ImportPlan;
  busy: boolean;
  freezing: boolean;
  onEditAlbum: (album: ImportPlanAlbumGroup) => void;
  onEditImage: (album: ImportPlanAlbumGroup, image: ImportPlanImage) => void;
  onMode: (mode: SourceFileMode) => void;
  onFreeze: () => void;
  onCommit: () => void;
  onAbandon: () => void;
}) {
  const [visibleAlbums, setVisibleAlbums] = useState(50);
  const albums = useMemo(() => planAlbumsForDisplay(plan), [plan]);
  const displayed = albums.slice(0, visibleAlbums);
  const moveMode = plan.source_file_mode === 'move_selected_without_backup';
  const locked = Boolean(plan.plan_hash);
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
            <Button variant="primary" onClick={onCommit} disabled={busy}>
              前往确认入库
            </Button>
          ) : (
            <Button
              variant="primary"
              onClick={onFreeze}
              disabled={busy || plan.kept_images.length === 0}
              loading={freezing}
              loadingLabel="正在锁定…"
            >
              锁定入库计划
            </Button>
          )
        }
      />

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
              onMode(event.target.checked ? 'move_selected_without_backup' : 'copy_and_archive')
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
                  onEditAlbum(album);
                }}
              >
                {album.included ? '排除整组' : '恢复整组'}
              </Button>
            </summary>
            <div className="plan-image-grid">
              {album.images.slice(0, 100).map((image) => (
                <button
                  type="button"
                  className={`plan-image-row ${image.included ? '' : 'plan-image-row--excluded'}`}
                  key={image.image_id}
                  disabled={busy || locked}
                  onClick={() => onEditImage(album, image)}
                >
                  <span>{image.relative_path}</span>
                  <span>{image.included ? '保留' : '排除'}</span>
                </button>
              ))}
            </div>
          </details>
        ))}
      </section>
      {visibleAlbums < albums.length && (
        <Button onClick={() => setVisibleAlbums((count) => count + 50)}>加载更多图集</Button>
      )}
      <div className="plan-footer-actions">
        <Button variant="danger" disabled={busy} onClick={onAbandon}>
          放弃这次导入
        </Button>
        {locked ? (
          <Button variant="primary" disabled={busy} onClick={onCommit}>
            前往确认入库
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={busy || plan.kept_images.length === 0}
            loading={freezing}
            loadingLabel="正在锁定…"
            onClick={onFreeze}
          >
            锁定入库计划
          </Button>
        )}
      </div>
    </div>
  );
}

export function ReviewPage({
  onNavigate,
  onGoCommit,
  onWorkflowAbandoned,
  onPlanEditPendingChange,
  initialImportRunId = null,
  initialPlan = null,
  initialShowPlan = false,
  enablePolling = true,
}: ReviewPageProps) {
  const queryClient = useQueryClient();
  const [importRunId, setImportRunId] = useState<string | null>(
    initialImportRunId ?? initialPlan?.import_run_id ?? null,
  );
  const [plan, setPlan] = useState<ImportPlan | null>(initialPlan);
  const [showPlan, setShowPlan] = useState(initialShowPlan || initialPlan !== null);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [actions, setActions] = useState<Record<string, ReviewGroupMemberAction>>({});
  const [message, setMessage] = useState<string | null>(null);
  const [planBusy, setPlanBusy] = useState(false);
  const planEditActive = useRef(false);
  const actionsGroupIdRef = useRef<string | null>(null);
  const actionsDirtyRef = useRef(false);
  const [preview, setPreview] = useState<{
    member: ReviewGroupMember;
    dataUrl: string | null;
  } | null>(null);

  const latestRun = useQuery({
    queryKey: ['latestReviewableImportRun'],
    queryFn: api.getLatestReviewableImportRun,
    enabled: !importRunId && !initialPlan,
    refetchInterval: enablePolling ? 2000 : false,
  });
  useEffect(() => {
    if (!importRunId && latestRun.data) setImportRunId(latestRun.data);
  }, [importRunId, latestRun.data]);

  const groupsQuery = useQuery({
    queryKey: ['reviewGroups', importRunId],
    queryFn: () => api.getReviewGroups(importRunId!),
    enabled: !!importRunId && !showPlan,
    // Loading groups is the explicit on-demand reconciliation boundary while
    // analysis is active. Progress may poll, but the complete connected graph
    // must not be rebuilt every 1.5 seconds.
    refetchInterval: false,
  });
  const progressQuery = useQuery({
    queryKey: ['reviewProgress', importRunId],
    queryFn: () => api.getReviewProgress(importRunId!),
    enabled: !!importRunId && !showPlan,
    refetchInterval: enablePolling ? 1500 : false,
  });
  const runsQuery = useQuery({
    queryKey: ['import-runs-dashboard'],
    queryFn: api.getImportRunsDashboard,
    enabled: !!importRunId && !showPlan,
    refetchInterval: enablePolling ? 1500 : false,
  });
  const frozenQuery = useQuery({
    queryKey: ['reviewFrozenImportPlanSummary', importRunId],
    queryFn: () => api.getFrozenImportPlanSummary(importRunId!),
    enabled: !!importRunId && !initialPlan,
  });
  useEffect(() => {
    if (frozenQuery.data) {
      setPlan(frozenQuery.data);
      setShowPlan(true);
    }
  }, [frozenQuery.data]);
  const draftQuery = useQuery({
    queryKey: ['reviewImportPlanDraftSummary', importRunId],
    queryFn: () => api.getImportPlanDraftSummary(importRunId!),
    enabled: !!importRunId && !initialPlan,
  });
  useEffect(() => {
    if (draftQuery.data && !frozenQuery.data) {
      setPlan(draftQuery.data);
      setShowPlan(true);
    }
  }, [draftQuery.data, frozenQuery.data]);

  const manualGroups = useMemo(
    () => (groupsQuery.data ?? []).filter((group) => group.requires_manual_review),
    [groupsQuery.data],
  );
  useEffect(() => {
    if (!selectedGroupId || !manualGroups.some((group) => group.group_id === selectedGroupId)) {
      setSelectedGroupId(
        manualGroups.find((group) => group.state === 'pending')?.group_id ??
          manualGroups[0]?.group_id ??
          null,
      );
    }
  }, [manualGroups, selectedGroupId]);

  const detailQuery = useQuery({
    queryKey: ['reviewGroupDetail', selectedGroupId],
    queryFn: () => api.getReviewGroupDetail(selectedGroupId!),
    enabled: !!selectedGroupId && !showPlan,
  });
  useEffect(() => {
    if (!detailQuery.data) return;
    if (actionsGroupIdRef.current === detailQuery.data.group_id && actionsDirtyRef.current) {
      return;
    }
    actionsGroupIdRef.current = detailQuery.data.group_id;
    actionsDirtyRef.current = false;
    setActions(
      Object.fromEntries(
        detailQuery.data.members.map((member) => [member.image_id, member.final_action]),
      ),
    );
  }, [detailQuery.data]);

  const submit = useMutation({
    mutationFn: async (detail: ReviewGroupDetail) => {
      const decisions: ReviewGroupMemberDecision[] = detail.members
        .filter((member) => member.image_source === 'import')
        .map((member) => ({
          image_id: member.image_id,
          image_source: 'import',
          final_action: actions[member.image_id] ?? member.final_action,
        }));
      await api.submitReviewGroupDecision(detail.group_id, decisions);
    },
    onSuccess: async () => {
      setMessage(null);
      setPlan(null);
      setShowPlan(false);
      await invalidateReviewWorkflowQueries(queryClient, importRunId ?? undefined);
      actionsGroupIdRef.current = null;
      actionsDirtyRef.current = false;
      setSelectedGroupId(null);
    },
    onError: (error) => setMessage(String(error)),
  });

  const generate = useMutation({
    mutationFn: () => api.generateImportPlan(importRunId!),
    onSuccess: (nextPlan) => {
      setPlan(nextPlan);
      setShowPlan(true);
      queryClient.setQueryData(
        ['reviewImportPlanDraftSummary', nextPlan.import_run_id],
        nextPlan,
      );
    },
    onError: (error) => setMessage(String(error)),
  });

  const freeze = useMutation({
    mutationFn: () => api.freezeImportPlan(importRunId!),
    onMutate: () => onPlanEditPendingChange?.(true),
    onSuccess: (nextPlan) => {
      setPlan(nextPlan);
      setShowPlan(true);
      queryClient.setQueryData(['reviewImportPlanDraftSummary', nextPlan.import_run_id], null);
      queryClient.setQueryData(['reviewFrozenImportPlanSummary', nextPlan.import_run_id], nextPlan);
      queryClient.setQueryData(['frozenImportPlanSummary', nextPlan.import_run_id], nextPlan);
      if (onGoCommit) onGoCommit(nextPlan.import_run_id);
      else onNavigate('commit');
    },
    onError: (error) => setMessage(String(error)),
    onSettled: () => onPlanEditPendingChange?.(false),
  });

  const applyPlanEdit = useCallback(
    async (edit: () => Promise<ImportPlan>) => {
      if (planEditActive.current) return;
      planEditActive.current = true;
      setPlanBusy(true);
      onPlanEditPendingChange?.(true);
      setMessage(null);
      try {
        const next = await edit();
        setPlan(next);
        queryClient.setQueryData(['reviewImportPlanDraftSummary', next.import_run_id], next);
      } catch (error) {
        setMessage(String(error));
      } finally {
        planEditActive.current = false;
        setPlanBusy(false);
        onPlanEditPendingChange?.(false);
      }
    },
    [onPlanEditPendingChange, queryClient],
  );

  if (plan && showPlan) {
    return (
      <>
        {message && (
          <StatusBanner tone="danger" title="计划更新失败">
            {message}
          </StatusBanner>
        )}
        <PlanView
          plan={plan}
          busy={planBusy || freeze.isPending}
          freezing={freeze.isPending}
          onEditAlbum={(album) =>
            void applyPlanEdit(() =>
              api.setImportPlanAlbumIncluded(plan.import_run_id, album.albumId, !album.included),
            )
          }
          onEditImage={(album, image) =>
            void applyPlanEdit(() =>
              api.setImportPlanImageIncluded(
                plan.import_run_id,
                image.image_id,
                album.albumId,
                !image.included,
              ),
            )
          }
          onMode={(mode) =>
            void applyPlanEdit(() => api.setImportPlanSourceFileMode(plan.import_run_id, mode))
          }
          onFreeze={() => freeze.mutate()}
          onCommit={() => (onGoCommit ? onGoCommit(plan.import_run_id) : onNavigate('commit'))}
          onAbandon={() => {
            void api
              .abandonFrozenImportWorkflow(plan.import_run_id)
              .then(() => {
                setPlan(null);
                setShowPlan(false);
                onWorkflowAbandoned?.();
                onNavigate('dashboard');
              })
              .catch((error) => setMessage(String(error)));
          }}
        />
      </>
    );
  }

  if (!importRunId && latestRun.isLoading) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="重复图片审核" description="正在查找可审核任务。" />
        <Skeleton width="100%" height={280} />
      </div>
    );
  }
  if (!importRunId) {
    return (
      <div className="review-page review-page--m3">
        <PageHeader title="重复图片审核" />
        <EmptyState
          icon={<AppIcon name="review" size={30} />}
          title="暂无待审核任务"
          description="完成一次导入分析后，包含不确定重复关系的图片组会出现在这里。"
          action={<Button onClick={() => onNavigate('scan')}>开始导入</Button>}
        />
      </div>
    );
  }

  const detail = detailQuery.data;
  const keepCount = detail
    ? detail.members.filter(
        (member) => (actions[member.image_id] ?? member.final_action) === 'keep',
      ).length
    : 0;
  const hasUnsavedGroupChanges = Boolean(
    detail &&
    (detail.state !== 'resolved' ||
      detail.members.some(
        (member) =>
          member.image_source === 'import' &&
          (actions[member.image_id] ?? member.final_action) !== member.final_action,
      )),
  );
  const allResolved = progressQuery.data?.all_decided ?? false;
  const activeRun = runsQuery.data?.find((run) => run.import_run_id === importRunId) ?? null;
  const analysisComplete = Boolean(
    activeRun &&
    activeRun.pending_albums === 0 &&
    activeRun.analyzing_albums === 0 &&
    activeRun.failed_albums === 0 &&
    activeRun.analyzed_albums + activeRun.review_required_albums === activeRun.total_albums,
  );
  const canFreeze = Boolean(
    allResolved &&
    analysisComplete &&
    activeRun &&
    ['review_required', 'ready_to_commit'].includes(activeRun.state),
  );
  const error = groupsQuery.error ?? progressQuery.error ?? detailQuery.error ?? runsQuery.error;

  return (
    <div className="review-page review-page--m3">
      <PageHeader
        title="按组审核重复图片"
        description="同一连通重复关系中的所有图片一次展示；每张导入图片都可独立保留或排除。"
        meta={
          <StatusBadge tone={allResolved ? 'success' : 'warning'}>
            {progressQuery.data?.resolved_count ?? 0} /{' '}
            {progressQuery.data?.total_review_groups ?? 0} 组已完成
          </StatusBadge>
        }
        actions={
          canFreeze ? (
            <Button variant="primary" loading={generate.isPending} onClick={() => generate.mutate()}>
              生成人工复核入库计划
            </Button>
          ) : undefined
        }
      />
      {(message || error) && (
        <StatusBanner tone="danger" title="审核操作未完成">
          {message ?? String(error)}
        </StatusBanner>
      )}
      {allResolved && !analysisComplete && (
        <StatusBanner
          tone="info"
          title="当前审核答案已保存，分析尚未完成"
          actions={<Button onClick={() => onNavigate('scan')}>继续分析</Button>}
        >
          后续发现的新相似项可能新增或合并审核组；已有图片选择会作为草稿保留。
        </StatusBanner>
      )}

      {manualGroups.length > 0 && (
        <nav className="review-group-tabs" aria-label="审核组">
          {manualGroups.map((group, index) => (
            <button
              type="button"
              key={group.group_id}
              className={group.group_id === selectedGroupId ? 'is-active' : ''}
              onClick={() => setSelectedGroupId(group.group_id)}
            >
              <span>组 {index + 1}</span>
              <small>{group.member_count} 张</small>
              <StatusBadge tone={group.state === 'resolved' ? 'success' : 'warning'}>
                {group.state === 'resolved' ? '草稿已保存' : '待审核'}
              </StatusBadge>
            </button>
          ))}
        </nav>
      )}

      {detailQuery.isLoading ? (
        <Skeleton width="100%" height={420} />
      ) : detail ? (
        <main className="review-group-workspace">
          <div className="review-group-heading">
            <div className="review-group-heading__title">
              <h2>{detail.members.length} 张关联图片</h2>
              <StatusBadge tone={hasUnsavedGroupChanges ? 'warning' : 'success'}>
                {detail.state === 'resolved'
                  ? hasUnsavedGroupChanges
                    ? '有未保存修改'
                    : '草稿已保存'
                  : '尚未保存'}
              </StatusBadge>
            </div>
            <div className="review-group-heading__actions">
              <span>
                保留 {keepCount} 张 · 排除 {detail.members.length - keepCount} 张
              </span>
              <Button
                variant="primary"
                loading={submit.isPending}
                disabled={keepCount === 0}
                onClick={() => submit.mutate(detail)}
              >
                {detail.state === 'resolved' ? '更新整组决定' : '保存整组决定'}
              </Button>
            </div>
          </div>
          <p className="review-group-instructions">
            {detail.state === 'resolved'
              ? '该审核组已有已保存草稿；冻结导入计划前仍可继续调整。'
              : '默认全部保留。库内图片为只读；组内至少需要保留一张图片。'}
          </p>
          <section className="review-group-grid" aria-label="重复图片组成员">
            {detail.members.map((member) => (
              <GroupMemberCard
                key={`${member.image_source}-${member.image_id}`}
                groupId={detail.group_id}
                member={member}
                action={actions[member.image_id] ?? member.final_action}
                keepCount={keepCount}
                groupResolved={detail.state === 'resolved'}
                onChange={(action) => {
                  actionsDirtyRef.current = true;
                  setActions((current) => ({ ...current, [member.image_id]: action }));
                }}
                onOpen={(nextMember, dataUrl) => setPreview({ member: nextMember, dataUrl })}
              />
            ))}
          </section>
          <GroupEvidence detail={detail} />
        </main>
      ) : canFreeze ? (
        <EmptyState
          title="所有审核组均已完成"
          description="现在可以生成入库计划，并在锁定前进行人工复核。"
          action={
            <Button variant="primary" loading={generate.isPending} onClick={() => generate.mutate()}>
              生成人工复核入库计划
            </Button>
          }
        />
      ) : (
        <EmptyState
          title="当前没有待审核组"
          description="继续分析时，新发现的相似项会增量出现在这里。"
        />
      )}

      {preview && (
        <ImagePreviewDialog
          dataUrl={preview.dataUrl}
          path={preview.member.source_path}
          onClose={() => setPreview(null)}
        />
      )}
    </div>
  );
}
