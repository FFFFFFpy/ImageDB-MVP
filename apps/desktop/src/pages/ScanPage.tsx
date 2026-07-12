import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import type {
  ImportAlbumStatus,
  ImportRunDashboard,
  ScanProgress,
  ScanSourceInfo,
} from '../lib/ipc/types';
import { AppIcon, Button, PageHeader, Progress, StatusBadge, StatusBanner } from '../components/ui';

interface ScanPageProps {
  initialImportRunId?: string | null;
  initialProgress?: ScanProgress | null;
  enablePolling?: boolean;
  onNavigate: (route: Route) => void;
}

interface ScanProgressEvent {
  state: string;
  import_run_id: string | null;
  current_stage: string;
  current_album: string | null;
  processed_images: number;
  total_albums: number;
  total_images: number;
  duplicate_count: number;
  error_count: number;
  errors: string[];
}

interface ScanDraft {
  sourcePath: string;
  sourceInfo: ScanSourceInfo | null;
}

const SCAN_DRAFT_STORAGE_KEY = 'imagedb.scan.draft';

const STAGE_LABELS: Record<string, string> = {
  idle: '空闲',
  scanning: '扫描目录',
  fingerprinting: '计算指纹',
  detecting_duplicates: '检测重复',
  completing: '完成中',
  completed: '已完成',
  cancelled: '已取消',
  failed: '失败',
  analyzing: '分析中',
  analyzed: '已分析',
  review_required: '待审核',
  ready_to_commit: '可生成入库计划',
};

const ALBUM_STATE_LABELS: Record<string, string> = {
  pending: '待分析',
  analyzing: '分析中',
  analyzed: '已分析',
  review_required: '待审核',
  failed: '失败',
};

function isScanSourceInfo(value: unknown): value is ScanSourceInfo {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<ScanSourceInfo>;
  return (
    typeof candidate.path === 'string' &&
    Array.isArray(candidate.albums) &&
    candidate.albums.every((album) => typeof album === 'string') &&
    typeof candidate.album_count === 'number'
  );
}

function loadScanDraft(): ScanDraft {
  try {
    const raw = window.localStorage.getItem(SCAN_DRAFT_STORAGE_KEY);
    if (!raw) return { sourcePath: '', sourceInfo: null };
    const parsed = JSON.parse(raw) as Partial<ScanDraft>;
    return {
      sourcePath: typeof parsed.sourcePath === 'string' ? parsed.sourcePath : '',
      sourceInfo: isScanSourceInfo(parsed.sourceInfo) ? parsed.sourceInfo : null,
    };
  } catch {
    return { sourcePath: '', sourceInfo: null };
  }
}

function saveScanDraft(draft: ScanDraft) {
  try {
    window.localStorage.setItem(SCAN_DRAFT_STORAGE_KEY, JSON.stringify(draft));
  } catch {
    // The draft is only a UI convenience; scanning itself does not depend on it.
  }
}

export function isTerminalScanState(state: string | null | undefined): boolean {
  return (
    state === 'ready_to_commit' ||
    state === 'review_required' ||
    state === 'completed' ||
    state === 'cancelled' ||
    state === 'failed'
  );
}

export function nextRouteForScanState(state: string | null | undefined): Route | null {
  if (state === 'review_required') return 'review';
  if (state === 'ready_to_commit') return 'review';
  return null;
}

export function nextActionLabelForScanState(state: string | null | undefined): string | null {
  const route = nextRouteForScanState(state);
  if (route === 'review') return '前往入库审核';
  return null;
}

function canResumeRun(run: ImportRunDashboard | null): boolean {
  if (!run || run.state === 'abandoned') return false;
  return run.pending_albums > 0 || run.analyzing_albums > 0;
}

function canAbandonRun(run: ImportRunDashboard | null): boolean {
  return Boolean(
    run && ['analyzing', 'scanning', 'fingerprinting', 'cancelled', 'failed'].includes(run.state),
  );
}

