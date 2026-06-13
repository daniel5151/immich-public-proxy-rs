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
  const assets = link.assets;
  const allowDownload = link.allowDownload ?? false;
  const allowUpload = link.allowUpload ?? false;

  const title = link.description || link.album?.albumName || (link.type === 'INDIVIDUAL' && assets.length === 1 ? (assets[0].originalFileName || 'Shared File') : 'Shared Files');
  const albumDescription = link.album?.description;

  const [selectedAssets, setSelectedAssets] = useState<Set<string>>(new Set());

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
  
  const [showNameModal, setShowNameModal] = useState(false);
  const [uploaderName, setUploaderName] = useState(() => localStorage.getItem('uploader_name') || '');

  const fileInputRef = useRef<HTMLInputElement>(null);
  const lgRef = useRef<LightGallery | null>(null);
  const galleryContainerRef = useRef<HTMLDivElement>(null);
  const observerRef = useRef<HTMLDivElement>(null);

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

  // Lazy load intersection observer
  useEffect(() => {
    const observerTarget = observerRef.current;
    if (!observerTarget) return;

    const observer = new IntersectionObserver((entries) => {
      const entry = entries[0];
      if (entry.isIntersecting && !isUploading) {
        setDisplayCount((current) => {
          if (current < assets.length) {
            return Math.min(current + 12, assets.length);
          }
          return current;
        });
      }
    }, {
      rootMargin: '200px 0px 200px 0px'
    });

    observer.observe(observerTarget);
    return () => observer.disconnect();
  }, [assets.length, isUploading]);

  // Group assets by date
  const groups = useMemo(() => {
    const groups: DateGroup[] = [];
    assets.forEach((asset, i) => {
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
  }, [assets]);

  // lightGallery initialization / update
  useEffect(() => {
    if (assets.length === 0) return;

    const el = galleryContainerRef.current;
    if (!el) return;

    if (lgRef.current) {
      lgRef.current.destroy();
      lgRef.current = null;
    }

    const itemsArray = assets.slice(0, displayCount).map((asset) => {
      const previewUrl = `/share/photo/${realKey}/${asset.id}/preview`;
      const thumbnailUrl = `/share/photo/${realKey}/${asset.id}/thumbnail`;

      if (asset.type === 'VIDEO') {
        const item = {
          video: {
            source: [
              {
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
  }, [displayCount, assets, realKey]);

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
    const name = uploaderName.trim();
    if (!name) {
      setShowNameModal(true);
    } else {
      fileInputRef.current?.click();
    }
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
      {(displayCount < assets.length || isUploading) && (
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
              <span>Uploading {uploadProgress.completed}/{uploadProgress.total}</span>
            </div>
          )}
          {!isUploading && uploadStatus?.type === 'success' && (
            <div className="toast-content success">
              <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
                <span>✅ Upload complete</span>
                <span style={{ fontSize: '0.8rem', opacity: 0.75 }}>
                  Reload the page to see your new photos.
                </span>
              </div>
            </div>
          )}
          {!isUploading && uploadStatus?.type === 'failed' && (
            <div className="toast-content failed">
              <span>❌ {uploadStatus.message}</span>
            </div>
          )}
        </div>
      )}

      {/* Uploader name modal dialog */}
      {showNameModal && (
        <div className="modal-overlay" onClick={() => setShowNameModal(false)}>
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
                <button type="button" className="header-btn" onClick={() => setShowNameModal(false)}>
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
    </div>
  );
}
