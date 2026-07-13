import { useQuery } from '@tanstack/react-query';
import {
  AppIcon,
  Button,
  EmptyState,
  PageHeader,
  Progress,
  Skeleton,
  StatusBadge,
} from '../components/ui';
import { formatTaggedStatus, taggedStatusCode } from '../lib/format';
import { api } from '../lib/ipc/api';
import type { DashboardNextAction } from '../lib/ipc/types';

interface DashboardPageProps {
  needsOnboarding: boolean;
  onConfigureDatabase: () => void;
  onGoScan: (importRunId?: string | null) => void;
  onGoReview: (importRunId: string) => void;
  onGoCommit: (importRunId: string) => void;
  onGoRecovery: () => void;
  onGoLibrary: () => void;
  enablePolling?: boolean;
}

interface NextActionPresentation {
  title: string;
  description: string;
  label: string;
  tone: 'default' | 'warning' | 'danger';
}

export function getNextActionPresentation(action: DashboardNextAction): NextActionPresentation {
  switch (action) {
    case 'recover':
      return {
        title: '继续未完成的入库事务',
        description: '入库事务需要按已有证据继续恢复，完成前不会归档源图集。',
        label: '前往恢复',
        tone: 'warning',
      };
    case 'inspect_transaction_failure':
      return {
        title: '处理失败事务',
        description: '事务已进入需要人工处置的终态，请先检查证据与文件状态。',
        label: '处理失败事务',
        tone: 'danger',
      };
    case 'review':
      return {
        title: '完成图片审核',
        description: '有候选图片等待确认；审核决定会写入后续导入计划。',
        label: '继续审核',
        tone: 'default',
      };
    case 'generate_plan':
      return {
        title: '生成导入计划',
        description: '审核已经完成，下一步将生成并检查可冻结的导入计划。',
        label: '前往入库审核',
        tone: 'default',
      };
    case 'resume_analysis':
      return {
        title: '继续分析图片',
        description: '继续上次明确保存的导入任务，不会创建重复任务。',
        label: '继续分析',
        tone: 'default',
      };
    case 'inspect_failed':
      return {
        title: '检查失败图集',
        description: '部分图集分析失败；先定位原因，再决定是否单独重试。',
        label: '查看失败图集',
        tone: 'warning',
      };
    case 'resume_commit':
      return {
        title: '继续执行入库',
        description: '已有 frozen plan 等待执行，入库仍将严格读取该计划。',
        label: '继续入库',
        tone: 'default',
      };
    case 'new_import':
      return {
        title: '导入一批新图片',
        description: '选择包含图集的目录，ImageDB 会先分析，不会修改源文件。',
        label: '开始导入',
        tone: 'default',
      };
  }
}

function formatDatabaseMode(mode: 'managed_local' | 'external' | null | undefined): string {
  if (mode === 'managed_local') return '托管本地 PostgreSQL';
  if (mode === 'external') return '外部 PostgreSQL';
  return '未配置';
}

function runStateLabel(state: string): string {
  const labels: Record<string, string> = {
    pending: '等待分析',
    analyzing: '正在分析',
    review_required: '等待审核',
    ready_to_commit: '可以生成计划',
    committing: '正在入库',
    recovery_required: '需要恢复',
    completed: '已完成',
    abandoned: '已放弃',
    failed: '失败',
    cancelled: '已取消',
  };
  return labels[state] ?? state;
}

function stateTone(state: string): 'neutral' | 'info' | 'success' | 'warning' | 'danger' {
  if (state === 'completed') return 'success';
  if (state === 'failed' || state === 'recovery_required') return 'danger';
  if (state === 'review_required' || state === 'cancelled') return 'warning';
  if (state === 'analyzing' || state === 'committing') return 'info';
  return 'neutral';
}

