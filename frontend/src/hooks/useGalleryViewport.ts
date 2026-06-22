import { useState, useEffect, useRef, useCallback } from 'react';
import type { RefObject } from 'react';
import type { SafeAsset } from '../types/generated/SafeAsset';
import { HEADER_GAP } from '../lib/layout';

// Initial lazy-load window when no deep link reaches further.
const DEFAULT_WINDOW = 40;

interface UseGalleryViewportArgs {
  // Full asset list, used only by the one-time URL-aware displayCount
  // initializer (which resolves a #lg slide / ?at id against the album as it
  // was shared, before any uploader filter is applied — at mount the two are
  // identical anyway).
  assets: SafeAsset[];
  // The active (possibly uploader-filtered) list everything else keys off.
  filteredAssets: SafeAsset[];
  headerHeight: number;
  // Lazy-load growth pauses while an upload is in flight. Passed as a ref so the
  // observer reads the live value at intersection time without re-arming on
  // every upload-state change, and so this hook can be constructed before the
  // upload hook (whose onAssetReady sink needs our setDisplayCount).
  isUploadingRef: RefObject<boolean>;
}

export interface GalleryViewport {
  displayCount: number;
  setDisplayCount: (updater: number | ((current: number) => number)) => void;
  // Reset the lazy-load window to the default (used when the uploader filter
  // changes and the old window size is meaningless for the new list).
  resetWindow: () => void;
  galleryContainerRef: RefObject<HTMLDivElement | null>;
  observerRef: RefObject<HTMLDivElement | null>;
  pendingScrollRef: RefObject<(() => HTMLElement | null | undefined) | null>;
  didRestoreScrollRef: RefObject<boolean>;
  scrollElementUnderHeader: (el: HTMLElement, smooth?: boolean) => void;
  scrollToAssetId: (assetId: string) => void;
}

/**
 * Owns the gallery viewport: the lazy-load render window (`displayCount`) and
 * the scroll-position machinery layered on top of it — the shared scroll
 * primitive, the pending-far-jump resolver, the ?at= scroll restore, the
 * lazy-load IntersectionObserver, and the lightbox-exit grid jump. These were
 * previously smeared across GalleryPage and threaded piecemeal into the
 * scrubber and lightbox; collecting them here gives the render-window + scroll
 * domain a single owner, and lets consumers depend on one `viewport` object
 * instead of a bag of raw refs and setters.
 *
 * All effect bodies and dependency arrays are preserved verbatim from the
 * original GalleryPage implementation — the comments encode real, hard-won bug
 * fixes (lazy-load observer re-arm, restore-once guard, far-jump resolver).
 */
export function useGalleryViewport({
  assets,
  filteredAssets,
  headerHeight,
  isUploadingRef,
}: UseGalleryViewportArgs): GalleryViewport {
  const [displayCount, setDisplayCount] = useState<number>(() => {
    // Render enough items up-front to satisfy both a deep-linked lightbox slide
    // (#lg=1&slide=N) and a restored scroll position (?at=N), taking whichever
    // reaches further into the album. Both are validated; out-of-range values
    // are ignored (and a bad slide hash is cleared so lightGallery can't crash).
    const defaultInitial = Math.min(DEFAULT_WINDOW, assets.length);
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

  const galleryContainerRef = useRef<HTMLDivElement>(null);
  const observerRef = useRef<HTMLDivElement>(null);
  // Holds a resolver for a scroll target that isn't in the DOM yet: after we
  // grow the lazy-load window, the effect below runs it each render until it
  // returns the element, then scrolls to it once and clears. Shared by the
  // date-group jump and the lightbox-exit asset jump.
  const pendingScrollRef = useRef<(() => HTMLElement | null | undefined) | null>(null);
  const didRestoreScrollRef = useRef(false);

  const resetWindow = useCallback(() => {
    setDisplayCount(Math.min(DEFAULT_WINDOW, filteredAssets.length));
  }, [filteredAssets.length]);

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
  const scrollToAssetId = (assetId: string) => {
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

  // Lazy load intersection observer — operates on filteredAssets
  useEffect(() => {
    const observerTarget = observerRef.current;
    if (!observerTarget) return;

    const observer = new IntersectionObserver((entries) => {
      const entry = entries[0];
      if (entry.isIntersecting && !isUploadingRef.current) {
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
  }, [filteredAssets.length, displayCount]);

  return {
    displayCount,
    setDisplayCount,
    resetWindow,
    galleryContainerRef,
    observerRef,
    pendingScrollRef,
    didRestoreScrollRef,
    scrollElementUnderHeader,
    scrollToAssetId,
  };
}
