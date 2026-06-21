interface SettingsModalProps {
  hasMultipleUploaders: boolean;
  allowUpload: boolean;
  distinctUploaders: string[];
  enabledUploaders: Set<string> | null;
  uploaderCounts: Map<string, number>;
  filteredCount: number;
  totalCount: number;
  uploaderName: string;
  onToggleUploader: (name: string) => void;
  onSelectAllUploaders: () => void;
  onClearAllUploaders: () => void;
  onNameChange: (value: string) => void;
  onNameBlur: () => void;
  onClose: () => void;
}

/** Unified settings modal: uploader filter checkboxes and the uploader-name field. */
export function SettingsModal({
  hasMultipleUploaders,
  allowUpload,
  distinctUploaders,
  enabledUploaders,
  uploaderCounts,
  filteredCount,
  totalCount,
  uploaderName,
  onToggleUploader,
  onSelectAllUploaders,
  onClearAllUploaders,
  onNameChange,
  onNameBlur,
  onClose,
}: SettingsModalProps) {
  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-container settings-modal" onClick={(e) => e.stopPropagation()}>
        <h3 className="modal-title">Settings</h3>

        {hasMultipleUploaders && (
          <div className="settings-section">
            <div className="settings-section-label">Filter by Uploader</div>
            <div className="filter-list">
              {distinctUploaders.map((name) => (
                <label key={name} className="filter-row">
                  <input
                    type="checkbox"
                    checked={enabledUploaders?.has(name) ?? true}
                    onChange={() => onToggleUploader(name)}
                  />
                  <span className="filter-name">{name}</span>
                  <span className="filter-count">{uploaderCounts.get(name) ?? 0}</span>
                </label>
              ))}
            </div>
            <div className="filter-actions">
              <button className="filter-link" onClick={onSelectAllUploaders}>
                Select All
              </button>
              <span className="filter-separator">·</span>
              <button className="filter-link" onClick={onClearAllUploaders}>
                Clear All
              </button>
            </div>
            <div className="filter-summary">
              Showing {filteredCount}/{totalCount}
            </div>
          </div>
        )}

        {allowUpload && (
          <div className="settings-section">
            <div className="settings-section-label">Uploader Name</div>
            <input
              type="text"
              className="modal-input settings-name-input"
              placeholder="Your Name"
              value={uploaderName}
              onChange={(e) => onNameChange(e.target.value)}
              onBlur={onNameBlur}
            />
          </div>
        )}

        <div className="modal-actions">
          <button className="modal-btn-confirm" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
