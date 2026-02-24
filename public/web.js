class LGallery {
  lightGallery
  element

  init(params = {}) {
    this.element = document.getElementById('lightgallery')
    if (!this.element) return;

    // Create the lightGallery instance using the existing DOM elements
    this.lightGallery = lightGallery(this.element, Object.assign({
      plugins: [lgZoom, lgThumbnail, lgVideo, lgFullscreen, lgHash],
      speed: 500,
      licenseKey: '8FFA6495-676C4D30-8BFC54B6-4D0A6CEC',
      selector: 'a:not(.tile-selector)', // Important: don't trigger on checkboxes
    }, params.lgConfig))
  }
}

window.lgallery = new LGallery()

// The Leptos component will call window.lgallery.init() when ready
