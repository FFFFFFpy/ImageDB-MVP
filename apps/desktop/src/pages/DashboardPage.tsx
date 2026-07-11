import { useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { formatTaggedStatus, taggedStatusCode } from '../lib/format';

interface DashboardPageProps {
  needsOnboarding: boolean;
  onConfigureDatabase: () => void;
  onGoScan: (importRunId?: string | null) => void;
  onGoReview: () => void;
  onGoRecovery: () => void;
}

function formatDatabaseMode(mode: 'managed_local' | 'external' | null | undefined): string {
  if (mode === 'managed_local') return '托管本地 PostgreSQL';
  if (mode === 'external') return '外部 PostgreSQL';
  return '未配置';
}

export function DashboardPage({
  needsOnboarding,
  onConfigureDatabase,
  onGoScan,
  onGoReview,
  onGoRecovery,
}: DashboardPageProps) {
  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: 5000,
  });
  const databaseInfo = useQuery({
    queryKey: ['database-info-dashboard'],
    queryFn: api.getDatabaseInfoDashboard,
    refetchInterval: 3000,
    enabled: !needsOnboarding,
  });

  if (needsOnboarding) {
    return (
      <div className="dashboard-page">
        <div className="empty-state">
          <h1>欢迎使用 ImageDB</h1>
          <p>数据库尚未配置。请先完成初始化设置。</p>
          <button className="btn-primary" onClick={onConfigureDatabase}>
            开始设置
          </button>
        </div>
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
      ? '已连接'
      : statusText;
  const databaseDetail =
    dbStatus.data?.mode === 'external' && dbStatus.data.external_config
      ? `外部 PostgreSQL：${dbStatus.data.external_config.host}:${dbStatus.data.external_config.port}/${dbStatus.data.external_config.database}`
      : dbStatus.data?.mode === 'managed_local' && dbStatus.data.managed_config
        ? `托管库：${dbStatus.data.managed_config.data_dir} : ${dbStatus.data.managed_config.port}`
        : dbStatus.data?.mode === null
          ? '尚未选择数据库模式'
          : null;
  const latestRun = info?.latest_run ?? null;
  const actionableRun = info?.latest_actionable_run ?? null;
  const pendingReviews = info?.imports.pending_review_count ?? 0;
  const failedAlbums = info?.imports.failed_album_count ?? 0;
  const recoveryRequiredRuns = info?.imports.recovery_required_run_count ?? 0;
  const hasRecoveryTask = actionableRun?.state === 'recovery_required';
  const hasResumableRun =
    !!actionableRun && (actionableRun.pending_albums > 0 || actionableRun.analyzing_albums > 0);
  const hasFailedRun = !!actionableRun && actionableRun.failed_albums > 0;
  const hasReviewTask = !!actionableRun && actionableRun.pending_reviews > 0;
  const isReadyToCommit = actionableRun?.state === 'ready_to_commit';
  const nextAction = hasRecoveryTask
    ? { label: '前往恢复', onClick: onGoRecovery }
    : hasReviewTask
      ? { label: '继续审核', onClick: onGoReview }
      : hasResumableRun
        ? { label: '继续分析', onClick: () => onGoScan(actionableRun.import_run_id) }
        : hasFailedRun
          ? { label: '查看失败图集', onClick: () => onGoScan(actionableRun.import_run_id) }
          : isReadyToCommit
            ? { label: '前往入库审核', onClick: onGoReview }
            : { label: '开始导入', onClick: () => onGoScan(null) };

  return (
    <div className="dashboard-page">
      <h1>工作台</h1>

      <div className="status-cards">
        <div className={`status-card-dashboard ${isConnected ? 'ok' : 'warn'}`}>
          <h3>数据库</h3>
          <p className={isConnected ? 'status-ok' : 'status-warn'}>{databaseStatusLabel}</p>
          {databaseDetail && <p className="status-card-detail">{databaseDetail}</p>}
        </div>

        <div
          className={`status-card-dashboard ${dbStatus.data?.pgvector_available ? 'ok' : 'warn'}`}
        >
          <h3>pgvector</h3>
          <p>{dbStatus.data?.pgvector_available ? '可用' : '不可用'}</p>
        </div>

        <div
          className={`status-card-dashboard ${dbStatus.data?.migration_version ? 'ok' : 'warn'}`}
        >
          <h3>迁移</h3>
          <p className="mono">{dbStatus.data?.migration_version ?? '未执行'}</p>
        </div>
      </div>

      <section className="scan-progress-section">
        <h2>数据库概览</h2>
        <div className="database-info-grid">
          <div className="scan-progress-card">
            <h3>数据库模式</h3>
            <p>{formatDatabaseMode(info?.database.mode ?? dbStatus.data?.mode)}</p>
          </div>
          <div className="scan-progress-card">
            <h3>图库根目录</h3>
            <p>{info?.library.library_root_count ?? 0}</p>
          </div>
          <div className="scan-progress-card">
            <h3>已入库图集</h3>
            <p>{info?.library.library_album_count ?? 0}</p>
          </div>
          <div className="scan-progress-card">
            <h3>已入库图片</h3>
            <p>{info?.library.library_image_count ?? 0}</p>
          </div>
          <div className="scan-progress-card">
            <h3>导入任务</h3>
            <p>{info?.imports.import_run_count ?? 0}</p>
          </div>
          <div className="scan-progress-card">
            <h3>导入图集</h3>
            <p>{info?.imports.import_album_count ?? 0}</p>
          </div>
          <div className="scan-progress-card">
            <h3>导入图片</h3>
            <p>{info?.imports.import_image_count ?? 0}</p>
          </div>
          <div className="scan-progress-card">
            <h3>待审核</h3>
            <p className={pendingReviews > 0 ? 'status-warn' : ''}>{pendingReviews}</p>
          </div>
          <div className="scan-progress-card">
            <h3>失败图集</h3>
            <p className={failedAlbums > 0 ? 'status-error' : ''}>{failedAlbums}</p>
          </div>
          <div className="scan-progress-card">
            <h3>需要恢复</h3>
            <p className={hasRecoveryTask ? 'status-error' : 'status-ok'}>{recoveryRequiredRuns}</p>
          </div>
          <div className="scan-progress-card">
            <h3>失败任务</h3>
            <p className={(info?.imports.failed_run_count ?? 0) > 0 ? 'status-error' : ''}>
              {info?.imports.failed_run_count ?? 0}
            </p>
          </div>
          <div className="scan-progress-card">
            <h3>冻结计划</h3>
            <p>{info?.imports.frozen_plan_count ?? 0}</p>
          </div>
        </div>
        {latestRun && (
          <p className="status-card-detail">
            最近任务：{latestRun.total_albums} 个图集，已分析 {latestRun.analyzed_albums}，待审核{' '}
            {latestRun.review_required_albums}，失败 {latestRun.failed_albums}
            {latestRun.state === 'abandoned' ? '，已放弃' : ''}
          </p>
        )}
      </section>

      <section className="scan-action-section">
        {isConnected ? (
          <>
            <h2>下一步</h2>
            <p>从当前数据库状态继续处理，或开始一次新的源目录导入。</p>
            <div className="toolbar">
              <button className="btn-primary" onClick={nextAction.onClick}>
                {nextAction.label}
              </button>
              <button className="btn-secondary" onClick={() => onGoScan(null)}>
                新建导入
              </button>
            </div>
          </>
        ) : (
          <>
            <h2>选择数据库模式</h2>
            <p>先初始化托管库，或连接外部 PostgreSQL。数据库就绪后才能开始导入。</p>
            <button className="btn-primary" onClick={onConfigureDatabase}>
              选择数据库模式
            </button>
          </>
        )}
      </section>
    </div>
  );
}
