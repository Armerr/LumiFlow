import type { AlbumsResponse } from './types'

export interface Page {
  mount(container: HTMLElement): void | Promise<void>
  unmount(): void
}

export type Route =
  | { page: 'fan' }
  | { page: 'album'; name: string }
  | { page: 'detail'; album: string; photoId: number }
let currentPage: Page | null = null
let cachedAlbums: AlbumsResponse | null = null

/** Minimal SPA router using History API. */
export const router = {
  get albums() {
    return cachedAlbums
  },

  setAlbums(a: AlbumsResponse) {
    cachedAlbums = a
  },

  /** Derive the current route from the URL. */
  parse(): Route {
    const path = window.location.pathname

    const albumMatch = path.match(/^\/album\/(.+)\/photo\/(\d+)$/)
    if (albumMatch) {
      return {
        page: 'detail',
        album: decodeURIComponent(albumMatch[1]),
        photoId: parseInt(albumMatch[2], 10),
      }
    }

    const gridMatch = path.match(/^\/album\/(.+)$/)
    if (gridMatch) {
      return { page: 'album', name: decodeURIComponent(gridMatch[1]) }
    }

    return { page: 'fan' }
  },

  /** Navigate to a route. */
  navigate(route: Route) {
    let url: string
    switch (route.page) {
      case 'fan':
        url = '/'
        break
      case 'album':
        url = `/album/${encodeURIComponent(route.name)}`
        break
      case 'detail':
        url = `/album/${encodeURIComponent(route.album)}/photo/${route.photoId}`
        break
    }
    history.pushState(null, '', url)
    this._dispatch()
  },

  /** Replace current route without adding history entry. */
  replace(route: Route) {
    let url: string
    switch (route.page) {
      case 'fan':
        url = '/'
        break
      case 'album':
        url = `/album/${encodeURIComponent(route.name)}`
        break
      case 'detail':
        url = `/album/${encodeURIComponent(route.album)}/photo/${route.photoId}`
        break
    }
    history.replaceState(null, '', url)
    this._dispatch()
  },

  /** Start listening to popstate and initial route. */
  async start(container: HTMLElement, pageLoader: (route: Route) => Promise<Page>) {
    const onRoute = async () => {
      const route = this.parse()

      if (currentPage) {
        currentPage.unmount()
        currentPage = null
      }

      container.innerHTML = ''
      currentPage = await pageLoader(route)
      await currentPage.mount(container)
    }

    window.addEventListener('popstate', onRoute)
    await onRoute()
  },

  /** Stop listening. */
  stop() {
    if (currentPage) {
      currentPage.unmount()
      currentPage = null
    }
  },

  _dispatch() {
    window.dispatchEvent(new PopStateEvent('popstate'))
  },
}
