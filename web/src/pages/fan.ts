import { api } from '../shared/api'
import { router } from '../shared/router'
import type { Page } from '../shared/router'
import type { Album } from '../shared/types'
import { FanScene } from './fan/FanScene'
import './fan.scss'

export function albumNavigationIdentity(album: Album): string {
  return album.id ?? album.name
}

export async function createFanPage(): Promise<Page> {
  let scene: FanScene | null = null

  return {
    async mount(container: HTMLElement) {
      let data
      try {
        data = await api.albums()
        router.setAlbums(data)
      } catch {
        container.innerHTML = '<div class="error-state"><p>无法加载相册</p></div>'
        return
      }

      if (data.albums.length === 0) {
        container.innerHTML = '<div class="empty-state"><p>暂无相册</p></div>'
        return
      }

      container.innerHTML = ''
      scene = new FanScene(container)
      scene.onAlbumClick = (album: Album) => {
        router.navigate({ page: 'album', name: albumNavigationIdentity(album) })
      }
      scene.setAlbums(data.albums)
    },

    unmount() {
      scene?.dispose()
      scene = null
    },
  }
}
