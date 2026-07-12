import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../lib/ipc/api';
import type { DatabaseState, ExternalConnectionConfig } from '../lib/ipc/types';
import { formatDiagnostic, formatTaggedStatus, taggedStatusCode } from '../lib/format';
import { useState } from 'react';
import { Button, PageHeader, StatusBadge, StatusBanner } from '../components/ui';

interface OnboardingPageProps {
  onComplete: () => void;
  initialMode?: 'managed' | 'external' | null;
  initialDatabaseState?: DatabaseState;
  enablePolling?: boolean;
}

export function OnboardingPage({
  onComplete,
  initialMode = null,
  initialDatabaseState,
  enablePolling = true,
}: OnboardingPageProps) {
  const [mode, setMode] = useState<'managed' | 'external' | null>(initialMode);
  const [connectedState, setConnectedState] = useState<DatabaseState | null>(null);
  const queryClient = useQueryClient();

  const dbStatus = useQuery({
    queryKey: ['database-status'],
    queryFn: api.getDatabaseStatus,
    initialData: initialDatabaseState,
    refetchInterval: enablePolling ? 2000 : false,
  });
  const databaseState = connectedState ?? dbStatus.data;
  const isConnected = databaseState && taggedStatusCode(databaseState.status) === 'connected';

  // When the DB becomes connected (either via the initial mutation or the
  // poll), surface one explicit entry action.
  if (isConnected && databaseState) {
    return (
      <div className="onboarding-page onboarding-page--m3">
        <PageHeader
          title="数据库已就绪"
          description="数据库连接正常，可以开始使用 ImageDB。"
          meta={<StatusBadge tone="success">连接成功</StatusBadge>}
        />
        <StatusBanner tone="success" title="首次配置完成">
          ImageDB 已验证 PostgreSQL、pgvector 和迁移版本。
        </StatusBanner>
        <DbStateSummary state={databaseState} />
        <div className="onboarding-actions">
          <Button variant="primary" onClick={onComplete}>
            进入应用
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="onboarding-page onboarding-page--m3">
      <PageHeader
        title="欢迎使用 ImageDB"
        description={
          mode === 'external'
            ? '正在配置外部 PostgreSQL。修正连接、权限或 TLS 参数后，可以在这里重新连接并初始化。'
            : mode === 'managed'
              ? '正在初始化托管 PostgreSQL。数据库就绪后即可进入应用。'
              : '请选择数据库模式以完成初始设置。'
        }
        meta={<StatusBadge tone="info">首次配置</StatusBadge>}
      />

      {databaseState && <DbStateSummary state={databaseState} />}

      <div className="mode-cards">
        <button
          type="button"
          className={`mode-card ${mode === 'managed' ? 'selected' : ''}`}
          aria-pressed={mode === 'managed'}
          onClick={() => {
            setMode('managed');
            setConnectedState(null);
          }}
        >
          <h3>托管模式（推荐）</h3>
          <p>应用自动管理本地 PostgreSQL 实例。无需手动安装和配置。</p>
          <span>适合个人本地使用</span>
        </button>
        <button
          type="button"
          className={`mode-card ${mode === 'external' ? 'selected' : ''}`}
          aria-pressed={mode === 'external'}
          onClick={() => {
            setMode('external');
            setConnectedState(null);
          }}
        >
          <h3>外部连接</h3>
          <p>连接已有的 PostgreSQL 数据库。需要提供连接参数。</p>
          <span>适合已有数据库环境</span>
        </button>
      </div>

      {mode === 'managed' && (
        <ManagedSetup
          onInitialized={(state) => {
            setConnectedState(state);
            queryClient.invalidateQueries({ queryKey: ['database-status'] });
          }}
        />
      )}
      {mode === 'external' && (
        <ExternalSetup
          onInitialized={(state) => {
            setConnectedState(state);
            queryClient.invalidateQueries({ queryKey: ['database-status'] });
          }}
        />
      )}
    </div>
  );
}

function DbStateSummary({ state }: { state: DatabaseState }) {
  const isConnected = taggedStatusCode(state.status) === 'connected';

  return (
    <div className="db-state-summary">
      <table>
        <tbody>
          <tr>
            <td>状态</td>
            <td className={isConnected ? 'status-ok' : ''}>{formatTaggedStatus(state.status)}</td>
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
              <li key={i}>{formatDiagnostic(d)}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

function ManagedSetup({ onInitialized }: { onInitialized: (state: DatabaseState) => void }) {
  const init = useMutation({
    mutationFn: api.initializeManagedDatabase,
    onSuccess: (state) => {
      onInitialized(state);
    },
  });

  return (
    <div className="setup-panel">
      <h3>初始化托管数据库</h3>
      <p>将在本地创建并启动 PostgreSQL 实例。</p>
      <Button
        variant="primary"
        onClick={() => init.mutate()}
        loading={init.isPending}
        loadingLabel="初始化中…"
      >
        开始初始化
      </Button>
      {init.data && <DbStateSummary state={init.data} />}
      {init.isError && <pre className="status-err">{String(init.error)}</pre>}
      {init.data && taggedStatusCode(init.data.status) === 'connected' && (
        <p className="status-ok">初始化成功，点击上方「进入应用」继续。</p>
      )}
    </div>
  );
}

function ExternalSetup({ onInitialized }: { onInitialized: (state: DatabaseState) => void }) {
  const [host, setHost] = useState('127.0.0.1');
  const [port, setPort] = useState('5432');
  const [database, setDatabase] = useState('imagedb');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [tlsMode, setTlsMode] =
    useState<NonNullable<ExternalConnectionConfig['tls_mode']>>('verify_full');
  const [caCertPath, setCaCertPath] = useState('');

  function buildConfig(): ExternalConnectionConfig {
    return {
      host,
      port: parseInt(port, 10),
      database,
      username,
      password: password || undefined,
      tls_mode: tlsMode,
      ca_cert_path: caCertPath || null,
      connect_timeout_secs: 10,
      query_timeout_secs: 15,
      profile_name: 'default',
    };
  }

  const testConn = useMutation({
    mutationFn: () => api.testExternalConnection(buildConfig()),
  });

  const initExt = useMutation({
    mutationFn: () => api.initializeExternalDatabase(buildConfig()),
    onSuccess: (state) => {
      onInitialized(state);
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
        <label>
          TLS 模式
          <select
            value={tlsMode}
            onChange={(e) =>
              setTlsMode(e.target.value as NonNullable<ExternalConnectionConfig['tls_mode']>)
            }
          >
            <option value="verify_full">验证 CA 和主机名</option>
            <option value="verify_ca">验证 CA</option>
            <option value="require">仅要求加密</option>
            <option value="disable">禁用</option>
          </select>
        </label>
        <label>
          CA 证书路径
          <input value={caCertPath} onChange={(e) => setCaCertPath(e.target.value)} />
        </label>
      </div>
      <div className="toolbar">
        <Button
          variant="secondary"
          onClick={() => testConn.mutate()}
          loading={testConn.isPending}
          loadingLabel="测试中…"
        >
          测试连接
        </Button>
        <Button
          variant="primary"
          onClick={() => initExt.mutate()}
          loading={initExt.isPending}
          loadingLabel="连接中…"
        >
          连接并初始化
        </Button>
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
              <tr>
                <td>读写</td>
                <td>{testConn.data.read_write_ok ? '可写' : '不可写'}</td>
              </tr>
              <tr>
                <td>迁移状态</td>
                <td>{testConn.data.migration_state_ok ? '兼容' : '不兼容'}</td>
              </tr>
            </tbody>
          </table>
          {testConn.data.checks.length > 0 && (
            <table>
              <tbody>
                {testConn.data.checks.map((check) => (
                  <tr key={check.code}>
                    <td className="mono">{check.code}</td>
                    <td>
                      {check.status === 'pass' ? '通过' : check.status === 'warn' ? '警告' : '失败'}
                    </td>
                    <td>{check.message}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {testConn.data.diagnostics.length > 0 && (
            <details className="diagnostics">
              <summary>诊断信息</summary>
              <ul>
                {testConn.data.diagnostics.map((d, i) => (
                  <li key={i}>{formatDiagnostic(d)}</li>
                ))}
              </ul>
            </details>
          )}
        </div>
      )}
      {initExt.data && taggedStatusCode(initExt.data.status) !== 'connected' && (
        <p className="status-err">外部库尚未就绪。请按诊断信息修正后重新连接并初始化。</p>
      )}
      {initExt.data && <DbStateSummary state={initExt.data} />}
      {(testConn.isError || initExt.isError) && (
        <pre className="status-err">{String(testConn.error ?? initExt.error)}</pre>
      )}
    </div>
  );
}
