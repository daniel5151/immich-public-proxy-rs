import { useState, useEffect, useMemo, useRef } from 'react';
import type { RefObject } from 'react';
import type { SafeAsset } from '../types/generated/SafeAsset';
import type { DateGroup } from '../types';
import type { GalleryViewport } from './useGalleryViewport';
import { HEADER_GAP, ACTIVE_LINE_GAP } from '../lib/layout';

const SCRUBBER_PAD = 6; // top & bottom breathing room inside the bar

export interface ScrubberSegment {
  label: string;
  shortLabel: string;
  year: string;
  height: number;
  top: number;
  hasDot: boolean;
  hasLabel: boolean;
}

interface UseDateScrubberArgs {
  filteredAssets: SafeAsset[];
  groups: DateGroup[];
  headerHeight: number;
  // The render-window + scroll-position machinery the scrubber drives and reads:
  // it grows displayCount to bring a scrubbed-to date into view, scrolls via the
  // shared primitive, queues far jumps through pendingScrollRef, and respects the
  // restore-once guard. Passed as one unit instead of six loose props.
  viewport: GalleryViewport;
  dateGroupRefs: RefObject<Map<string, HTMLDivElement>>;
  lgOpenRef: RefObject<boolean>;
}

export interface UseDateScrubberResult {
  scrubberRef: RefObject<HTMLDivElement | null>;
  scrubberTop: number;
  scrubberHeight: number;
  isScrubbing: boolean;
  setIsScrubbing: (v: boolean) => void;
  scrubLabel: string;
  scrubY: number;
  activeDateLabel: string;
  activeIndicatorY: number;
  scrubberSegments: ScrubberSegment[];
  scrubToClientY: (clientY: number) => void;
  resetTailAnchor: () => void;
  SCRUBBER_PAD: number;
}

/**
 * Owns the immich-style proportional date scrubber: its dimensions, the
 * proportional segment bar, the scroll-driven active-position indicator (with
 * the bottom "tail" sweep), the ?at= URL position persistence, and the
 * drag-to-scrub interaction. Extracted verbatim from GalleryPage; all state
 * shared with the lightbox-exit and URL-restore paths (scrollElementUnderHeader,
 * pendingScrollRef, headerHeight, displayCount, the gallery/date-group refs, and
 * the lightbox/restore guards) is passed in.
 */
export function useDateScrubber({
  filteredAssets,
  groups,
  headerHeight,
  viewport,
  dateGroupRefs,
  lgOpenRef,
}: UseDateScrubberArgs): UseDateScrubberResult {
  const {
    displayCount,
    setDisplayCount,
    galleryContainerRef,
    didRestoreScrollRef,
    pendingScrollRef,
    scrollElementUnderHeader,
  } = viewport;
  const scrubberRef = useRef<HTMLDivElement | null>(null);
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

  // A tail anchor captured under the old filter is meaningless for the new
  // (differently sized) list; the filter-reset effect in GalleryPage calls this
  // so the indicator re-derives cleanly.
  const resetTailAnchor = () => { tailAnchorRef.current = null; };

  // Derive the scrubber's box from the measured header height (published by the
  // page's header-measurement effect) plus the viewport height. Kept separate
  // from that effect so the scrubber owns its own dimensions; it re-measures on
  // window resize since scrubberHeight depends on window.innerHeight.
  useEffect(() => {
    const measure = () => {
      setScrubberTop(headerHeight);
      setScrubberHeight(Math.max(0, window.innerHeight - headerHeight - 8));
    };
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [headerHeight]);

  // Proportional segments (immich-style): each date group gets a pixel height
  // proportional to its asset share, so the whole album maps onto the bar and
  // the first/last dates are always pinned to the very top/bottom. Dots and
  // labels are decimated by pixel distance so they never crowd or overflow.
  const scrubberSegments = useMemo(() => {
    const totalAssets = filteredAssets.length;
    const usable = scrubberHeight - SCRUBBER_PAD * 2;
    if (groups.length === 0 || totalAssets === 0 || usable <= 0) {
      return [] as ScrubberSegment[];
    }
    const MIN_DOT_DISTANCE = 8;
    const MIN_LABEL_DISTANCE = 28;
    const out: ScrubberSegment[] = [];
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
        const el = dateGroupRefs.current?.get(groups[i].label);
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
        const nextEl = dateGroupRefs.current?.get(groups[activeIdx + 1].label);
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
  }, [groups, headerHeight, scrubberSegments, scrubberHeight, displayCount, filteredAssets.length]); // eslint-disable-line react-hooks/exhaustive-deps

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

  return {
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
  };
}
