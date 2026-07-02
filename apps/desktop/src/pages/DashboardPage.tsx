import { useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';

interface DashboardPageProps {
  needsOnboarding: boolean;
  onGoOnboarding: () => void;
}

export function DashboardPage({ needsOnboarding, onGoOnboarding }: DashboardPageProps) {
  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: 10000,
  });

  if (needsOnboarding) {
    return (
      <div className="dashboard-page">
        <div className="empty-state">
          <h1>欢迎使用 ImageDB</h1>
          <p>数据库尚未配置。请先完成初始设置。</p>
          <button className="btn-primary" onClick={onGoOnboarding}>
            开始设置
          </button>
        </div>
      </div>
    );
  }

  const isConnected = dbStatus.data?.status === 'connected';

  return (
    <div className="dashboard-page">
      <h1>工作台</h1>

      <div className="status-cards">
        <div className={`status-card-dashboard ${isConnected ? 'ok' : 'warn'}`}>
          <h3>数据库</h3>
          <p className={isConnected ? 'status-ok' : 'status-warn'}>
            {isConnected ? '已连接' : (dbStatus.data?.status ?? '加载中…')}
          </p>
          {dbStatus.data?.managed_config && (
            <p className="mono">
              {dbStatus.data.managed_config.data_dir} : {dbStatus.data.managed_config.port}
            </p>
          )}
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

      <section className="coming-soon">
        <h2>即将推出的功能</h2>
        <ul>
          <li>新建导入 - 选择源目录，开始图集分析</li>
          <li>导入历史 - 查看已完成的导入记录</li>
          <li>图库浏览 - 浏览已入库的图集</li>
        </ul>
      </section>
    </div>
  );
}
