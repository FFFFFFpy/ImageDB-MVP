import { Button } from './Button';
import { EmptyState } from './EmptyState';
import { IconButton } from './IconButton';
import { PageHeader } from './PageHeader';
import { Progress } from './Progress';
import { Skeleton } from './Skeleton';
import { StatusBadge } from './StatusBadge';
import { StatusBanner } from './StatusBanner';
import { StatusIcon } from './StatusIcon';
import { Tooltip } from './Tooltip';

export function UiShowcase() {
  return (
    <div className="imagedb-ui-showcase">
      <PageHeader
        title="M3 基础组件"
        description="开发期视觉夹具，用于检查交互状态、中文文案、键盘焦点与缩放。"
        meta={<StatusBadge tone="info">Fixture</StatusBadge>}
      />

      <section>
        <h2>操作</h2>
        <div className="imagedb-ui-showcase__row">
          <Button variant="primary">开始分析</Button>
          <Button variant="secondary">查看详情</Button>
          <Button variant="quiet">暂不处理</Button>
          <Button variant="danger">放弃任务</Button>
          <Button loading loadingLabel="正在生成计划…">
            生成计划
          </Button>
          <Tooltip content="关闭当前预览">
            <IconButton label="关闭预览" icon={<StatusIcon name="error" />} />
          </Tooltip>
        </div>
      </section>

      <section>
        <h2>状态</h2>
        <div className="imagedb-ui-showcase__row">
          <StatusBadge>等待处理</StatusBadge>
          <StatusBadge tone="info">正在分析</StatusBadge>
          <StatusBadge tone="success">已完成</StatusBadge>
          <StatusBadge tone="warning">等待审核</StatusBadge>
          <StatusBadge tone="danger">处理失败</StatusBadge>
        </div>
        <StatusBanner tone="warning" title="源目录在分析后发生变化">
          请检查变更后重新开始任务；ImageDB 不会覆盖原始快照证据。
        </StatusBanner>
      </section>

      <section>
        <h2>进度与占位</h2>
        <Progress label="分析图集" value={4} max={6} detail="当前：可爱宠物（1,034 张）" />
        <Progress label="统计图片" detail="正在读取图集目录…" />
        <div className="imagedb-ui-showcase__skeletons">
          <Skeleton height={72} />
          <Skeleton height={72} />
          <Skeleton height={72} />
        </div>
      </section>

      <EmptyState
        title="还没有导入任务"
        description="选择一个包含图集的目录，ImageDB 会先分析图片，不会修改源文件。"
        action={<Button variant="primary">新建导入</Button>}
      />
    </div>
  );
}
