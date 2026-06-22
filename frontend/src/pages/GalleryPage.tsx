import { useState, useEffect, useRef, useMemo } from 'react';
import type { ShareDetails } from '../types/generated/ShareDetails';
import type { SafeAsset } from '../types/generated/SafeAsset';
import lightGallery from 'lightgallery';
import { groupAssetsByDate } from '../lib/groupAssetsByDate';
import { DropOverlay } from '../components/DropOverlay';
import { UploadToast } from '../components/UploadToast';
import { NameModal } from '../components/NameModal';
import { SettingsModal } from '../components/SettingsModal';
import { GalleryGrid } from '../components/GalleryGrid';
import { useLightGallery } from '../hooks/useLightGallery';
import { useUploaderFilter } from '../hooks/useUploaderFilter';
import { useSelection } from '../hooks/useSelection';
import { useDateScrubber } from '../hooks/useDateScrubber';
import { useUpload } from '../hooks/useUpload';
import { useGalleryViewport } from '../hooks/useGalleryViewport';

import 'lightgallery/css/lightgallery-bundle.css';

type LightGallery = ReturnType<typeof lightGallery>;

interface GalleryPageProps {
  details: ShareDetails;
}

export function GalleryPage({ details }: GalleryPageProps) {
  const { link } = details;
  const realKey = link.key;
  const [assets, setAssets] = useState<SafeAsset[]>(() => link.assets);
  const allowDownload = link.allowDownload ?? false;
  const allowUpload = link.allowUpload ?? false;

  const title = link.description || link.album?.albumName || (link.type === 'INDIVIDUAL' && assets.length === 1 ? (assets[0].originalFileName || 'Shared File') : 'Shared Files');
  const albumDescription = link.album?.description;

  const [showSettingsModal, setShowSettingsModal] = useState(false);

  // Uploader filter: counts, distinct names, filtered view, and toggle.
  const {
    enabledUploaders,
    setEnabledUploaders,
    uploaderCounts,
    distinctUploaders,
    hasMultipleUploaders,
    isFilterActive,
    filteredAssets,
    toggleUploader,
  } = useUploaderFilter(assets);

  // Multi-select state + handlers (single, shift-range, group, download URL).
  const {
    selectedAssets,
    setSelectedAssets,
    onToggleAsset,
    getGroupSelectionStatus,
    onToggleGroup,
    downloadSelectionUrl,
  } = useSelection(filteredAssets, realKey);

  const [isDownloading, setIsDownloading] = useState(false);

  const lgRef = useRef<LightGallery | null>(null);
  const dateGroupRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  // Lightbox <-> gallery position sync. While the lightbox is open its slide
  // hash (#lg=1&slide=<assetId>) is the single source of truth for position, so
  // ?at is dropped; on close we write ?at from the slide last viewed and scroll
  // the grid there, so the gallery position "comes along" out of the slideshow.
  const lgOpenRef = useRef(false);
  const currentSlideIdRef = useRef<string | null>(null);
  // Remember which thumbnails have finished loading. The `loaded` class is
  // applied imperatively in onLoad for the immediate paint, but className is
  // React-controlled — so we must re-add it from here on every render, or a
  // selection re-render would drop it and restart the shimmer over the image.
  const loadedAssetsRef = useRef<Set<string>>(new Set());
  const headerRef = useRef<HTMLDivElement | null>(null);
  const [headerHeight, setHeaderHeight] = useState(96);

  // Live mirror of useUpload's isUploading, read by the viewport's lazy-load
  // observer. Declared here (before both hooks) so the viewport hook can take it
  // while useUpload — constructed after viewport because its onAssetReady sink
  // needs viewport.setDisplayCount — writes it synchronously on each render.
  const isUploadingRef = useRef(false);

  // Gallery viewport: the lazy-load render window (displayCount) plus all the
  // scroll-position machinery layered on it (shared scroll primitive, far-jump
  // resolver, ?at= restore, lazy-load observer, lightbox-exit grid jump).
  const viewport = useGalleryViewport({
    assets,
    filteredAssets,
    headerHeight,
    isUploadingRef,
  });
  // Only the render window and the two JSX-bound refs are read directly here; the
  // scroll primitives (scrollElementUnderHeader, pendingScrollRef,
  // didRestoreScrollRef) are consumed by the scrubber/lightbox via `viewport`.
  const { displayCount, setDisplayCount, galleryContainerRef, observerRef } = viewport;

  // Insert a freshly-uploaded asset into the gallery in album order, then grow
  // the lazy-load window by one so it's rendered. Owns setAssets/setDisplayCount,
  // so it stays in the component and is handed to the upload hook as the sink for
  // each ready asset.
  const insertAndSortAsset = (newAsset: SafeAsset) => {
    setAssets((prev) => {
      if (prev.some((a) => a.id === newAsset.id)) return prev;
      const next = [...prev, newAsset];
      const order = details.link.album?.order;
      if (order === 'asc') {
        next.sort((a, b) => {
          const ad = a.fileCreatedAt ? new Date(a.fileCreatedAt).getTime() : 0;
          const bd = b.fileCreatedAt ? new Date(b.fileCreatedAt).getTime() : 0;
          return ad - bd;
        });
      } else if (order === 'desc') {
        next.sort((a, b) => {
          const ad = a.fileCreatedAt ? new Date(a.fileCreatedAt).getTime() : 0;
          const bd = b.fileCreatedAt ? new Date(b.fileCreatedAt).getTime() : 0;
          return bd - ad;
        });
      }
      return next;
    });
    viewport.setDisplayCount((c) => c + 1);
  };

  // Upload subsystem: progress/toast state, drag-and-drop, the uploader-name
  // prompt modal, and the file-POST loop wired to the SSE/poll status watcher.
  // `uploaderName`/`setUploaderName` are also consumed by the SettingsModal.
  const {
    isUploading,
    uploadProgress,
    uploadStatus,
    isDragOver,
    showNameModal,
    uploaderName,
    setUploaderName,
    fileInputRef,
    triggerFileInput,
    handleCancelNameModal,
    onConfirmName,
    onFileChange,
    onDragEnter,
    onDragLeave,
    onDragOver,
    onDrop,
  } = useUpload({ realKey, allowUpload, onAssetReady: insertAndSortAsset });

  // Bridge isUploading into a ref so the viewport's lazy-load observer can read
  // the live value at intersection time. The viewport hook is constructed
  // *before* useUpload (its onAssetReady sink needs viewport.setDisplayCount),
  // so it can't take isUploading as a value; the ref breaks that ordering cycle.
  isUploadingRef.current = isUploading;

  // CSP compliance: error boundary for missing thumbnails
  useEffect(() => {
    const handleError = (e: ErrorEvent) => {
      const target = e.target as HTMLElement;
      if (target.tagName === 'IMG') {
        const item = target.closest('.gallery-item');
        if (item) item.classList.add('thumb-error');
      }
    };
    window.addEventListener('error', handleError, true);
    return () => window.removeEventListener('error', handleError, true);
  }, []);

  // Keyboard navigation
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Don't intercept when typing in inputs
      const tag = (e.target as HTMLElement).tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;

      // Escape: clear selection
      if (e.key === 'Escape' && selectedAssets.size > 0) {
        e.preventDefault();
        setSelectedAssets(new Set());
      }

      // Ctrl/Cmd+A: select all visible
      if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
        e.preventDefault();
        const visibleIds = filteredAssets.slice(0, displayCount).map(a => a.id);
        setSelectedAssets(new Set(visibleIds));
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [selectedAssets.size, filteredAssets, displayCount, setSelectedAssets]);

  // Group filteredAssets by date
  const groups = useMemo(() => groupAssetsByDate(filteredAssets), [filteredAssets]);

  // Album stats
  const albumStats = useMemo(() => {
    let photos = 0;
    let videos = 0;
    for (const a of filteredAssets) {
      if (a.type === 'VIDEO') videos++;
      else photos++;
    }
    const parts: string[] = [];
    if (photos > 0) parts.push(`${photos} photo${photos !== 1 ? 's' : ''}`);
    if (videos > 0) parts.push(`${videos} video${videos !== 1 ? 's' : ''}`);
    return parts.join(' · ');
  }, [filteredAssets]);

  // Measure the real header height (it wraps to two rows on mobile, and its box
  // includes the safe-area inset padding) and publish it as a CSS variable so
  // every layout offset — gallery padding, sticky date headers, scrubber — is
  // driven off one source of truth instead of hard-coded magic numbers.
  useEffect(() => {
    const measure = () => {
      const h = headerRef.current?.offsetHeight ?? 96;
      setHeaderHeight(h);
      document.documentElement.style.setProperty('--header-h', `${h}px`);
    };
    measure();
    window.addEventListener('resize', measure);
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(measure) : null;
    if (ro && headerRef.current) ro.observe(headerRef.current);
    return () => {
      window.removeEventListener('resize', measure);
      ro?.disconnect();
    };
  }, []);

  // Date scrubber: dimensions, proportional bar, scroll-driven indicator (with
  // the bottom tail sweep), ?at= URL persistence, and drag-to-scrub. Shares the
  // page-level scroll primitives (scrollElementUnderHeader, pendingScrollRef,
  // headerHeight, displayCount, the gallery/date-group refs, and the
  // lightbox/restore guards), so they're passed in.
  const {
    scrubberRef,
    scrubberTop,
    scrubberHeight,
    isScrubbing,
    setIsScrubbing,
    scrubLabel,
    scrubY,
    activeDateLabel,
    activeIndicatorY,
    scrubberSegments,
    scrubToClientY,
    resetTailAnchor,
    SCRUBBER_PAD,
  } = useDateScrubber({
    filteredAssets,
    groups,
    headerHeight,
    viewport,
    dateGroupRefs,
    lgOpenRef,
  });

  // Reset displayCount when filter changes. Skip the initial mount run so a
  // deep-linked slide (e.g. #lg=1&slide=61) keeps the larger displayCount the
  // initializer computed — otherwise we'd clobber it back to 40 before
  // lightGallery builds its dynamicEl, and lgHash would open an out-of-range
  // slide and crash.
  const didMountFilterResetRef = useRef(false);
  useEffect(() => {
    if (!didMountFilterResetRef.current) {
      didMountFilterResetRef.current = true;
      return;
    }
    setDisplayCount(Math.min(40, filteredAssets.length));
    // A tail anchor captured under the old filter is meaningless for the new
    // (differently sized) list; clear it so the indicator re-derives cleanly.
    resetTailAnchor();
  }, [enabledUploaders]); // eslint-disable-line react-hooks/exhaustive-deps

  // lightGallery instance + lazy filmstrip + slideshow<->grid sync.
  useLightGallery({
    filteredAssets,
    realKey,
    allowDownload,
    viewport,
    lgRef,
    lgOpenRef,
    currentSlideIdRef,
  });

  // Toggles and selection behaviors
  // Reset the download-all button once the download has actually kicked off or
  // the user dismissed the browser's save dialog. We can't observe an <a>-driven
  // download directly, so we reset on the next window focus (fires when a save
  // dialog is confirmed/cancelled) with a short timeout fallback for browsers
  // that download in the background without stealing focus.
  const handleDownloadAll = () => {
    setIsDownloading(true);
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      window.removeEventListener('focus', finish);
      clearTimeout(fallback);
      setIsDownloading(false);
    };
    const fallback = setTimeout(finish, 2000);
    window.addEventListener('focus', finish);
  };

  // Settings modal open handler
  const openSettings = () => {
    // Initialize enabled uploaders on first open if multiple uploaders exist
    if (enabledUploaders === null && hasMultipleUploaders) {
      setEnabledUploaders(new Set(distinctUploaders));
    }
    // Sync uploader name from localStorage
    setUploaderName(localStorage.getItem('uploader_name') || '');
    setShowSettingsModal(true);
  };

  const closeSettings = () => {
    // Save uploader name on close
    const name = uploaderName.trim();
    if (name) {
      localStorage.setItem('uploader_name', name);
    }
    setShowSettingsModal(false);
  };

  const handleSettingsNameBlur = () => {
    const name = uploaderName.trim();
    if (name) localStorage.setItem('uploader_name', name);
  };

  return (
    <div
      id="gallery-root"
      onDragEnter={onDragEnter}
      onDragLeave={onDragLeave}
      onDragOver={onDragOver}
      onDrop={onDrop}
    >
      <title>{title}</title>

      {/* Drag-and-drop overlay */}
      <DropOverlay visible={isDragOver} />

      {/* Selection floating bar */}
      <div id="selection-bar" className={selectedAssets.size > 0 ? 'active' : ''}>
        <button className="icon-btn" onClick={() => setSelectedAssets(new Set())}>
          ✕
        </button>
        <div className="selection-count">{selectedAssets.size} selected</div>
        <div className="selection-actions">
          <a className="icon-btn" href={downloadSelectionUrl()} title="Download selection">
            ↓
          </a>
        </div>
      </div>

      {/* Main Header */}
      <div id="header" ref={headerRef}>
        <h1>{title}</h1>
        <div className="header-actions">
          {allowUpload && (
            <div id="upload-action">
              <button
                className={isUploading ? 'header-btn disabled' : 'header-btn'}
                disabled={isUploading}
                onClick={triggerFileInput}
              >
                <img src="/images/align-top-svgrepo-com.svg" alt="" className="header-icon" />
                <span>Upload</span>
              </button>
              <input
                type="file"
                multiple
                accept="image/*,video/*"
                className="hidden-file-input"
                ref={fileInputRef}
                onChange={onFileChange}
              />
            </div>
          )}
          {allowDownload && (
            <div id="download-all">
              <a
                href={`/share/${realKey}/download`}
                title="Download all"
                className={`header-btn ${isDownloading ? 'disabled' : ''}`}
                onClick={handleDownloadAll}
              >
                {isDownloading ? (
                  <span className="loader-small" />
                ) : (
                  <img src="/images/align-bottom-svgrepo-com.svg" alt="" className="header-icon" />
                )}
                <span>{isDownloading ? 'Preparing…' : 'Download all'}</span>
              </a>
            </div>
          )}
          {(allowUpload || hasMultipleUploaders) && (
            <button
              onClick={openSettings}
              title="Settings"
              className="header-btn-circle"
              style={{ position: 'relative' }}
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="header-icon"
              >
                <circle cx="12" cy="12" r="3"></circle>
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
              </svg>
              {isFilterActive && <span className="filter-dot" />}
            </button>
          )}
        </div>
      </div>

      {albumDescription && (
        <div id="album-description">
          <h2>{albumDescription}</h2>
        </div>
      )}

      {albumStats && (
        <div className="album-stats">{albumStats}</div>
      )}

      {/* LightGallery Grid container */}
      <GalleryGrid
        containerRef={galleryContainerRef}
        groups={groups}
        displayCount={displayCount}
        realKey={realKey}
        selectedAssets={selectedAssets}
        loadedAssetsRef={loadedAssetsRef}
        dateGroupRefs={dateGroupRefs}
        getGroupSelectionStatus={getGroupSelectionStatus}
        onToggleGroup={onToggleGroup}
        onToggleAsset={onToggleAsset}
      />

      {/* Date scrubber — proportional, full-height (immich-style) */}
      {groups.length > 1 && scrubberHeight > 0 && (
        <div
          ref={scrubberRef}
          className={`date-scrubber ${isScrubbing ? 'scrubbing' : ''}`}
          style={{ top: `${scrubberTop}px`, height: `${scrubberHeight}px`, paddingTop: SCRUBBER_PAD, paddingBottom: SCRUBBER_PAD }}
          onMouseDown={(e) => { e.preventDefault(); setIsScrubbing(true); scrubToClientY(e.clientY); }}
          onTouchStart={(e) => {
            if (e.touches.length > 0) { setIsScrubbing(true); scrubToClientY(e.touches[0].clientY); }
          }}
        >
          {/* Floating date label that follows the cursor while scrubbing */}
          {isScrubbing && scrubLabel && (
            <div className="scrubber-bubble" style={{ top: `${scrubY}px` }}>
              {scrubLabel}
            </div>
          )}

          {/* Active scroll-position indicator line */}
          {!isScrubbing && activeDateLabel && (
            <div className="scrubber-indicator" style={{ top: `${activeIndicatorY}px` }} />
          )}

          {/* Proportional segments with decimated dots + year labels */}
          {scrubberSegments.map((seg) => (
            <div
              key={seg.label}
              className="scrubber-segment"
              style={{ height: `${seg.height}px` }}
            >
              {seg.hasLabel && (
                <span className="scrubber-year">{seg.year}</span>
              )}
              {seg.hasDot && <span className="scrubber-dot" />}
            </div>
          ))}
        </div>
      )}

      <div id="loading-observer" ref={observerRef} style={{ height: '1px', width: '100%' }}></div>

      {/* Loading spinners / status logs */}
      {(displayCount < filteredAssets.length || isUploading) && (
        <div id="loading-spinner">
          <span className="loader"></span>
        </div>
      )}

      {/* Upload toast notifications */}
      <UploadToast isUploading={isUploading} uploadProgress={uploadProgress} uploadStatus={uploadStatus} />

      {/* Upload name prompt modal (for upload button flow) */}
      {showNameModal && (
        <NameModal
          uploaderName={uploaderName}
          onNameChange={setUploaderName}
          onConfirm={onConfirmName}
          onCancel={handleCancelNameModal}
        />
      )}

      {/* Unified Settings modal */}
      {showSettingsModal && (
        <SettingsModal
          hasMultipleUploaders={hasMultipleUploaders}
          allowUpload={allowUpload}
          distinctUploaders={distinctUploaders}
          enabledUploaders={enabledUploaders}
          uploaderCounts={uploaderCounts}
          filteredCount={filteredAssets.length}
          totalCount={assets.length}
          uploaderName={uploaderName}
          onToggleUploader={toggleUploader}
          onSelectAllUploaders={() => setEnabledUploaders(new Set(distinctUploaders))}
          onClearAllUploaders={() => setEnabledUploaders(new Set())}
          onNameChange={setUploaderName}
          onNameBlur={handleSettingsNameBlur}
          onClose={closeSettings}
        />
      )}
    </div>
  );
}
