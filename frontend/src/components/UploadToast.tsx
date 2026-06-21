interface UploadToastProps {
  isUploading: boolean;
  uploadProgress: { completed: number; total: number };
  uploadStatus: { type: 'success' | 'failed'; message?: string } | null;
}

/** Bottom-corner toast reporting upload progress and the final outcome. */
export function UploadToast({ isUploading, uploadProgress, uploadStatus }: UploadToastProps) {
  if (!isUploading && !uploadStatus) return null;
  return (
    <div id="upload-toast">
      {isUploading && (
        <div className="toast-content uploading">
          <span className="loader-small"></span>
          <div className="toast-text">
            <span className="toast-progress">Uploading {uploadProgress.completed}/{uploadProgress.total}</span>
            <span className="toast-warning">⚠️ Don't close this window! ⚠️</span>
          </div>
        </div>
      )}
      {!isUploading && uploadStatus?.type === 'success' && (
        <div className="toast-content success">
          <span>✅ Upload complete</span>
        </div>
      )}
      {!isUploading && uploadStatus?.type === 'failed' && (
        <div className="toast-content failed">
          <span>❌ {uploadStatus.message}</span>
        </div>
      )}
    </div>
  );
}
