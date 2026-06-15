import { useState, useEffect, useRef, useMemo } from 'react';
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
    <div className="container" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100vh', background: '#262626', margin: 0 }}>
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
    <main className="container" style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100vh', flexDirection: 'column' }}>
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
    // Check if there is a hash pointing to a slide index on load, and make sure it's valid
    const defaultInitial = Math.min(40, assets.length);
    try {
      const hash = window.location.hash;
      if (hash) {
        const params = new URLSearchParams(hash.substring(1));
        const slideStr = params.get('slide');
        if (slideStr) {
          const slideIndex = parseInt(slideStr, 10);
          if (!isNaN(slideIndex) && slideIndex >= 0) {
            if (slideIndex < assets.length) {
              return Math.min(Math.max(defaultInitial, slideIndex + 1), assets.length);
            } else {
              // The requested slide is out of bounds. Clear the hash to prevent lightGallery from crashing.
              window.location.hash = '';
            }
          }
        }
      }
    } catch {
      // Ignore parsing errors
    }
    return defaultInitial;
  });
  const [isUploading, setIsUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState({ completed: 0, total: 0 });
  const [uploadStatus, setUploadStatus] = useState<{ type: 'success' | 'failed'; message?: string } | null>(null);

  // Upload name prompt modal (kept separate from settings for the upload flow)
  const [showNameModal, setShowNameModal] = useState(false);
  const [uploaderName, setUploaderName] = useState(() => localStorage.getItem('uploader_name') || '');

  const fileInputRef = useRef<HTMLInputElement>(null);
  const lgRef = useRef<LightGallery | null>(null);
  const galleryContainerRef = useRef<HTMLDivElement>(null);
  const observerRef = useRef<HTMLDivElement>(null);

  // Reset displayCount when filter changes
  useEffect(() => {
    setDisplayCount(Math.min(40, filteredAssets.length));
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

  // lightGallery initialization / update — uses filteredAssets
  useEffect(() => {
    if (filteredAssets.length === 0) return;

    const el = galleryContainerRef.current;
    if (!el) return;

    if (lgRef.current) {
      lgRef.current.destroy();
      lgRef.current = null;
    }

    const itemsArray = filteredAssets.slice(0, displayCount).map((asset) => {
      const previewUrl = `/share/photo/${realKey}/${asset.id}/preview`;
      const thumbnailUrl = `/share/photo/${realKey}/${asset.id}/thumbnail`;

      if (asset.type === 'VIDEO') {
        const item = {
          video: {
            source: [
              {
                src: `/share/video/${realKey}/${asset.id}`,
                type: asset.originalMimeType || 'video/mp4'
              }
            ],
            attributes: {
              playsinline: true,
              controls: true
            }
          },
          poster: previewUrl,
          thumb: thumbnailUrl,
          downloadUrl: asset.downloadUrl
        };
        return item as unknown as GalleryItem;
      } else {
        const item = {
          src: previewUrl,
          thumb: thumbnailUrl,
          downloadUrl: asset.downloadUrl
        };
        return item as GalleryItem;
      }
    });

    const lgConfig = {
      plugins: [
        lgZoom,
        lgThumbnail,
        lgVideo,
        lgFullscreen,
        lgHash
      ],
      speed: 500,
      licenseKey: '8FFA6495-676C4D30-8BFC54B6-4D0A6CEC',
      selector: '.gallery-item',
      download: true,
      counter: false,
      hideScrollbar: true,
      preload: 2,
      mobileSettings: { controls: false, showCloseIcon: true, download: true },
      dynamic: true,
      dynamicEl: itemsArray
    };

    lgRef.current = lightGallery(el, lgConfig);

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
      if (lgRef.current) {
        lgRef.current.destroy();
        lgRef.current = null;
      }
    };
  }, [displayCount, filteredAssets, realKey]);

  // Toggles and selection behaviors
  const onToggleAsset = (id: string) => {
    setSelectedAssets((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
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
  };

  const onConfirmName = (e: React.FormEvent) => {
    e.preventDefault();
    const name = uploaderName.trim();
    if (name) {
      localStorage.setItem('uploader_name', name);
      setShowNameModal(false);
      // Small timeout to allow state to settle before file dialog
      setTimeout(() => {
        fileInputRef.current?.click();
      }, 50);
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

  const onFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const filesList = e.target.files;
    if (!filesList || filesList.length === 0) return;

    const files = Array.from(filesList);
    // Reset file input value so same selection triggers event next time
    e.target.value = '';

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

  return (
    <div id="gallery-root">
      <title>{title}</title>
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
      <div id="header">
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
              <a href={`/share/${realKey}/download`} title="Download all" className="header-btn">
                <img src="/images/align-bottom-svgrepo-com.svg" alt="" className="header-icon" />
                <span>Download all</span>
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

      {/* LightGallery Grid container */}
      <div id="lightgallery" ref={galleryContainerRef}>
        {groups.map((group) => {
          const groupStartIndex = group.items[0].globalIndex;
          if (displayCount <= groupStartIndex) return null;

          const isGroupSelected = getGroupSelectionStatus(group.items);
          const visibleGroupItems = group.items.filter((item) => item.globalIndex < displayCount);

          return (
            <div key={group.label} className="gallery-date-group">
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
                      style={{ flexBasis, flexGrow: aspect }}
                    >
                      <div
                        className="tile-selector"
                        onClick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          onToggleAsset(asset.id);
                        }}
                      >
                        <span className="check-icon"></span>
                      </div>
                      <a
                        className="gallery-item"
                        data-index={globalIndex}
                        href={`/share/photo/${realKey}/${asset.id}/preview`}
                        onClick={(e) => e.preventDefault()} // Let lightGallery click handler take over
                      >
                        <img
                          loading="lazy"
                          src={`/share/photo/${realKey}/${asset.id}/thumbnail`}
                          alt=""
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
