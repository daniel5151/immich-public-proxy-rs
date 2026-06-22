import { useState, useEffect, useRef, useMemo, useCallback } from 'react';
import type { ShareDetails } from '../types/generated/ShareDetails';
import type { SafeAsset } from '../types/generated/SafeAsset';
import lightGallery from 'lightgallery';
import { groupAssetsByDate } from '../lib/groupAssetsByDate';
import { startBatchStatusPoll, startStatusStream } from '../lib/uploadStatus';
import { DropOverlay } from '../components/DropOverlay';
import { UploadToast } from '../components/UploadToast';
import { NameModal } from '../components/NameModal';
import { SettingsModal } from '../components/SettingsModal';
import { GalleryGrid } from '../components/GalleryGrid';
import { useLightGallery } from '../hooks/useLightGallery';
import { useUploaderFilter } from '../hooks/useUploaderFilter';
import { useSelection } from '../hooks/useSelection';
import { useDateScrubber } from '../hooks/useDateScrubber';
import { HEADER_GAP } from '../lib/layout';

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

  const [displayCount, setDisplayCount] = useState<number>(() => {
    // Render enough items up-front to satisfy both a deep-linked lightbox slide
    // (#lg=1&slide=N) and a restored scroll position (?at=N), taking whichever
    // reaches further into the album. Both are validated; out-of-range values
    // are ignored (and a bad slide hash is cleared so lightGallery can't crash).
    const defaultInitial = Math.min(40, assets.length);
    let needed = defaultInitial;
    try {
      const params = new URLSearchParams(window.location.hash.substring(1));
      const slideName = params.get('slide');
      if (slideName) {
        // slide is now an asset id (customSlideName). Resolve it to an index in
        // the full asset list so we pre-render enough tiles for lightGallery to
        // open it. Falls back to legacy numeric indices for old shared links.
        let slideIndex = assets.findIndex((a) => a.id === slideName);
        if (slideIndex < 0) {
          const legacy = parseInt(slideName, 10);
          slideIndex = !isNaN(legacy) && legacy >= 0 && legacy < assets.length ? legacy : -1;
        }
        if (slideIndex >= 0) {
          needed = Math.max(needed, Math.min(slideIndex + 1, assets.length));
        } else {
          // Unknown slide id. Clear the hash so lightGallery can't crash on it.
          window.location.hash = '';
        }
      }
    } catch {
      // Ignore parsing errors
    }
    try {
      const atStr = new URLSearchParams(window.location.search).get('at');
      if (atStr) {
        // ?at is an asset id; resolve to an index (legacy numeric fallback).
        let at = assets.findIndex((a) => a.id === atStr);
        if (at < 0) {
          const legacy = parseInt(atStr, 10);
          at = !isNaN(legacy) && legacy > 0 && legacy < assets.length ? legacy : -1;
        }
        if (at > 0) {
          // +12 so a little of the next row is rendered below the anchor.
          needed = Math.max(needed, Math.min(at + 12, assets.length));
        }
      }
    } catch {
      // Ignore parsing errors
    }
    return Math.min(needed, assets.length);
  });
  const [isUploading, setIsUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState({ completed: 0, total: 0 });
  const [uploadStatus, setUploadStatus] = useState<{ type: 'success' | 'failed'; message?: string } | null>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  const [isDownloading, setIsDownloading] = useState(false);
  const dragCounterRef = useRef(0);

  // Upload name prompt modal (kept separate from settings for the upload flow)
  const [showNameModal, setShowNameModal] = useState(false);
  const [uploaderName, setUploaderName] = useState(() => localStorage.getItem('uploader_name') || '');

  const fileInputRef = useRef<HTMLInputElement>(null);
  const pendingDropFilesRef = useRef<File[] | null>(null);
  const lgRef = useRef<LightGallery | null>(null);
  const galleryContainerRef = useRef<HTMLDivElement>(null);
  const dateGroupRefs = useRef<Map<string, HTMLDivElement>>(new Map());
  // Holds a resolver for a scroll target that isn't in the DOM yet: after we
  // grow the lazy-load window, the effect below runs it each render until it
  // returns the element, then scrolls to it once and clears. Shared by the
  // date-group jump and the lightbox-exit asset jump.
  const pendingScrollRef = useRef<(() => HTMLElement | null | undefined) | null>(null);
  // Lightbox <-> gallery position sync. While the lightbox is open its slide
  // hash (#lg=1&slide=<assetId>) is the single source of truth for position, so
  // ?at is dropped; on close we write ?at from the slide last viewed and scroll
  // the grid there, so the gallery position "comes along" out of the slideshow.
  const lgOpenRef = useRef(false);
  const currentSlideIdRef = useRef<string | null>(null);
  const observerRef = useRef<HTMLDivElement>(null);
  // Remember which thumbnails have finished loading. The `loaded` class is
  // applied imperatively in onLoad for the immediate paint, but className is
  // React-controlled — so we must re-add it from here on every render, or a
  // selection re-render would drop it and restart the shimmer over the image.
  const loadedAssetsRef = useRef<Set<string>>(new Set());
  const headerRef = useRef<HTMLDivElement | null>(null);
  const [headerHeight, setHeaderHeight] = useState(96);

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

  // Auto-dismiss upload toast
  useEffect(() => {
    if (!uploadStatus || isUploading) return;
    const delay = uploadStatus.type === 'failed' ? 8000 : 4000;
    const timer = setTimeout(() => setUploadStatus(null), delay);
    return () => clearTimeout(timer);
  }, [uploadStatus, isUploading]);

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

  // Scroll the page so `el` sits just under the sticky header. Centralises the
  // getBoundingClientRect()+scrollY-headerOffset math that was copy-pasted in
  // five places. Closes over the current headerHeight, so callers that run
  // inside effects must keep headerHeight in their dependency list.
  const scrollElementUnderHeader = useCallback((el: HTMLElement, smooth = false) => {
    const top = el.getBoundingClientRect().top + window.scrollY - (headerHeight + HEADER_GAP);
    window.scrollTo({ top, behavior: smooth ? 'smooth' : 'auto' });
  }, [headerHeight]);

  // Scroll the grid so a given asset id sits just under the header, growing the
  // lazy-load window first if that tile hasn't been rendered yet. Used to carry
  // the lightbox position back to the gallery when the slideshow closes.
  const scrollGridToAssetId = (assetId: string) => {
    const idx = filteredAssets.findIndex((a) => a.id === assetId);
    if (idx <= 0) return; // unknown id, or already at the very top
    const findEl = () => galleryContainerRef.current?.querySelector<HTMLElement>(
      `.gallery-item[data-asset-id="${assetId}"]`
    );
    const el = findEl();
    if (el) {
      scrollElementUnderHeader(el);
      return;
    }
    // Not rendered yet — grow the window to include it, then let the pending
    // effect scroll once it mounts.
    if (displayCount <= idx) {
      setDisplayCount(Math.min(idx + 12, filteredAssets.length));
    }
    pendingScrollRef.current = findEl;
  };

  // Once a pending far-jump target (date group or lightbox-exit asset) has been
  // rendered, scroll to it and clear. The resolver returns the element when it
  // finally exists; until then this is a no-op on each displayCount render.
  useEffect(() => {
    const resolve = pendingScrollRef.current;
    if (!resolve) return;
    const el = resolve();
    if (el) {
      pendingScrollRef.current = null;
      scrollElementUnderHeader(el);
    }
  }, [displayCount, headerHeight, scrollElementUnderHeader]);

  // --- Persist scroll position in the URL (?at=<assetIndex>) -----------------
  // We anchor on the global index of the topmost asset currently under the
  // header rather than a pixel offset or scroll fraction, so the position stays
  // correct across viewport resizes and column-count changes (phone <-> desktop)
  // and composes cleanly with the lightbox deep link, which lives in the hash
  // (?at=80#lg=1&slide=61). Lightbox open/slide/close all preserve the query
  // string, so the two halves restore independently.

  // Restore once on mount: the displayCount initializer already grew the render
  // window to include ?at=, so we just need to scroll to that tile after it
  // paints. Guarded so later displayCount growth (lazy load, scrubber) can't
  // re-trigger a jump.
  const didRestoreScrollRef = useRef(false);
  useEffect(() => {
    if (didRestoreScrollRef.current) return;
    // ?at is now an asset id (filter-independent), so a position captured while
    // a filter was active still resolves correctly after reload, when the
    // filter is gone. Legacy numeric ?at values still work as a fallback.
    let targetId: string | null = null;
    try {
      targetId = new URLSearchParams(window.location.search).get('at');
    } catch { /* ignore */ }
    if (!targetId) {
      didRestoreScrollRef.current = true; // nothing to restore
      return;
    }
    let targetIdx = filteredAssets.findIndex((a) => a.id === targetId);
    if (targetIdx < 0) {
      const legacy = parseInt(targetId, 10);
      targetIdx = !isNaN(legacy) && legacy > 0 && legacy < filteredAssets.length ? legacy : -1;
    }
    if (targetIdx <= 0) {
      didRestoreScrollRef.current = true; // not in the current (filtered) view
      return;
    }
    const el = galleryContainerRef.current?.querySelector<HTMLElement>(
      `.gallery-item[data-index="${targetIdx}"]`
    );
    if (el) {
      didRestoreScrollRef.current = true;
      scrollElementUnderHeader(el);
    }
    // If not rendered yet, leave the guard unset so the next displayCount/render
    // pass retries (the initializer should have grown the window already).
    //
    // Intentionally keyed on filteredAssets.length, not the array: this restores
    // ONCE on mount and must not re-run (re-jump) when the same-length list is
    // re-derived.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [displayCount, filteredAssets.length, headerHeight, scrollElementUnderHeader]);

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
    displayCount,
    setDisplayCount,
    galleryContainerRef,
    dateGroupRefs,
    lgOpenRef,
    didRestoreScrollRef,
    pendingScrollRef,
    scrollElementUnderHeader,
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

  // Lazy load intersection observer — operates on filteredAssets
  useEffect(() => {
    const observerTarget = observerRef.current;
    if (!observerTarget) return;

    const observer = new IntersectionObserver((entries) => {
      const entry = entries[0];
      if (entry.isIntersecting && !isUploading) {
        setDisplayCount((current) => {
          if (current < filteredAssets.length) {
            return Math.min(current + 12, filteredAssets.length);
          }
          return current;
        });
      }
    }, {
      rootMargin: '200px 0px 200px 0px'
    });

    observer.observe(observerTarget);
    return () => observer.disconnect();
    // Re-arm on every displayCount change. An IntersectionObserver only fires on
    // intersection *transitions*, not while the target stays intersecting. After
    // a fast scroll to the bottom, one batch (+12) often isn't enough to push the
    // 1px sentinel back out of the 200px rootMargin, so it remains intersecting,
    // no further transition occurs, and the spinner hangs until the user scrolls
    // up (exit) and back down (re-entry). Re-observing after each batch makes the
    // browser re-report the sentinel's current state, chaining batches until it's
    // genuinely out of view. Self-terminates once displayCount === length (the
    // functional update returns `current` unchanged, so deps stop changing).
  }, [filteredAssets.length, isUploading, displayCount]);

  // lightGallery instance + lazy filmstrip + slideshow<->grid sync.
  useLightGallery({
    filteredAssets,
    realKey,
    allowDownload,
    galleryContainerRef,
    lgRef,
    lgOpenRef,
    currentSlideIdRef,
    scrollGridToAssetId,
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
    setDisplayCount((c) => c + 1);
  };

  // Upload-status watchers live in ./lib/uploadStatus. Both expose the same
  // { add(id), done() } controller; `USE_SSE` selects server-push (SSE) over the
  // batched client poll. `insertAndSortAsset` is the gallery-side sink for each
  // ready asset (it owns setAssets/setDisplayCount, so it stays in the component).
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
      ? startStatusStream(realKey, { onAssetReady: insertAndSortAsset })
      : startBatchStatusPoll(realKey, { onAssetReady: insertAndSortAsset });

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
