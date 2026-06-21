import { useEffect } from 'react';
import type { RefObject } from 'react';
import lightGallery from 'lightgallery';
import lgZoom from 'lightgallery/plugins/zoom';
import lgThumbnail from 'lightgallery/plugins/thumbnail';
import lgVideo from 'lightgallery/plugins/video';
import lgFullscreen from 'lightgallery/plugins/fullscreen';
import lgHash from 'lightgallery/plugins/hash';
import type { GalleryItem } from 'lightgallery/lg-utils';
import type { SafeAsset } from '../types/generated/SafeAsset';

type LightGallery = ReturnType<typeof lightGallery>;

interface UseLightGalleryArgs {
  filteredAssets: SafeAsset[];
  realKey: string;
  galleryContainerRef: RefObject<HTMLDivElement | null>;
  lgRef: RefObject<LightGallery | null>;
  lgOpenRef: RefObject<boolean>;
  currentSlideIdRef: RefObject<string | null>;
  scrollGridToAssetId: (assetId: string) => void;
}

/**
 * Owns the lightGallery instance: (re)builds it from the full filtered asset
 * list, wires lazy filmstrip-thumbnail loading, the contributor label in the
 * toolbar, and the slideshow<->grid position sync. Extracted verbatim from
 * GalleryPage; all shared state is passed in as refs/values.
 */
export function useLightGallery({
  filteredAssets,
  realKey,
  galleryContainerRef,
  lgRef,
  lgOpenRef,
  currentSlideIdRef,
  scrollGridToAssetId,
}: UseLightGalleryArgs) {
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
    const updateContributorInToolbar = (idx: number) => {
      const counter = document.querySelector('.lg-counter');
      if (!counter) return;

      let contribSpan = counter.querySelector('.lg-counter-contributor') as HTMLSpanElement | null;
      if (!contribSpan) {
        contribSpan = document.createElement('span');
        contribSpan.className = 'lg-counter-contributor';
        counter.appendChild(contribSpan);
      }

      const asset = filteredAssets[idx];
      if (asset && asset.uploaderName) {
        contribSpan.textContent = ` • ${asset.uploaderName}`;
        contribSpan.style.display = 'inline';
      } else {
        contribSpan.style.display = 'none';
      }
    };
    const onLgAfterOpen = () => {
      lgOpenRef.current = true;
      // The filmstrip DOM is now visible; start lazily filling its thumbnails.
      // Retry on the next frame in case the strip isn't queryable yet this tick.
      ensureThumbObserver();
      requestAnimationFrame(() => {
        ensureThumbObserver();
        if (lgRef.current) {
          const idx = lgRef.current.index ?? 0;
          loadThumbsAround(idx);
          updateContributorInToolbar(idx);
        }
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
        updateContributorInToolbar(idx);
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
  // lightbox (which also caused the counter to jump). scrollGridToAssetId is
  // likewise omitted on purpose: it's re-created each render and only invoked
  // from the lightbox close callback, not needed as a re-init trigger.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filteredAssets, realKey]);
}
