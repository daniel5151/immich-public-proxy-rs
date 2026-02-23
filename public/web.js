// How many thumbnails to load per "page" fetched from Immich
const PER_PAGE = 50

class LGallery {
  items
  lightGallery
  element
  index = PER_PAGE

  /**
   * Create a lightGallery instance and populate it with the first page of gallery items
   */
  init(params = {}) {
    this.element = document.getElementById('lightgallery')
    this.items = params.items || []

    // Ensure DOM has enough items loaded if accessed directly via a #slide hash link
    const slideMatch = window.location.hash.match(/slide=([^&]+)/)
    if (slideMatch) {
      const slideName = slideMatch[1]
      const targetIndex = this.items.findIndex(item => item.html.includes(`data-slide-name="${slideName}"`))
      if (targetIndex >= this.index) {
        const endIndex = targetIndex + Math.min(50, this.items.length - targetIndex)
        let newContent = ''
        this.items.slice(this.index, endIndex).forEach(item => {
          newContent += item.html + '\n'
        })
        this.element.insertAdjacentHTML('beforeend', newContent)
        this.index = endIndex
      }
    }

    // Create the lightGallery instance
    this.lightGallery = lightGallery(this.element, Object.assign({
      plugins: [lgZoom, lgThumbnail, lgVideo, lgFullscreen, lgHash],
      speed: 500,
      /*
      This license key was graciously provided by LightGallery under their
      GPLv3 open-source project license:
      */
      licenseKey: '8FFA6495-676C4D30-8BFC54B6-4D0A6CEC'
      /*
      Please do not take it and use it for other projects, as it was provided
      specifically for Immich Public Proxy.

      For your own projects you can use the default license key of
      0000-0000-000-0000 as per their docs:

      https://www.lightgalleryjs.com/docs/settings/#licenseKey
      */
    }, params.lgConfig))
    this.items = params.items

    const spinner = document.getElementById('loading-spinner')
    if (spinner) {
      const observer = new IntersectionObserver((entries) => {
        if (entries[0].isIntersecting) {
          lgallery.loadMoreItems(observer, spinner)
        }
      }, { rootMargin: '200px' })
      observer.observe(spinner)
    }
  }

  /**
   * Load more gallery items as per lightGallery docs
   * https://www.lightgalleryjs.com/demos/infinite-scrolling/
   */
  loadMoreItems(observer, spinner) {
    if (this.index < this.items.length) {
      // Append new thumbnails
      this.items
        .slice(this.index, this.index + PER_PAGE)
        .forEach(item => {
          this.element.insertAdjacentHTML('beforeend', item.html + '\n')
        })
      this.index += PER_PAGE
      this.lightGallery.refresh()
    } else {
      // Remove the loading spinner and stop observing once all items are loaded
      observer.disconnect()
      spinner.remove()
    }
  }
}
window.lgallery = new LGallery()

// Auto-initialize when data is ready
const initInterval = setInterval(() => {
  if (window.GALLERY_DATA && window.lightGallery) {
    clearInterval(initInterval)
    window.lgallery.init(window.GALLERY_DATA)
  }
}, 50)