export function DashboardPage({
  needsOnboarding,
  onConfigureDatabase,
  onGoScan,
  onGoReview,
  onGoCommit,
  onGoRecovery,
  onGoLibrary,
  enablePolling = true,
}: DashboardPageProps) {
  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: enablePolling ? 5000 : false,
  });
  const databaseInfo = useQuery({
    queryKey: ['database-info-dashboard'],
    queryFn: api.getDatabaseInfoDashboard,
    refetchInterval: enablePolling ? 3000 : false,
    enabled: !needsOnboarding,
  });

  if (needsOnboarding) {
    return (
      <div className="dashboard-page dashboard-page--empty">
        <EmptyState
          title="欢迎使用 ImageDB"
          description="先完成本地数据库配置，之后所有图片分析与入库都在你的设备上进行。"
          action={
            <Button variant="primary" onClick={onConfigureDatabase}>
              开始设置
            </Button>
          }
        />
      </div>
    );
  }

  const statusCode = taggedStatusCode(dbStatus.data?.status);
  const statusText = formatTaggedStatus(dbStatus.data?.status);
  const info = databaseInfo.data;
  const isConnected = statusCode === 'connected';
  const isInitialLoading = dbStatus.isLoading && !dbStatus.data;
  const isManagedStartRetry =
    statusCode === 'error' &&
    statusText.includes('Managed PostgreSQL failed to start') &&
    dbStatus.data?.mode === 'managed_local';
  const isDatabaseRecovering =
    isInitialLoading || statusCode === 'initializing' || isManagedStartRetry;
  const databaseStatusLabel = isDatabaseRecovering
    ? '托管 PostgreSQL 正在启动 / 恢复中'
    : isConnected
      ? '数据库已连接'
      : statusText;
  const databaseDetail =
    dbStatus.data?.mode === 'external' && dbStatus.data.external_config
      ? `${dbStatus.data.external_config.host}:${dbStatus.data.external_config.port}/${dbStatus.data.external_config.database}`
      : dbStatus.data?.mode === 'managed_local' && dbStatus.data.managed_config
        ? `${dbStatus.data.managed_config.data_dir} : ${dbStatus.data.managed_config.port}`
        : dbStatus.data?.mode === null
          ? '尚未选择数据库模式'
          : null;

  const latestRun = info?.latest_run ?? null;
  const actionableRun = info?.latest_actionable_run ?? null;
  const nextActionCode = info?.next_action ?? 'new_import';
  const nextAction = getNextActionPresentation(nextActionCode);
  const nextActionClick = () => {
    switch (nextActionCode) {
      case 'recover':
      case 'inspect_transaction_failure':
        onGoRecovery();
        return;
      case 'review':
      case 'generate_plan':
        if (actionableRun?.import_run_id) onGoReview(actionableRun.import_run_id);
        return;
      case 'resume_analysis':
      case 'inspect_failed':
        onGoScan(actionableRun?.import_run_id ?? null);
        return;
      case 'resume_commit':
        if (actionableRun?.import_run_id) onGoCommit(actionableRun.import_run_id);
        return;
      case 'new_import':
        onGoScan(null);
    }
  };
  const processedAlbums = latestRun
    ? latestRun.analyzed_albums + latestRun.review_required_albums + latestRun.failed_albums
    : 0;

  return (
    <div className="dashboard-page">
      <PageHeader
        title="工作台"
        description="图片留在本地，按当前任务一步一步完成整理与入库。"
        actions={
          <StatusBadge tone={isConnected ? 'success' : isDatabaseRecovering ? 'info' : 'warning'}>
            {databaseStatusLabel}
          </StatusBadge>
        }
      />

      {databaseInfo.isLoading && !info ? (
        <div className="dashboard-skeleton" aria-label="正在加载工作台" role="status">
          <Skeleton height={180} />
          <Skeleton height={140} />
        </div>
      ) : (
        <>
          <section
            className={`next-task next-task--${nextAction.tone}`}
            aria-labelledby="next-task-title"
          >
            <div className="next-task__icon" aria-hidden="true">
              <AppIcon name={nextActionCode === 'new_import' ? 'import' : 'arrow'} size={24} />
            </div>
            <div className="next-task__copy">
              <span className="next-task__eyebrow">下一步</span>
              <h2 id="next-task-title">{isConnected ? nextAction.title : '先连接数据库'}</h2>
              <p>
                {isConnected
                  ? nextAction.description
                  : '初始化托管库，或连接外部 PostgreSQL；数据库就绪后才能开始导入。'}
              </p>
              {actionableRun && isConnected && nextActionCode !== 'new_import' && (
                <p className="next-task__context mono" title={actionableRun.source_root}>
                  {actionableRun.source_root}
                </p>
              )}
            </div>
            <div className="next-task__actions">
              <Button
                variant="primary"
                aria-label={isConnected ? nextAction.label : '选择数据库模式'}
                onClick={isConnected ? nextActionClick : onConfigureDatabase}
              >
                {isConnected ? nextAction.label : '选择数据库模式'}
                <AppIcon name="arrow" size={18} />
              </Button>
              {isConnected && nextActionCode !== 'new_import' && (
                <Button variant="quiet" onClick={() => onGoScan(null)}>
                  新建导入
                </Button>
              )}
            </div>
          </section>

          <div className="dashboard-grid">
            <section
              className="dashboard-panel dashboard-panel--recent"
              aria-labelledby="recent-run-title"
            >
              <div className="dashboard-panel__header">
                <div>
                  <span className="section-kicker">最近任务</span>
                  <h2 id="recent-run-title">{latestRun ? '导入任务进度' : '还没有导入任务'}</h2>
                </div>
                {latestRun && (
                  <StatusBadge tone={stateTone(latestRun.state)}>
                    {runStateLabel(latestRun.state)}
                  </StatusBadge>
                )}
              </div>
              {latestRun ? (
                <>
                  <p className="dashboard-path mono" title={latestRun.source_root}>
                    {latestRun.source_root}
                  </p>
                  <Progress
                    label="图集处理"
                    value={processedAlbums}
                    max={Math.max(latestRun.total_albums, 1)}
                    detail={`${latestRun.total_albums} 个图集 · ${latestRun.total_images} 张图片`}
                  />
                  <div className="run-facts">
                    <span>
                      已分析 <strong>{latestRun.analyzed_albums}</strong>
                    </span>
                    <span>
                      待审核 <strong>{latestRun.review_required_albums}</strong>
                    </span>
                    <span>
                      失败 <strong>{latestRun.failed_albums}</strong>
                    </span>
                  </div>
                </>
              ) : (
                <p className="dashboard-panel__empty">首次导入会在这里显示分析和审核进度。</p>
              )}
            </section>

            <section className="dashboard-panel" aria-labelledby="library-title">
              <div className="dashboard-panel__header">
                <div>
                  <span className="section-kicker">本地图库</span>
                  <h2 id="library-title">图库概览</h2>
                </div>
                <AppIcon name="commit" />
              </div>
              <div className="library-stats">
                <div>
                  <strong>{info?.library.library_album_count ?? 0}</strong>
                  <span>图集</span>
                </div>
                <div>
                  <strong>{info?.library.library_image_count ?? 0}</strong>
                  <span>图片</span>
                </div>
                <div>
                  <strong>{info?.library.library_root_count ?? 0}</strong>
                  <span>位置</span>
                </div>
              </div>
              <p className="dashboard-panel__hint">
                {formatDatabaseMode(info?.database.mode ?? dbStatus.data?.mode)}
              </p>
              <Button variant="quiet" onClick={onGoLibrary} disabled={!isConnected}>
                查看图库明细
                <AppIcon name="arrow" size={16} />
              </Button>
            </section>
          </div>

          <section className="system-health" aria-label="系统健康">
            <div className="system-health__summary">
              <span className={`health-dot ${isConnected ? 'is-ok' : 'is-warning'}`} />
              <strong>{isConnected ? '系统就绪' : databaseStatusLabel}</strong>
              <span>{dbStatus.data?.pgvector_available ? 'pgvector 可用' : 'pgvector 不可用'}</span>
              <span>迁移 {dbStatus.data?.migration_version ?? '未执行'}</span>
            </div>
            {databaseDetail && (
              <details>
                <summary>连接详情</summary>
                <p className="mono">{databaseDetail}</p>
              </details>
            )}
          </section>
        </>
      )}
    </div>
  );
}
