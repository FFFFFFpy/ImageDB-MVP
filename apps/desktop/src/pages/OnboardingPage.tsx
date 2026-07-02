import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { DatabaseState } from '../lib/ipc/types';
import { useState } from 'react';

interface OnboardingPageProps {
  onComplete: () => void;
}

export function OnboardingPage({ onComplete }: OnboardingPageProps) {
  const [mode, setMode] = useState<'managed' | 'external' | null>(null);

  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    refetchInterval: 3000,
  });

  if (dbStatus.data?.status === 'connected') {
    return (
      <div className="onboarding-page">
        <h1>数据库已就绪</h1>
        <p>数据库连接正常，可以开始使用 ImageDB。</p>
        <DbStateSummary state={dbStatus.data} />
        <button className="btn-primary" onClick={onComplete}>
          进入应用
        </button>
      </div>
    );
  }

  return (
    <div className="onboarding-page">
      <h1>欢迎使用 ImageDB</h1>
      <p>请选择数据库模式以完成初始设置。</p>

      {dbStatus.data && <DbStateSummary state={dbStatus.data} />}

      <div className="mode-cards">
        <div
          className={`mode-card ${mode === 'managed' ? 'selected' : ''}`}
          onClick={() => setMode('managed')}
        >
          <h3>托管模式（推荐）</h3>
          <p>应用自动管理本地 PostgreSQL 实例。无需手动安装和配置。</p>
        </div>
        <div
          className={`mode-card ${mode === 'external' ? 'selected' : ''}`}
          onClick={() => setMode('external')}
        >
          <h3>外部连接</h3>
          <p>连接已有的 PostgreSQL 数据库。需要提供连接参数。</p>
        </div>
      </div>

      {mode === 'managed' && <ManagedSetup onComplete={onComplete} />}
      {mode === 'external' && <ExternalSetup onComplete={onComplete} />}
    </div>
  );
}

function DbStateSummary({ state }: { state: DatabaseState }) {
  const statusLabel = () => {
    switch (state.status) {
      case 'connected':
        return '已连接';
      case 'ready':
        return '就绪';
      case 'not_initialized':
        return '未初始化';
      case 'initializing':
        return '初始化中';
      default:
        return state.status;
    }
  };

  return (
    <div className="db-state-summary">
      <table>
        <tbody>
          <tr>
            <td>状态</td>
            <td className={state.status === 'connected' ? 'status-ok' : ''}>{statusLabel()}</td>
          </tr>
          {state.mode && (
            <tr>
              <td>模式</td>
              <td>{state.mode === 'managed_local' ? '托管' : '外部'}</td>
            </tr>
          )}
          {state.pgvector_available && (
            <tr>
              <td>pgvector</td>
              <td className="status-ok">可用</td>
            </tr>
          )}
          {state.migration_version && (
            <tr>
              <td>迁移版本</td>
              <td className="mono">{state.migration_version}</td>
            </tr>
          )}
        </tbody>
      </table>
      {state.diagnostics.length > 0 && (
        <details className="diagnostics">
          <summary>诊断信息 ({state.diagnostics.length})</summary>
          <ul>
            {state.diagnostics.map((d, i) => (
              <li key={i}>{d}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

function ManagedSetup({ onComplete }: { onComplete: () => void }) {
  const init = useMutation({
    mutationFn: api.initializeManagedDatabase,
    onSuccess: (state) => {
      if (state.status === 'connected') {
        onComplete();
      }
    },
  });

  return (
    <div className="setup-panel">
      <h3>初始化托管数据库</h3>
      <p>将在本地创建并启动 PostgreSQL 实例。</p>
      <button className="btn-primary" onClick={() => init.mutate()} disabled={init.isPending}>
        {init.isPending ? '初始化中…' : '开始初始化'}
      </button>
      {init.data && <DbStateSummary state={init.data} />}
      {init.isError && <pre className="status-err">{String(init.error)}</pre>}
    </div>
  );
}

function ExternalSetup({ onComplete }: { onComplete: () => void }) {
  const [host, setHost] = useState('127.0.0.1');
  const [port, setPort] = useState('5432');
  const [database, setDatabase] = useState('imagedb');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');

  const testConn = useMutation({
    mutationFn: () =>
      api.testExternalConnection({
        host,
        port: parseInt(port, 10),
        database,
        username,
        password: password || undefined,
      }),
  });

  const initExt = useMutation({
    mutationFn: () =>
      api.initializeExternalDatabase({
        host,
        port: parseInt(port, 10),
        database,
        username,
        password: password || undefined,
      }),
    onSuccess: (state) => {
      if (state.status === 'connected') {
        onComplete();
      }
    },
  });

  return (
    <div className="setup-panel">
      <h3>外部 PostgreSQL 连接</h3>
      <div className="form-grid">
        <label>
          主机
          <input value={host} onChange={(e) => setHost(e.target.value)} />
        </label>
        <label>
          端口
          <input type="number" value={port} onChange={(e) => setPort(e.target.value)} />
        </label>
        <label>
          数据库名
          <input value={database} onChange={(e) => setDatabase(e.target.value)} />
        </label>
        <label>
          用户名
          <input value={username} onChange={(e) => setUsername(e.target.value)} />
        </label>
        <label>
          密码
          <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </label>
      </div>
      <div className="toolbar">
        <button onClick={() => testConn.mutate()} disabled={testConn.isPending}>
          {testConn.isPending ? '测试中…' : '测试连接'}
        </button>
        <button
          className="btn-primary"
          onClick={() => initExt.mutate()}
          disabled={initExt.isPending}
        >
          {initExt.isPending ? '连接中…' : '连接并初始化'}
        </button>
      </div>
      {testConn.data && (
        <div className="check-result">
          <table>
            <tbody>
              <tr>
                <td>连接</td>
                <td>{testConn.data.connection_ok ? '成功' : '失败'}</td>
              </tr>
              <tr>
                <td>版本</td>
                <td>{testConn.data.version ?? '未知'}</td>
              </tr>
              <tr>
                <td>pgvector</td>
                <td>{testConn.data.pgvector_available ? '可用' : '不可用'}</td>
              </tr>
              <tr>
                <td>建表权限</td>
                <td>{testConn.data.can_create_tables ? '有' : '无'}</td>
              </tr>
            </tbody>
          </table>
          {testConn.data.diagnostics.length > 0 && (
            <details className="diagnostics">
              <summary>诊断信息</summary>
              <ul>
                {testConn.data.diagnostics.map((d, i) => (
                  <li key={i}>{d}</li>
                ))}
              </ul>
            </details>
          )}
        </div>
      )}
      {initExt.data && <DbStateSummary state={initExt.data} />}
      {(testConn.isError || initExt.isError) && (
        <pre className="status-err">{String(testConn.error ?? initExt.error)}</pre>
      )}
    </div>
  );
}
