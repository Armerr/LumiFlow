import { api } from '../shared/api'
import { router } from '../shared/router'
import type { Page } from '../shared/router'
import type { AlbumDetail } from '../shared/types'
import { albumPhotoCount, VerticalPhotoGrid } from './grid/VerticalPhotoGrid'
import './grid.scss'

interface GridPageParams { name: string }
export async function createGridPage({ name }: GridPageParams): Promise<Page> {
  let grid: VerticalPhotoGrid | null = null

  return {
    async mount(container: HTMLElement) {
      let album: AlbumDetail
      try {
        album = await api.album(name)
      } catch {
        container.innerHTML = '<div class="error-state"><p>无法加载相册</p></div>'
        return
      }

      const photoCount = albumPhotoCount(album)
      if (photoCount === 0) {
        container.innerHTML = '<div class="empty-state"><p>此相册暂无照片</p></div>'
        return
      }

      container.innerHTML = `
        <button class="back-btn" id="grid-back">← 返回</button>
        <main class="grid-page">
          ${renderGridHeader(album, photoCount)}
          <div class="vertical-grid-stage" aria-label="相册照片"></div>
        </main>
      `

      document.getElementById('grid-back')?.addEventListener('click', () => {
        router.navigate({ page: 'fan' })
      })

      const scroller = container.querySelector<HTMLElement>('.grid-page')
      const stage = container.querySelector<HTMLElement>('.vertical-grid-stage')
      if (!scroller || !stage) return
      grid = new VerticalPhotoGrid(scroller, stage, name, photoCount, album.photos)
      grid.mount()
    },

    unmount() {
      grid?.dispose()
      grid = null
    },
  }
}

export function renderGridHeader(album: Pick<AlbumDetail, 'name' | 'description'>, photoCount: number): string {
  const description = album.description
    ? `<p class="album-description">${escapeHtml(album.description)}</p>`
    : ''

  return `<div class="grid-header">
    <h2>${escapeHtml(album.name)}</h2>
    ${description}
    <span class="photo-count">${photoCount} 张照片</span>
  </div>`
}

function escapeHtml(s: string): string { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;') }
