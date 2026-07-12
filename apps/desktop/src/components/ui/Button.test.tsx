import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Button } from './Button';

describe('Button', () => {
  it('uses the local primary adapter class and a safe native type', () => {
    render(<Button variant="primary">开始分析</Button>);

    const button = screen.getByRole('button', { name: '开始分析' });
    expect(button).toHaveAttribute('type', 'button');
    expect(button).toHaveClass('imagedb-button', 'imagedb-button--primary');
  });

  it('exposes a stable loading label and prevents duplicate actions', () => {
    render(
      <Button loading loadingLabel="正在生成计划…">
        生成导入计划
      </Button>,
    );

    const button = screen.getByRole('button', { name: '正在生成计划…' });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute('aria-busy', 'true');
  });

  it('maps danger to a distinct local variant', () => {
    render(<Button variant="danger">放弃任务</Button>);

    expect(screen.getByRole('button', { name: '放弃任务' })).toHaveClass('imagedb-button--danger');
  });
});
