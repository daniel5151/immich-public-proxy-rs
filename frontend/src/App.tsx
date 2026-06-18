import { useState, useEffect, useRef, useMemo, useCallback } from 'react';
import type { ShareDetails } from './types/generated/ShareDetails';
import type { SafeAsset } from './types/generated/SafeAsset';
import lightGallery from 'lightgallery';
import lgZoom from 'lightgallery/plugins/zoom';
import lgThumbnail from 'lightgallery/plugins/thumbnail';
import lgVideo from 'lightgallery/plugins/video';
import lgFullscreen from 'lightgallery/plugins/fullscreen';
import lgHash from 'lightgallery/plugins/hash';
import type { GalleryItem } from 'lightgallery/lg-utils';

import 'lightgallery/css/lightgallery-bundle.css';

type LightGallery = ReturnType<typeof lightGallery>;

export default function App() {
  const [shareKey] = useState<string>(() => {
    const pathname = window.location.pathname;
    const parts = pathname.split('/').filter(Boolean);
    if ((parts[0] === 'share' || parts[0] === 's') && parts[1]) {
      return parts[1];
    }
    return '';
  });
  const [details, setDetails] = useState<ShareDetails | null>(null);
  const [loading, setLoading] = useState<boolean>(() => {
    const pathname = window.location.pathname;
    const parts = pathname.split('/').filter(Boolean);
    return !!((parts[0] === 'share' || parts[0] === 's') && parts[1]);
  });
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!shareKey) return;

    let active = true;
    const fetchDetails = async () => {
      try {
        const res = await fetch(`/api/share/${shareKey}`);
        if (!active) return;
        if (!res.ok) {
          const errData = await res.json().catch(() => ({}));
          throw new Error(errData.error || `Failed to fetch share details (${res.status})`);
        }
        const data: ShareDetails = await res.json();
        setDetails(data);
        setError(null);
      } catch (e) {
        if (active) {
          const message = e instanceof Error ? e.message : 'Failed to load share details';
          setError(message);
        }
      } finally {
        if (active) {
          setLoading(false);
        }
      }
    };

    fetchDetails();
    return () => {
      active = false;
    };
  }, [shareKey]);

  if (loading) {
    return (
      <div id="loading-spinner">
        <span className="loader"></span>
      </div>
    );
  }

  if (!shareKey) {
    return <HomePage />;
  }

  if (error) {
    return <div className="error-msg">Error: {error}</div>;
  }

  if (details?.passwordRequired) {
    return <PasswordPage shareKey={shareKey} />;
  }

  if (details) {
    return <GalleryPage details={details} />;
  }

  return <div className="error-msg">Error: Invalid share key</div>;
}

function HomePage() {
  return (
    <div className="container" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100dvh', background: '#262626', margin: 0 }}>
      <a href="https://github.com/alangrainger/immich-public-proxy">
        <img src="/images/ipp.svg" alt="Immich Public Proxy" style={{ maxWidth: '280px', height: '280px', opacity: 0.3 }} />
      </a>
    </div>
  );
}

interface PasswordPageProps {
  shareKey: string;
}

function PasswordPage({ shareKey }: PasswordPageProps) {
  return (
    <main className="container" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100dvh', flexDirection: 'column' }}>
      <div style={{ background: '#333', padding: '2rem', borderRadius: '8px', boxShadow: '0 4px 6px rgba(0,0,0,0.1)', textAlign: 'center' }}>
        <form id="unlock" method="post" action="/share/unlock" style={{ display: 'flex', flexDirection: 'column', gap: '1rem' }}>
          <input
            type="password"
            name="password"
            placeholder="Password"
            aria-label="Password"
            required
            autoFocus
            style={{ padding: '0.5rem', fontSize: '1rem', border: '1px solid #555', borderRadius: '4px', background: '#222', color: '#fff' }}
          />
          <input type="hidden" name="key" value={shareKey} />
          <button type="submit" style={{ padding: '0.5rem 1rem', fontSize: '1rem', background: '#007bff', color: '#fff', border: 'none', borderRadius: '4px', cursor: 'pointer' }}>
            Unlock
          </button>
        </form>
      </div>
    </main>
  );
}

