import { api } from '../shared/api'
import { router } from '../shared/router'
import type { Page } from '../shared/router'
import type { Album, ScanStatus } from '../shared/types'
import { FanScene } from './fan/FanScene'
import { albumFilterQuery, type AlbumFilter } from '../shared/albumPresentation'
import { mountFilterBar } from './fan/FilterBar'
import './fan.scss'

function renderFilteredEmpty(): string {
  return '<div class="empty-state"><p>当前筛选条件下暂无相册</p></div>'
}

export function albumNavigationIdentity(album: Album): string {
  return album.id ?? album.name
}

export function createFanPage(initialFilter: AlbumFilter = {}): Page {
  let scene: FanScene | null = null
  let timer: number | null = null
  let disposed = false
  let tearDownFilter: (() => void) | null = null
  let currentFilter = { ...initialFilter }

  const clearTimer = () => {
    clearTimeout(timer ?? undefined)
    timer = null
  }

  const loadAlbums = async (container: HTMLElement): Promise<void> => {
    if (disposed) return

    const query = albumFilterQuery(currentFilter)
    let data
    try {
      data = await api.albums(query)
      router.setAlbums(data)
    } catch {
      container.innerHTML = '<div class="error-state"><p>无法加载相册</p></div>'
      return
    }
    if (disposed) return

    scene?.dispose()
    scene = null

    if (data.albums.length === 0) {
      container.innerHTML = renderFilteredEmpty()
      return
    }

    container.innerHTML = '<div class="fan-filter-bar" id="fan-filter-bar"></div>'
    scene = new FanScene(container)
    scene.onAlbumClick = (album: Album) => {
      router.navigate({ page: 'album', name: albumNavigationIdentity(album) })
    }
    scene.setAlbums(data.albums)

    const filterRoot = container.querySelector<HTMLElement>('#fan-filter-bar')
    if (filterRoot) {
      tearDownFilter?.()
      tearDownFilter = mountFilterBar(filterRoot, currentFilter, {
        onChange: (next) => {
          currentFilter = next
          void loadAlbums(container)
        },
      })
    }
  }

  return {
    async mount(container: HTMLElement) {
      const load = async (): Promise<void> => {
        if (disposed) return
        let status: ScanStatus
        try {
          status = await api.status()
        } catch {
          container.innerHTML = '<div class="error-state"><p>无法读取索引状态</p></div>'
          return
        }

        if (disposed) return
        if (status.state === 'error') {
          container.innerHTML = renderScanStatus(status)
          return
        }
        if (status.state !== 'ready') {
          container.innerHTML = renderScanStatus(status)
          timer = setTimeout(() => { void load() }, 1000)
          return
        }

        await loadAlbums(container)
      }

      await load()
    },

    unmount() {
      disposed = true
      clearTimer()
      tearDownFilter?.()
      tearDownFilter = null
      scene?.dispose()
      scene = null
    },
  }
}

export function renderScanStatus(status: ScanStatus): string {
  if (status.state === 'error') {
    return `<main class="scan-status-page scan-status-error">
      <p class="scan-kicker">INDEX ERROR</p>
      <h1>照片索引失败</h1>
      <p class="scan-error">${escapeHtml(status.error || '未知错误')}</p>
      <p class="scan-hint">请检查容器日志、照片目录和数据目录权限，然后重启服务。</p>
    </main>`
  }

  const speed = status.elapsed_seconds > 0
    ? `${Math.round(status.processed / status.elapsed_seconds)} 张/秒`
    : '准备中'
  const phase = status.phase === 'building_albums' ? '正在生成相册' : '正在读取照片与 EXIF'
  return `<main class="scan-status-page">
    <div class="scan-orbit" aria-hidden="true"><span></span></div>
    <p class="scan-kicker">INITIAL LIBRARY INDEX</p>
    <h1>正在建立照片索引</h1>
    <p class="scan-phase">${phase}</p>
    <div class="scan-metrics">
      <div><strong>${status.found}</strong><span>已发现 ${status.found} 张</span></div>
      <div><strong>${status.processed}</strong><span>已处理 ${status.processed} 张</span></div>
      <div><strong>${status.workers}</strong><span>并发线程</span></div>
    </div>
    <p class="scan-meta">${speed} · ${status.elapsed_seconds} 秒 · ${status.errors} 个错误</p>
    <p class="scan-hint">首次扫描可能需要一些时间，请保持此页面打开；完成后会自动进入相册。</p>
  </main>`
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}
