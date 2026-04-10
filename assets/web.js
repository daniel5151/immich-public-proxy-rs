class LGallery {
  lightGallery
  element

  init(params = {}) {
    this.element = document.getElementById('lightgallery')
    if (!this.element) return;

    if (this.lightGallery) {
      this.lightGallery.destroy();
    }

    const lgConfig = Object.assign({
      plugins: [lgZoom, lgThumbnail, lgVideo, lgFullscreen, lgHash],
      speed: 500,
      // This license key was graciously provided by LightGallery under their
      // GPLv3 open-source project license:
      licenseKey: '8FFA6495-676C4D30-8BFC54B6-4D0A6CEC',
      selector: '.gallery-item',
      download: true,
      counter: false,
      hideScrollbar: true,
      preload: 2,
      mobileSettings: { controls: false, showCloseIcon: true, download: true },
    }, params.lgConfig);

    if (params.items && params.items.length > 0) {
      // Dynamic mode: uses the items array instead of the DOM
      lgConfig.dynamic = true;
      lgConfig.dynamicEl = params.items;

      const initAndOpen = (index) => {
        if (!this.lightGallery) {
          this.lightGallery = lightGallery(this.element, lgConfig);
        }
        if (typeof index === 'number') {
          this.lightGallery.openGallery(index);
        }
      };

      // Handle clicks on gallery items manually in dynamic mode
      const handleClick = (e) => {
        const target = e.target.closest('.gallery-item');
        if (target && this.element.contains(target)) {
          const indexAttr = target.getAttribute('data-index') || target.getAttribute('attr:data-index');
          const index = parseInt(indexAttr);
          if (!isNaN(index)) {
            e.preventDefault();
            e.stopPropagation();
            initAndOpen(index);
          }
        }
      };

      // Remove old listener if re-initializing
      this.element.removeEventListener('click', this._clickHandler);
      this._clickHandler = handleClick;
      this.element.addEventListener('click', this._clickHandler);

      // If URL has a hash, we must initialize it immediately so lgHash can parse it and open
      if (window.location.hash.indexOf('lg=1') !== -1) {
        initAndOpen();
      }

    } else {
      // Selector mode: uses the DOM attributes
      this.lightGallery = lightGallery(this.element, lgConfig);
    }
  }
}

// Delegated error handler for thumbnail images (replaces inline onerror="" for CSP compliance).
// Uses capture phase because 'error' events on <img> do not bubble.
document.addEventListener('error', function (e) {
  if (e.target.tagName === 'IMG') {
    var link = e.target.closest('.gallery-item');
    if (link) link.classList.add('thumb-error');
  }
}, true);

window.lgallery = new LGallery();

// Expose an explicit init function for Leptos to call
window.initLightGallery = function () {
  var el = document.getElementById('lightgallery');
  if (!el || !window.lgallery) return;

  var raw = el.getAttribute('data-gallery-items');
  if (!raw) return;

  try {
    var items = JSON.parse(raw);
    window.lgallery.init({ lgConfig: {}, items: items });
  } catch (e) {
    console.error('Failed to parse gallery items:', e);
  }
};

// Expose a clean-up function to prevent memory leaks on navigation
window.destroyLightGallery = function () {
  if (window.lgallery && window.lgallery.lightGallery) {
    window.lgallery.lightGallery.destroy();
    window.lgallery.lightGallery = null;
  }
};
