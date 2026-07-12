import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import {
  Button,
  EmptyState,
  IconButton,
  PageHeader,
  Progress,
  Skeleton,
  StatusBadge,
  StatusBanner,
  StatusIcon,
  Tooltip,
} from '.';

describe('M3 UI foundation', () => {
  it('exposes icon-only actions with an accessible name', () => {
    render(<IconButton label="关闭预览" icon={<StatusIcon name="error" />} />);
    expect(screen.getByRole('button', { name: '关闭预览' })).toHaveAttribute('title', '关闭预览');
  });

  it('uses an alert role for dangerous status banners', () => {
    render(
      <StatusBanner tone="danger" title="恢复证据冲突">
        为保护图库，ImageDB 不会自动修改文件。
      </StatusBanner>,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('恢复证据冲突');
    expect(screen.getByRole('alert')).toHaveTextContent('不会自动修改文件');
  });

  it('renders status text in addition to its icon and color', () => {
    render(<StatusBadge tone="warning">等待审核</StatusBadge>);
    expect(screen.getByText('等待审核')).toBeVisible();
  });

  it('exposes determinate and indeterminate progress semantics', () => {
    const { rerender } = render(<Progress label="分析图集" value={4} max={6} />);
    expect(screen.getByRole('progressbar', { name: '分析图集' })).toHaveAttribute(
      'aria-valuenow',
      '4',
    );
    expect(screen.getByText('67%')).toBeVisible();

    rerender(<Progress label="统计图片" />);
    expect(screen.getByRole('progressbar', { name: '统计图片' })).not.toHaveAttribute(
      'aria-valuenow',
    );
    expect(screen.getByText('正在统计')).toBeVisible();
  });

  it('provides meaningful loading and empty state labels', () => {
    render(
      <>
        <Skeleton label="正在加载最近任务" />
        <EmptyState
          title="还没有导入任务"
          description="选择一个包含图集的目录，开始第一次分析。"
          action={<Button variant="primary">新建导入</Button>}
        />
      </>,
    );

    expect(screen.getByRole('status', { name: '正在加载最近任务' })).toBeVisible();
    expect(screen.getByRole('heading', { name: '还没有导入任务' })).toBeVisible();
    expect(screen.getByRole('button', { name: '新建导入' })).toBeVisible();
  });

  it('keeps page hierarchy and contextual actions together', () => {
    render(
      <PageHeader
        title="新建导入"
        description="选择源目录并检查发现的图集。"
        meta={<StatusBadge tone="success">源文件只读</StatusBadge>}
        actions={<Button>查看历史</Button>}
      />,
    );

    expect(screen.getByRole('heading', { level: 1, name: '新建导入' })).toBeVisible();
    expect(screen.getByText('源文件只读')).toBeVisible();
    expect(screen.getByRole('button', { name: '查看历史' })).toBeVisible();
  });

  it('uses the standard tooltip variant for product controls', () => {
    render(
      <Tooltip content="缩小图片" trigger="click">
        <button type="button">缩小</button>
      </Tooltip>,
    );

    fireEvent.click(screen.getByRole('button', { name: '缩小' }));
    expect(screen.getByText('缩小图片')).toBeVisible();
  });
});
