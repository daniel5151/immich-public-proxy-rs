import { useState, useMemo } from 'react';
import type { SafeAsset } from '../types/generated/SafeAsset';

/**
 * Uploader-filter state derived from the full asset list: per-uploader counts,
 * the distinct sorted names, and the filtered asset view. `enabledUploaders`
 * is `null` until the user first narrows the set (null = show everything),
 * which is deliberately never persisted in the URL.
 */
export function useUploaderFilter(assets: SafeAsset[]) {
  const [enabledUploaders, setEnabledUploaders] = useState<Set<string> | null>(null);

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

  const toggleUploader = (name: string) => {
    setEnabledUploaders((prev) => {
      const next = new Set(prev ?? distinctUploaders);
      if (next.has(name)) {
        next.delete(name);
      } else {
        next.add(name);
      }
      return next;
    });
  };

  return {
    enabledUploaders,
    setEnabledUploaders,
    uploaderCounts,
    distinctUploaders,
    hasMultipleUploaders,
    isFilterActive,
    filteredAssets,
    toggleUploader,
  };
}
