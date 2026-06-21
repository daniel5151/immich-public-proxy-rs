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
  const scrubberRef = useRef<HTMLDivElement | null>(null);
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
  const scrubRafRef = useRef<number | null>(null);
  const [scrubberHeight, setScrubberHeight] = useState(0);
  const [scrubberTop, setScrubberTop] = useState(96);
  const [isScrubbing, setIsScrubbing] = useState(false);
  const [scrubLabel, setScrubLabel] = useState('');
  const [scrubY, setScrubY] = useState(0);
  const [activeDateLabel, setActiveDateLabel] = useState<string>('');
  // 0..1 position of the indicator over the *whole* usable bar, derived from
  // real scroll position so it glides continuously instead of snapping between
  // band tops. Stored as a fraction so it stays correct across bar resizes.
  const [indicatorFrac, setIndicatorFrac] = useState(0);
  // While in the bottom "tail" (where the page bottoms out before any further
  // group can become active), anchor the sweep so it eases to the bar bottom
  // without jumping at handoff.
  const tailAnchorRef = useRef<{ startFrac: number; startScroll: number } | null>(null);
  const observerRef = useRef<HTMLDivElement>(null);
  // Remember which thumbnails have finished loading. The `loaded` class is
  // applied imperatively in onLoad for the immediate paint, but className is
  // React-controlled — so we must re-add it from here on every render, or a
  // selection re-render would drop it and restart the shimmer over the image.
  const loadedAssetsRef = useRef<Set<string>>(new Set());
  const headerRef = useRef<HTMLDivElement | null>(null);
  const [headerHeight, setHeaderHeight] = useState(96);

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
    tailAnchorRef.current = null;
  }, [enabledUploaders]); // eslint-disable-line react-hooks/exhaustive-deps

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
      setScrubberTop(h);
      setScrubberHeight(Math.max(0, window.innerHeight - h - 8));
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

  const SCRUBBER_PAD = 6; // top & bottom breathing room inside the bar

  // Layout offsets used when anchoring the page under the sticky header.
  // HEADER_GAP: breathing room left below the header when we scroll an element
  // to the top. ACTIVE_LINE_GAP: extra drop to the notional "active line" used
  // by the date tracker to decide which group is current.
  const HEADER_GAP = 10;
  const ACTIVE_LINE_GAP = 20;

  // Scroll the page so `el` sits just under the sticky header. Centralises the
  // getBoundingClientRect()+scrollY-headerOffset math that was copy-pasted in
  // five places. Closes over the current headerHeight, so callers that run
  // inside effects must keep headerHeight in their dependency list.
  const scrollElementUnderHeader = useCallback((el: HTMLElement, smooth = false) => {
    const top = el.getBoundingClientRect().top + window.scrollY - (headerHeight + HEADER_GAP);
    window.scrollTo({ top, behavior: smooth ? 'smooth' : 'auto' });
  }, [headerHeight]);

  // Proportional segments (immich-style): each date group gets a pixel height
  // proportional to its asset share, so the whole album maps onto the bar and
  // the first/last dates are always pinned to the very top/bottom. Dots and
  // labels are decimated by pixel distance so they never crowd or overflow.
  const scrubberSegments = useMemo(() => {
    const totalAssets = filteredAssets.length;
    const usable = scrubberHeight - SCRUBBER_PAD * 2;
    if (groups.length === 0 || totalAssets === 0 || usable <= 0) {
      return [] as {
        label: string; shortLabel: string; year: string; height: number; top: number;
        hasDot: boolean; hasLabel: boolean;
      }[];
    }
    const MIN_DOT_DISTANCE = 8;
    const MIN_LABEL_DISTANCE = 28;
    const out: {
      label: string; shortLabel: string; year: string; height: number; top: number;
      hasDot: boolean; hasLabel: boolean;
    }[] = [];
    let top = 0;
    let sinceDot = MIN_DOT_DISTANCE;
    let sinceLabel = MIN_LABEL_DISTANCE;
    let prevYear: string | null = null;
    groups.forEach((group, i) => {
      const h = (group.items.length / totalAssets) * usable;
      const parts = group.label.split(', ');
      const shortLabel = parts.length >= 2 ? parts[1] : group.label;
      const year = parts.length >= 3 ? parts[2] : (parts.at(-1) ?? '');
      let hasDot = false;
      let hasLabel = false;
      if (i === 0) {
        hasDot = true; hasLabel = true; sinceDot = 0; sinceLabel = 0; prevYear = year;
      } else {
        if (sinceDot >= MIN_DOT_DISTANCE) { hasDot = true; sinceDot = 0; }
        if (year !== prevYear && sinceLabel >= MIN_LABEL_DISTANCE) {
          hasLabel = true; sinceLabel = 0; prevYear = year;
        }
      }
      out.push({ label: group.label, shortLabel, year, height: h, top, hasDot, hasLabel });
      top += h; sinceDot += h; sinceLabel += h;
    });
    return out;
  }, [groups, filteredAssets.length, scrubberHeight]);

  // Pixel position of the active date along the bar (for the indicator line).
  const activeIndicatorY = useMemo(() => {
    const usable = scrubberHeight - SCRUBBER_PAD * 2;
    if (usable <= 0) return 0;
    // indicatorFrac is a continuous 0..1 position over the whole usable bar,
    // computed from real scroll position (with an anchored sweep through the
    // bottom tail), so the line glides and always reaches the very bottom.
    return SCRUBBER_PAD + indicatorFrac * usable;
  }, [scrubberHeight, indicatorFrac]);

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

  // Scroll the grid so the asset at a given GLOBAL INDEX sits just under the
  // header, growing the lazy-load window first if needed. This is the scrubber
  // drag target: mapping the pointer to a fractional asset index (rather than a
  // whole date group) lets a drag scroll *through* a large day's photos
  // proportionally to pointer travel, instead of snapping to the day's start.
  // Mirrors the indicator, which is also keyed on global index / total.
  const scrollToAssetIndex = (idx: number) => {
    const clamped = Math.max(0, Math.min(filteredAssets.length - 1, idx));
    const findEl = () => galleryContainerRef.current?.querySelector<HTMLElement>(
      `.gallery-item[data-index="${clamped}"]`
    );
    if (clamped <= 0) { window.scrollTo({ top: 0 }); return; }
    const el = findEl();
    if (el) { scrollElementUnderHeader(el); return; }
    if (displayCount <= clamped) {
      setDisplayCount(Math.min(clamped + 24, filteredAssets.length));
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

  // Track active date group via scroll position
  useEffect(() => {
    if (groups.length === 0) return;
    const headerOffset = headerHeight + HEADER_GAP;

    // Visual scroll response: active date label + scrubber indicator position.
    // Heavy (O(groups) DOM walk), so it runs at most once per frame via rAF.
    const updateIndicator = () => {
      const headerLine = headerOffset + ACTIVE_LINE_GAP;
      const usable = scrubberHeight - SCRUBBER_PAD * 2;
      if (usable <= 0 || scrubberSegments.length === 0) return;

      // Active group = the last one whose top has scrolled up past the header
      // line. The trailing groups in the final viewport never reach it (the
      // page bottoms out first), which is exactly why we need the tail handling
      // below to still carry the indicator to the bottom.
      let activeIdx = 0;
      for (let i = 0; i < groups.length; i++) {
        const el = dateGroupRefs.current.get(groups[i].label);
        // isConnected guard: after a filter change, date labels can collide
        // with a previous render, so the map may hold a DETACHED node. Detached
        // nodes report rect.top === 0, which would pass the test below and drag
        // activeIdx to a late group — pinning the indicator near the bottom.
        if (el && el.isConnected && el.getBoundingClientRect().top <= headerLine) activeIdx = i;
      }
      setActiveDateLabel(groups[activeIdx]?.label ?? '');

      // --- Indicator position: drive it straight off the TOP-VISIBLE asset's
      // global index, not accumulated per-group DOM geometry. Loading more
      // content *below* the viewport cannot change which asset sits at the top
      // or its index, so this is immune to the lazy-load reflow that made the
      // indicator bob as rows streamed in. It is mathematically equivalent to
      // the old proportional band math (segment heights are themselves
      // asset-count proportional, so band position == index / total), just
      // measured from one stable number instead of many shifting ones.
      //
      // Sub-tile interpolation off the top tile's own height keeps the glide
      // smooth between index steps. We still find activeIdx above for the date
      // label and the tail-sweep detection; only the fraction changed.
      const total = filteredAssets.length;
      let topIdx = groups[activeIdx]?.items[0].globalIndex ?? 0;
      let intraTile = 0;
      const gridEl = galleryContainerRef.current;
      if (gridEl) {
        const tiles = gridEl.querySelectorAll<HTMLElement>('.gallery-item[data-index]');
        for (const tile of tiles) {
          const r = tile.getBoundingClientRect();
          if (r.bottom > headerLine) {
            topIdx = parseInt(tile.getAttribute('data-index') || '0', 10) || 0;
            if (r.height > 0) intraTile = Math.min(1, Math.max(0, (headerLine - r.top) / r.height));
            break;
          }
        }
      }
      const bandFrac = total > 0 ? (topIdx + intraTile) / total : 0;

      // The anchored tail sweep (easing the indicator to the very bottom of the
      // bar) is only meaningful once the whole album is rendered. Until then the
      // page is still growing, so scrollHeight / "remaining scroll" is bogus —
      // gating on allRendered is what stops the jumps during initial load.
      const allRendered = displayCount >= filteredAssets.length;
      const maxScroll = document.documentElement.scrollHeight - window.innerHeight;
      const scrollRemaining = Math.max(0, maxScroll - window.scrollY);

      // Distance the page must still scroll for the *next* group to reach the
      // header line. Infinite for the last group (there is no next).
      let needed = Infinity;
      if (activeIdx < groups.length - 1) {
        const nextEl = dateGroupRefs.current.get(groups[activeIdx + 1].label);
        if (nextEl && nextEl.isConnected) needed = Math.max(0, nextEl.getBoundingClientRect().top - headerLine);
      }

      // Tail: the page will bottom out before the next group can activate, so
      // the active-group walk can't reach the end on its own. Only valid once
      // everything is rendered (see allRendered note above).
      const isTail = allRendered && (activeIdx >= groups.length - 1 || scrollRemaining <= needed);

      if (!isTail) {
        tailAnchorRef.current = null;
        setIndicatorFrac(Math.min(1, Math.max(0, bandFrac)));
        return;
      }

      // At the very bottom, the indicator must sit at the very end of the bar,
      // full stop. The anchored sweep below derives its progress from
      // (scrollY - startScroll) / (maxScroll - startScroll); when the final
      // day-group is short, the tail is entered only once we're already at
      // maxScroll, so startScroll ≈ maxScroll, the span collapses to ~0, and the
      // sweep can never advance past startFrac (you can't scroll past the
      // bottom). That left the indicator frozen short of the end. Pin to 1
      // whenever we're genuinely bottomed out, before the degenerate sweep runs.
      if (scrollRemaining <= 1) {
        tailAnchorRef.current = null;
        setIndicatorFrac(1);
        return;
      }

      // Anchor the tail at the band position where we entered it (so there's no
      // jump), then sweep continuously to the very bottom of the bar as the
      // remaining page scroll is consumed.
      if (!tailAnchorRef.current) {
        tailAnchorRef.current = { startFrac: bandFrac, startScroll: window.scrollY };
      }
      const anchor = tailAnchorRef.current;
      const span = Math.max(1, maxScroll - anchor.startScroll);
      const sweep = Math.min(1, Math.max(0, (window.scrollY - anchor.startScroll) / span));
      const frac = anchor.startFrac + (1 - anchor.startFrac) * sweep;
      setIndicatorFrac(Math.min(1, Math.max(0, frac)));
    };

    // Persist the current top asset to the URL (?at=<assetId>), debounced so we
    // don't spam history. Skipped until the initial restore has settled, so we
    // never overwrite the incoming ?at= before it's applied.
    const writePosition = () => {
      if (!didRestoreScrollRef.current) return;
      // While the lightbox is open the slide hash owns position; don't fight it
      // by writing ?at from whatever the grid behind it happens to show.
      if (lgOpenRef.current) return;
      const container = galleryContainerRef.current;
      if (!container) return;
      const headerLine = headerHeight + HEADER_GAP;
      // Topmost asset still (partly) below the header line. Anchor on its asset
      // id rather than its list position, so the saved ?at survives a reload
      // even if the active uploader filter (not persisted) changes the list.
      let topId: string | null = null;
      let topIdx = 0;
      const items = container.querySelectorAll<HTMLElement>('.gallery-item[data-index]');
      for (const item of items) {
        if (item.getBoundingClientRect().bottom > headerLine) {
          topId = item.getAttribute('data-asset-id');
          topIdx = parseInt(item.getAttribute('data-index') || '0', 10) || 0;
          break;
        }
      }
      try {
        const url = new URL(window.location.href);
        if (topIdx > 0 && topId) {
          url.searchParams.set('at', topId);
        } else {
          url.searchParams.delete('at'); // at the top -> clean URL
        }
        // Preserve any lightbox hash; only replace state (no new history entry).
        history.replaceState(history.state, '', url.pathname + url.search + url.hash);
      } catch { /* ignore */ }
    };

    // Single scroll subscription feeding both consumers: the indicator update is
    // rAF-throttled (coalesces bursts to one run per frame), the URL write is
    // debounced (fires 200ms after scrolling settles).
    let rafId: number | null = null;
    let writeTimer: ReturnType<typeof setTimeout> | null = null;
    const onScroll = () => {
      if (rafId == null) {
        rafId = requestAnimationFrame(() => { rafId = null; updateIndicator(); });
      }
      if (writeTimer) clearTimeout(writeTimer);
      writeTimer = setTimeout(writePosition, 200);
    };

    window.addEventListener('scroll', onScroll, { passive: true });
    updateIndicator(); // set initial indicator (no URL write until user scrolls)
    return () => {
      window.removeEventListener('scroll', onScroll);
      if (rafId != null) cancelAnimationFrame(rafId);
      if (writeTimer) clearTimeout(writeTimer);
    };
  }, [groups, headerHeight, scrubberSegments, scrubberHeight, displayCount, filteredAssets.length]);

  // Map a pointer Y (relative to the bar's usable area) to the date group whose
  // proportional band contains it, and scrub the page there.
  const scrubToClientY = (clientY: number) => {
    const bar = scrubberRef.current;
    if (!bar || scrubberSegments.length === 0) return;
    const rect = bar.getBoundingClientRect();
    const usable = scrubberHeight - SCRUBBER_PAD * 2;
    const y = Math.max(0, Math.min(usable, clientY - rect.top - SCRUBBER_PAD));
    // Map pointer -> fractional GLOBAL asset index (same space as the
    // indicator). Because segment heights are asset-count proportional, the
    // pointer's pixel position over the bar is already a linear measure of
    // album progress, so dragging through a thick day scrolls through its
    // photos proportionally instead of locking to the day's first photo.
    const total = filteredAssets.length;
    const frac = usable > 0 ? y / usable : 0;
    const targetIdx = Math.max(0, Math.min(total - 1, Math.round(frac * total)));
    // Label bubble: show the date of the band the pointer is over.
    let target = scrubberSegments[0];
    for (const seg of scrubberSegments) {
      if (y >= seg.top && y < seg.top + seg.height) { target = seg; break; }
      if (y >= seg.top) target = seg;
    }
    setScrubY(SCRUBBER_PAD + y);
    setScrubLabel(target.label);
    scrollToAssetIndex(targetIdx);
  };

  // While dragging the scrubber, track pointer globally (mouse + touch).
  useEffect(() => {
    if (!isScrubbing) return;
    const onMove = (clientY: number) => {
      if (scrubRafRef.current !== null) return;
      scrubRafRef.current = requestAnimationFrame(() => {
        scrubRafRef.current = null;
        scrubToClientY(clientY);
      });
    };
    const onMouseMove = (e: MouseEvent) => { e.preventDefault(); onMove(e.clientY); };
    const onTouchMove = (e: TouchEvent) => {
      if (e.touches.length > 0) onMove(e.touches[0].clientY);
    };
    const stop = () => setIsScrubbing(false);
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', stop);
    window.addEventListener('touchmove', onTouchMove, { passive: true });
    window.addEventListener('touchend', stop);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', stop);
      window.removeEventListener('touchmove', onTouchMove);
      window.removeEventListener('touchend', stop);
      if (scrubRafRef.current !== null) {
        cancelAnimationFrame(scrubRafRef.current);
        scrubRafRef.current = null;
      }
    };
    // scrubToClientY is re-created every render; depending on it would tear down
    // and re-bind the drag listeners mid-drag. The deps that actually affect the
    // mapping are listed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isScrubbing, scrubberSegments, scrubberHeight]);

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
