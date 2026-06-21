interface NameModalProps {
  uploaderName: string;
  onNameChange: (value: string) => void;
  onConfirm: (e: React.FormEvent) => void;
  onCancel: () => void;
}

/** Prompt for the uploader's name before an upload-button-initiated upload. */
export function NameModal({ uploaderName, onNameChange, onConfirm, onCancel }: NameModalProps) {
  return (
    <div className="modal-overlay" onClick={onCancel}>
      <div className="modal-container" onClick={(e) => e.stopPropagation()}>
        <h3 className="modal-title">Uploader Name</h3>
        <p className="modal-desc">Please enter your name to attribute your uploaded photos.</p>
        <form onSubmit={onConfirm}>
          <input
            type="text"
            className="modal-input"
            placeholder="Your Name"
            value={uploaderName}
            required
            autoFocus
            onChange={(e) => onNameChange(e.target.value)}
          />
          <div className="modal-actions">
            <button type="button" className="header-btn" onClick={onCancel}>
              Cancel
            </button>
            <button
              type="submit"
              className="modal-btn-confirm"
              disabled={!uploaderName.trim()}
            >
              Confirm
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