interface GalleryPageProps {
  details: ShareDetails;
}

interface DateGroup {
  label: string;
  items: { globalIndex: number; asset: SafeAsset }[];
}

function GalleryPage({ details }: GalleryPageProps) {
  const { link } = details;
  const realKey = link.key;
  const [assets, setAssets] = useState<SafeAsset[]>(() => link.assets);
  const allowDownload = link.allowDownload ?? false;
  const allowUpload = link.allowUpload ?? false;

  const title = link.description || link.album?.albumName || (link.type === 'INDIVIDUAL' && assets.length === 1 ? (assets[0].originalFileName || 'Shared File') : 'Shared Files');
  const albumDescription = link.album?.description;

  const [selectedAssets, setSelectedAssets] = useState<Set<string>>(new Set());

  // --- Filter by Uploader state ---
  const [enabledUploaders, setEnabledUploaders] = useState<Set<string> | null>(null);
  const [showSettingsModal, setShowSettingsModal] = useState(false);

  // Compute uploader counts and distinct names from the full asset list
  const uploaderCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const asset of assets) {
      const name = asset.uploaderName ?? 'Unknown';
      counts.set(name, (counts.get(name) ?? 0) + 1);
    }
    return counts;
  }, [assets]);

  const distinctUploaders = useMemo(() => {
    return Array.from(uploaderCounts.keys()).sort((a, b) => a.localeCompare(b));
  }, [uploaderCounts]);

  const hasMultipleUploaders = distinctUploaders.length >= 2;

  const isFilterActive = enabledUploaders !== null && enabledUploaders.size < distinctUploaders.length;

  // Filtered assets based on uploader selection
  const filteredAssets = useMemo(() => {
    if (!enabledUploaders) return assets;
    return assets.filter((a) => {
      const name = a.uploaderName ?? 'Unknown';
      return enabledUploaders.has(name);
    });
  }, [assets, enabledUploaders]);

  // --- End filter state ---

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
  const pendingScrollRef = useRef<string | null>(null);
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
  const lastSelectedIndexRef = useRef<number | null>(null);
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

  // Sync selection state with body class
  useEffect(() => {
    if (selectedAssets.size > 0) {
      document.body.classList.add('selection-mode');
    } else {
      document.body.classList.remove('selection-mode');
    }
    return () => document.body.classList.remove('selection-mode');
  }, [selectedAssets.size]);

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
  }, [selectedAssets.size, filteredAssets, displayCount]);

  // Group filteredAssets by date
  const groups = useMemo(() => {
    const groups: DateGroup[] = [];
    filteredAssets.forEach((asset, i) => {
      let dateLabel = 'Unknown Date';
      if (asset.fileCreatedAt) {
        try {
          const date = new Date(asset.fileCreatedAt);
          if (!isNaN(date.getTime())) {
            dateLabel = date.toLocaleDateString('en-US', {
              weekday: 'short',
              month: 'short',
              day: 'numeric',
              year: 'numeric'
            });
          }
        } catch {
          // Fallback
        }
      }

      if (groups.length > 0 && groups[groups.length - 1].label === dateLabel) {
        groups[groups.length - 1].items.push({ globalIndex: i, asset });
      } else {
        groups.push({
          label: dateLabel,
          items: [{ globalIndex: i, asset }]
        });
      }
    });
    return groups;
  }, [filteredAssets]);

  // Album stats
  const albumStats = useMemo(() => {
    const photos = filteredAssets.filter(a => a.type !== 'VIDEO').length;
    const videos = filteredAssets.filter(a => a.type === 'VIDEO').length;
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

  // Scroll the page to a given date group, expanding the lazy-load window first
  // if that group hasn't been rendered yet (so far-away jumps actually work).
  const scrollToGroup = (label: string) => {
    const idx = groups.findIndex((g) => g.label === label);
    if (idx < 0) return;
    const group = groups[idx];
    const startIndex = group.items[0].globalIndex;
    const el = dateGroupRefs.current.get(label);
    if (el) {
      scrollElementUnderHeader(el, !isScrubbing);
      return;
    }
    // Not rendered yet — grow displayCount to include it, then scroll next frame.
    if (displayCount <= startIndex) {
      setDisplayCount(Math.min(startIndex + 24, filteredAssets.length));
      pendingScrollRef.current = label;
    }
  };

  // Once a pending (far-jump) group has been rendered, scroll to it.
  useEffect(() => {
    const label = pendingScrollRef.current;
    if (!label) return;
    const el = dateGroupRefs.current.get(label);
    if (el) {
      pendingScrollRef.current = null;
      scrollElementUnderHeader(el);
    }
  }, [displayCount, headerHeight, scrollElementUnderHeader]);

  // Scroll the grid so a given asset id sits just under the header, growing the
  // lazy-load window first if that tile hasn't been rendered yet. Used to carry
  // the lightbox position back to the gallery when the slideshow closes.
  const pendingGridScrollIdRef = useRef<string | null>(null);
  const scrollGridToAssetId = (assetId: string) => {
    const idx = filteredAssets.findIndex((a) => a.id === assetId);
    if (idx <= 0) return; // unknown id, or already at the very top
    const el = galleryContainerRef.current?.querySelector<HTMLElement>(
      `.gallery-item[data-asset-id="${assetId}"]`
    );
    if (el) {
      scrollElementUnderHeader(el);
      return;
    }
    // Not rendered yet — grow the window to include it, then scroll next frame.
    if (displayCount <= idx) {
      setDisplayCount(Math.min(idx + 12, filteredAssets.length));
    }
    pendingGridScrollIdRef.current = assetId;
  };

  // Once a pending (deep) lightbox-exit target has rendered, scroll to it.
  useEffect(() => {
    const assetId = pendingGridScrollIdRef.current;
    if (!assetId) return;
    const el = galleryContainerRef.current?.querySelector<HTMLElement>(
      `.gallery-item[data-asset-id="${assetId}"]`
    );
    if (el) {
      pendingGridScrollIdRef.current = null;
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

      const seg = scrubberSegments[activeIdx];
      const activeEl = dateGroupRefs.current.get(groups[activeIdx].label);
      const activeConnected = !!activeEl && activeEl.isConnected;
      const thisTop = activeConnected ? activeEl.getBoundingClientRect().top : headerLine;
      const passed = Math.max(0, headerLine - thisTop);

      // Intra-band progress from the ACTIVE group's own pixel height. This is
      // robust while lazy-loading: it never depends on the *next* group being
      // in the DOM yet. (Deriving it from the next group's position made
      // `needed` Infinity for not-yet-rendered groups, which falsely tripped
      // the tail sweep below and made the indicator race to the bottom and then
      // snap back as rows streamed in — the erratic jumping.)
      const activeH = activeConnected ? activeEl.offsetHeight : 0;
      const inBand = activeH > 0 ? passed / activeH : 0;
      const bandFrac = (seg.top + Math.min(1, Math.max(0, inBand)) * seg.height) / usable;

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
    let target = scrubberSegments[0];
    for (const seg of scrubberSegments) {
      if (y >= seg.top && y < seg.top + seg.height) { target = seg; break; }
      if (y >= seg.top) target = seg;
    }
    setScrubY(SCRUBBER_PAD + y);
    setScrubLabel(target.label);
    scrollToGroup(target.label);
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
  }, [filteredAssets.length, isUploading]);

  // lightGallery initialization / update — uses filteredAssets
  useEffect(() => {
    if (filteredAssets.length === 0) return;

    const el = galleryContainerRef.current;
    if (!el) return;

    if (lgRef.current) {
      lgRef.current.destroy();
      lgRef.current = null;
    }

    // Build dynamicEl from the FULL filtered set, not the lazy-loaded slice, so
    // lightGallery's counter reads "N of {total}" instead of "N of {batch}" and
    // doesn't jump as more thumbnails load. data-index on each tile is the global
    // index into filteredAssets, so openGallery(index) still maps 1:1.
    //
    // The lgThumbnail filmstrip plugin sets <img>.src eagerly for EVERY item in
    // dynamicEl at construction time (no lazy-load option), which would fetch
    // every thumbnail in the album the moment the page loads — even if the
    // lightbox is never opened. To avoid that we hand each item a 1x1 transparent
    // placeholder as its `thumb` and stash the real thumbnail URL in
    // thumbUrlByIndex; the real URLs are swapped in lazily once the filmstrip is
    // open and scrolling (see the IntersectionObserver wiring below).
    const THUMB_PLACEHOLDER =
      'data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7';
    const thumbUrlByIndex: string[] = new Array(filteredAssets.length);
    const itemsArray = filteredAssets.map((asset, i) => {
      const previewUrl = `/share/photo/${realKey}/${asset.id}/preview`;
      const thumbnailUrl = `/share/photo/${realKey}/${asset.id}/thumbnail`;
      thumbUrlByIndex[i] = thumbnailUrl;

      if (asset.type === 'VIDEO') {
        const item = {
          slideName: asset.id,
          video: {
            source: [
              {
                // The proxy serves Immich's /video/playback, which always
                // transcodes to H.264/AAC MP4 regardless of the original
                // container. Advertising the original MIME (e.g.
                // video/quicktime) makes the browser reject the <source> it
                // can actually play, so always declare video/mp4.
                src: `/share/video/${realKey}/${asset.id}`,
                type: 'video/mp4'
              }
            ],
            attributes: {
              playsinline: true,
              controls: true
            }
          },
          poster: previewUrl,
          thumb: THUMB_PLACEHOLDER,
          downloadUrl: asset.downloadUrl
        };
        return item as unknown as GalleryItem;
      } else {
        const item = {
          slideName: asset.id,
          src: previewUrl,
          thumb: THUMB_PLACEHOLDER,
          downloadUrl: asset.downloadUrl
        };
        return item as GalleryItem;
      }
    });

    const isMobile = window.matchMedia('(max-width: 640px)').matches;
    const plugins = isMobile
      ? [lgZoom, lgVideo, lgFullscreen, lgHash]
      : [lgZoom, lgThumbnail, lgVideo, lgFullscreen, lgHash];

    const lgConfig = {
      plugins,
      speed: 200,
      licenseKey: '8FFA6495-676C4D30-8BFC54B6-4D0A6CEC',
      selector: '.gallery-item',
      download: true,
      counter: true,
      // Identify slides by asset id, not list position. The hash becomes
      // #lg=1&slide=<assetId>, which resolves to the right photo even when the
      // active uploader filter (deliberately not persisted in the URL) makes
      // the filtered list differ from the one the link was created against.
      customSlideName: true,
      hideScrollbar: true,
      swipeToClose: true,
      preload: 2,
      mobileSettings: { controls: false, showCloseIcon: true, download: true },
      dynamic: true,
      dynamicEl: itemsArray
    };

    lgRef.current = lightGallery(el, lgConfig);

    // --- Lazy filmstrip thumbnails -----------------------------------------
    // dynamicEl handed every item a 1x1 placeholder thumb (see above) so the
    // lgThumbnail plugin can't eager-fetch the whole album. Swap in the real
    // thumbnail URL only for filmstrip tiles near the viewport (via an
    // IntersectionObserver scoped to the strip) plus a window around the active
    // slide, so the backend only sees thumbnails the user is actually near.
    let thumbObserver: IntersectionObserver | null = null;

    // The lightbox (and its filmstrip) mounts on document.body, NOT inside the
    // grid container `el`, so scope these queries to the document. Only one
    // gallery instance exists at a time (destroy() runs before any rebuild).
    const findThumbOuter = () => document.querySelector('.lg-thumb-outer');

    // Concurrency-limited loader for filmstrip thumbnails. Naively setting
    // img.src the moment a tile is flagged let the observer's initial burst (or
    // a fast filmstrip drag) kick off dozens of parallel requests at once,
    // which crippled page performance. Instead we queue requested tiles and let
    // at most THUMB_MAX_CONCURRENT loads run at a time; when a slot frees we
    // re-check that the tile is STILL near the visible strip before fetching,
    // so thumbnails the user has already scrolled past are dropped, not loaded.
    const THUMB_MAX_CONCURRENT = 6;
    // Keep this in sync with the observer rootMargin below.
    const THUMB_NEAR_MARGIN_PX = 300;
    const thumbQueue: HTMLImageElement[] = [];
    const thumbQueued = new Set<HTMLImageElement>();
    let thumbInFlight = 0;

    // Is the tile still within (visible strip ± margin)? Uses live geometry so a
    // tile that has scrolled far away by the time its turn comes is skipped.
    const isThumbNearViewport = (img: HTMLImageElement): boolean => {
      const outer = findThumbOuter();
      if (!outer) return false;
      const o = outer.getBoundingClientRect();
      const r = img.getBoundingClientRect();
      // Filmstrip scrolls horizontally; only the x-axis matters in practice.
      return (
        r.right >= o.left - THUMB_NEAR_MARGIN_PX &&
        r.left <= o.right + THUMB_NEAR_MARGIN_PX
      );
    };

    const pumpThumbQueue = () => {
      while (thumbInFlight < THUMB_MAX_CONCURRENT && thumbQueue.length > 0) {
        const img = thumbQueue.shift()!;
        thumbQueued.delete(img);
        if (img.getAttribute('data-lg-loaded') === '1') continue;
        const idAttr = img.getAttribute('data-lg-item-id');
        if (idAttr == null) continue;
        const url = thumbUrlByIndex[parseInt(idAttr, 10)];
        if (!url) continue;
        // Drop tiles that have since scrolled out of range; the observer will
        // re-enqueue them if they come back into view.
        if (!isThumbNearViewport(img)) {
          thumbObserver?.observe(img);
          continue;
        }
        img.setAttribute('data-lg-loaded', '1');
        thumbObserver?.unobserve(img);
        thumbInFlight++;
        const done = () => {
          img.removeEventListener('load', done);
          img.removeEventListener('error', done);
          thumbInFlight--;
          pumpThumbQueue();
        };
        img.addEventListener('load', done);
        img.addEventListener('error', done);
        img.src = url;
      }
    };

    const enqueueThumb = (img: HTMLImageElement) => {
      if (img.getAttribute('data-lg-loaded') === '1') return;
      if (thumbQueued.has(img)) return;
      thumbQueued.add(img);
      thumbQueue.push(img);
      pumpThumbQueue();
    };

    const ensureThumbObserver = () => {
      const outer = findThumbOuter();
      if (!outer) return;
      if (!thumbObserver) {
        thumbObserver = new IntersectionObserver(
          (entries) => {
            for (const entry of entries) {
              if (entry.isIntersecting) enqueueThumb(entry.target as HTMLImageElement);
            }
          },
          // Preload roughly a strip-width of thumbnails on either side of the
          // visible filmstrip window before they scroll into view.
          { root: outer, rootMargin: `0px ${THUMB_NEAR_MARGIN_PX}px 0px ${THUMB_NEAR_MARGIN_PX}px`, threshold: 0 }
        );
      }
      outer.querySelectorAll('img[data-lg-item-id]').forEach((node) => {
        const img = node as HTMLImageElement;
        if (img.getAttribute('data-lg-loaded') !== '1') thumbObserver!.observe(img);
      });
    };
    const loadThumbsAround = (idx: number, radius = 8) => {
      const outer = findThumbOuter();
      if (!outer) return;
      const lo = Math.max(0, idx - radius);
      const hi = Math.min(thumbUrlByIndex.length - 1, idx + radius);
      for (let i = lo; i <= hi; i++) {
        const img = outer.querySelector(
          `img[data-lg-item-id="${i}"]`
        ) as HTMLImageElement | null;
        if (img) enqueueThumb(img);
      }
    };

    // Keep the gallery scroll position in sync with the slideshow. The hash
    // plugin already mirrors the active slide into #lg=1&slide=<assetId>; we
    // read that id from each slide event by index into the dynamicEl we built.
    const slideIdAt = (idx: number): string | null => {
      const item = itemsArray[idx] as unknown as { slideName?: string };
      return item?.slideName ?? null;
    };
    const onLgAfterOpen = () => {
      lgOpenRef.current = true;
      // The filmstrip DOM is now visible; start lazily filling its thumbnails.
      // Retry on the next frame in case the strip isn't queryable yet this tick.
      ensureThumbObserver();
      requestAnimationFrame(() => {
        ensureThumbObserver();
        if (lgRef.current) loadThumbsAround(lgRef.current.index ?? 0);
      });
      // The slide hash now owns position; drop the redundant ?at so the two
      // don't disagree (and so a shared in-lightbox URL stays clean).
      try {
        const url = new URL(window.location.href);
        if (url.searchParams.has('at')) {
          url.searchParams.delete('at');
          history.replaceState(history.state, '', url.pathname + url.search + url.hash);
        }
      } catch { /* ignore */ }
    };
    const onLgAfterSlide = (e: Event) => {
      const idx = (e as CustomEvent<{ index: number }>).detail?.index;
      if (typeof idx === 'number') {
        currentSlideIdRef.current = slideIdAt(idx);
        // Eagerly fill the filmstrip around the active slide so the visible
        // thumbnails are present even before the observer catches up.
        ensureThumbObserver();
        loadThumbsAround(idx);
      }
    };
    const onLgAfterClose = () => {
      lgOpenRef.current = false;
      const slideId = currentSlideIdRef.current;
      if (!slideId) return;
      // Carry the slideshow position back to the gallery: write ?at and scroll
      // the grid so the photo you left on is where you land. The hash plugin
      // clears #lg on close, so ?at becomes the surviving position on reload.
      try {
        const url = new URL(window.location.href);
        const idx = filteredAssets.findIndex((a) => a.id === slideId);
        if (idx > 0) {
          url.searchParams.set('at', slideId);
        } else {
          url.searchParams.delete('at');
        }
        history.replaceState(history.state, '', url.pathname + url.search + url.hash);
      } catch { /* ignore */ }
      scrollGridToAssetId(slideId);
    };
    el.addEventListener('lgAfterOpen', onLgAfterOpen);
    el.addEventListener('lgAfterSlide', onLgAfterSlide);
    el.addEventListener('lgAfterClose', onLgAfterClose);

    const handleClick = (e: Event) => {
      const target = (e.target as HTMLElement).closest('.gallery-item');
      if (target && el.contains(target)) {
        const indexAttr = target.getAttribute('data-index');
        if (indexAttr) {
          const index = parseInt(indexAttr, 10);
          if (!isNaN(index) && lgRef.current) {
            e.preventDefault();
            e.stopPropagation();
            lgRef.current.openGallery(index);
          }
        }
      }
    };

    el.addEventListener('click', handleClick);

    return () => {
      el.removeEventListener('click', handleClick);
      el.removeEventListener('lgAfterOpen', onLgAfterOpen);
      el.removeEventListener('lgAfterSlide', onLgAfterSlide);
      el.removeEventListener('lgAfterClose', onLgAfterClose);
      if (thumbObserver) {
        thumbObserver.disconnect();
        thumbObserver = null;
      }
      if (lgRef.current) {
        lgRef.current.destroy();
        lgRef.current = null;
      }
    };
  // NB: displayCount intentionally omitted — dynamicEl is built from the full
  // filteredAssets, so lazy-load growth must NOT tear down and rebuild the
  // lightbox (which also caused the counter to jump).
  }, [filteredAssets, realKey]);

  // Toggles and selection behaviors
  const onToggleAsset = (id: string, index?: number, shiftKey?: boolean) => {
    // Capture the anchor BEFORE we overwrite the ref at the end of this call.
    // setSelectedAssets runs its updater asynchronously, so reading
    // lastSelectedIndexRef inside it would see the just-written value and
    // collapse the range to the single clicked tile.
    const anchorIndex = lastSelectedIndexRef.current;
    setSelectedAssets((prev) => {
      const next = new Set(prev);
      // Shift-click range select
      if (shiftKey && index != null && anchorIndex != null) {
        const start = Math.min(anchorIndex, index);
        const end = Math.max(anchorIndex, index);
        for (let i = start; i <= end; i++) {
          if (i < filteredAssets.length) {
            next.add(filteredAssets[i].id);
          }
        }
      } else if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
    if (index != null) {
      lastSelectedIndexRef.current = index;
    }
  };

  const getGroupSelectionStatus = (groupItems: { asset: SafeAsset }[]) => {
    const allSelected = groupItems.every((item) => selectedAssets.has(item.asset.id));
    return allSelected;
  };

  const onToggleGroup = (groupItems: { asset: SafeAsset }[]) => {
    setSelectedAssets((prev) => {
      const next = new Set(prev);
      const allSelected = groupItems.every((item) => prev.has(item.asset.id));
      if (allSelected) {
        groupItems.forEach((item) => next.delete(item.asset.id));
      } else {
        groupItems.forEach((item) => next.add(item.asset.id));
      }
      return next;
    });
  };

  const downloadSelectionUrl = () => {
    if (selectedAssets.size === 0) return '';
    const idsStr = Array.from(selectedAssets).join(',');
    return `/share/${realKey}/download?asset_ids=${encodeURIComponent(idsStr)}`;
  };

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

  const pollAssetStatus = async (assetId: string) => {
    // Poll every 500ms up to 40 times (20 seconds max)
    const maxAttempts = 40;
    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      try {
        const res = await fetch(`/share/${realKey}/status/${assetId}`);
        if (res.status === 200) {
          const newAsset: SafeAsset = await res.json();
          insertAndSortAsset(newAsset);
          return;
        } else if (res.status !== 202) {
          console.error(`Status polling failed for asset ${assetId}: ${res.status}`);
          return;
        }
      } catch (err) {
        console.error(`Status polling request failed for asset ${assetId}:`, err);
      }
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    console.warn(`Status polling timed out for asset ${assetId}`);
  };

  const uploadFiles = async (files: File[]) => {
    if (files.length === 0) return;

    const count = files.length;
    setIsUploading(true);
    setUploadProgress({ completed: 0, total: count });
    setUploadStatus(null);

    const encodedName = encodeURIComponent(uploaderName.trim());
    let success = true;
    let failedName = '';

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
        const res = await fetch(`/share/${realKey}/upload`, {
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
        pollAssetStatus(uploadResult.id);
      } catch {
        success = false;
        failedName = file.name;
        break;
      }

      setUploadProgress((p) => ({ ...p, completed: p.completed + 1 }));
    }

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
      {isDragOver && (
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
      )}

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
      <div id="lightgallery" ref={galleryContainerRef}>
        {groups.map((group) => {
          const groupStartIndex = group.items[0].globalIndex;
          if (displayCount <= groupStartIndex) return null;

          const isGroupSelected = getGroupSelectionStatus(group.items);
          const visibleGroupItems = group.items.filter((item) => item.globalIndex < displayCount);

          return (
            <div
              key={group.label}
              className="gallery-date-group"
              ref={(el) => {
                if (el) dateGroupRefs.current.set(group.label, el);
                else dateGroupRefs.current.delete(group.label);
              }}
            >
              <div className="gallery-date-header">
                <span className="date-label">{group.label}</span>
                <div
                  className={`date-selector ${isGroupSelected ? 'selected' : ''}`}
                  onClick={() => onToggleGroup(group.items)}
                >
                  <span className="check-icon"></span>
                </div>
              </div>

              <div className="gallery-date-items">
                {visibleGroupItems.map(({ globalIndex, asset }) => {
                  const isAssetSelected = selectedAssets.has(asset.id);
                  const isVideo = asset.type === 'VIDEO';

                  // Dynamic layout basis calculation
                  const aspect = (asset.width || 4) / (asset.height || 3);
                  const flexBasis = `${250 * aspect}px`;

                  return (
                    <div
                      key={asset.id}
                      className={`tile-wrapper ${isAssetSelected ? 'selected' : ''} ${loadedAssetsRef.current.has(asset.id) ? 'loaded' : ''}`}
                      style={{ flexBasis, flexGrow: aspect }}
                    >
                      <div
                        className="tile-selector"
                        onClick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          onToggleAsset(asset.id, globalIndex, e.shiftKey);
                        }}
                      >
                        <span className="check-icon"></span>
                      </div>
                      <a
                        className="gallery-item"
                        data-index={globalIndex}
                        data-asset-id={asset.id}
                        href={`/share/photo/${realKey}/${asset.id}/preview`}
                        onClick={(e) => {
                          e.preventDefault();
                          if (selectedAssets.size > 0) {
                            e.stopPropagation();
                            onToggleAsset(asset.id, globalIndex, e.shiftKey);
                          }
                          // Otherwise let lightGallery click handler take over
                        }}
                      >
                        <img
                          loading="lazy"
                          src={`/share/photo/${realKey}/${asset.id}/thumbnail`}
                          alt=""
                          onLoad={(e) => {
                            loadedAssetsRef.current.add(asset.id);
                            const wrapper = (e.target as HTMLElement).closest('.tile-wrapper');
                            if (wrapper) wrapper.classList.add('loaded');
                          }}
                        />
                        {asset.uploaderName && (
                          <div className={`uploader-badge ${asset.uploaderIsFallback ? 'using-owner-data' : ''}`}>
                            {asset.uploaderName}
                          </div>
                        )}
                        {isVideo && <div className="play-icon"></div>}
                      </a>
                    </div>
                  );
                })}
              </div>
            </div>
          );
        })}
      </div>

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
      {(isUploading || uploadStatus) && (
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
      )}

      {/* Upload name prompt modal (for upload button flow) */}
      {showNameModal && (
        <div className="modal-overlay" onClick={handleCancelNameModal}>
          <div className="modal-container" onClick={(e) => e.stopPropagation()}>
            <h3 className="modal-title">Uploader Name</h3>
            <p className="modal-desc">Please enter your name to attribute your uploaded photos.</p>
            <form onSubmit={onConfirmName}>
              <input
                type="text"
                className="modal-input"
                placeholder="Your Name"
                value={uploaderName}
                required
                autoFocus
                onChange={(e) => setUploaderName(e.target.value)}
              />
              <div className="modal-actions">
                <button type="button" className="header-btn" onClick={handleCancelNameModal}>
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
      )}

      {/* Unified Settings modal */}
      {showSettingsModal && (
        <div className="modal-overlay" onClick={closeSettings}>
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
                        onChange={() => {
                          setEnabledUploaders((prev) => {
                            const next = new Set(prev ?? distinctUploaders);
                            if (next.has(name)) {
                              next.delete(name);
                            } else {
                              next.add(name);
                            }
                            return next;
                          });
                        }}
                      />
                      <span className="filter-name">{name}</span>
                      <span className="filter-count">{uploaderCounts.get(name) ?? 0}</span>
                    </label>
                  ))}
                </div>
                <div className="filter-actions">
                  <button
                    className="filter-link"
                    onClick={() => setEnabledUploaders(new Set(distinctUploaders))}
                  >
                    Select All
                  </button>
                  <span className="filter-separator">·</span>
                  <button
                    className="filter-link"
                    onClick={() => setEnabledUploaders(new Set())}
                  >
                    Clear All
                  </button>
                </div>
                <div className="filter-summary">
                  Showing {filteredAssets.length}/{assets.length}
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
                  onChange={(e) => setUploaderName(e.target.value)}
                  onBlur={() => {
                    const name = uploaderName.trim();
                    if (name) localStorage.setItem('uploader_name', name);
                  }}
                />
              </div>
            )}

            <div className="modal-actions">
              <button className="modal-btn-confirm" onClick={closeSettings}>
                Done
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
