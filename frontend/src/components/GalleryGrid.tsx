import type { RefObject } from 'react';
import type { SafeAsset } from '../types/generated/SafeAsset';
import type { DateGroup } from '../types';

interface GalleryGridProps {
  containerRef: RefObject<HTMLDivElement | null>;
  groups: DateGroup[];
  displayCount: number;
  realKey: string;
  selectedAssets: Set<string>;
  loadedAssetsRef: RefObject<Set<string>>;
  dateGroupRefs: RefObject<Map<string, HTMLDivElement>>;
  getGroupSelectionStatus: (groupItems: { asset: SafeAsset }[]) => boolean;
  onToggleGroup: (groupItems: { asset: SafeAsset }[]) => void;
  onToggleAsset: (id: string, index?: number, shiftKey?: boolean) => void;
}

/**
 * The date-grouped thumbnail grid. Pure view over the visible asset window;
 * lazy-load growth is driven by `displayCount` from the parent. The container
 * ref is forwarded so the lightGallery effect can keep querying `#lightgallery`.
 */
export function GalleryGrid({
  containerRef,
  groups,
  displayCount,
  realKey,
  selectedAssets,
  loadedAssetsRef,
  dateGroupRefs,
  getGroupSelectionStatus,
  onToggleGroup,
  onToggleAsset,
}: GalleryGridProps) {
  return (
    <div id="lightgallery" ref={containerRef}>
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
                    className={`tile-wrapper ${isAssetSelected ? 'selected' : ''}`}
                    // Seed the 'loaded' class from a ref callback (post-render,
                    // so reading the ref is allowed) rather than during render.
                    // This keeps an already-loaded thumbnail from replaying the
                    // shimmer/fade-in when the tile re-renders (e.g. on selection
                    // or filter changes), while the <img> onLoad below is what
                    // first adds the class as each thumbnail finishes loading.
                    ref={(el) => {
                      if (el && loadedAssetsRef.current.has(asset.id)) el.classList.add('loaded');
                    }}
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
  );
}
