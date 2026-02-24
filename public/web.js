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
      licenseKey: '8FFA6495-676C4D30-8BFC54B6-4D0A6CEC',
      selector: '.gallery-item',
      download: true,
      counter: false,
      hideScrollbar: true,
      mobileSettings: { controls: false, showCloseIcon: true, download: true },
    }, params.lgConfig);

    if (params.items && params.items.length > 0) {
      // Dynamic mode: uses the items array instead of the DOM
      lgConfig.dynamic = true;
      lgConfig.dynamicEl = params.items;

      this.lightGallery = lightGallery(this.element, lgConfig);

      // Handle clicks on gallery items manually in dynamic mode
      const handleClick = (e) => {
        const target = e.target.closest('.gallery-item');
        if (target && this.element.contains(target)) {
          const indexAttr = target.getAttribute('data-index') || target.getAttribute('attr:data-index');
          const index = parseInt(indexAttr);
          if (!isNaN(index)) {
            e.preventDefault();
            e.stopPropagation();
            this.lightGallery.openGallery(index);
          }
        }
      };

      // Remove old listener if re-initializing
      this.element.removeEventListener('click', this._clickHandler);
      this._clickHandler = handleClick;
      this.element.addEventListener('click', this._clickHandler);

    } else {
      // Selector mode: uses the DOM attributes
      this.lightGallery = lightGallery(this.element, lgConfig);
    }
  }
}

window.lgallery = new LGallery()
