interface DropOverlayProps {
  visible: boolean;
}

/** Full-screen overlay shown while files are dragged over the gallery. */
export function DropOverlay({ visible }: DropOverlayProps) {
  if (!visible) return null;
  return (
    <div className="drop-overlay">
      <div className="drop-overlay-content">
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
          <polyline points="17 8 12 3 7 8" />
          <line x1="12" y1="3" x2="12" y2="15" />
        </svg>
        <span>Drop to upload</span>
      </div>
    </div>
  );
}
