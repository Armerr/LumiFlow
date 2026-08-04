import { api } from '../shared/api'
import { router } from '../shared/router'
import type { Page } from '../shared/router'
import type { Album, ScanStatus } from '../shared/types'
import { FanScene } from './fan/FanScene'
import { albumFilterQuery, type AlbumFilter } from '../shared/albumPresentation'
import { mountFilterBar } from './fan/FilterBar'
import './fan.scss'

export function renderFilteredEmpty(hasFilter: boolean): string {
  if (!hasFilter) return '<div class="empty-state"><p>照片库中暂时没有可展示的相册</p></div>'
  return `<div class="empty-state">
    <p>当前时间没有相册</p>
    <button class="empty-state-action" id="fan-filter-reset" type="button">查看全部相册</button>
  </div>`
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
  let availableDates: string[] = []

  const clearTimer = () => {
    clearTimeout(timer ?? undefined)
    timer = null
  }

  const loadAlbums = async (container: HTMLElement): Promise<boolean> => {
    if (disposed) return

    const query = albumFilterQuery(currentFilter)
    let data
    try {
      data = await api.albums(query)
      router.setAlbums(data)
    } catch {
      container.innerHTML = '<div class="error-state"><p>无法加载相册</p></div>'
      return false
    }
    if (disposed) return false

    if (!query) {
      availableDates = [...new Set(
        data.albums
          .map((album) => album.date_start)
          .filter((date): date is string => /^\d{4}-\d{2}-\d{2}$/.test(date ?? '')),
      )]
    }

    scene?.dispose()
    scene = null
    tearDownFilter?.()
    tearDownFilter = null

    if (data.albums.length === 0) {
      if (!query) {
        container.innerHTML = renderFilteredEmpty(false)
        return false
      }

      container.innerHTML = `<div id="fan-filter-bar"></div>${renderFilteredEmpty(true)}`
      const filterRoot = container.querySelector<HTMLElement>('#fan-filter-bar')!
      tearDownFilter = mountFilterBar(filterRoot, currentFilter, availableDates, {
        onChange: (next) => {
          currentFilter = next
          void loadAlbums(container)
        },
      })
      container.querySelector<HTMLButtonElement>('#fan-filter-reset')?.addEventListener('click', () => {
        currentFilter = {}
        void loadAlbums(container)
      })
      return false
    }

    container.innerHTML = '<div id="fan-filter-bar"></div>'
    scene = new FanScene(container)
    scene.onAlbumClick = (album: Album) => {
      router.navigate({ page: 'album', name: albumNavigationIdentity(album) })
    }
    scene.setAlbums(data.albums)

    const filterRoot = container.querySelector<HTMLElement>('#fan-filter-bar')
    if (filterRoot) {
      tearDownFilter = mountFilterBar(filterRoot, currentFilter, availableDates, {
        onChange: (next) => {
          currentFilter = next
          void loadAlbums(container)
        },
      })
    }
    return true
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
          // Keep an existing index usable while a watcher or recovery scan runs.
          if (status.has_index && await loadAlbums(container)) return
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
    <p class="scan-kicker">LIBRARY SYNC</p>
    <h1>正在同步照片库</h1>
    <p class="scan-phase">${phase}</p>
    <div class="scan-metrics">
      <div><strong>${status.found}</strong><span>已发现 ${status.found} 张</span></div>
      <div><strong>${status.processed}</strong><span>已校验 ${status.processed} 张</span></div>
      <div><strong>${status.workers}</strong><span>并发线程</span></div>
    </div>
    <p class="scan-meta">${speed} · ${status.elapsed_seconds} 秒 · ${status.errors} 个错误</p>
    <p class="scan-hint">首次建立索引可能需要一些时间；后续同步会复用未变化照片的数据。</p>
  </main>`
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}
