import { useState, useRef, useEffect } from 'react';
import type { SafeAsset } from '../types/generated/SafeAsset';
import { startBatchStatusPoll, startStatusStream } from '../lib/uploadStatus';

interface UseUploadArgs {
  realKey: string;
  allowUpload: boolean;
  // Gallery-side sink for each ready asset. It owns setAssets/setDisplayCount,
  // so it stays in the component and is passed in here.
  onAssetReady: (asset: SafeAsset) => void;
}

/**
 * Owns the whole upload subsystem extracted from GalleryPage: upload progress
 * state, the drag-and-drop overlay state, the uploader-name prompt modal, the
 * file-POST loop wired to the SSE/poll status watcher, and the auto-dismiss
 * toast effect. `uploaderName`/`setUploaderName` are exposed because the
 * SettingsModal also reads and edits the uploader name. `onAssetReady` is the
 * gallery-side sink invoked as each uploaded asset becomes ready. Extracted
 * verbatim from GalleryPage; behavior-preserving.
 */
export function useUpload({ realKey, allowUpload, onAssetReady }: UseUploadArgs) {
  const [isUploading, setIsUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState({ completed: 0, total: 0 });
  const [uploadStatus, setUploadStatus] = useState<{ type: 'success' | 'failed'; message?: string } | null>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  const dragCounterRef = useRef(0);

  // Upload name prompt modal (kept separate from settings for the upload flow)
  const [showNameModal, setShowNameModal] = useState(false);
  const [uploaderName, setUploaderName] = useState(() => localStorage.getItem('uploader_name') || '');

  const fileInputRef = useRef<HTMLInputElement>(null);
  const pendingDropFilesRef = useRef<File[] | null>(null);

  // Auto-dismiss upload toast
  useEffect(() => {
    if (!uploadStatus || isUploading) return;
    const delay = uploadStatus.type === 'failed' ? 8000 : 4000;
    const timer = setTimeout(() => setUploadStatus(null), delay);
    return () => clearTimeout(timer);
  }, [uploadStatus, isUploading]);

  // Upload helpers
  const triggerFileInput = () => {
    const name = (localStorage.getItem('uploader_name') || '').trim();
    if (!name) {
      setUploaderName('');
      setShowNameModal(true);
    } else {
      fileInputRef.current?.click();
    }
  };

  const handleCancelNameModal = () => {
    setShowNameModal(false);
    setUploaderName(localStorage.getItem('uploader_name') || '');
    pendingDropFilesRef.current = null;
  };

  const onConfirmName = (e: React.FormEvent) => {
    e.preventDefault();
    const name = uploaderName.trim();
    if (name) {
      localStorage.setItem('uploader_name', name);
      setShowNameModal(false);
      // If the name modal was triggered by a drag-and-drop, upload those
      // stashed files directly instead of popping the file picker.
      const pending = pendingDropFilesRef.current;
      if (pending && pending.length > 0) {
        pendingDropFilesRef.current = null;
        void uploadFiles(pending);
      } else {
        // Small timeout to allow state to settle before file dialog
        setTimeout(() => {
          fileInputRef.current?.click();
        }, 50);
      }
    }
  };

  // Upload-status watchers live in ./lib/uploadStatus. Both expose the same
  // { add(id), done() } controller; `USE_SSE` selects server-push (SSE) over the
  // batched client poll. `onAssetReady` is the gallery-side sink for each ready
  // asset (it owns setAssets/setDisplayCount, so it stays in the component).
  const USE_SSE = true;

  const uploadFiles = async (files: File[]) => {
    if (files.length === 0) return;

    const count = files.length;
    setIsUploading(true);
    setUploadProgress({ completed: 0, total: count });
    setUploadStatus(null);

    const encodedName = encodeURIComponent(uploaderName.trim());
    let success = true;
    let failedName = '';

    // Start the status watcher up front so each asset can flow back into the
    // gallery as soon as it's ready. Both implementations share the same
    // { add(id), done() } controller shape; `USE_SSE` selects server-push (SSE)
    // vs. the batched client poll. (For SSE, `add` only buffers ids and the stream
    // actually opens on `done()` — see startStatusStream for why.)
    const poller = USE_SSE
      ? startStatusStream(realKey, { onAssetReady })
      : startBatchStatusPoll(realKey, { onAssetReady });

    // SSE uses a server-tracked, session-scoped pending set: tag every upload with
    // the controller's session token so the open stream picks the asset up on its
    // next tick. The poll path has no session token, so the param is simply omitted.
    const sessionToken = 'sessionToken' in poller ? poller.sessionToken : undefined;
    const uploadUrl = sessionToken
      ? `/share/${realKey}/upload?session=${encodeURIComponent(sessionToken)}`
      : `/share/${realKey}/upload`;

    for (let i = 0; i < count; i++) {
      const file = files[i];
      const fileDate = new Date(file.lastModified).toISOString();

      const formData = new FormData();
      formData.append('assetData', file, file.name);
      formData.append('deviceAssetId', file.name);
      formData.append('deviceId', 'immich-public-proxy');
      formData.append('fileCreatedAt', fileDate);
      formData.append('fileModifiedAt', fileDate);

      try {
        const res = await fetch(uploadUrl, {
          method: 'POST',
          body: formData,
          headers: {
            'x-uploader-name': encodedName,
          },
        });
        if (!res.ok) {
          success = false;
          failedName = file.name;
          break;
        }
        const uploadResult: { id: string } = await res.json();
        poller.add(uploadResult.id);
      } catch {
        success = false;
        failedName = file.name;
        break;
      }

      setUploadProgress((p) => ({ ...p, completed: p.completed + 1 }));
    }

    // Signal the poller that no more ids are coming; it finishes draining and exits.
    poller.done();

    if (success) {
      setUploadStatus({ type: 'success' });
    } else {
      setUploadStatus({ type: 'failed', message: `Failed to upload: ${failedName}` });
    }
    setIsUploading(false);
  };

  const onFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const filesList = e.target.files;
    if (!filesList || filesList.length === 0) return;
    const files = Array.from(filesList);
    // Reset file input value so same selection triggers event next time
    e.target.value = '';
    await uploadFiles(files);
  };

  // Drag-and-drop handlers
  const onDragEnter = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!allowUpload) return;
    dragCounterRef.current++;
    if (e.dataTransfer.types.includes('Files')) {
      setIsDragOver(true);
    }
  };

  const onDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    dragCounterRef.current--;
    if (dragCounterRef.current === 0) {
      setIsDragOver(false);
    }
  };

  const onDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
  };

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragOver(false);
    dragCounterRef.current = 0;
    if (!allowUpload || isUploading) return;
    const files = Array.from(e.dataTransfer.files);
    if (files.length === 0) return;
    const name = (localStorage.getItem('uploader_name') || '').trim();
    if (!name) {
      // No uploader name yet: stash the dropped files and prompt for a name.
      // They'll be uploaded on confirm, so we don't lose this drop or pop the
      // file picker.
      pendingDropFilesRef.current = files;
      setUploaderName('');
      setShowNameModal(true);
      return;
    }
    void uploadFiles(files);
  };

  return {
    // State
    isUploading,
    uploadProgress,
    uploadStatus,
    isDragOver,
    showNameModal,
    // Shared with SettingsModal
    uploaderName,
    setUploaderName,
    // Refs
    fileInputRef,
    // Handlers
    triggerFileInput,
    handleCancelNameModal,
    onConfirmName,
    onFileChange,
    onDragEnter,
    onDragLeave,
    onDragOver,
    onDrop,
  };
}