function normalizedSourcePath(path: string): string {
  return path.trim().replace(/\\/g, '/').replace(/\/+$/, '').toLocaleLowerCase();
}

export function ScanPage({
  initialImportRunId = null,
  initialProgress = null,
  enablePolling = true,
  onNavigate,
}: ScanPageProps) {
  const queryClient = useQueryClient();
  const [sourcePath, setSourcePath] = useState(() => loadScanDraft().sourcePath);
  const [sourceInfo, setSourceInfo] = useState<ScanSourceInfo | null>(
    () => loadScanDraft().sourceInfo,
  );
  const [activeImportRunId, setActiveImportRunId] = useState<string | null>(initialImportRunId);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [isValidating, setIsValidating] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(initialProgress);
  const [scanEvent, setScanEvent] = useState<ScanProgressEvent | null>(null);
  const [isScanning, setIsScanning] = useState(
    () => Boolean(initialProgress) && !isTerminalScanState(initialProgress?.state),
  );
  const [globalScanBusyRunId, setGlobalScanBusyRunId] = useState<string | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);
  const [isAbandoning, setIsAbandoning] = useState(false);
  const eventListenerRef = useRef<(() => void) | null>(null);
  const validationRequestRef = useRef(0);

  const runsQuery = useQuery({
    queryKey: ['import-runs-dashboard'],
    queryFn: api.getImportRunsDashboard,
    refetchInterval: enablePolling ? (isScanning ? 1500 : 5000) : false,
  });

  const activeRun = useMemo(
    () => runsQuery.data?.find((run) => run.import_run_id === activeImportRunId) ?? null,
    [activeImportRunId, runsQuery.data],
  );

  const albumsQuery = useQuery({
    queryKey: ['import-run-albums', activeImportRunId],
    queryFn: () => api.getImportRunAlbums(activeImportRunId!),
    enabled: Boolean(activeImportRunId),
    refetchInterval: enablePolling ? (isScanning ? 1500 : 5000) : false,
  });

  const retryAlbum = useMutation({
    mutationFn: api.retryImportAlbum,
    onSuccess: async (album) => {
      setActiveImportRunId(album.import_run_id);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['import-run-albums', album.import_run_id] }),
      ]);
    },
  });

  useEffect(() => {
    if (initialImportRunId) {
      validationRequestRef.current += 1;
      setActiveImportRunId(initialImportRunId);
      setScanEvent(null);
      setProgress(initialProgress);
      setIsScanning(Boolean(initialProgress) && !isTerminalScanState(initialProgress?.state));
      setGlobalScanBusyRunId(null);
      setIsValidating(false);
    } else {
      setActiveImportRunId(null);
      setScanEvent(null);
      setProgress(null);
      setIsScanning(false);
    }
  }, [initialImportRunId, initialProgress]);

  useEffect(() => {
    if (activeRun?.source_root) {
      setSourcePath(activeRun.source_root);
      setSourceInfo((current) =>
        current &&
        normalizedSourcePath(current.path) === normalizedSourcePath(activeRun.source_root)
          ? current
          : null,
      );
    }
  }, [activeRun?.source_root]);

  useEffect(() => {
    return () => {
      eventListenerRef.current?.();
    };
  }, []);

  useEffect(() => {
    saveScanDraft({ sourcePath, sourceInfo });
  }, [sourcePath, sourceInfo]);

  useEffect(() => {
    if (!enablePolling) return;
    let cancelled = false;
    (async () => {
      try {
        const p = await api.getScanProgress();
        if (cancelled) return;
        if (p && p.state && p.state !== 'idle') {
          const terminal = isTerminalScanState(p.state);
          // A Dashboard-selected run remains authoritative. An older terminal
          // tracker is ignored; a different live scan is recorded only as a
          // global conflict so it cannot replace the selected run.
          if (initialImportRunId && p.import_run_id !== initialImportRunId) {
            if (!terminal) setGlobalScanBusyRunId(p.import_run_id ?? '正在初始化');
            return;
          }
          if (!initialImportRunId && terminal) return;
          setProgress(p);
          if (p.import_run_id) setActiveImportRunId(p.import_run_id);
          setIsScanning(!terminal);
          setGlobalScanBusyRunId(null);
        }
      } catch {
        // No scan in flight.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [enablePolling, initialImportRunId]);

  const attachScanListener = useCallback(async () => {
    eventListenerRef.current?.();
    eventListenerRef.current = null;
    const unlisten = await listen<ScanProgressEvent>('scan-progress', (event) => {
      setScanEvent(event.payload);
      if (event.payload.import_run_id) {
        setActiveImportRunId(event.payload.import_run_id);
      }
      if (isTerminalScanState(event.payload.state)) {
        setIsScanning(false);
      }
    });
    eventListenerRef.current = unlisten;
    return unlisten;
  }, []);

  const handleValidate = useCallback(async () => {
    const requestedPath = sourcePath.trim();
    if (!requestedPath) {
      setValidationError('请输入源目录路径');
      return;
    }
    const requestId = ++validationRequestRef.current;
    setIsValidating(true);
    setValidationError(null);
    setSourceInfo(null);
    setActiveImportRunId(null);
    setScanEvent(null);
    setProgress(null);
    try {
      const info = await api.validateSourceDirectory(requestedPath);
      if (validationRequestRef.current !== requestId) return;
      setSourceInfo(info);
      if (info.album_count === 0) {
        setValidationError('未找到图集（一级子目录）。');
      }
    } catch (e) {
      if (validationRequestRef.current !== requestId) return;
      setValidationError(String(e));
    } finally {
      if (validationRequestRef.current === requestId) setIsValidating(false);
    }
  }, [sourcePath]);

  const handleSelectDirectory = useCallback(async () => {
    setValidationError(null);
    try {
      const selectedPath = await api.selectSourceDirectory();
      if (!selectedPath) return;
      validationRequestRef.current += 1;
      setSourcePath(selectedPath);
      setSourceInfo(null);
      setActiveImportRunId(null);
      setScanEvent(null);
      setProgress(null);
      setIsValidating(true);
      const info = await api.validateSourceDirectory(selectedPath);
      setSourceInfo(info);
      if (info.album_count === 0) setValidationError('未找到图集（一级子目录）。');
    } catch (error) {
      setValidationError(String(error));
    } finally {
      setIsValidating(false);
    }
  }, []);

  const handleStartScan = useCallback(async () => {
    if (
      !sourceInfo ||
      sourceInfo.album_count === 0 ||
      normalizedSourcePath(sourceInfo.path) !== normalizedSourcePath(sourcePath) ||
      activeImportRunId ||
      globalScanBusyRunId
    ) {
      return;
    }
    setScanError(null);
    setActiveImportRunId(null);
    setScanEvent(null);
    setProgress(null);
    setIsScanning(true);

    let unlisten: (() => void) | null = null;
    try {
      unlisten = await attachScanListener();
      await api.startScan(sourcePath.trim());
    } catch (e) {
      setScanError(String(e));
      setIsScanning(false);
      unlisten?.();
      if (eventListenerRef.current === unlisten) eventListenerRef.current = null;
    }
  }, [activeImportRunId, attachScanListener, globalScanBusyRunId, sourceInfo, sourcePath]);

  const handleResumeScan = useCallback(async () => {
    if (!activeImportRunId || globalScanBusyRunId) return;
    setScanError(null);
    setScanEvent(null);
    setProgress(null);
    setIsScanning(true);

    let unlisten: (() => void) | null = null;
    try {
      unlisten = await attachScanListener();
      await api.resumeImportRun(activeImportRunId);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['import-run-albums', activeImportRunId] }),
      ]);
    } catch (e) {
      setScanError(String(e));
      setIsScanning(false);
      unlisten?.();
      if (eventListenerRef.current === unlisten) eventListenerRef.current = null;
    }
  }, [activeImportRunId, attachScanListener, globalScanBusyRunId, queryClient]);

  const handleCancelScan = useCallback(async () => {
    try {
      await api.cancelScan();
    } catch (e) {
      setScanError(String(e));
    }
  }, []);

  const handleAbandonAndRestart = useCallback(async () => {
    if (!activeImportRunId || !activeRun || globalScanBusyRunId) return;
    setScanError(null);
    setIsAbandoning(true);
    let unlisten: (() => void) | null = null;
    try {
      await api.abandonImportRun(activeImportRunId);
      const info = await api.validateSourceDirectory(activeRun.source_root);
      if (info.album_count === 0) throw new Error('未找到图集（一级子目录）。');
      setSourcePath(activeRun.source_root);
      setSourceInfo(info);
      setActiveImportRunId(null);
      setScanEvent(null);
      setProgress(null);
      setIsScanning(true);
      unlisten = await attachScanListener();
      await api.startScan(activeRun.source_root);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
        queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
      ]);
    } catch (e) {
      setScanError(String(e));
      setIsScanning(false);
      unlisten?.();
      if (eventListenerRef.current === unlisten) eventListenerRef.current = null;
    } finally {
      setIsAbandoning(false);
    }
  }, [activeImportRunId, activeRun, attachScanListener, globalScanBusyRunId, queryClient]);

  useEffect(() => {
    if (!enablePolling || (!isScanning && !globalScanBusyRunId)) return;
    const interval = setInterval(async () => {
      try {
        const p = await api.getScanProgress();
        const terminal = isTerminalScanState(p.state);
        if (p.state === 'idle' && !p.import_run_id && !isScanning) {
          // The global tracker has returned to its empty state, so a scan
          // that belonged to another run is no longer blocking this page.
          setGlobalScanBusyRunId(null);
          return;
        }
        if (activeImportRunId && p.import_run_id && p.import_run_id !== activeImportRunId) {
          setGlobalScanBusyRunId(terminal ? null : p.import_run_id);
          if (!terminal) setIsScanning(false);
          return;
        }
        if (activeImportRunId && !p.import_run_id && !terminal && !isScanning) {
          setGlobalScanBusyRunId('正在初始化');
          return;
        }
        setProgress(p);
        if (p.import_run_id) setActiveImportRunId(p.import_run_id);
        setGlobalScanBusyRunId(null);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ['import-runs-dashboard'] }),
          queryClient.invalidateQueries({ queryKey: ['database-info-dashboard'] }),
          p.import_run_id
            ? queryClient.invalidateQueries({ queryKey: ['import-run-albums', p.import_run_id] })
            : Promise.resolve(),
        ]);
        if (terminal) {
          // A missed terminal event must not let an older running event mask
          // the authoritative polled terminal state.
          setScanEvent(null);
          setIsScanning(false);
        }
      } catch {
        // ignore
      }
    }, 2000);
    return () => clearInterval(interval);
  }, [activeImportRunId, enablePolling, globalScanBusyRunId, isScanning, queryClient]);

  const displayProgress = scanEvent ?? progress;
  const isFinished = !isScanning && isTerminalScanState(displayProgress?.state);
  const nextRoute = nextRouteForScanState(displayProgress?.state);
  const nextActionLabel = nextActionLabelForScanState(displayProgress?.state);
  const albumRows = albumsQuery.data ?? [];
  const isAnyScanBusy = isScanning || globalScanBusyRunId !== null;
  const sourceInfoMatchesPath =
    sourceInfo !== null &&
    normalizedSourcePath(sourceInfo.path) === normalizedSourcePath(sourcePath);
  const albumCounts = activeRun
    ? {
        total: activeRun.total_albums,
        analyzed: activeRun.analyzed_albums + activeRun.review_required_albums,
        analyzing: activeRun.analyzing_albums,
        pending: activeRun.pending_albums,
        review: activeRun.review_required_albums,
        failed: activeRun.failed_albums,
      }
    : null;

  return (
    <div className="scan-page scan-page--m3">
      <PageHeader
        title="新建导入"
        description="选择源目录，验证图集后开始分析。分析阶段只读取图片，不修改任何源文件。"
        meta={activeRun ? <StatusBadge tone="info">任务进行中</StatusBadge> : undefined}
      />

      <section className="scan-source-panel" aria-labelledby="scan-source-title">
        <div className="scan-section-heading">
          <span className="scan-step">1</span>
          <div>
            <h2 id="scan-source-title">选择源目录</h2>
            <p>目录的一级子目录会作为图集。</p>
          </div>
        </div>

        <div className="scan-source-picker">
          <span className="scan-source-picker__icon" aria-hidden="true">
            <AppIcon name="import" size={22} />
          </span>
          <input
            type="text"
            aria-label="源目录路径"
            placeholder="输入源目录路径，例如 D:\\Photos\\2024"
            value={sourcePath}
            onChange={(e) => {
              validationRequestRef.current += 1;
              setIsValidating(false);
              setSourcePath(e.target.value);
              setSourceInfo(null);
              setValidationError(null);
              setActiveImportRunId(null);
              setScanEvent(null);
              setProgress(null);
            }}
            disabled={isScanning || isValidating}
          />
          <Button
            onClick={handleSelectDirectory}
            disabled={isScanning || isValidating}
            loading={isValidating}
            loadingLabel="正在读取…"
          >
            选择目录
          </Button>
          <Button
            variant="quiet"
            onClick={handleValidate}
            disabled={isValidating || isScanning || !sourcePath.trim()}
          >
            验证
          </Button>
        </div>

        {validationError && (
          <StatusBanner tone="danger" title="无法使用这个目录">
            {validationError}
          </StatusBanner>
        )}

        {sourceInfo && sourceInfo.album_count > 0 && sourceInfoMatchesPath && (
          <div className="scan-discovery">
            <div className="scan-discovery__summary">
              <StatusBadge tone="success">目录验证通过</StatusBadge>
              <p>
                找到 <strong>{sourceInfo.album_count}</strong> 个图集
              </p>
            </div>
            <div className="scan-album-preview-list">
              {sourceInfo.albums.slice(0, 6).map((album) => (
                <div key={album} className="scan-album-preview">
                  <AppIcon name="commit" size={18} />
                  <span title={album}>{album}</span>
                </div>
              ))}
            </div>
            {sourceInfo.albums.length > 6 && (
              <p className="scan-discovery__more">另有 {sourceInfo.albums.length - 6} 个图集</p>
            )}
            <StatusBanner tone="info" title="源文件保持不变">
              分析只会读取图片并写入数据库；正式入库前不会移动、覆盖或归档源图集。
            </StatusBanner>
          </div>
        )}

        <div className="scan-primary-actions">
          {sourceInfo &&
            sourceInfo.album_count > 0 &&
            sourceInfoMatchesPath &&
            !activeImportRunId &&
            !isScanning &&
            !isFinished && (
              <Button
                variant="primary"
                onClick={handleStartScan}
                disabled={globalScanBusyRunId !== null}
              >
                开始分析
              </Button>
            )}

          {activeRun && canResumeRun(activeRun) && !isScanning && (
            <>
              <Button
                variant="primary"
                onClick={handleResumeScan}
                disabled={globalScanBusyRunId !== null}
              >
                继续分析
              </Button>
              <p className="scan-run-context mono">
                将继续任务 {activeRun.import_run_id}，不会要求重新输入源目录。
              </p>
              {canAbandonRun(activeRun) && (
                <Button
                  variant="danger"
                  onClick={handleAbandonAndRestart}
                  disabled={globalScanBusyRunId !== null || isAbandoning}
                  loading={isAbandoning}
                  loadingLabel="正在放弃旧任务..."
                >
                  放弃旧 checkpoint，重新分析
                </Button>
              )}
            </>
          )}

          {activeRun && canAbandonRun(activeRun) && !canResumeRun(activeRun) && !isScanning && (
            <>
              <Button
                variant="danger"
                onClick={handleAbandonAndRestart}
                disabled={globalScanBusyRunId !== null || isAbandoning}
                loading={isAbandoning}
                loadingLabel="正在放弃旧任务..."
              >
                放弃旧 checkpoint，重新分析
              </Button>
              <p className="scan-run-context">
                保留旧任务作为历史证据，并为当前源目录创建全新的分析任务。
              </p>
            </>
          )}
        </div>
      </section>

      <div className="scan-messages" aria-live="polite">
        {scanError && (
          <StatusBanner tone="danger" title="分析操作失败">
            {scanError}
          </StatusBanner>
        )}
        {retryAlbum.isError && (
          <StatusBanner tone="danger" title="图集重试失败">
            重试失败：{String(retryAlbum.error)}
          </StatusBanner>
        )}
        {globalScanBusyRunId && (
          <StatusBanner tone="warning" title="另一个分析任务正在运行">
            另一个分析任务正在运行（{globalScanBusyRunId}）；当前任务的继续、重试和新建操作已暂停。
          </StatusBanner>
        )}
        {runsQuery.isError && (
          <StatusBanner tone="danger" title="无法加载导入任务">
            加载导入任务失败：{String(runsQuery.error)}
          </StatusBanner>
        )}
        {activeImportRunId && albumsQuery.isError && (
          <StatusBanner tone="danger" title="无法加载图集状态">
            加载图集状态失败：{String(albumsQuery.error)}
          </StatusBanner>
        )}
      </div>

      {(isScanning || isFinished) && displayProgress && (
        <section className="scan-progress-panel" aria-labelledby="scan-progress-title">
          <div className="scan-section-heading">
            <span className="scan-step">2</span>
            <div>
              <h2 id="scan-progress-title">分析进度</h2>
              <p>{STAGE_LABELS[displayProgress.current_stage] ?? displayProgress.current_stage}</p>
            </div>
            <StatusBadge
              tone={displayProgress.state === 'failed' ? 'danger' : isFinished ? 'success' : 'info'}
            >
              {STAGE_LABELS[displayProgress.state] ?? displayProgress.state}
            </StatusBadge>
          </div>

          <Progress
            label={
              displayProgress.current_album
                ? `正在处理：${displayProgress.current_album}`
                : '分析图片'
            }
            value={displayProgress.total_images ? displayProgress.processed_images : undefined}
            max={displayProgress.total_images || undefined}
            detail={`${displayProgress.processed_images} / ${displayProgress.total_images || '正在统计'} 张图片`}
          />

          <div className="scan-progress-facts">
            <span>
              <strong>{displayProgress.total_albums}</strong> 图集
            </span>
            <span>
              <strong>{displayProgress.duplicate_count}</strong> 重复候选
            </span>
            <span className={displayProgress.error_count > 0 ? 'is-danger' : ''}>
              <strong>{displayProgress.error_count}</strong> 错误
            </span>
          </div>

          {isScanning && (
            <Button variant="danger" onClick={handleCancelScan}>
              取消扫描
            </Button>
          )}

          {displayProgress.errors.length > 0 && (
            <details className="scan-errors">
              <summary>错误详情 ({displayProgress.errors.length})</summary>
              <ul>
                {displayProgress.errors.map((err, i) => (
                  <li key={i} className="mono">
                    {err}
                  </li>
                ))}
              </ul>
            </details>
          )}

          {isFinished && (
            <div className="scan-finished-actions">
              {nextRoute && nextActionLabel && (
                <Button variant="primary" onClick={() => onNavigate(nextRoute)}>
                  {nextActionLabel}
                </Button>
              )}
              <Button
                onClick={() => {
                  setScanEvent(null);
                  setProgress(null);
                  setActiveImportRunId(null);
                  setIsScanning(false);
                }}
              >
                重置
              </Button>
            </div>
          )}
        </section>
      )}

      <section className="scan-albums-panel" aria-labelledby="scan-albums-title">
        <div className="scan-section-heading">
          <span className="scan-step">3</span>
          <div>
            <h2 id="scan-albums-title">图集流程</h2>
            <p>每个图集独立推进；失败图集可以单独重试。</p>
          </div>
        </div>

        {activeRun && albumCounts && (
          <div className="scan-summary-strip">
            <span>
              <strong>{albumCounts.total}</strong> 总图集
            </span>
            <span>
              <strong>{albumCounts.analyzed}</strong> 已分析
            </span>
            <span>
              <strong>{albumCounts.analyzing}</strong> 分析中
            </span>
            <span>
              <strong>{albumCounts.pending}</strong> 待分析
            </span>
            <span>
              <strong>{albumCounts.review}</strong> 待审核
            </span>
            <span>
              <strong>{albumCounts.failed}</strong> 失败
            </span>
          </div>
        )}

        <div className="scan-album-table-wrap">
          <table className="scan-album-table">
            {activeRun && (
              <thead>
                <tr>
                  <th>图集</th>
                  <th>图片</th>
                  <th>状态</th>
                  <th>重复候选</th>
                  <th>待审核</th>
                  <th>错误</th>
                  <th>操作</th>
                </tr>
              </thead>
            )}
            <tbody>
              {activeRun &&
                albumRows.map((album: ImportAlbumStatus) => (
                  <tr key={album.id}>
                    <td data-label="图集">
                      <strong>{album.source_name}</strong>
                    </td>
                    <td data-label="图片">{album.image_count}</td>
                    <td data-label="状态">
                      <StatusBadge
                        tone={
                          album.state === 'failed'
                            ? 'danger'
                            : album.state === 'review_required'
                              ? 'warning'
                              : album.state === 'analyzing'
                                ? 'info'
                                : album.state === 'analyzed'
                                  ? 'success'
                                  : 'neutral'
                        }
                      >
                        {ALBUM_STATE_LABELS[album.state] ?? album.state}
                      </StatusBadge>
                    </td>
                    <td data-label="重复候选">{album.duplicate_candidate_count}</td>
                    <td data-label="待审核">{album.review_candidate_count}</td>
                    <td data-label="错误" className="scan-album-error">
                      {album.last_error_message ?? ''}
                    </td>
                    <td data-label="操作">
                      <div className="scan-row-actions">
                        {activeRun.state !== 'abandoned' && album.state === 'failed' && (
                          <Button
                            onClick={() => retryAlbum.mutate(album.id)}
                            disabled={isAnyScanBusy || retryAlbum.isPending}
                          >
                            重试
                          </Button>
                        )}
                        {activeRun.state !== 'abandoned' && album.review_candidate_count > 0 && (
                          <Button variant="quiet" onClick={() => onNavigate('review')}>
                            审核
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                ))}
              {activeRun && albumsQuery.isPending && (
                <tr>
                  <td colSpan={7}>正在加载图集状态...</td>
                </tr>
              )}
              {activeRun && albumsQuery.isError && (
                <tr>
                  <td colSpan={7}>图集状态加载失败，请稍后重试。</td>
                </tr>
              )}
              {activeRun && albumsQuery.isSuccess && albumRows.length === 0 && (
                <tr>
                  <td colSpan={7}>暂无图集状态。验证源目录后开始分析。</td>
                </tr>
              )}
              {!activeRun && (
                <tr>
                  <td>
                    {runsQuery.isPending
                      ? '正在加载导入任务...'
                      : runsQuery.isError
                        ? '导入任务加载失败，请稍后重试。'
                        : '暂无图集状态。验证源目录后开始分析。'}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}
