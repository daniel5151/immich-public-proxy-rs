import { useState, useEffect, useRef } from 'react';
import type { ShareDetails } from './types/generated/ShareDetails';
import type { SafeAsset } from './types/generated/SafeAsset';

export default function App() {
  const [shareKey, setShareKey] = useState<string>('');
  const [details, setDetails] = useState<ShareDetails | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  // Router logic
  useEffect(() => {
    const pathname = window.location.pathname;
    const parts = pathname.split('/').filter(Boolean);
    if ((parts[0] === 'share' || parts[0] === 's') && parts[1]) {
      setShareKey(parts[1]);
    } else {
      setLoading(false);
    }
  }, []);

  const fetchDetails = async (key: string, password?: string) => {
    try {
      setLoading(true);
      let url = `/api/share/${key}`;
      if (password) {
        url += `?password=${encodeURIComponent(password)}`;
      }
      const res = await fetch(url);
      if (!res.ok) {
        const errData = await res.json().catch(() => ({}));
        throw new Error(errData.error || `Failed to fetch share details (${res.status})`);
      }
      const data: ShareDetails = await res.json();
      setDetails(data);
      setError(null);
    } catch (e: any) {
      setError(e.message || 'Failed to load share details');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (shareKey) {
      fetchDetails(shareKey);
    }
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
    return <GalleryPage details={details} shareKey={shareKey} />;
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
  const [displayCount, setDisplayCount] = useState<number>(Math.min(40, assets.length));
  const [isUploading, setIsUploading] = useState(false);
  const [uploadProgress, setUploadProgress] = useState({ completed: 0, total: 0 });
  const [uploadStatus, setUploadStatus] = useState<{ type: 'success' | 'failed'; message?: string } | null>(null);
  
  const [showNameModal, setShowNameModal] = useState(false);
  const [uploaderName, setUploaderName] = useState(() => localStorage.getItem('uploader_name') || '');

  const fileInputRef = useRef<HTMLInputElement>(null);
  const lgRef = useRef<any>(null);

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
    const observerTarget = document.getElementById('loading-observer');
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
      rootMargin: '1000px 0px 1000px 0px'
    });

    observer.observe(observerTarget);
    return () => observer.disconnect();
  }, [assets.length, isUploading]);

  // Group assets by date
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

  // lightGallery initialization / update
  const initLg = () => {
    const el = document.getElementById('lightgallery');
    if (!el || !(window as any).lightGallery) return;

    if (lgRef.current) {
      lgRef.current.destroy();
      lgRef.current = null;
    }

    const itemsArray = assets.map((asset) => {
      const previewUrl = `/share/photo/${realKey}/${asset.id}/preview`;
      const thumbnailUrl = `/share/photo/${realKey}/${asset.id}/thumbnail`;

      if (asset.type === 'VIDEO') {
        const item: any = {
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
          thumb: thumbnailUrl
        };
        if (asset.downloadUrl) {
          item.downloadUrl = asset.downloadUrl;
        }
        return item;
      } else {
        const item: any = {
          src: previewUrl,
          thumb: thumbnailUrl
        };
        if (asset.downloadUrl) {
          item.downloadUrl = asset.downloadUrl;
        }
        return item;
      }
    });

    const lgConfig = {
      plugins: [
        (window as any).lgZoom,
        (window as any).lgThumbnail,
        (window as any).lgVideo,
        (window as any).lgFullscreen,
        (window as any).lgHash
      ].filter(Boolean),
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

    lgRef.current = (window as any).lightGallery(el, lgConfig);

    const handleClick = (e: MouseEvent) => {
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

    el.removeEventListener('click', handleClick as any);
    el.addEventListener('click', handleClick as any);

    return () => {
      el.removeEventListener('click', handleClick as any);
      if (lgRef.current) {
        lgRef.current.destroy();
        lgRef.current = null;
      }
    };
  };

  useEffect(() => {
    if (assets.length > 0) {
      const cleanup = initLg();
      return () => {
        cleanup?.();
      };
    }
  }, [displayCount]);

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
      } catch (e) {
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
      <div id="lightgallery">
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

      <div id="loading-observer" style={{ height: '1px', width: '100%' }}></div>

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
