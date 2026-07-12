import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { ImagePreviewDialog } from './ImagePreviewDialog';

afterEach(cleanup);

describe('ImagePreviewDialog', () => {
  test('exposes modal semantics and closes with Escape', () => {
    const onClose = vi.fn();
    render(<ImagePreviewDialog dataUrl={null} path="相册 (1)/图片.jpg" onClose={onClose} />);

    expect(screen.getByRole('dialog', { name: '图片预览' })).toHaveAttribute('aria-modal', 'true');
    expect(screen.getByRole('status')).toHaveTextContent('正在加载预览');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledOnce();
  });

  test('returns focus to the opener after closing', () => {
    const opener = document.createElement('button');
    document.body.append(opener);
    opener.focus();
    const { rerender, unmount } = render(
      <ImagePreviewDialog dataUrl="data:image/png;base64,AA==" path="图片.jpg" onClose={vi.fn()} />,
    );

    expect(screen.getByRole('dialog')).toHaveFocus();
    rerender(
      <ImagePreviewDialog
        dataUrl="data:image/png;base64,AA=="
        path="更新后的图片.jpg"
        onClose={vi.fn()}
      />,
    );
    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });
});
