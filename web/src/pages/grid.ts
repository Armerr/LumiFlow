import { api } from '../shared/api'
import { router } from '../shared/router'
import type { Page } from '../shared/router'
import type { AlbumDetail } from '../shared/types'
import { GridScene } from './grid/GridScene'
import './grid.scss'

interface GridPageParams { name: string }
export async function createGridPage({ name }: GridPageParams): Promise<Page> {
  let scene: GridScene | null = null


  return {
    async mount(container: HTMLElement) {
      let album: AlbumDetail
      try {
        album = await api.album(name)
      } catch {
        container.innerHTML = '<div class="error-state"><p>无法加载相册</p></div>'
        return
      }

      if (album.photos.length === 0) {
        container.innerHTML = '<div class="empty-state"><p>此相册暂无照片</p></div>'
        return
      }

      container.innerHTML = `
        <button class="back-btn" id="grid-back">← 返回</button>
        <div class="grid-page">
          <div class="grid-header">
            <h2>${escapeHtml(album.name)}</h2>
            <span class="photo-count">${album.photos.length} 张照片</span>
          </div>
          <div class="js-grid"></div>
        </div>
      `

      document.getElementById('grid-back')?.addEventListener('click', () => {
        router.navigate({ page: 'fan' })
      })

      scene = new GridScene(container)
      scene.init(name, album.photos)
      scene.onPhotoClick = (idx: number) => {
        router.navigate({ page: 'detail', album: name, photoId: idx })
      }
    },

    unmount() {
      scene?.dispose()
    },
  }
}

function escapeHtml(s: string): string { return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;') }
