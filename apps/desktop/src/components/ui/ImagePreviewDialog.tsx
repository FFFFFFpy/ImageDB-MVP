import { useEffect, useId, useRef } from 'react';
import { IconButton } from './IconButton';

interface ImagePreviewDialogProps {
  dataUrl: string | null;
  path: string;
  onClose: () => void;
}

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function ImagePreviewDialog({ dataUrl, path, onClose }: ImagePreviewDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  const titleId = useId();
  onCloseRef.current = onClose;

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = dialogRef.current;
    dialog?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab' || !dialog) return;

      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE));
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (
        event.shiftKey &&
        (document.activeElement === first || document.activeElement === dialog)
      ) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      opener?.focus();
    };
  }, []);

  return (
    <div className="image-preview-modal" onMouseDown={onClose}>
      <div
        ref={dialogRef}
        className="image-preview-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="image-preview-dialog__header">
          <h2 id={titleId}>图片预览</h2>
          <IconButton
            label="关闭图片预览"
            icon={<span aria-hidden="true">×</span>}
            onClick={onClose}
          />
        </div>
        {dataUrl ? (
          <img src={dataUrl} alt={`导入计划图片：${path}`} />
        ) : (
          <div className="image-preview-loading" role="status">
            正在加载预览…
          </div>
        )}
        <div className="image-preview-dialog__path mono">{path}</div>
      </div>
    </div>
  );
}
