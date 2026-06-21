import { useState, useRef, useEffect } from 'react';
import type { SafeAsset } from '../types/generated/SafeAsset';

/**
 * Multi-select state for gallery tiles: single toggle, shift-click range
 * select (anchored on the last-clicked index), whole-date-group toggle, and
 * the download URL for the current selection. Also keeps the body
 * `selection-mode` class in sync so the CSS can react to selection.
 */
export function useSelection(filteredAssets: SafeAsset[], realKey: string) {
  const [selectedAssets, setSelectedAssets] = useState<Set<string>>(new Set());
  const lastSelectedIndexRef = useRef<number | null>(null);

  // Sync selection state with body class
  useEffect(() => {
    if (selectedAssets.size > 0) {
      document.body.classList.add('selection-mode');
    } else {
      document.body.classList.remove('selection-mode');
    }
    return () => document.body.classList.remove('selection-mode');
  }, [selectedAssets.size]);

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

  return {
    selectedAssets,
    setSelectedAssets,
    onToggleAsset,
    getGroupSelectionStatus,
    onToggleGroup,
    downloadSelectionUrl,
  };
}
