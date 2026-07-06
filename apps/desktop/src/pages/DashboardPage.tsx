import { useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import { formatTaggedStatus, taggedStatusCode } from '../lib/format';

interface DashboardPageProps {
  needsOnboarding: boolean;
  onConfigureDatabase: () => void;
  onGoScan: () => void;
}

export function DashboardPage({
  needsOnboarding,
  onConfigureDatabase,
  onGoScan,
}: DashboardPageProps) {
  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: 5000,
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

      <section className="scan-action-section">
        {isConnected ? (
          <>
            <h2>新建导入</h2>
            <p>选择源目录，扫描图集并检测精确重复。</p>
            <button className="btn-primary" onClick={onGoScan}>
              开始导入
            </button>
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
