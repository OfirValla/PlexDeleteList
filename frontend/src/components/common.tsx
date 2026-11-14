import { useEffect, useState, type ReactNode } from 'react';
import type { Action } from '../api';

const paths = {
  trash: 'M4 7h16M9 7V4h6v3m-9 0 1 13h10l1-13M10 11v6M14 11v6',
  refresh: 'M20 11a8 8 0 0 0-14.9-3M4 4v4h4M4 13a8 8 0 0 0 14.9 3M20 20v-4h-4',
  close: 'M6 6l12 12M18 6 6 18',
  search: 'M11 18a7 7 0 1 1 0-14 7 7 0 0 1 0 14zm5-2 4 4',
  chevron: 'M9 6l6 6-6 6',
  film: 'M4 4h16v16H4zM8 4v16M16 4v16M4 9h4M4 15h4M16 9h4M16 15h4',
  spark: 'M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z',
};

export function Icon({ name, size = 16 }: { name: keyof typeof paths; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={paths[name]} />
    </svg>
  );
}

/** Poster image with a text placeholder for missing or broken artwork. */
export function Poster({ url, title }: { url: string | null; title: string }) {
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [url]);
  if (!url || failed) {
    return (
      <div className="poster poster-empty">
        <Icon name="film" size={28} />
        <span>{title}</span>
      </div>
    );
  }
  return <img className="poster" src={url} alt="" loading="lazy" decoding="async" onError={() => setFailed(true)} />;
}

/**
 * The delete / redownload pair. `active` is this target's own mark; `inherited` is a mark on a parent
 * (whole series or season) that already covers it, which locks the buttons.
 */
export function MarkButtons({
  active,
  inherited,
  onToggle,
  labels = false,
  disabled = false,
}: {
  active: Action | null;
  inherited?: Action | null;
  onToggle: (action: Action) => void;
  labels?: boolean;
  disabled?: boolean;
}) {
  const state = inherited ?? active;
  const button = (action: Action, icon: 'trash' | 'refresh', text: string) => (
    <button
      type="button"
      className={`mark-btn ${action} ${state === action ? 'on' : ''} ${labels ? 'with-label' : ''}`}
      disabled={disabled || !!inherited}
      aria-pressed={state === action}
      title={
        inherited
          ? `Covered by a "${inherited}" mark on the parent`
          : state === action
            ? `Remove ${text.toLowerCase()} mark`
            : `Mark for ${text.toLowerCase()}`
      }
      onClick={(e) => {
        e.stopPropagation();
        onToggle(action);
      }}
    >
      <Icon name={icon} />
      {labels && <span>{state === action ? `Marked: ${text.toLowerCase()}` : text}</span>}
    </button>
  );
  return (
    <div className="mark-buttons">
      {button('delete', 'trash', 'Delete')}
      {button('redownload', 'refresh', 'Redownload')}
    </div>
  );
}

export function Modal({
  onClose,
  children,
  className = '',
}: {
  onClose: () => void;
  children: ReactNode;
  className?: string;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    document.addEventListener('keydown', onKey);
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', onKey);
      document.body.style.overflow = '';
    };
  }, [onClose]);
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`modal ${className}`} role="dialog" aria-modal="true">
        {children}
      </div>
    </div>
  );
}

export function AiBadge() {
  return (
    <span className="badge ai" title="Suggested by an AI assistant via MCP">
      <Icon name="spark" size={11} /> AI
    </span>
  );
}
