import { api } from '../../shared/api'
import { router } from '../../shared/router'
import type { AlbumDetail, Photo } from '../../shared/types'
import {
  getPageOffsets,
  getPhotoPosition,
  getVerticalGridMetrics,
  getVisiblePhotoRange,
  PHOTO_PAGE_SIZE,
  type VerticalGridMetrics,
} from './gridLayout'

export class VerticalPhotoGrid {
  private readonly pages = new Map<number, Photo[]>()
  private readonly rendered = new Map<number, HTMLElement>()
  private readonly loading = new Set<number>()
  private metrics: VerticalGridMetrics
  private frame: number | null = null
  private disposed = false
  private wantedPages = new Set<number>()

  constructor(
    private readonly scroller: HTMLElement,
    private readonly stage: HTMLElement,
    private readonly albumId: string,
    private photoCount: number,
    firstPage: Photo[],
  ) {
    this.pages.set(0, firstPage)
    this.metrics = getVerticalGridMetrics(this.stage.clientWidth, photoCount)
  }

  mount(): void {
    this.scroller.addEventListener('scroll', this.scheduleRender, { passive: true })
    window.addEventListener('resize', this.scheduleRender)
    this.render()
  }

  dispose(): void {
    this.disposed = true
    this.wantedPages.clear()
    this.scroller.removeEventListener('scroll', this.scheduleRender)
    window.removeEventListener('resize', this.scheduleRender)
    if (this.frame !== null) cancelAnimationFrame(this.frame)
    this.frame = null
    this.rendered.clear()
    this.pages.clear()
    this.loading.clear()
  }

  private scheduleRender = (): void => {
    if (this.frame !== null) return
    this.frame = requestAnimationFrame(() => {
      this.frame = null
      this.render()
    })
  }

  private render(): void {
    if (this.disposed) return

    this.metrics = getVerticalGridMetrics(this.stage.clientWidth, this.photoCount)
    this.stage.style.height = `${this.metrics.totalHeight}px`

    const stageTop = this.stage.offsetTop
    const scrollTop = Math.max(this.scroller.scrollTop - stageTop, 0)
    const range = getVisiblePhotoRange(
      scrollTop,
      this.scroller.clientHeight,
      this.metrics,
      this.photoCount,
    )
    const wantedPages = getPageOffsets(range, PHOTO_PAGE_SIZE)
    this.wantedPages = new Set(wantedPages)
    const wantedPageSet = this.wantedPages

    for (const offset of this.pages.keys()) {
      if (!wantedPageSet.has(offset)) this.pages.delete(offset)
    }

    for (const [index, element] of this.rendered) {
      if (index < range.start || index >= range.end) {
        element.remove()
        this.rendered.delete(index)
      }
    }

    for (let index = range.start; index < range.end; index += 1) {
      const photo = this.photoAt(index)
      const existing = this.rendered.get(index)
      if (!photo) {
        if (existing) {
          existing.remove()
          this.rendered.delete(index)
        }
        continue
      }

      const position = getPhotoPosition(index, this.metrics)
      const item = existing ?? this.createItem(photo, index)
      item.style.left = `${position.left}px`
      item.style.top = `${position.top}px`
      item.style.width = `${this.metrics.itemWidth}px`
      item.style.height = `${this.metrics.itemHeight}px`
      if (!existing) {
        this.stage.appendChild(item)
        this.rendered.set(index, item)
      }
    }

    const missingPages = wantedPages.filter((offset) => !this.pages.has(offset) && !this.loading.has(offset))
    if (missingPages.length > 0) void this.loadPages(missingPages)
  }

  private photoAt(index: number): Photo | undefined {
    const pageOffset = Math.floor(index / PHOTO_PAGE_SIZE) * PHOTO_PAGE_SIZE
    return this.pages.get(pageOffset)?.[index - pageOffset]
  }

  private createItem(photo: Photo, index: number): HTMLButtonElement {
    const button = document.createElement('button')
    button.className = 'vertical-grid-item'
    button.type = 'button'
    button.setAttribute('aria-label', photo.name)

    const image = document.createElement('img')
    image.src = api.thumbUrl(this.albumId, photo)
    image.alt = photo.name
    image.decoding = 'async'
    image.draggable = false
    button.appendChild(image)
    button.addEventListener('click', () => {
      router.navigate({ page: 'detail', album: this.albumId, photoId: index })
    })
    return button
  }

  private async loadPages(offsets: number[]): Promise<void> {
    offsets.forEach((offset) => this.loading.add(offset))
    await Promise.all(offsets.map(async (offset) => {
      let shouldRender = false
      try {
        const album = await api.album(this.albumId, offset, PHOTO_PAGE_SIZE)
        if (!this.disposed && this.wantedPages.has(offset)) {
          this.photoCount = albumPhotoCount(album)
          this.pages.set(offset, album.photos)
          shouldRender = true
        }
      } catch {
        // A later scroll or resize can retry the missing page.
      } finally {
        this.loading.delete(offset)
      }
      if (shouldRender && !this.disposed && this.wantedPages.has(offset)) this.render()
    }))
  }
}

export function albumPhotoCount(album: Pick<AlbumDetail, 'photo_count' | 'photos'>): number {
  return album.photo_count ?? album.photos.length
}
